//! `loams byoc confluent key create|rotate` and `loams byoc confluent check`
//! (design 80 §7.2, D1827; plan CC1 Task 6).
//!
//! `key create` makes a Kafka API key for Loams' data service account with the
//! user's own Cloud API key, writes it straight into the install's cloud
//! secret store and prints only the key's id. `key rotate` does the same,
//! asks the connect role to restart the integration's instances, waits a
//! grace period and deletes the old keys. `check` compares the cluster's
//! cloud and region with the VM's and tries the endpoints from this host.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand, ValueEnum};

use crate::store::{CliStore, SecretSink, Target};
use crate::{Client, CloudKey, DEFAULT_BASE, Error, clusters, keys};

/// `loams byoc …`.
#[derive(Debug, Args)]
pub struct ByocArgs {
    #[command(subcommand)]
    pub command: ByocCommand,
}

#[derive(Debug, Subcommand)]
pub enum ByocCommand {
    /// Confluent Cloud next to a BYOC install (design 80): keys and checks.
    Confluent(ConfluentArgs),
}

#[derive(Debug, Args)]
pub struct ConfluentArgs {
    #[command(subcommand)]
    pub command: ConfluentCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfluentCommand {
    /// Kafka API keys for Loams, written to the cloud secret store.
    Key {
        #[command(subcommand)]
        command: Box<KeyCommand>,
    },
    /// Is the cluster next to the VM, and can this host reach it?
    Check(CheckOpts),
    /// What Confluent billed, per cluster and line type (the Costs API; an
    /// organisation-level key: BillingAdmin or OrganizationAdmin).
    Costs(CostsOpts),
}

#[derive(Debug, Clone, Args)]
pub struct CostsOpts {
    /// The first UTC day (YYYY-MM-DD).
    #[arg(long)]
    pub from: String,
    /// The day after the last (end-exclusive) [default: tomorrow, UTC].
    #[arg(long)]
    pub to: Option<String>,
    /// Only lines for these resources (lkc-…; repeat).
    #[arg(long = "resource")]
    pub resources: Vec<String>,
    /// Only lines in this environment (env-…).
    #[arg(long)]
    pub environment: Option<String>,
    /// The Confluent Cloud API.
    #[arg(long, env = "CONFLUENT_API_URL", default_value = DEFAULT_BASE, hide = true)]
    pub api_url: String,
}

#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    /// Create a key for the data service account and store it (prints the id only).
    Create(KeyOpts),
    /// Create a new key, store it, restart the integration's instances, then
    /// delete the old keys after --grace.
    Rotate(RotateOpts),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Cloud {
    Aws,
    Gcp,
    Azure,
}

impl Cloud {
    fn name(self) -> &'static str {
        match self {
            Cloud::Aws => "aws",
            Cloud::Gcp => "gcp",
            Cloud::Azure => "azure",
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct KeyOpts {
    /// The integration's name: secrets `confluent-<integration>-api-key` and `-api-secret`.
    #[arg(long, default_value = "confluent")]
    pub integration: String,
    /// The key's owner: the data service account (the module's `data_service_account_id`).
    #[arg(long)]
    pub service_account: String,
    /// The Kafka cluster (lkc-…).
    #[arg(long)]
    pub cluster: String,
    /// The cluster's environment (env-…).
    #[arg(long)]
    pub environment: String,
    /// Also a Schema Registry key (lsrc-…), stored as `-sr-key` and `-sr-secret`.
    #[arg(long)]
    pub schema_registry: Option<String>,
    /// The install's cloud.
    #[arg(long, value_enum)]
    pub cloud: Cloud,
    /// The install's secret prefix (BY1 naming: loams/<install>).
    #[arg(long)]
    pub secret_prefix: String,
    /// AWS: the Secrets Manager region [default: the aws CLI's].
    #[arg(long, env = "AWS_REGION")]
    pub region: Option<String>,
    /// Google Cloud: the project [default: gcloud's].
    #[arg(long)]
    pub project: Option<String>,
    /// Azure: the Key Vault.
    #[arg(long)]
    pub vault: Option<String>,
    /// The Confluent Cloud API.
    #[arg(long, env = "CONFLUENT_API_URL", default_value = DEFAULT_BASE, hide = true)]
    pub api_url: String,
    /// Run this program instead of aws, gcloud or az.
    #[arg(long, env = "LOAMS_BYOC_SECRET_CLI", hide = true)]
    pub secret_cli: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct RotateOpts {
    #[command(flatten)]
    pub key: KeyOpts,
    /// How long the old keys stay valid after the new one is stored.
    #[arg(long, default_value = "1h", value_parser = humantime::parse_duration)]
    pub grace: Duration,
    /// The connect role's admin API, to restart the instances (reachable over the tailnet).
    #[arg(long)]
    pub connect_admin: Option<String>,
    /// The admin API's bearer token (secrets/connect/connect-admin-token on the VM).
    #[arg(long)]
    pub admin_token_file: Option<PathBuf>,
    /// An instance that uses this integration (repeat for each).
    #[arg(long = "restart-instance")]
    pub restart_instances: Vec<String>,
    /// Keep the old keys (print how to delete them later).
    #[arg(long)]
    pub keep_old: bool,
    /// How long to wait after storing the new key before restarting the
    /// instances: the VM copies Confluent keys from the secret store into the
    /// connect role's files every 5 minutes (`loams-byoc-confluent.timer`).
    /// Pass 0s after `sudo systemctl reload loams-byoc` on the VM.
    #[arg(long, default_value = "6m", value_parser = humantime::parse_duration)]
    pub refresh_wait: Duration,
}

#[derive(Debug, Clone, Args)]
pub struct CheckOpts {
    /// The VM's cloud.
    #[arg(long, value_enum)]
    pub cloud: Cloud,
    /// The VM's region (us-east-1, us-central1, eastus).
    #[arg(long)]
    pub region: String,
    /// The Kafka cluster (lkc-…).
    #[arg(long)]
    pub cluster: String,
    /// The cluster's environment (env-…).
    #[arg(long)]
    pub environment: String,
    /// The Schema Registry URL to try, if the install uses one.
    #[arg(long)]
    pub schema_registry_url: Option<String>,
    /// How long each reachability try may take.
    #[arg(long, default_value = "5s", value_parser = humantime::parse_duration)]
    pub timeout: Duration,
    /// The Confluent Cloud API.
    #[arg(long, env = "CONFLUENT_API_URL", default_value = DEFAULT_BASE, hide = true)]
    pub api_url: String,
    /// Loams' data service account (sa-…): describe its ACLs and check them
    /// against D1826, with the Kafka key in KAFKA_API_KEY / KAFKA_API_SECRET
    /// (one that may describe the cluster's ACLs; the data key usually may not).
    #[arg(long)]
    pub service_account: Option<String>,
    /// The Kafka key for the ACL check (read from the environment by `run`).
    #[arg(skip)]
    pub acl_key: Option<CloudKey>,
}

/// The ids `key create` made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub kafka_key: String,
    pub sr_key: Option<String>,
}

fn check_id(value: &str, prefix: &str, flag: &str) -> Result<(), Error> {
    let rest = value.strip_prefix(prefix).unwrap_or_default();
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::Usage(format!(
            "{flag} is a {prefix}… id, not '{value}'"
        )));
    }
    Ok(())
}

fn validate(o: &KeyOpts) -> Result<(), Error> {
    let name_ok = !o.integration.is_empty()
        && o.integration.len() <= 31
        && o.integration
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !o.integration.starts_with('-');
    if !name_ok {
        return Err(Error::Usage(
            "--integration is lowercase letters, digits and - (up to 31)".into(),
        ));
    }
    let prefix_ok = !o.secret_prefix.is_empty()
        && o.secret_prefix.len() <= 100
        && o.secret_prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c));
    if !prefix_ok {
        return Err(Error::Usage(
            "--secret-prefix holds letters, digits, / _ and - (BY1 naming, loams/<install>)".into(),
        ));
    }
    let owner_ok = check_id(&o.service_account, "sa-", "--service-account").is_ok()
        || check_id(&o.service_account, "u-", "--service-account").is_ok();
    if !owner_ok {
        return Err(Error::Usage(format!(
            "--service-account is an sa-… (or u-…) id, not '{}'",
            o.service_account
        )));
    }
    check_id(&o.cluster, "lkc-", "--cluster")?;
    check_id(&o.environment, "env-", "--environment")?;
    if let Some(sr) = &o.schema_registry {
        check_id(sr, "lsrc-", "--schema-registry")?;
    }
    if o.cloud == Cloud::Azure && o.vault.is_none() {
        return Err(Error::Usage("--cloud azure needs --vault".into()));
    }
    Ok(())
}

