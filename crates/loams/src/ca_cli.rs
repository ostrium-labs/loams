//! `loams ca` (design §83 §6.2–§6.6, CT1 Task 3): the deployment CA's
//! operator commands. They open the stores the way `loams migrate` does
//! (`--data-dir`, `--meta`, `--bucket`) and write the CA document through
//! the same [`loams_ca::Authority`] the `meta` nodes serve it with.
//!
//! - `init` makes the root and the first intermediate. The root's key is
//!   written once, to `--root-key-out` (mode 0600), for the deployment's
//!   secret store; it is kept nowhere else. The intermediate's key is sealed
//!   with the key file `--sealing-key-file` (default
//!   `LOAMS_CA_SEALING_KEY_FILE`, else `<data-dir>/ca/sealing.key`), which
//!   `init` creates and every `meta` node needs.
//! - `token` issues a single-use join token for a grant (`--grant
//!   role/meta`, or a full `spiffe://` ID), printed or written to `--out`
//!   (mode 0600).
//! - `revoke` adds a node, a serial or an identity to the deny list.
//! - `rotate intermediate` needs the root's key (`--root-key-file`).
//! - `status` shows the CA as JSON.
//!
//! With the embedded metastore, run them with the engine stopped, as
//! `migrate` runs (CT1 R3.6).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;

use clap::{Args, Subcommand};
use loams::ca::{MetaCaDoc, sealing_key_file};
use loams_ca::{Authority, KeySealer, Revoke};
use loams_transport::SpiffeId;

use crate::migrate_cli::{StoreArgs, open};

