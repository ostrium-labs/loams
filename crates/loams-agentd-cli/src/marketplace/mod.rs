//! `loams marketplace` (design §73 §7.1–§7.2, D1465, D1466; plan MK1 Task 10):
//! the Seed Exchange from a terminal, through the running local agentd.
//!
//! - `search`, `info`: listings from the verified indexes.
//! - `install`: consent first. The plan from `PlanInstall` is printed in
//!   plain words and the person confirms it; a script passes `--yes` with the
//!   `--plan-digest` it reviewed (`--dry-run` prints it), never a blind yes.
//!   Secrets are read without echo, or from `--secret NAME=@FILE`; never from
//!   the command line itself. `InstallListing` then installs exactly that plan.
//! - `update`: an update that widens anything answers `consent_required`; the
//!   new plan is shown and confirmed the same way.
//! - `uninstall`, `list`, `source list|add|remove`.
//!
//! Text by default, `--json` for machines (the ndjson replies as they are).

mod render;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use loams_agentd_proto::marketplace::{
    AddMarketplaceSourceParams, AvailableUpdateList, CONSENT_REQUIRED, FEATURE,
    InstallListingParams, InstallPlan, InstalledListingList, InstalledListingReply, ListingDetail,
    ListingRef, ListingSearchResults, ListingTier, MarketplaceSource, MarketplaceSourceList,
    PlanInstallReply, RemoveMarketplaceSourceParams, SearchListingsParams, UninstallListingParams,
    UpdateListingParams,
};
use loams_agentd_rpc::{RpcClient, RpcError, methods};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::connect;
use crate::term::{StdTerminal, Terminal};

/// `loams marketplace …`.
#[derive(Debug, Args)]
pub struct MarketplaceArgs {
    /// The agent daemon's directory [default: Loams Desktop's
    /// `<userData>/agentd`, or LOAMS_AGENTD_CONFIG's directory].
    #[arg(long, global = true, value_name = "DIR")]
    pub agentd_dir: Option<PathBuf>,
    /// Print the daemon's replies as JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: MarketplaceCommand,
}

#[derive(Debug, Subcommand)]
pub enum MarketplaceCommand {
    /// Find listings: every word must match the id, title, summary or a tool.
    Search(SearchArgs),
    /// One listing: what it is, its versions and their revocations.
    Info(ListingArgs),
    /// Show the install plan, ask, then install exactly that plan.
    Install(InstallArgs),
    /// Remove an install and its secrets. Its data stays.
    Uninstall(UninstallArgs),
    /// The installed listings, with updates and revocations.
    List,
    /// Without an id, the available updates; with one, update it. An update
    /// that asks for more than the installed version shows its new plan and
    /// asks again.
    Update(UpdateArgs),
    /// The indexes listings come from.
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Tier {
    FirstParty,
    Verified,
    Community,
}

impl From<Tier> for ListingTier {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::FirstParty => ListingTier::FirstParty,
            Tier::Verified => ListingTier::Verified,
            Tier::Community => ListingTier::Community,
        }
    }
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Words to match; none lists everything.
    pub query: Vec<String>,
    /// `agent`, `harness` or `mcp-server` (repeatable).
    #[arg(long)]
    pub kind: Vec<String>,
    /// Such as `mastra` (repeatable).
    #[arg(long)]
    pub framework: Vec<String>,
    /// Such as `typescript` (repeatable).
    #[arg(long)]
    pub language: Vec<String>,
    /// The least conformance level: `core`, `compatible` or `native`.
    #[arg(long)]
    pub min_level: Option<String>,
    /// Repeatable.
    #[arg(long, value_enum)]
    pub tier: Vec<Tier>,
    /// Only this source's listings.
    #[arg(long)]
    pub source: Option<String>,
}