/// The secret store target for `o`.
#[must_use]
pub fn target(o: &KeyOpts) -> Target {
    match o.cloud {
        Cloud::Aws => Target::Aws {
            region: o.region.clone(),
        },
        Cloud::Gcp => Target::Gcp {
            project: o.project.clone(),
        },
        Cloud::Azure => Target::Azure {
            vault: o.vault.clone().unwrap_or_default(),
        },
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Creates one key and stores its id and secret under `<short>-key`-style
/// names; undoes the key if the store refuses.
async fn create_and_store<S: SecretSink>(
    c: &Client,
    store: &S,
    o: &KeyOpts,
    resource: &str,
    shorts: (&str, &str),
    out: &mut dyn Write,
) -> Result<String, Error> {
    let t = target(o);
    let display = format!("loams-{}-{}", o.integration, unix_now());
    let description = format!(
        "Loams BYOC: integration {} (loams byoc confluent key)",
        o.integration
    );
    let key = keys::create(
        c,
        &o.service_account,
        resource,
        &o.environment,
        &display,
        &description,
    )
    .await?;
    let id_name = t.secret_name(&o.secret_prefix, &o.integration, shorts.0);
    let secret_name = t.secret_name(&o.secret_prefix, &o.integration, shorts.1);
    let stored = async {
        store
            .put(&id_name, &crate::Secret::new(key.id.clone()))
            .await?;
        store.put(&secret_name, &key.secret).await
    }
    .await;
    if let Err(e) = stored {
        // The secret was shown once and is now lost: the key is useless.
        let undo = match keys::delete(c, &key.id).await {
            Ok(()) => format!("deleted the new key {}", key.id),
            Err(d) => format!(
                "could not delete the new key {} ({d}); delete it by hand",
                key.id
            ),
        };
        return Err(Error::Store(format!("{e}; {undo}")));
    }
    let _ = writeln!(
        out,
        "created API key {} for {} on {resource}; wrote {id_name} and {secret_name}",
        key.id, o.service_account
    );
    Ok(key.id)
}

/// `loams byoc confluent key create`.
pub async fn key_create<S: SecretSink>(
    c: &Client,
    store: &S,
    o: &KeyOpts,
    out: &mut dyn Write,
) -> Result<Created, Error> {
    validate(o)?;
    let kafka_key =
        create_and_store(c, store, o, &o.cluster, ("api-key", "api-secret"), out).await?;
    let sr_key = match &o.schema_registry {
        Some(sr) => Some(create_and_store(c, store, o, sr, ("sr-key", "sr-secret"), out).await?),
        None => None,
    };
    Ok(Created { kafka_key, sr_key })
}

/// `loams byoc confluent key rotate`. `admin` is the HTTP client for the
/// connect role's admin API.
pub async fn key_rotate<S: SecretSink>(
    c: &Client,
    store: &S,
    admin: &reqwest::Client,
    o: &RotateOpts,
    out: &mut dyn Write,
) -> Result<Created, Error> {
    validate(&o.key)?;
    if !o.restart_instances.is_empty() && o.connect_admin.is_none() {
        return Err(Error::Usage(
            "--restart-instance needs --connect-admin (and --admin-token-file)".into(),
        ));
    }
    let token = match &o.admin_token_file {
        Some(p) => Some(crate::Secret::new(
            std::fs::read_to_string(p)
                .map_err(|e| Error::Usage(format!("--admin-token-file {}: {e}", p.display())))?
                .trim()
                .to_string(),
        )),
        None => None,
    };
    let mut old: Vec<String> = keys::list(c, &o.key.service_account, &o.key.cluster)
        .await?
        .into_iter()
        .map(|k| k.id)
        .collect();
    if let Some(sr) = &o.key.schema_registry {
        old.extend(
            keys::list(c, &o.key.service_account, sr)
                .await?
                .into_iter()
                .map(|k| k.id),
        );
    }
    let created = key_create(c, store, &o.key, out).await?;
    old.retain(|id| *id != created.kafka_key && Some(id) != created.sr_key.as_ref());

    let mut restarted = true;
    if let Some(base) = &o.connect_admin {
        if o.restart_instances.is_empty() {
            let _ = writeln!(
                out,
                "warning: --connect-admin without --restart-instance restarts nothing; running instances keep the old key"
            );
            restarted = false;
        } else if !o.refresh_wait.is_zero() {
            let _ = writeln!(
                out,
                "waiting {} for the VM to copy the new key into the connect role's files",
                humantime::format_duration(o.refresh_wait)
            );
            let _ = out.flush();
            tokio::time::sleep(o.refresh_wait).await;
        }
        for id in &o.restart_instances {
            let url = format!(
                "{}/v1/connect/instances/{id}:restart",
                base.trim_end_matches('/')
            );
            let mut req = admin.post(&url);
            if let Some(t) = &token {
                req = req.bearer_auth(t.expose());
            }
            match req.send().await {
                Ok(r) if r.status().is_success() => {
                    let _ = writeln!(out, "restarted instance {id}");
                }
                Ok(r) => {
                    restarted = false;
                    let _ = writeln!(
                        out,
                        "warning: restarting instance {id} answered HTTP {}",
                        r.status().as_u16()
                    );
                }
                Err(e) => {
                    restarted = false;
                    let _ = writeln!(
                        out,
                        "warning: could not restart instance {id}: {}",
                        e.without_url()
                    );
                }
            }
        }
    } else {
        let _ = writeln!(
            out,
            "the VM copies the new key into the connect role's files within 5 minutes (or at once with `sudo systemctl reload loams-byoc`); then restart the integration's connector instances (or pass --connect-admin and --restart-instance)"
        );
    }

    if old.is_empty() {
        return Ok(created);
    }
    if o.keep_old || !restarted {
        let why = if restarted {
            "--keep-old"
        } else {
            "an instance did not restart"
        };
        let _ = writeln!(
            out,
            "kept the old keys ({why}): {}; delete them once every instance uses the new key",
            old.join(", ")
        );
        return Ok(created);
    }
    let _ = writeln!(
        out,
        "waiting {} before deleting the old keys",
        humantime::format_duration(o.grace)
    );
    let _ = out.flush();
    tokio::time::sleep(o.grace).await;
    for id in &old {
        keys::delete(c, id).await?;
        let _ = writeln!(out, "deleted old API key {id}");
    }
    Ok(created)
}

/// `loams byoc confluent check`: true when nothing failed (a cross-region
/// cluster is a warning, not a failure).
pub async fn check(c: &Client, o: &CheckOpts, out: &mut dyn Write) -> Result<bool, Error> {
    check_id(&o.cluster, "lkc-", "--cluster")?;
    check_id(&o.environment, "env-", "--environment")?;
    let cl = clusters::get(c, &o.environment, &o.cluster).await?;
    let mut ok = true;
    let _ = writeln!(
        out,
        "ok   cluster {} ({}, {} {})",
        cl.id, cl.kind, cl.cloud, cl.region
    );
    if cl.cloud == o.cloud.name() && cl.region == o.region {
        let _ = writeln!(out, "ok   the cluster is in the VM's cloud and region");
    } else {
        let _ = writeln!(
            out,
            "warn the cluster is in {} {} and the VM in {} {}: every byte between them pays the cloud's internet data transfer and adds tens of milliseconds a round trip (design 80 §6.1)",
            cl.cloud,
            cl.region,
            o.cloud.name(),
            o.region
        );
    }
    let mut targets = vec![("bootstrap", cl.bootstrap.clone())];
    if let Some(sr) = &o.schema_registry_url {
        let host = sr
            .trim_start_matches("https://")
            .split(['/', ':'])
            .next()
            .unwrap_or_default()
            .to_string();
        targets.push(("Schema Registry", format!("{host}:443")));
    }
    for (what, addr) in targets {
        match tokio::time::timeout(o.timeout, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => {
                let _ = writeln!(out, "ok   {what} {addr} is reachable from this host");
            }
            Ok(Err(e)) => {
                ok = false;
                let _ = writeln!(out, "FAIL {what} {addr}: {e}");
            }
            Err(_) => {
                ok = false;
                let _ = writeln!(
                    out,
                    "FAIL {what} {addr}: no answer in {}",
                    humantime::format_duration(o.timeout)
                );
            }
        }
    }
    if !acl_check(c, &cl, o, out).await {
        ok = false;
    }
    Ok(ok)
}

/// The DescribeAcls sanity check (plan CC1 Task 6): false only when a grant is
/// wider than D1826; a key that may not describe ACLs skips it.
async fn acl_check(c: &Client, cl: &clusters::Cluster, o: &CheckOpts, out: &mut dyn Write) -> bool {
    let Some(sa) = &o.service_account else {
        let _ = writeln!(
            out,
            "skip ACLs: pass --service-account (and KAFKA_API_KEY/KAFKA_API_SECRET) to check Loams' grants"
        );
        return true;
    };
    if check_id(sa, "sa-", "--service-account").is_err() {
        let _ = writeln!(out, "FAIL --service-account is an sa-… id, not '{sa}'");
        return false;
    }
    let Some(key) = &o.acl_key else {
        let _ = writeln!(
            out,
            "skip ACLs: set KAFKA_API_KEY and KAFKA_API_SECRET to a Kafka key that may describe ACLs"
        );
        return true;
    };
    if cl.rest.is_empty() {
        let _ = writeln!(
            out,
            "skip ACLs: Confluent gave no REST endpoint for {}",
            cl.id
        );
        return true;
    }
    let rest = c.for_endpoint(&cl.rest, key.clone());
    let acls = match crate::acls::list(&rest, &cl.id, &format!("User:{sa}")).await {
        Ok(a) => a,
        Err(Error::Unauthorized | Error::Forbidden(_)) => {
            let _ = writeln!(
                out,
                "skip ACLs: the Kafka key {} may not describe ACLs on {}",
                key.id(),
                cl.id
            );
            return true;
        }
        Err(e) => {
            let _ = writeln!(out, "warn ACLs: {e}");
            return true;
        }
    };
    let findings = crate::acls::audit(&acls);
    let mut ok = true;
    for f in &findings {
        match f.level {
            crate::acls::Level::Fail => {
                ok = false;
                let _ = writeln!(out, "FAIL ACL {}", f.message);
            }
            crate::acls::Level::Warn => {
                let _ = writeln!(out, "warn ACL {}", f.message);
            }
        }
    }
    if findings.is_empty() {
        let _ = writeln!(
            out,
            "ok   {} ACLs for {sa}, none wider than D1826",
            acls.len()
        );
    }
    ok
}

/// `loams byoc confluent costs`: totals per resource and line type, then the
/// credits and the grand total. Amounts are USD as Confluent reports them.
pub async fn costs_report(
    c: &Client,
    o: &CostsOpts,
    now_unix: i64,
    out: &mut dyn Write,
) -> Result<(), Error> {
    use crate::costs::{self, Day};
    let from: Day = o.from.parse()?;
    let to: Day = match &o.to {
        Some(t) => t.parse()?,
        None => Day::from_unix(now_unix).plus_days(1),
    };
    for r in &o.resources {
        check_id(r, "lkc-", "--resource")?;
    }
    if let Some(e) = &o.environment {
        check_id(e, "env-", "--environment")?;
    }
    let lines: Vec<costs::CostLine> = costs::list(c, from, to)
        .await?
        .into_iter()
        .filter(|l| o.resources.is_empty() || o.resources.contains(&l.resource_id))
        .filter(|l| {
            o.environment
                .as_ref()
                .is_none_or(|e| l.environment == *e || l.resource_id.is_empty())
        })
        .collect();
    let mut by: std::collections::BTreeMap<(String, String), (f64, f64, f64)> =
        std::collections::BTreeMap::new();
    let (mut original, mut amount, mut credits) = (0.0_f64, 0.0_f64, 0.0_f64);
    for l in &lines {
        if l.resource_id.is_empty() {
            credits += l.amount;
            continue;
        }
        let e = by
            .entry((l.resource_id.clone(), l.line_type.clone()))
            .or_insert((0.0, 0.0, 0.0));
        e.0 += l.quantity.unwrap_or(0.0);
        e.1 += l.original_amount;
        e.2 += l.amount;
        original += l.original_amount;
        amount += l.amount;
    }
    let _ = writeln!(
        out,
        "Confluent costs {from} to {to} (end exclusive, UTC; lines lag up to 72 hours)"
    );
    let _ = writeln!(
        out,
        "{:<16} {:<28} {:>14} {:>12} {:>12}",
        "resource", "line type", "quantity", "list $", "billed $"
    );
    for ((res, ty), (q, orig, amt)) in &by {
        let _ = writeln!(out, "{res:<16} {ty:<28} {q:>14.4} {orig:>12.4} {amt:>12.4}");
    }
    let _ = writeln!(
        out,
        "list ${original:.4}, billed ${amount:.4}, credits and other lines ${credits:.4}, net ${:.4}",
        amount + credits
    );
    Ok(())
}

/// Installs ring as the process's TLS provider (as the loams binary does).
pub fn install_crypto() {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}

fn http(https_only: bool) -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .https_only(https_only)
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("loams-confluent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Usage(e.to_string()))
}

async fn dispatch(args: &ByocArgs, out: &mut dyn Write) -> Result<bool, Error> {
    let ByocCommand::Confluent(cf) = &args.command;
    match &cf.command {
        ConfluentCommand::Key { command } => {
            let (o, rotate) = match command.as_ref() {
                KeyCommand::Create(o) => (o, None),
                KeyCommand::Rotate(r) => (&r.key, Some(r)),
            };
            let c = Client::new(
                &o.api_url,
                CloudKey::from_env()?,
                http(o.api_url.starts_with("https://"))?,
            );
            let mut store = CliStore::new(target(o));
            if let Some(p) = &o.secret_cli {
                store = store.with_program(p);
            }
            match rotate {
                None => key_create(&c, &store, o, out).await.map(|_| true),
                // The admin API is on the tailnet (plain HTTP inside WireGuard).
                Some(r) => key_rotate(&c, &store, &http(false)?, r, out)
                    .await
                    .map(|_| true),
            }
        }
        ConfluentCommand::Costs(o) => {
            let c = Client::new(
                &o.api_url,
                CloudKey::from_env()?,
                http(o.api_url.starts_with("https://"))?,
            );
            let now = i64::try_from(unix_now()).unwrap_or(i64::MAX);
            costs_report(&c, o, now, out).await.map(|()| true)
        }
        ConfluentCommand::Check(o) => {
            let mut o = o.clone();
            let (id, secret) = (
                std::env::var("KAFKA_API_KEY").unwrap_or_default(),
                std::env::var("KAFKA_API_SECRET").unwrap_or_default(),
            );
            if !id.is_empty() && !secret.is_empty() {
                o.acl_key = Some(CloudKey::new(id, crate::Secret::new(secret)));
            }
            let o = &o;
            let c = Client::new(
                &o.api_url,
                CloudKey::from_env()?,
                http(o.api_url.starts_with("https://"))?,
            );
            check(&c, o, out).await
        }
    }
}

/// `loams byoc …`, with its own runtime (it starts no server).
#[must_use]
pub fn run(args: &ByocArgs) -> ExitCode {
    install_crypto();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("loams byoc: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout();
    match rt.block_on(dispatch(args, &mut stdout)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("loams byoc confluent: {e}");
            ExitCode::FAILURE
        }
    }
}
