//! The text forms of the daemon's replies. The install plan is the consent
//! screen of design §73 §7.2 in plain words: what the agent can do, what it
//! can't, what you supply, and where it comes from.

use std::io::{self, Write};

use loams_agentd_proto::marketplace::{
    AvailableUpdateList, InstallPlan, InstalledListing, InstalledListingList, ListingDetail,
    ListingRevocation, ListingSearchResults, ListingSummary, ListingTier, MarketplaceSourceList,
    RevocationSeverity,
};
use serde_json::Value;

fn tier(tier: ListingTier) -> &'static str {
    match tier {
        ListingTier::FirstParty => "first-party",
        ListingTier::Verified => "verified",
        ListingTier::Community => "community",
    }
}

fn revocation(out: &mut dyn Write, revocation: &ListingRevocation) -> io::Result<()> {
    let what = match revocation.severity {
        RevocationSeverity::Warn => "warning",
        RevocationSeverity::Disable => "revoked, will not run",
    };
    write!(
        out,
        "  ! {what} ({}): {}",
        revocation.versions, revocation.reason
    )?;
    if let Some(advisory) = &revocation.advisory {
        write!(out, " ({advisory})")?;
    }
    writeln!(out)
}

/// Each column's width: its widest cell.
fn widths(rows: &[Vec<String>]) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect()
}

/// One row, every cell but the last padded to its column's width.
fn row(out: &mut dyn Write, cells: &[String], widths: &[usize]) -> io::Result<()> {
    let mut line = String::new();
    for (c, cell) in cells.iter().enumerate() {
        if c + 1 == cells.len() {
            line.push_str(cell);
        } else {
            let pad = widths[c].saturating_sub(cell.chars().count());
            line.push_str(cell);
            line.push_str(&" ".repeat(pad + 2));
        }
    }
    writeln!(out, "{}", line.trim_end())
}

fn table(out: &mut dyn Write, rows: &[Vec<String>]) -> io::Result<()> {
    let widths = widths(rows);
    rows.iter().try_for_each(|cells| row(out, cells, &widths))
}

pub fn search(out: &mut dyn Write, found: &ListingSearchResults) -> io::Result<()> {
    if found.results.is_empty() {
        return writeln!(out, "No listings match.");
    }
    let rows: Vec<Vec<String>> = found
        .results
        .iter()
        .map(|summary| {
            vec![
                summary.id.clone(),
                summary.latest.clone().unwrap_or_else(|| "-".into()),
                summary.kind.clone(),
                tier(summary.tier).into(),
                format!("{} — {}", summary.title, summary.summary),
            ]
        })
        .collect();
    let widths = widths(&rows);
    for (summary, cells) in found.results.iter().zip(&rows) {
        row(out, cells, &widths)?;
        if let Some(r) = &summary.revocation {
            revocation(out, r)?;
        }
    }
    Ok(())
}

fn facts(summary: &ListingSummary) -> String {
    let mut facts = vec![
        summary.kind.clone(),
        format!("{} tier", tier(summary.tier)),
        format!("from {}", summary.source),
    ];
    if let Some(framework) = &summary.framework {
        facts.push(format!("framework {framework}"));
    }
    if let Some(language) = &summary.language {
        facts.push(language.clone());
    }
    if let Some(level) = &summary.level {
        facts.push(format!("conformance {level}"));
    }
    facts.join(" · ")
}