/// `loams ca <command>`.
#[derive(Debug, Subcommand)]
pub(crate) enum CaCommand {
    /// Make the deployment's root and intermediate (once).
    Init {
        #[command(flatten)]
        stores: StoreArgs,
        #[command(flatten)]
        sealing: Sealing,
        /// The cluster id, used as the SPIFFE trust domain [default: a new
        /// random id].
        #[arg(long)]
        cluster: Option<String>,
        /// Where to write the root's private key (mode 0600; must not
        /// exist). Move it to the deployment's secret store.
        #[arg(long)]
        root_key_out: PathBuf,
    },
    /// The CA's state, as JSON.
    Status {
        #[command(flatten)]
        stores: StoreArgs,
    },
    /// A single-use join token, valid for an hour, for the identities it
    /// grants.
    Token {
        #[command(flatten)]
        stores: StoreArgs,
        /// An identity the token grants: `node/<id>`, `role/<role>`,
        /// `svc/<service>`, or a full `spiffe://` ID. Repeat for several.
        #[arg(long = "grant", required = true)]
        grants: Vec<String>,
        /// Write the token to this file (mode 0600) instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Put a node, a certificate or an identity on the deny list.
    Revoke {
        #[command(flatten)]
        stores: StoreArgs,
        #[command(flatten)]
        target: RevokeTarget,
    },
    /// Rotate the intermediate (the root's rotation is CT1 Task 14).
    Rotate {
        #[command(subcommand)]
        what: RotateCommand,
    },
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub(crate) struct RevokeTarget {
    /// A cluster node's id: revokes `spiffe://<cluster>/node/<id>`.
    #[arg(long)]
    node: Option<String>,
    /// A certificate's serial number, hex.
    #[arg(long)]
    serial: Option<String>,
    /// An identity: `role/<role>`, `svc/<service>` or a `spiffe://` ID.
    #[arg(long)]
    identity: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RotateCommand {
    /// A new intermediate signed by the root. The old one stops issuing and
    /// stays in the trust bundle for 30 days.
    Intermediate {
        #[command(flatten)]
        stores: StoreArgs,
        #[command(flatten)]
        sealing: Sealing,
        /// The root's private key (PEM), from the secret store.
        #[arg(long)]
        root_key_file: PathBuf,
    },
    /// Not yet: a new root cross-signed by the old one (CT1 Task 14).
    Root,
}

#[derive(Debug, Args)]
pub(crate) struct Sealing {
    /// The age key file the intermediate's key is sealed to [default:
    /// LOAMS_CA_SEALING_KEY_FILE, else <data-dir>/ca/sealing.key].
    #[arg(long)]
    sealing_key_file: Option<PathBuf>,
}

/// A grant or identity as the operator wrote it: a full ID, or the short
/// form in this CA's cluster.
fn spiffe_id(cluster: &str, s: &str) -> Result<SpiffeId, String> {
    let full = if s.starts_with("spiffe://") {
        s.to_string()
    } else {
        format!("spiffe://{cluster}/{s}")
    };
    full.parse::<SpiffeId>()
        .map_err(|e| format!("{s:?} is not an identity: {e}"))
}

/// Writes `contents` to a new file with mode 0600.
fn write_private(path: &Path, contents: &str, overwrite: bool) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    if overwrite {
        o.create(true).truncate(true);
    } else {
        o.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    f.write_all(contents.as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn stores_of(command: &CaCommand) -> Option<&StoreArgs> {
    match command {
        CaCommand::Init { stores, .. }
        | CaCommand::Status { stores }
        | CaCommand::Token { stores, .. }
        | CaCommand::Revoke { stores, .. }
        | CaCommand::Rotate {
            what: RotateCommand::Intermediate { stores, .. },
        } => Some(stores),
        CaCommand::Rotate {
            what: RotateCommand::Root,
        } => None,
    }
}

pub(crate) async fn run(command: &CaCommand) -> ExitCode {
    let Some(stores) = stores_of(command) else {
        eprintln!(
            "loams ca rotate root: not yet. The root lives 10 years; its cross-signed \
             rotation comes with CT1 Task 14 (ruling R3.7)"
        );
        return ExitCode::from(78);
    };
    // Checked before the stores open, so a bad path changes nothing.
    if let CaCommand::Init { root_key_out, .. } = command
        && root_key_out.exists()
    {
        eprintln!(
            "loams ca init: {} exists; the root key is written to a new file only",
            root_key_out.display()
        );
        return ExitCode::from(78);
    }
    let sealer = match command {
        CaCommand::Init { sealing, .. } => KeySealer::create(&sealing_key_file(
            sealing.sealing_key_file.as_deref(),
            &stores.data_dir,
        )),
        CaCommand::Rotate {
            what: RotateCommand::Intermediate { sealing, .. },
        } => KeySealer::open_file(&sealing_key_file(
            sealing.sealing_key_file.as_deref(),
            &stores.data_dir,
        )),
        _ => Err("not needed".to_string()),
    };
    if let (CaCommand::Init { .. } | CaCommand::Rotate { .. }, Err(err)) = (command, &sealer) {
        eprintln!("loams ca: the sealing key: {err}");
        return ExitCode::FAILURE;
    }
    let opened = match open(stores).await {
        Ok(opened) => opened,
        Err(err) => {
            eprintln!("loams ca: {err}");
            return ExitCode::FAILURE;
        }
    };
    let Some(bucket) = opened.cx.bucket.clone() else {
        eprintln!("loams ca: no bucket");
        opened.close().await;
        return ExitCode::FAILURE;
    };
    let authority = Authority::new(
        Arc::new(MetaCaDoc::new(opened.cx.meta.clone(), bucket)),
        sealer,
    );
    let result = execute(command, &authority).await;
    opened.close().await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("loams ca: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn execute(command: &CaCommand, ca: &Authority) -> Result<(), String> {
    let now = SystemTime::now();
    match command {
        CaCommand::Init {
            cluster,
            root_key_out,
            ..
        } => {
            let cluster = cluster
                .clone()
                .unwrap_or_else(|| ulid::Ulid::generate().to_string().to_lowercase());
            let root = ca.init(&cluster, now).await.map_err(|e| e.to_string())?;
            write_private(root_key_out, root.expose_pem(), false).map_err(|e| {
                format!(
                    "the CA is initialized but its root key could not be written ({e}); \
                     without it the intermediate cannot be rotated"
                )
            })?;
            println!(
                "{}",
                serde_json::json!({
                    "cluster": cluster,
                    "root_key": root_key_out.display().to_string(),
                })
            );
        }
        CaCommand::Status { .. } => {
            let status = ca.status(now).await.map_err(|e| e.to_string())?;
            for warning in &status.warnings {
                eprintln!("loams ca: warning: {warning}");
            }
            println!(
                "{}",
                serde_json::to_string(&status).map_err(|e| e.to_string())?
            );
        }
        CaCommand::Token { grants, out, .. } => {
            let cluster = ca.cluster().await.map_err(|e| e.to_string())?;
            let ids = grants
                .iter()
                .map(|g| spiffe_id(&cluster, g))
                .collect::<Result<Vec<_>, _>>()?;
            let token = ca.create_token(ids, now).await.map_err(|e| e.to_string())?;
            match out {
                Some(path) => {
                    write_private(path, &format!("{}\n", token.expose()), true)?;
                    eprintln!("loams ca: token {} written to {}", token.id, path.display());
                }
                None => println!("{}", token.expose()),
            }
        }
        CaCommand::Revoke { target, .. } => {
            let revoke = match (&target.node, &target.serial, &target.identity) {
                (Some(node), _, _) => Revoke::Node(node.clone()),
                (_, Some(serial), _) => Revoke::Serial(
                    hex::decode(serial.trim_start_matches("0x"))
                        .map_err(|e| format!("--serial {serial:?}: {e}"))?,
                ),
                (_, _, Some(identity)) => {
                    let cluster = ca.cluster().await.map_err(|e| e.to_string())?;
                    Revoke::Identity(spiffe_id(&cluster, identity)?)
                }
                (None, None, None) => return Err("nothing to revoke".to_string()),
            };
            ca.revoke(revoke, now).await.map_err(|e| e.to_string())?;
            eprintln!(
                "loams ca: revoked; nodes drop matching connections as the list reaches them"
            );
        }
        CaCommand::Rotate {
            what: RotateCommand::Intermediate { root_key_file, .. },
        } => {
            let root_key = std::fs::read_to_string(root_key_file)
                .map_err(|e| format!("{}: {e}", root_key_file.display()))?;
            ca.rotate_intermediate(&root_key, now)
                .await
                .map_err(|e| e.to_string())?;
            eprintln!(
                "loams ca: a new intermediate issues from now; the old one stays in the bundle for 30 days"
            );
        }
        CaCommand::Rotate {
            what: RotateCommand::Root,
        } => unreachable!("handled before the stores open"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_full_identities_parse() {
        assert_eq!(
            spiffe_id("c1", "role/meta").unwrap().to_string(),
            "spiffe://c1/role/meta"
        );
        assert_eq!(
            spiffe_id("c1", "spiffe://c1/svc/agentd")
                .unwrap()
                .to_string(),
            "spiffe://c1/svc/agentd"
        );
        assert!(spiffe_id("c1", "nonsense").is_err());
    }
}
