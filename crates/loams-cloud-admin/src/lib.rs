//! `loams cloud admin …`: the Loams Cloud try-out's admin client (design 78
//! §25.8, D1746; plan LC1, LC2a Tasks 4 and 5).
//!
//! ```text
//! loams cloud admin login | logout
//! loams cloud admin mfa enroll | verify [--code 123456]
//! loams cloud admin waitlist list [--status confirmed] [--since 2026-11-01] [--source github] [--limit 50]
//! loams cloud admin waitlist delete <waitlist-id>
//! loams cloud admin invite <waitlist-id>... | --from-list ids.txt [--expires-in-days 14]
//! loams cloud admin invite list [--state pending]
//! loams cloud admin invite revoke <invite-id>
//! loams cloud admin invite resend <invite-id> [--extend]
//! loams cloud admin audit [--subject <id>] [--type invite.sent] [--since …]
//! ```
//!
//! Sign-in is WorkOS CLI Auth (the device flow); the refresh token is kept in
//! the OS keychain. Every admin call also needs a TOTP step-up within 15
//! minutes: `mfa verify` does it. Results print as JSON on stdout.

pub mod api;
pub mod auth;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::api::{AdminApi, AuditQuery, WaitlistQuery};
use crate::auth::{KEYRING_SERVICE, KeyringStore, TokenStore, WorkOs};

/// Errors the client reports.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{code}: {message}{}", request_id.as_ref().map(|r| format!(" (request {r})")).unwrap_or_default())]
    Api {
        status: u16,
        code: String,
        message: String,
        request_id: Option<String>,
    },
    #[error("sign-in: {0}")]
    SignIn(String),
    #[error("WorkOS: {0}")]
    WorkOs(String),
    #[error("keychain: {0}")]
    Store(String),
    #[error("not signed in: run `loams cloud admin login`")]
    NotSignedIn,
    #[error("{0}")]
    Usage(String),
    #[error("bad response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// `loams cloud …`.
#[derive(Debug, Args)]
pub struct CloudArgs {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Debug, Subcommand)]
pub enum CloudCommand {
    /// Loams Cloud administration, for allow-listed Ostrium admins only.
    Admin(AdminArgs),
}