pub fn detail(out: &mut dyn Write, detail: &ListingDetail) -> io::Result<()> {
    let summary = &detail.summary;
    let listing = &detail.listing;
    writeln!(out, "{} ({})", summary.title, summary.id)?;
    writeln!(out, "{}", summary.summary)?;
    writeln!(out, "{}", facts(summary))?;
    if let Some(version) = listing["version"].as_str() {
        writeln!(out, "Version:    {version}")?;
    }
    if let Some(publisher) = listing["publisher"]["name"].as_str() {
        writeln!(out, "Publisher:  {publisher}")?;
    }
    if let Some(licence) = listing["licence"].as_str() {
        writeln!(out, "Licence:    {licence}")?;
    }
    if let Some(repository) = listing["source"]["repository"].as_str() {
        writeln!(out, "Source:     {repository}")?;
    }
    v2_fields(out, detail)?;
    let tools: Vec<String> = listing["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some(format!("{} ({})", t["name"].as_str()?, t["risk"].as_str()?)))
        .collect();
    if !tools.is_empty() {
        writeln!(out, "Tools:      {}", tools.join(", "))?;
    }
    writeln!(out, "Versions:")?;
    for version in &detail.versions {
        let mut line = format!("  {}", version.version);
        if summary.latest.as_deref() == Some(version.version.as_str()) {
            line.push_str("  latest");
        }
        if let Some(why) = &version.yanked {
            line.push_str(&format!("  yanked: {why}"));
        }
        writeln!(out, "{line}")?;
        if let Some(r) = &version.revocation {
            revocation(out, r)?;
        }
    }
    writeln!(
        out,
        "See its permissions with `loams marketplace install {} --dry-run`.",
        summary.id
    )
}

fn storage(kind: &str) -> &str {
    match kind {
        "object" => "object storage",
        "sql-mysql" => "MySQL",
        "sql-postgres" => "Postgres",
        "kafka" => "Kafka",
        "kv" => "key-value storage",
        other => other,
    }
}

fn package(package: &Value) -> Option<String> {
    let kind = package["type"].as_str()?;
    let what = match kind {
        "oci" => format!(
            "{}@{}",
            package["image"].as_str()?,
            package["digest"].as_str()?
        ),
        "helm" => format!(
            "{} {}",
            package["chart"].as_str()?,
            package["version"].as_str()?
        ),
        "kustomize" | "wasm" => package["url"].as_str()?.to_owned(),
        "binary" => format!(
            "{} targets",
            package["targets"].as_array().map_or(0, Vec::len)
        ),
        _ => return None,
    };
    Some(format!("{kind} {what}"))
}

fn needs(requires: &Value) -> Vec<String> {
    let mut needs: Vec<String> = strings(&requires["storage"])
        .iter()
        .map(|kind| storage(kind).to_owned())
        .collect();
    if let Some(cpu) = requires["cpu_millis"].as_u64() {
        needs.push(format!("{cpu}m CPU"));
    }
    if let Some(memory) = requires["memory_mib"].as_u64() {
        needs.push(format!("{memory} MiB"));
    }
    if let Some(count) = requires["gpu"]["count"].as_u64() {
        let vendor = requires["gpu"]["vendor"].as_str().unwrap_or("any");
        needs.push(format!("{count} GPU ({vendor})"));
    }
    needs
}

/// A tier as a name: `Community`, `Verified`, `First-party`.
fn tier_name(tier: ListingTier) -> &'static str {
    match tier {
        ListingTier::FirstParty => "First-party",
        ListingTier::Verified => "Verified",
        ListingTier::Community => "Community",
    }
}