#[derive(Debug, Args)]
pub struct ListingArgs {
    /// The listing id, `<namespace>/<name>`.
    pub id: String,
    /// A version [default: the latest].
    #[arg(long)]
    pub version: Option<String>,
    /// A source [default: the first that has the listing].
    #[arg(long)]
    pub source: Option<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    #[command(flatten)]
    pub listing: ListingArgs,
    /// A secret from a file: `NAME=@FILE` (`@-` reads stdin). Secrets are
    /// never taken from the command line itself. Others are asked for.
    #[arg(long, value_name = "NAME=@FILE")]
    pub secret: Vec<String>,
    /// A non-secret setting, `NAME=VALUE`.
    #[arg(long, value_name = "NAME=VALUE")]
    pub config: Vec<String>,
    /// Print the plan and its digest; install nothing.
    #[arg(long, conflicts_with = "yes")]
    pub dry_run: bool,
    /// Install without asking, if the plan's digest is still `--plan-digest`.
    #[arg(long, requires = "plan_digest")]
    pub yes: bool,
    /// The digest of the plan you reviewed (`--dry-run` prints it).
    #[arg(long, requires = "yes", value_name = "DIGEST")]
    pub plan_digest: Option<String>,
}

#[derive(Debug, Args)]
pub struct UninstallArgs {
    pub id: String,
    /// Do not ask.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// The installed listing [default: list the available updates].
    pub id: Option<String>,
    /// A version [default: the latest].
    #[arg(long, requires = "id")]
    pub version: Option<String>,
    /// Print the new version's plan and its digest; update nothing.
    #[arg(long, requires = "id", conflicts_with = "yes")]
    pub dry_run: bool,
    /// Update without asking, if a widening update's plan digest is still
    /// `--plan-digest`.
    #[arg(long, requires_all = ["id", "plan_digest"])]
    pub yes: bool,
    /// The digest of the new plan you reviewed (`--dry-run` prints it).
    #[arg(long, requires = "yes", value_name = "DIGEST")]
    pub plan_digest: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum SourceCommand {
    /// The sources, in search order.
    List,
    /// Add an index, kept only if it verifies against `--identity`.
    Add {
        /// A short local name.
        name: String,
        /// The index URL (`https://…/index.json`).
        url: String,
        /// The certificate identity the index must be signed by.
        #[arg(long)]
        identity: String,
        /// The OIDC issuer [default: GitHub Actions'].
        #[arg(long)]
        issuer: Option<String>,
    },
    /// Remove a source. The official one stays.
    Remove { name: String },
}

/// `loams marketplace …` as a process: connect to the local agentd and run
/// the command. Errors go to stderr, with exit 1.
pub fn run(args: &MarketplaceArgs) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("loams marketplace: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut term = StdTerminal::new();
    let result = runtime.block_on(async {
        let dir = connect::agentd_dir(args.agentd_dir.as_deref(), |name| std::env::var_os(name))
            .ok_or_else(|| {
                anyhow!("no home directory: name the agent daemon's directory with --agentd-dir")
            })?;
        let agentd = connect::connect(&dir).await?;
        if !agentd.has(FEATURE) {
            bail!(
                "the agent daemon ({}) has no marketplace: update Loams Desktop",
                agentd.hello.version
            );
        }
        execute(args, &agentd.client, &mut term).await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = term.out().flush();
            eprintln!("loams marketplace: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Run `args` against a connected daemon.
pub async fn execute(
    args: &MarketplaceArgs,
    client: &RpcClient,
    term: &mut dyn Terminal,
) -> anyhow::Result<()> {
    let cx = Cx {
        client,
        json: args.json,
    };
    match &args.command {
        MarketplaceCommand::Search(search) => cx.search(search, term).await,
        MarketplaceCommand::Info(listing) => cx.info(listing, term).await,
        MarketplaceCommand::Install(install) => cx.install(install, term).await,
        MarketplaceCommand::Uninstall(uninstall) => cx.uninstall(uninstall, term).await,
        MarketplaceCommand::List => cx.list(term).await,
        MarketplaceCommand::Update(update) => cx.update(update, term).await,
        MarketplaceCommand::Source { command } => cx.source(command, term).await,
    }
}

struct Cx<'a> {
    client: &'a RpcClient,
    json: bool,
}

/// The reason prefix of a daemon error (`consent_required: …`).
fn reason(error: &RpcError) -> Option<&str> {
    match error {
        RpcError::Failed(text) | RpcError::Fatal(text) => {
            text.split_once(": ").map(|(reason, _)| reason)
        }
        _ => None,
    }
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

impl Cx<'_> {
    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &impl Serialize,
    ) -> Result<T, RpcError> {
        let params =
            serde_json::to_value(params).map_err(|e| RpcError::BadParams(e.to_string()))?;
        self.client.call_as(method, params).await
    }

    fn print_json(&self, term: &mut dyn Terminal, value: &impl Serialize) -> anyhow::Result<()> {
        let out = term.out();
        serde_json::to_writer_pretty(&mut *out, value)?;
        writeln!(out)?;
        Ok(())
    }

    async fn search(&self, args: &SearchArgs, term: &mut dyn Terminal) -> anyhow::Result<()> {
        let params = SearchListingsParams {
            query: args.query.join(" "),
            kinds: args.kind.clone(),
            frameworks: args.framework.clone(),
            languages: args.language.clone(),
            min_level: args.min_level.clone(),
            tiers: args.tier.iter().map(|&t| t.into()).collect(),
            source: args.source.clone(),
        };
        let found: ListingSearchResults = self.call(methods::SEARCH_LISTINGS, &params).await?;
        if self.json {
            return self.print_json(term, &found);
        }
        render::search(term.out(), &found)?;
        Ok(())
    }

    fn listing_ref(args: &ListingArgs) -> ListingRef {
        ListingRef {
            id: args.id.clone(),
            version: args.version.clone(),
            source: args.source.clone(),
        }
    }

    async fn info(&self, args: &ListingArgs, term: &mut dyn Terminal) -> anyhow::Result<()> {
        let detail: ListingDetail = self
            .call(methods::GET_LISTING, &Self::listing_ref(args))
            .await?;
        if self.json {
            return self.print_json(term, &detail);
        }
        render::detail(term.out(), &detail)?;
        Ok(())
    }

    async fn install(&self, args: &InstallArgs, term: &mut dyn Terminal) -> anyhow::Result<()> {
        let id = &args.listing.id;
        let reply: PlanInstallReply = self
            .call(methods::PLAN_INSTALL, &Self::listing_ref(&args.listing))
            .await?;
        let plan = reply.plan;
        if args.dry_run {
            if self.json {
                return self.print_json(term, &PlanInstallReply { plan });
            }
            let out = term.out();
            render::plan(out, &plan)?;
            writeln!(
                out,
                "\nNothing was installed. To install exactly this plan:"
            )?;
            writeln!(
                out,
                "  loams marketplace install {id} --version {} --source {} --yes --plan-digest {}",
                plan.version, plan.summary.source, plan.digest
            )?;
            return Ok(());
        }

        let mut secrets = secret_flags(&args.secret, &plan)?;
        let mut config = config_flags(&args.config)?;
        if args.yes {
            let reviewed = args.plan_digest.as_deref().unwrap_or_default();
            if reviewed != plan.digest {
                bail!(
                    "plan_changed: the plan for {id} is now {}, not the {reviewed} you reviewed; nothing was installed. Review it again with `loams marketplace install {id} --dry-run`",
                    plan.digest
                );
            }
        } else {
            if self.json {
                bail!(
                    "--json installs only with --yes --plan-digest <digest>: review the plan with `--dry-run --json` first"
                );
            }
            if !term.interactive() {
                bail!(
                    "no terminal to confirm the plan on: review it with `loams marketplace install {id} --dry-run`, then run again with --yes --plan-digest <digest>"
                );
            }
            render::plan(term.out(), &plan)?;
            let answer = term.ask(&format!(
                "\nInstall {} {} with exactly these permissions? [y/N] ",
                plan.summary.title, plan.version
            ))?;
            if !is_yes(&answer) {
                bail!("cancelled: nothing was installed");
            }
            self.ask_config(&plan, &mut config, term).await?;
            for secret in &plan.secrets {
                if secrets.contains_key(&secret.name) {
                    continue;
                }
                let keep = if secret.required {
                    "keep the one agentd has"
                } else {
                    "skip it"
                };
                let value = term.ask_secret(&format!(
                    "{} ({}; empty to {keep}): ",
                    secret.name, secret.description
                ))?;
                if !value.is_empty() {
                    secrets.insert(secret.name.clone(), value);
                }
            }
        }
        if args.yes {
            self.require_config(&plan, &config).await?;
        }

        let params = InstallListingParams {
            id: id.clone(),
            version: Some(plan.version.clone()),
            source: Some(plan.summary.source.clone()),
            plan_digest: plan.digest.clone(),
            config,
            secrets,
        };
        let installed: InstalledListingReply = self.call(methods::INSTALL_LISTING, &params).await?;
        if self.json {
            return self.print_json(term, &installed);
        }
        render::installed_note(term.out(), &installed.installed)?;
        Ok(())
    }

    /// The listing's required non-secret settings with no default.
    async fn required_config(&self, plan: &InstallPlan) -> anyhow::Result<Vec<(String, String)>> {
        let detail: ListingDetail = self
            .call(
                methods::GET_LISTING,
                &ListingRef {
                    id: plan.summary.id.clone(),
                    version: Some(plan.version.clone()),
                    source: Some(plan.summary.source.clone()),
                },
            )
            .await?;
        Ok(detail.listing["config"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["secret"].as_bool() != Some(true))
            .filter(|c| c["required"].as_bool() == Some(true) && c.get("default").is_none())
            .filter_map(|c| {
                Some((
                    c["name"].as_str()?.to_owned(),
                    c["description"].as_str().unwrap_or_default().to_owned(),
                ))
            })
            .collect())
    }

    async fn ask_config(
        &self,
        plan: &InstallPlan,
        config: &mut BTreeMap<String, String>,
        term: &mut dyn Terminal,
    ) -> anyhow::Result<()> {
        for (name, description) in self.required_config(plan).await? {
            if config.contains_key(&name) {
                continue;
            }
            let value = term.ask(&format!("{name} ({description}): "))?;
            if value.is_empty() {
                bail!("cancelled: {name} is required; nothing was installed");
            }
            config.insert(name, value);
        }
        Ok(())
    }

    async fn require_config(
        &self,
        plan: &InstallPlan,
        config: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for (name, _) in self.required_config(plan).await? {
            if !config.contains_key(&name) {
                bail!(
                    "{} needs the setting {name}: pass --config {name}=VALUE",
                    plan.summary.id
                );
            }
        }
        Ok(())
    }

    async fn uninstall(&self, args: &UninstallArgs, term: &mut dyn Terminal) -> anyhow::Result<()> {
        if !args.yes {
            if !term.interactive() {
                bail!(
                    "no terminal to confirm on: pass --yes to uninstall {}",
                    args.id
                );
            }
            let answer = term.ask(&format!(
                "Uninstall {}? Its secrets are deleted; its data stays. [y/N] ",
                args.id
            ))?;
            if !is_yes(&answer) {
                bail!("cancelled: nothing was uninstalled");
            }
        }
        let reply: serde_json::Value = self
            .call(
                methods::UNINSTALL_LISTING,
                &UninstallListingParams {
                    id: args.id.clone(),
                },
            )
            .await?;
        if self.json {
            return self.print_json(term, &reply);
        }
        writeln!(
            term.out(),
            "Uninstalled {}. Its data stays in agentd's directory.",
            args.id
        )?;
        Ok(())
    }

    async fn installed(&self) -> Result<InstalledListingList, RpcError> {
        self.call(methods::LIST_INSTALLED_LISTINGS, &serde_json::json!({}))
            .await
    }

    async fn list(&self, term: &mut dyn Terminal) -> anyhow::Result<()> {
        let installed = self.installed().await?;
        if self.json {
            return self.print_json(term, &installed);
        }
        render::installed(term.out(), &installed)?;
        Ok(())
    }

    async fn update(&self, args: &UpdateArgs, term: &mut dyn Terminal) -> anyhow::Result<()> {
        let Some(id) = &args.id else {
            let updates: AvailableUpdateList = self
                .call(methods::CHECK_LISTING_UPDATES, &serde_json::json!({}))
                .await?;
            if self.json {
                return self.print_json(term, &updates);
            }
            render::updates(term.out(), &updates)?;
            return Ok(());
        };
        let installed = self.installed().await?;
        let current = installed
            .installed
            .iter()
            .find(|i| &i.id == id)
            .ok_or_else(|| anyhow!("{id} is not installed"))?;
        let plan_ref = ListingRef {
            id: id.clone(),
            version: args.version.clone(),
            source: Some(current.source.clone()),
        };
        if args.dry_run {
            let reply: PlanInstallReply = self.call(methods::PLAN_INSTALL, &plan_ref).await?;
            if self.json {
                return self.print_json(term, &reply);
            }
            let out = term.out();
            writeln!(out, "Installed: {} {}.\n", id, current.version)?;
            render::plan(out, &reply.plan)?;
            writeln!(
                out,
                "\nNothing was updated. To update to exactly this plan:"
            )?;
            writeln!(
                out,
                "  loams marketplace update {id} --version {} --yes --plan-digest {}",
                reply.plan.version, reply.plan.digest
            )?;
            return Ok(());
        }
        let mut params = UpdateListingParams {
            id: id.clone(),
            version: args.version.clone(),
            plan_digest: args.plan_digest.clone(),
        };
        let updated = match self
            .call::<InstalledListingReply>(methods::UPDATE_LISTING, &params)
            .await
        {
            Ok(updated) => updated,
            Err(error) if reason(&error) == Some(CONSENT_REQUIRED) && !args.yes => {
                if self.json || !term.interactive() {
                    return Err(anyhow::Error::new(error).context(format!(
                        "review the new plan with `loams marketplace update {id} --dry-run`, then run again with --yes --plan-digest <digest>"
                    )));
                }
                let reply: PlanInstallReply = self.call(methods::PLAN_INSTALL, &plan_ref).await?;
                let plan = reply.plan;
                let why = error.to_string();
                let why = why
                    .strip_prefix(&format!("{CONSENT_REQUIRED}: "))
                    .unwrap_or(&why);
                let out = term.out();
                writeln!(
                    out,
                    "This update asks for more than {} {} has: {why}.\n",
                    id, current.version
                )?;
                render::plan(out, &plan)?;
                let answer = term.ask(&format!(
                    "\nUpdate {} to {} with exactly these permissions? [y/N] ",
                    id, plan.version
                ))?;
                if !is_yes(&answer) {
                    bail!("cancelled: {id} stays at {}", current.version);
                }
                params.version = Some(plan.version.clone());
                params.plan_digest = Some(plan.digest);
                self.call(methods::UPDATE_LISTING, &params).await?
            }
            Err(error) => return Err(error.into()),
        };
        if self.json {
            return self.print_json(term, &updated);
        }
        writeln!(
            term.out(),
            "Updated {} from {} to {}.",
            id,
            current.version,
            updated.installed.version
        )?;
        Ok(())
    }

    async fn source(&self, command: &SourceCommand, term: &mut dyn Terminal) -> anyhow::Result<()> {
        match command {
            SourceCommand::List => {
                let list: MarketplaceSourceList = self
                    .call(methods::LIST_MARKETPLACE_SOURCES, &serde_json::json!({}))
                    .await?;
                if self.json {
                    return self.print_json(term, &list);
                }
                render::sources(term.out(), &list)?;
            }
            SourceCommand::Add {
                name,
                url,
                identity,
                issuer,
            } => {
                let added: MarketplaceSource = self
                    .call(
                        methods::ADD_MARKETPLACE_SOURCE,
                        &AddMarketplaceSourceParams {
                            name: name.clone(),
                            url: url.clone(),
                            identity: identity.clone(),
                            issuer: issuer.clone(),
                        },
                    )
                    .await?;
                if self.json {
                    return self.print_json(term, &added);
                }
                writeln!(
                    term.out(),
                    "Added {}: {} listings, signed by {}.",
                    added.name,
                    added.listings,
                    added.identity
                )?;
            }
            SourceCommand::Remove { name } => {
                let reply: serde_json::Value = self
                    .call(
                        methods::REMOVE_MARKETPLACE_SOURCE,
                        &RemoveMarketplaceSourceParams { name: name.clone() },
                    )
                    .await?;
                if self.json {
                    return self.print_json(term, &reply);
                }
                writeln!(term.out(), "Removed {name}.")?;
            }
        }
        Ok(())
    }
}

/// `--secret NAME=@FILE`: the file's contents without a final newline. A
/// literal value is refused (it would sit in shell history and `ps`), and so
/// is a name the plan does not ask for.
fn secret_flags(flags: &[String], plan: &InstallPlan) -> anyhow::Result<BTreeMap<String, String>> {
    let mut secrets = BTreeMap::new();
    let mut stdin_used = false;
    for flag in flags {
        let Some((name, value)) = flag.split_once('=') else {
            bail!("--secret {flag}: expected NAME=@FILE");
        };
        let Some(path) = value.strip_prefix('@') else {
            bail!(
                "--secret {name}: secrets are read from a file (NAME=@FILE, or @- for stdin), never from the command line"
            );
        };
        if !plan.secrets.iter().any(|s| s.name == name) {
            bail!("{} asks for no secret {name}", plan.summary.id);
        }
        let mut text = if path == "-" {
            if stdin_used {
                bail!("--secret: only one secret can come from stdin");
            }
            stdin_used = true;
            std::io::read_to_string(std::io::stdin()).context("read a secret from stdin")?
        } else {
            std::fs::read_to_string(path).with_context(|| format!("read the secret {name}"))?
        };
        while text.ends_with(['\n', '\r']) {
            text.pop();
        }
        secrets.insert(name.to_owned(), text);
    }
    Ok(secrets)
}

/// `--config NAME=VALUE`.
fn config_flags(flags: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    flags
        .iter()
        .map(|flag| {
            flag.split_once('=')
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .ok_or_else(|| anyhow!("--config {flag}: expected NAME=VALUE"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_or_yes_confirms() {
        for yes in ["y", "Y", "yes", " YES "] {
            assert!(is_yes(yes), "{yes}");
        }
        for no in ["", "n", "no", "yep", "sure"] {
            assert!(!is_yes(no), "{no}");
        }
    }

    #[test]
    fn the_reason_is_the_error_prefix() {
        let error = RpcError::Failed("consent_required: updating x asks for more".into());
        assert_eq!(reason(&error), Some(CONSENT_REQUIRED));
        assert_eq!(reason(&RpcError::Closed), None);
    }

    #[test]
    fn config_flags_need_a_name_and_value() {
        let config = config_flags(&["a=1".into(), "b=x=y".into()]).expect("parsed");
        assert_eq!(config["a"], "1");
        assert_eq!(config["b"], "x=y");
        assert!(config_flags(&["a".into()]).is_err());
    }
}
