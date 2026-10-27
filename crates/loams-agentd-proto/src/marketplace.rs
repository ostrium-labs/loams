//! The Seed Exchange, the marketplace in agentd (design §73 §5–§7, D1462,
//! D1465, D1466; plan MK1 Tasks 4 and 4b): `MarketplaceService`'s ndjson
//! twins.
//!
//! - Sources: `ListMarketplaceSources`, `AddMarketplaceSource`,
//!   `RemoveMarketplaceSource`.
//! - Listings: `SearchListings`, `GetListing`.
//! - Installs: `PlanInstall` (creates nothing), `InstallListing` (exactly the
//!   confirmed plan, by its digest; `plan_changed` otherwise),
//!   `UninstallListing` (the agent's data stays), `ListInstalledListings`,
//!   `CheckListingUpdates` and `UpdateListing` (`consent_required` when the
//!   update widens a permission and no new digest is given).
//!
//! Listing fields keep the listing schema's strings: a kind is `agent`,
//! `harness` or `mcp-server`, a framework `mastra`, a language `typescript`,
//! a level `core`. Listing parts (the listing itself, its Place, models,
//! cost and provenance) are the listing's JSON. None of these is open to the
//! `mcp` role.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The daemon feature these methods add to `Hello` and `GetHost`.
pub const FEATURE: &str = "marketplace";

/// Error reason: `InstallListing` with a digest that is not the plan's now.
pub const PLAN_CHANGED: &str = "plan_changed";
/// Error reason: `UpdateListing` to a version that widens a permission,
/// without the new plan's digest.
pub const CONSENT_REQUIRED: &str = "consent_required";
/// Error reason: a yanked version, or one a `disable` revocation covers.
pub const LISTING_REVOKED: &str = "listing_revoked";
/// Error reason: an index, signature, integrity or provenance check failed,
/// or a package runs code at install.
pub const UNTRUSTED_LISTING: &str = "untrusted_listing";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSource {
    /// A short local name (`official` for the Seed Exchange).
    pub name: String,
    /// The index URL (`…/index.json`).
    pub url: String,
    /// The certificate identity the index must be signed by.
    pub identity: String,
    pub issuer: String,
    pub official: bool,
    /// When the index was last fetched and verified, or 0.
    pub refreshed_ms: i64,
    /// Why the last refresh failed.
    #[serde(default)]
    pub error: Option<String>,
    pub listings: u32,
}

/// `ListMarketplaceSources`, in search order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSourceList {
    pub sources: Vec<MarketplaceSource>,
}

/// `AddMarketplaceSource`: kept only once its index verifies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddMarketplaceSourceParams {
    pub name: String,
    pub url: String,
    pub identity: String,
    /// Default GitHub Actions' issuer.
    #[serde(default)]
    pub issuer: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoveMarketplaceSourceParams {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ListingTier {
    FirstParty,
    Verified,
    Community,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RevocationSeverity {
    /// A banner.
    Warn,
    /// The agent is stopped and its runs blocked.
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingRevocation {
    pub severity: RevocationSeverity,
    pub reason: String,
    /// The revoked range, in Cargo's `VersionReq` syntax.
    pub versions: String,
    #[serde(default)]
    pub advisory: Option<String>,
}

/// `SearchListings`: every filter that is set must match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchListingsParams {
    /// Words matched against id, title, summary and tool names.
    #[serde(default)]
    pub query: String,
    /// `agent`, `harness`, `mcp-server`.
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    /// `core`, `compatible` or `native`.
    #[serde(default)]
    pub min_level: Option<String>,
    #[serde(default)]
    pub tiers: Vec<ListingTier>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingSummary {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub tier: ListingTier,
    /// The version a new install gets.
    #[serde(default)]
    pub latest: Option<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    /// The latest version's revocation.
    #[serde(default)]
    pub revocation: Option<ListingRevocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingSearchResults {
    pub results: Vec<ListingSummary>,
}

/// `GetListing` and `PlanInstall`: a version (default the latest) from a
/// source (default the first that has the listing).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingRef {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingVersion {
    pub version: String,
    /// Why it was yanked.
    #[serde(default)]
    pub yanked: Option<String>,
    #[serde(default)]
    pub revocation: Option<ListingRevocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListingDetail {
    pub summary: ListingSummary,
    /// The listing at the asked version, as schema v1 JSON.
    #[ts(type = "Record<string, unknown>")]
    pub listing: serde_json::Value,
    /// Every version, ascending.
    pub versions: Vec<ListingVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanTool {
    pub name: String,
    pub description: String,
    /// `read`, `write`, `egress` or `destructive`.
    pub risk: String,
    /// Asks the person every time, whatever the thread's mode.
    pub always_asks: bool,
    #[serde(default)]
    pub mcp_server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanSecret {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanDependency {
    pub id: String,
    /// The version range the listing asks for.
    pub version: String,
    /// Already installed in range, so it is reused.
    pub installed: bool,
}

/// The consent plan (D1466).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    /// Binds `InstallListing` to exactly this plan.
    pub digest: String,
    pub summary: ListingSummary,
    pub version: String,
    /// The listing's `place`.
    #[ts(type = "Record<string, unknown> | null")]
    pub place: serde_json::Value,
    /// Most dangerous first.
    pub tools: Vec<PlanTool>,
    pub egress: Vec<String>,
    pub dependencies: Vec<PlanDependency>,
    pub secrets: Vec<PlanSecret>,
    #[ts(type = "Record<string, unknown> | null")]
    pub models: serde_json::Value,
    #[ts(type = "Record<string, unknown> | null")]
    pub cost: serde_json::Value,
    pub licence: String,
    #[ts(type = "Record<string, unknown> | null")]
    pub provenance: serde_json::Value,
    pub source_repository: String,
    /// Days since the version was published, when the index says.
    #[serde(default)]
    pub version_age_days: Option<i32>,
    /// What the agent is not allowed, in plain words.
    pub cannot: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanInstallReply {
    pub plan: InstallPlan,
}

/// `InstallListing`. `secrets` go to agentd's vault only; `Debug` lists
/// their names, never their values.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstallListingParams {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// The digest of the plan the person confirmed.
    pub plan_digest: String,
    /// Non-secret configuration, by input name.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl std::fmt::Debug for InstallListingParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallListingParams")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("source", &self.source)
            .field("plan_digest", &self.plan_digest)
            .field("config", &self.config)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstalledListing {
    pub id: String,
    pub version: String,
    pub source: String,
    pub kind: String,
    pub installed_ms: i64,
    #[serde(default)]
    pub revocation: Option<ListingRevocation>,
    /// A newer version.
    #[serde(default)]
    pub update: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstalledListingReply {
    pub installed: InstalledListing,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallListingParams {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstalledListingList {
    pub installed: Vec<InstalledListing>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub id: String,
    pub from: String,
    pub to: String,
    /// The new version asks for more, so it needs consent again.
    pub widens: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdateList {
    pub updates: Vec<AvailableUpdate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateListingParams {
    pub id: String,
    /// Default the latest.
    #[serde(default)]
    pub version: Option<String>,
    /// Required when the update widens a permission.
    #[serde(default)]
    pub plan_digest: Option<String>,
}