#[derive(Debug, Args)]
pub struct AdminArgs {
    /// The control plane's API root.
    #[arg(
        long,
        env = "LOAMS_CLOUD_API",
        default_value = "https://api.loams.dev/cloud/v1"
    )]
    pub api: String,
    /// The WorkOS environment's client id (public; staging and production differ).
    #[arg(long, env = "LOAMS_CLOUD_WORKOS_CLIENT_ID")]
    pub workos_client_id: Option<String>,
    /// The Ostrium WorkOS organisation's id (`org_…`), where the cloud_admin role lives.
    #[arg(long, env = "LOAMS_CLOUD_OSTRIUM_ORG")]
    pub ostrium_org: Option<String>,
    /// WorkOS's API root.
    #[arg(
        long,
        env = "LOAMS_CLOUD_WORKOS_API",
        default_value = "https://api.workos.com",
        hide = true
    )]
    pub workos_api: String,
    #[command(subcommand)]
    pub command: AdminCommand,
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Sign in with WorkOS CLI Auth (a code to approve in the browser).
    Login,
    /// Forget the stored session.
    Logout,
    /// The TOTP step-up.
    Mfa {
        #[command(subcommand)]
        command: MfaCommand,
    },
    /// The waitlist.
    Waitlist {
        #[command(subcommand)]
        command: WaitlistCommand,
    },
    /// Invite confirmed waitlist entries (at most 50 a batch), or manage invites.
    #[command(args_conflicts_with_subcommands = true)]
    Invite(InviteArgs),
    /// The audit log.
    Audit {
        #[arg(long)]
        subject: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MfaCommand {
    /// Enrol a TOTP factor (once per admin): prints the secret and otpauth URI once.
    Enroll,
    /// Verify a code: admin calls work for 15 minutes after.
    Verify {
        /// The six-digit code; read from stdin when absent.
        #[arg(long)]
        code: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WaitlistCommand {
    /// List entries, newest first.
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Delete an entry: its address is suppressed and its invite revoked.
    Delete { id: String },
}

#[derive(Debug, Args)]
pub struct InviteArgs {
    #[command(subcommand)]
    pub command: Option<InviteCommand>,
    /// Waitlist ids (`wl_…`) to invite.
    pub waitlist_ids: Vec<String>,
    /// A file of waitlist ids, one per line.
    #[arg(long)]
    pub from_list: Option<PathBuf>,
    /// 1 to 30 days [default: 14, the server's].
    #[arg(long)]
    pub expires_in_days: Option<u8>,
}

#[derive(Debug, Subcommand)]
pub enum InviteCommand {
    /// List invites, newest first.
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Revoke an invite (at WorkOS too).
    Revoke { id: String },
    /// Resend an invite's email (at most 3 times); --extend sends a new one for the full term.
    Resend {
        id: String,
        #[arg(long)]
        extend: bool,
    },
}

/// The waitlist ids of an invite batch: the arguments plus `--from-list`, de-duplicated, in order.
///
/// # Errors
///
/// An unreadable list, a malformed id, or no ids at all, or more than 50.
pub fn batch_ids(args: &InviteArgs) -> Result<Vec<String>, Error> {
    let mut ids: Vec<String> = args.waitlist_ids.clone();
    if let Some(path) = &args.from_list {
        let f = std::io::BufReader::new(std::fs::File::open(path)?);
        for line in f.lines() {
            let line = line?;
            let id = line.split('#').next().unwrap_or("").trim();
            if !id.is_empty() {
                ids.push(id.to_owned());
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    if let Some(bad) = ids.iter().find(|id| !is_waitlist_id(id)) {
        return Err(Error::Usage(format!("{bad:?} is not a waitlist id (wl_…)")));
    }
    match ids.len() {
        0 => Err(Error::Usage("give at least one waitlist id".into())),
        n if n > 50 => Err(Error::Usage(format!("{n} ids: a batch is at most 50"))),
        _ => Ok(ids),
    }
}

fn is_waitlist_id(id: &str) -> bool {
    id.strip_prefix("wl_").is_some_and(|r| {
        r.len() == 22
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Everything a command needs from outside: HTTP, the keychain, stdin and stdout.
pub struct Env<'a> {
    pub http: reqwest::Client,
    pub store: &'a dyn TokenStore,
    pub stdin: &'a mut dyn BufRead,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

impl std::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env").finish_non_exhaustive()
    }
}

/// Installs ring as the process's TLS provider (as the loams binary does); a
/// reqwest client can't be built before it. Safe to call more than once.
pub fn install_crypto() {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}

fn workos(args: &AdminArgs, http: &reqwest::Client) -> Result<WorkOs, Error> {
    let client_id = args.workos_client_id.clone().ok_or_else(|| {
        Error::Usage("set --workos-client-id or LOAMS_CLOUD_WORKOS_CLIENT_ID".into())
    })?;
    Ok(WorkOs::new(
        http.clone(),
        args.workos_api.clone(),
        client_id,
    ))
}

async fn session(args: &AdminArgs, env: &Env<'_>) -> Result<AdminApi, Error> {
    let w = workos(args, &env.http)?;
    let org = args
        .ostrium_org
        .clone()
        .ok_or_else(|| Error::Usage("set --ostrium-org or LOAMS_CLOUD_OSTRIUM_ORG".into()))?;
    let refresh = env.store.load(&w.store_key())?.ok_or(Error::NotSignedIn)?;
    let (access, rotated) = w.refresh(&refresh, &org).await?;
    env.store.save(&w.store_key(), &rotated)?;
    Ok(AdminApi::new(env.http.clone(), args.api.clone(), access))
}

fn print(out: &mut dyn Write, v: &serde_json::Value) -> Result<(), Error> {
    serde_json::to_writer_pretty(&mut *out, v)?;
    writeln!(out)?;
    Ok(())
}

/// Runs one admin command.
///
/// # Errors
///
/// Any failure, reported by `run` with its code and request id.
pub async fn admin(args: &AdminArgs, env: &mut Env<'_>) -> Result<(), Error> {
    match &args.command {
        AdminCommand::Login => {
            let w = workos(args, &env.http)?;
            let auth = w.authorize_device().await?;
            writeln!(
                env.stderr,
                "Open {} and enter the code {}{}",
                auth.verification_uri,
                auth.user_code,
                auth.verification_uri_complete
                    .as_ref()
                    .map(|u| format!("\n(or open {u})"))
                    .unwrap_or_default()
            )?;
            let refresh = w.poll(&auth).await?;
            env.store.save(&w.store_key(), &refresh)?;
            writeln!(
                env.stderr,
                "Signed in. Run `loams cloud admin mfa verify` before admin calls."
            )?;
            Ok(())
        }
        AdminCommand::Logout => {
            let w = workos(args, &env.http)?;
            env.store.delete(&w.store_key())?;
            writeln!(env.stderr, "Signed out.")?;
            Ok(())
        }
        AdminCommand::Mfa { command } => {
            let api = session(args, env).await?;
            match command {
                MfaCommand::Enroll => {
                    let v = api.mfa_enroll().await?;
                    writeln!(
                        env.stderr,
                        "Add this secret to your authenticator app now; it is not shown again."
                    )?;
                    print(env.stdout, &v)
                }
                MfaCommand::Verify { code } => {
                    let code = match code {
                        Some(c) => c.clone(),
                        None => {
                            write!(env.stderr, "TOTP code: ")?;
                            env.stderr.flush()?;
                            let mut line = String::new();
                            env.stdin.read_line(&mut line)?;
                            line.trim().to_owned()
                        }
                    };
                    print(env.stdout, &api.mfa_verify(&code).await?)
                }
            }
        }
        AdminCommand::Waitlist { command } => {
            let api = session(args, env).await?;
            match command {
                WaitlistCommand::List {
                    status,
                    since,
                    source,
                    cursor,
                    limit,
                } => {
                    let q = WaitlistQuery {
                        status: status.clone(),
                        since: since.clone(),
                        source: source.clone(),
                        cursor: cursor.clone(),
                        limit: *limit,
                    };
                    print(env.stdout, &api.waitlist(&q).await?)
                }
                WaitlistCommand::Delete { id } => {
                    if !is_waitlist_id(id) {
                        return Err(Error::Usage(format!("{id:?} is not a waitlist id (wl_…)")));
                    }
                    api.waitlist_delete(id).await?;
                    print(env.stdout, &serde_json::json!({ "deleted": id }))
                }
            }
        }
        AdminCommand::Invite(inv) => {
            match &inv.command {
                Some(InviteCommand::List {
                    state,
                    cursor,
                    limit,
                }) => {
                    let api = session(args, env).await?;
                    print(
                        env.stdout,
                        &api.invites(state.as_ref(), cursor.as_ref(), *limit).await?,
                    )
                }
                Some(InviteCommand::Revoke { id }) => {
                    let api = session(args, env).await?;
                    print(env.stdout, &api.revoke(id).await?)
                }
                Some(InviteCommand::Resend { id, extend }) => {
                    let api = session(args, env).await?;
                    print(env.stdout, &api.resend(id, *extend).await?)
                }
                None => {
                    let ids = batch_ids(inv)?;
                    if let Some(d) = inv.expires_in_days
                        && !(1..=30).contains(&d)
                    {
                        return Err(Error::Usage("--expires-in-days is 1 to 30".into()));
                    }
                    let api = session(args, env).await?;
                    // One key per invocation; the client retries a lost response with it.
                    let key = format!("cli-{}", ulid::Ulid::generate());
                    print(
                        env.stdout,
                        &api.invite(&ids, inv.expires_in_days, &key).await?,
                    )
                }
            }
        }
        AdminCommand::Audit {
            subject,
            r#type,
            since,
            cursor,
            limit,
        } => {
            let api = session(args, env).await?;
            let q = AuditQuery {
                subject: subject.clone(),
                r#type: r#type.clone(),
                since: since.clone(),
                cursor: cursor.clone(),
                limit: *limit,
            };
            print(env.stdout, &api.audit(&q).await?)
        }
    }
}

/// `loams cloud …`, with its own runtime (it starts no server).
#[must_use]
pub fn run(args: &CloudArgs) -> ExitCode {
    install_crypto();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("loams cloud: {e}");
            return ExitCode::FAILURE;
        }
    };
    let http = match reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("loams-cloud-admin/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("loams cloud: {e}");
            return ExitCode::FAILURE;
        }
    };
    let store = KeyringStore::new(KEYRING_SERVICE);
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let CloudCommand::Admin(a) = &args.command;
    let mut env = Env {
        http,
        store: &store,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
    };
    match rt.block_on(admin(a, &mut env)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams cloud admin: {e}");
            ExitCode::FAILURE
        }
    }
}
