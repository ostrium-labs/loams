//! `loams-house-mine merge`: inventories into a ranking (HE0 Task 2).

use std::path::PathBuf;

use anyhow::Context as _;

use crate::inventory::{self, Inventory};
use crate::profile;

/// Arguments of `merge`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The inventories to merge.
    #[arg(required = true)]
    pub inventories: Vec<PathBuf>,
    /// The number of applications that puts a feature in the profile.
    #[arg(long, default_value_t = 3)]
    pub min_apps: usize,
    /// A gate application, whose features are in the profile regardless;
    /// repeatable.
    #[arg(long = "gate", default_values_t = ["langfuse".to_owned(), "openpanel".to_owned(), "signoz".to_owned()])]
    pub gate: Vec<String>,
    /// Where to write the ranking.
    #[arg(long)]
    pub out: PathBuf,
}

/// Runs `merge`.
///
/// # Errors
///
/// An unreadable inventory, or one that fails its checks.
pub fn run(args: &Args) -> anyhow::Result<()> {
    let mut inventories = Vec::new();
    for path in &args.inventories {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let inv: Inventory =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        inventory::check(&inv).with_context(|| format!("checking {}", path.display()))?;
        inventories.push(inv);
    }
    let ranking = profile::merge(&inventories, args.min_apps, &args.gate);
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&ranking)? + "\n")
        .with_context(|| format!("writing {}", args.out.display()))?;
    eprintln!(
        "{} applications, {} features, {} in the profile",
        ranking.apps.len(),
        ranking.features.len(),
        ranking.features.iter().filter(|f| f.in_profile).count()
    );
    Ok(())
}