/// What listing v2 adds, each line only when the listing has it: where it
/// runs, its package, what it needs, what it depends on, and the tier of
/// the version shown.
fn v2_fields(out: &mut dyn Write, detail: &ListingDetail) -> io::Result<()> {
    let listing = &detail.listing;
    if !detail.summary.targets.is_empty() {
        writeln!(out, "Targets:    {}", detail.summary.targets.join(", "))?;
    }
    if let Some(package) = package(&listing["package"]) {
        writeln!(out, "Package:    {package}")?;
    }
    let needs = needs(&listing["requires"]);
    if !needs.is_empty() {
        writeln!(out, "Needs:      {}", needs.join(", "))?;
    }
    let depends: Vec<String> = listing["depends_on"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| Some(format!("{} {}", d["id"].as_str()?, d["version"].as_str()?)))
        .collect();
    if !depends.is_empty() {
        writeln!(out, "Depends on: {}", depends.join(", "))?;
    }
    let shown = listing["version"].as_str();
    let tier = detail
        .versions
        .iter()
        .find(|v| Some(v.version.as_str()) == shown)
        .and_then(|v| v.tier);
    // Only an index v2 sets a version's tier.
    if let Some(tier) = tier {
        writeln!(out, "Tier:       {}", tier_name(tier))?;
    }
    Ok(())
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn models(models: &Value) -> Option<String> {
    if models.is_null() {
        return None;
    }
    let mut parts = Vec::new();
    let api = strings(&models["api"]);
    if !api.is_empty() {
        parts.push(format!("over {}", api.join(", ")));
    }
    let requires = &models["requires"];
    let mut needs = Vec::new();
    if requires["tools"].as_bool() == Some(true) {
        needs.push("tool calls".to_owned());
    }
    if requires["vision"].as_bool() == Some(true) {
        needs.push("vision".to_owned());
    }
    if let Some(min) = requires["context_min"].as_u64() {
        needs.push(format!("a context of at least {min} tokens"));
    }
    if !needs.is_empty() {
        parts.push(format!("needing {}", needs.join(", ")));
    }
    if let (Some(provider), Some(model)) = (
        models["suggested"]["provider"].as_str(),
        models["suggested"]["model"].as_str(),
    ) {
        parts.push(format!("suggested {provider}/{model}"));
    }
    Some(parts.join("; "))
}

fn budgets(cost: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = cost["typical_tokens_per_run"].as_u64() {
        parts.push(format!("about {n} tokens a run"));
    }
    if let Some(n) = cost["max_tokens_per_run"].as_u64() {
        parts.push(format!("at most {n} tokens a run"));
    }
    if let Some(n) = cost["typical_runs_per_day"].as_u64() {
        parts.push(format!("about {n} runs a day"));
    }
    if let Some(usd) = cost["budget_usd_per_run"].as_f64() {
        parts.push(format!("${usd:.2} a run"));
    }
    if let Some(usd) = cost["budget_usd_per_day"].as_f64() {
        parts.push(format!("${usd:.2} a day"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The consent screen (§73 §7.2) as text.
pub fn plan(out: &mut dyn Write, plan: &InstallPlan) -> io::Result<()> {
    let summary = &plan.summary;
    writeln!(
        out,
        "{} {} ({}, {})",
        summary.title, plan.version, summary.kind, summary.id
    )?;
    writeln!(out, "{}", facts(summary))?;
    if !plan.licence.is_empty() {
        writeln!(out, "Licence:     {}", plan.licence)?;
    }
    let provenance = match (
        plan.provenance["type"].as_str(),
        plan.provenance["ref"].as_str(),
    ) {
        (Some(kind), Some(at)) => format!("{kind}, checked against {at}"),
        _ => "none (no package is downloaded)".to_owned(),
    };
    writeln!(out, "Provenance:  {provenance}")?;
    if !plan.source_repository.is_empty() {
        writeln!(out, "Built from:  {}", plan.source_repository)?;
    }
    match plan.version_age_days {
        Some(days) => writeln!(out, "Published:   {days} days ago")?,
        None => writeln!(out, "Published:   unknown (the index has no date)")?,
    }
    if let Some(r) = &summary.revocation {
        revocation(out, r)?;
    }

    writeln!(out, "\nIt can:")?;
    if plan.tools.is_empty() {
        writeln!(out, "  Call no tools.")?;
    } else {
        writeln!(out, "  Call these tools, most dangerous first:")?;
        let rows: Vec<Vec<String>> = plan
            .tools
            .iter()
            .map(|tool| {
                let mut what = tool.description.clone();
                if let Some(server) = &tool.mcp_server {
                    what.push_str(&format!(" (from {server})"));
                }
                if tool.always_asks {
                    what.push_str(" — always asks you first");
                }
                vec![
                    format!("    {}", tool.risk.to_uppercase()),
                    tool.name.clone(),
                    what,
                ]
            })
            .collect();
        table(out, &rows)?;
    }
    let place = &plan.place;
    let data: Vec<String> = place["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(format!(
                "{} the {} {}",
                d["access"].as_str()?,
                d["kind"].as_str()?,
                d["name"].as_str()?
            ))
        })
        .collect();
    if !data.is_empty() {
        writeln!(out, "  Use your data: {}.", data.join("; "))?;
    }
    if !plan.egress.is_empty() {
        writeln!(out, "  Reach these hosts: {}.", plan.egress.join(", "))?;
    }
    let roles = strings(&place["roles"]);
    if !roles.is_empty() {
        writeln!(out, "  Act with these Loams roles: {}.", roles.join(", "))?;
    }
    for dependency in &plan.dependencies {
        let how = if dependency.installed {
            "already installed, reused"
        } else {
            "installed with it"
        };
        writeln!(
            out,
            "  Use the MCP server {} {} ({how}).",
            dependency.id, dependency.version
        )?;
    }
    if let Some(models) = models(&plan.models) {
        writeln!(out, "  Use models through agentd's model proxy {models}.")?;
    }
    if let Some(budgets) = budgets(&plan.cost) {
        writeln!(out, "  Spend, as proposed: {budgets}.")?;
    }

    writeln!(out, "\nIt can't:")?;
    for line in &plan.cannot {
        writeln!(out, "  {line}")?;
    }

    if !plan.secrets.is_empty() {
        writeln!(
            out,
            "\nYou supply these secrets (kept in your OS keyring, never in a file):"
        )?;
        for secret in &plan.secrets {
            let required = if secret.required {
                "required"
            } else {
                "optional"
            };
            writeln!(
                out,
                "  {} ({required}): {}",
                secret.name, secret.description
            )?;
        }
    }
    writeln!(out, "\nPlan digest: {}", plan.digest)
}

pub fn installed_note(out: &mut dyn Write, installed: &InstalledListing) -> io::Result<()> {
    writeln!(
        out,
        "Installed {} {} from {}.",
        installed.id, installed.version, installed.source
    )?;
    match installed.kind.as_str() {
        "harness" => writeln!(out, "Pick it in Studio's harness list."),
        "mcp-server" => writeln!(out, "Threads get its tools from their next run."),
        _ => Ok(()),
    }
}

/// `YYYY-MM-DD` (UTC) of Unix milliseconds.
pub fn date(ms: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn installed(out: &mut dyn Write, list: &InstalledListingList) -> io::Result<()> {
    if list.installed.is_empty() {
        return writeln!(out, "Nothing is installed.");
    }
    let rows: Vec<Vec<String>> = list
        .installed
        .iter()
        .map(|installed| {
            vec![
                installed.id.clone(),
                installed.version.clone(),
                installed.kind.clone(),
                format!("from {}", installed.source),
                format!("installed {}", date(installed.installed_ms)),
                installed
                    .update
                    .as_ref()
                    .map(|update| format!("update: {update}"))
                    .unwrap_or_default(),
            ]
        })
        .collect();
    let widths = widths(&rows);
    for (installed, cells) in list.installed.iter().zip(&rows) {
        row(out, cells, &widths)?;
        if let Some(r) = &installed.revocation {
            revocation(out, r)?;
        }
    }
    Ok(())
}

pub fn updates(out: &mut dyn Write, updates: &AvailableUpdateList) -> io::Result<()> {
    if updates.updates.is_empty() {
        return writeln!(out, "Everything is up to date.");
    }
    for update in &updates.updates {
        let note = if update.widens {
            "  asks for more: you confirm its new plan"
        } else {
            ""
        };
        writeln!(out, "{} {} → {}{note}", update.id, update.from, update.to)?;
    }
    writeln!(out, "Update one with `loams marketplace update <id>`.")
}

pub fn sources(out: &mut dyn Write, list: &MarketplaceSourceList) -> io::Result<()> {
    for source in &list.sources {
        let refreshed = if source.refreshed_ms > 0 {
            format!("refreshed {}", date(source.refreshed_ms))
        } else {
            "never refreshed".to_owned()
        };
        let name = if source.official {
            format!("{} (official)", source.name)
        } else {
            source.name.clone()
        };
        table(
            out,
            &[vec![
                name,
                source.url.clone(),
                format!("{} listings", source.listings),
                refreshed,
            ]],
        )?;
        writeln!(out, "  signed by {} ({})", source.identity, source.issuer)?;
        if let Some(error) = &source.error {
            writeln!(out, "  ! last refresh failed: {error}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use loams_agentd_proto::marketplace::ListingVersion;

    use super::*;

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400_000), "2000-02-29");
        assert_eq!(date(1_792_972_800_000), "2026-10-26");
    }

    fn app_detail(listing: Value, tier: Option<ListingTier>) -> ListingDetail {
        ListingDetail {
            summary: ListingSummary {
                id: "acme/notes-app".into(),
                source: "official".into(),
                kind: "app".into(),
                title: "Notes".into(),
                summary: "Notes".into(),
                tier: ListingTier::Community,
                latest: Some("1.2.0".into()),
                framework: None,
                language: None,
                level: None,
                revocation: None,
                targets: vec!["cluster".into()],
            },
            listing,
            versions: vec![ListingVersion {
                version: "1.2.0".into(),
                yanked: None,
                revocation: None,
                tier,
            }],
        }
    }

    fn render(detail: &ListingDetail) -> String {
        let mut out = Vec::new();
        super::detail(&mut out, detail).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn info_renders_v2_fields() {
        let listing: Value = serde_json::from_str(include_str!(
            "../../../../schemas/marketplace/testdata/v2/valid/app-oci.json"
        ))
        .unwrap();
        let text = render(&app_detail(listing, Some(ListingTier::Community)));
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| {
                ["Targets:", "Package:", "Needs:", "Depends on:", "Tier:"]
                    .iter()
                    .any(|p| l.starts_with(p))
            })
            .collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[0].ends_with("Targets:    cluster"), "{text}");
        assert!(
            lines[1].starts_with("Package:    oci ghcr.io/acme/notes@sha256:"),
            "{text}"
        );
        assert!(
            lines[2].ends_with("Needs:      object storage, 500m CPU, 512 MiB"),
            "{text}"
        );
        assert!(lines[3].ends_with("Depends on: acme/pg ^16"), "{text}");
        assert!(lines[4].ends_with("Tier:       Community"), "{text}");
    }

    #[test]
    fn v1_listings_print_no_v2_lines() {
        let listing = serde_json::json!({ "version": "1.2.0", "licence": "MIT" });
        let mut detail = app_detail(listing, None);
        detail.summary.targets.clear();
        let text = render(&detail);
        for label in ["Targets:", "Package:", "Needs:", "Depends on:", "Tier:"] {
            assert!(!text.contains(label), "{text}");
        }
    }
}
