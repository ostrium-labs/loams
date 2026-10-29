//! `EngineRpc`'s marketplace methods (design §73 §5–§7; plan MK1 Task 4b):
//! `MarketplaceService`'s ndjson twins over the daemon's
//! [`MarketplaceHost`]. An error answers with its reason first
//! (`plan_changed: …`, `not_found: …`), as every agentd error does.

use loams_agentd_marketplace::index::{Kind, Level, Tier};
use loams_agentd_marketplace::market::{ListingView, Query, SourceInfo, Summary};
use loams_agentd_marketplace::revocations::{Revocation, Severity};
use loams_agentd_marketplace::service::{InstallRequest, ServiceError};
use loams_agentd_marketplace::{MarketplaceHost, Source};
use loams_agentd_proto::OkReply;
use loams_agentd_proto::marketplace as wire;
use loams_agentd_proto::rpc::{calls, methods};
use loams_agentd_rpc::{RpcError, RpcReply, params_of, reply};

use super::EngineRpc;

fn failed(error: &ServiceError) -> RpcError {
    RpcError::Failed(error.with_reason())
}

fn bad(message: impl Into<String>) -> RpcError {
    RpcError::BadParams(message.into())
}

fn source_of(info: SourceInfo) -> wire::MarketplaceSource {
    wire::MarketplaceSource {
        name: info.source.name,
        url: info.source.url,
        identity: info.source.identity,
        issuer: info.source.issuer,
        official: info.source.official,
        refreshed_ms: info.refreshed_ms,
        error: info.error,
        listings: u32::try_from(info.listings).unwrap_or(u32::MAX),
    }
}

fn tier_of(tier: Tier) -> wire::ListingTier {
    match tier {
        Tier::FirstParty => wire::ListingTier::FirstParty,
        Tier::Verified => wire::ListingTier::Verified,
        Tier::Community => wire::ListingTier::Community,
    }
}

fn tier_from(tier: wire::ListingTier) -> Tier {
    match tier {
        wire::ListingTier::FirstParty => Tier::FirstParty,
        wire::ListingTier::Verified => Tier::Verified,
        wire::ListingTier::Community => Tier::Community,
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Core => "core",
        Level::Compatible => "compatible",
        Level::Native => "native",
    }
}

fn level_from(name: &str) -> Result<Level, RpcError> {
    match name {
        "core" => Ok(Level::Core),
        "compatible" => Ok(Level::Compatible),
        "native" => Ok(Level::Native),
        other => Err(bad(format!("no conformance level {other}"))),
    }
}

fn kind_from(name: &str) -> Result<Kind, RpcError> {
    match name {
        "agent" => Ok(Kind::Agent),
        "harness" => Ok(Kind::Harness),
        "mcp-server" => Ok(Kind::McpServer),
        other => Err(bad(format!("no listing kind {other}"))),
    }
}

fn revocation_of(revocation: &Revocation) -> wire::ListingRevocation {
    wire::ListingRevocation {
        severity: match revocation.severity {
            Severity::Warn => wire::RevocationSeverity::Warn,
            Severity::Disable => wire::RevocationSeverity::Disable,
        },
        reason: revocation.reason.clone(),
        versions: revocation.versions.clone(),
        advisory: revocation.advisory.clone(),
    }
}

fn summary_of(summary: Summary) -> wire::ListingSummary {
    wire::ListingSummary {
        id: summary.id,
        source: summary.source,
        kind: summary.kind.as_str().into(),
        title: summary.title,
        summary: summary.summary,
        tier: tier_of(summary.tier),
        latest: summary.latest,
        framework: summary.framework,
        language: summary.language,
        level: summary.level.map(|l| level_name(l).to_owned()),
        revocation: summary.revocation.as_ref().map(revocation_of),
    }
}

fn detail_of(view: ListingView) -> wire::ListingDetail {
    wire::ListingDetail {
        listing: view.version.listing.clone(),
        versions: view
            .versions
            .iter()
            .map(|(version, revocation)| wire::ListingVersion {
                version: version.version.clone(),
                yanked: version.yanked.clone(),
                revocation: revocation.as_ref().map(revocation_of),
            })
            .collect(),
        summary: summary_of(view.summary),
    }
}

fn installed_of(
    installed: loams_agentd_marketplace::install::Installed,
    revocation: Option<&Revocation>,
    update: Option<String>,
) -> wire::InstalledListing {
    wire::InstalledListing {
        id: installed.id,
        version: installed.version,
        source: installed.source,
        kind: installed.kind,
        installed_ms: installed.installed_ms,
        revocation: revocation.map(revocation_of),
        update,
    }
}

fn some(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

impl EngineRpc {
    fn market(&self) -> Result<&MarketplaceHost, RpcError> {
        self.marketplace.get().ok_or_else(|| {
            RpcError::Failed("engine_not_ready: the marketplace is not running".into())
        })
    }

    /// Re-registers the installs' harnesses now, so a `ListHarnesses` right
    /// after an install, uninstall or update already shows the change.
    fn marketplace_changed(&self, host: &MarketplaceHost) {
        crate::marketplace::sync(host, &self.registry);
    }

    async fn installed_view(
        &self,
        host: &MarketplaceHost,
        id: &str,
    ) -> Result<wire::InstalledListing, RpcError> {
        host.list_installed()
            .await
            .into_iter()
            .find(|view| view.installed.id == id)
            .map(|view| installed_of(view.installed, view.revocation.as_ref(), view.update))
            .ok_or_else(|| RpcError::Failed(format!("not_found: {id} is not installed")))
    }

    pub(super) async fn handle_marketplace(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let host = self.market()?;
        match method {
            methods::LIST_MARKETPLACE_SOURCES => {
                let sources = host
                    .list_sources()
                    .await
                    .into_iter()
                    .map(source_of)
                    .collect();
                reply::<calls::ListMarketplaceSources, _>(&wire::MarketplaceSourceList { sources })
            }
            methods::ADD_MARKETPLACE_SOURCE => {
                let p = params_of::<calls::AddMarketplaceSource>(params)?;
                let added = host
                    .add_source(Source {
                        name: p.name,
                        url: p.url,
                        identity: p.identity,
                        issuer: p.issuer.unwrap_or_default(),
                        official: false,
                    })
                    .await
                    .map_err(|e| failed(&e))?;
                reply::<calls::AddMarketplaceSource, _>(&source_of(added))
            }
            methods::REMOVE_MARKETPLACE_SOURCE => {
                let p = params_of::<calls::RemoveMarketplaceSource>(params)?;
                host.remove_source(&p.name).await.map_err(|e| failed(&e))?;
                reply::<calls::RemoveMarketplaceSource, _>(&OkReply::OK)
            }
            methods::SEARCH_LISTINGS => {
                let p = params_of::<calls::SearchListings>(params)?;
                let query = Query {
                    text: p.query,
                    kinds: p
                        .kinds
                        .iter()
                        .map(|k| kind_from(k))
                        .collect::<Result<_, _>>()?,
                    frameworks: p.frameworks,
                    languages: p.languages,
                    min_level: p.min_level.as_deref().map(level_from).transpose()?,
                    tiers: p.tiers.into_iter().map(tier_from).collect(),
                    source: p.source.and_then(some),
                };
                let results = host
                    .search(&query)
                    .await
                    .into_iter()
                    .map(summary_of)
                    .collect();
                reply::<calls::SearchListings, _>(&wire::ListingSearchResults { results })
            }
            methods::GET_LISTING => {
                let p = params_of::<calls::GetListing>(params)?;
                let view = host
                    .get_listing(
                        &p.id,
                        p.version.and_then(some).as_deref(),
                        p.source.and_then(some).as_deref(),
                    )
                    .await
                    .map_err(|e| failed(&e))?;
                reply::<calls::GetListing, _>(&detail_of(view))
            }
            methods::PLAN_INSTALL => {
                let p = params_of::<calls::PlanInstall>(params)?;
                let (plan, digest, view) = host
                    .plan_install(
                        &p.id,
                        p.version.and_then(some).as_deref(),
                        p.source.and_then(some).as_deref(),
                    )
                    .await
                    .map_err(|e| failed(&e))?;
                let listing = &view.version.listing;
                let plan = wire::InstallPlan {
                    digest,
                    summary: summary_of(view.summary.clone()),
                    version: plan.version,
                    place: listing.get("place").cloned().unwrap_or_default(),
                    tools: plan
                        .tools
                        .into_iter()
                        .map(|t| wire::PlanTool {
                            name: t.name,
                            description: t.description,
                            risk: serde_json::to_value(t.risk)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default(),
                            always_asks: t.always_asks,
                            mcp_server: t.mcp_server,
                        })
                        .collect(),
                    egress: plan.egress,
                    dependencies: plan
                        .dependencies
                        .into_iter()
                        .map(|d| wire::PlanDependency {
                            id: d.id,
                            version: d.version,
                            installed: d.installed,
                        })
                        .collect(),
                    secrets: plan
                        .secrets
                        .into_iter()
                        .map(|s| wire::PlanSecret {
                            name: s.name,
                            description: s.description,
                            required: s.required,
                        })
                        .collect(),
                    models: plan.models,
                    cost: plan.cost,
                    licence: plan.licence,
                    provenance: plan.provenance,
                    source_repository: plan.source_repository,
                    // The index carries no publish date yet (MK1 Task 4b ruling).
                    version_age_days: None,
                    cannot: plan.cannot,
                };
                reply::<calls::PlanInstall, _>(&wire::PlanInstallReply { plan })
            }
            methods::INSTALL_LISTING => {
                let p = params_of::<calls::InstallListing>(params)?;
                let installed = host
                    .install(InstallRequest {
                        id: p.id,
                        version: p.version.and_then(some),
                        source: p.source.and_then(some),
                        plan_digest: p.plan_digest,
                        config: p.config,
                        secrets: p.secrets,
                    })
                    .await
                    .map_err(|e| failed(&e))?;
                self.marketplace_changed(host);
                let installed = self.installed_view(host, &installed.id).await?;
                reply::<calls::InstallListing, _>(&wire::InstalledListingReply { installed })
            }
            methods::UNINSTALL_LISTING => {
                let p = params_of::<calls::UninstallListing>(params)?;
                // Retires the agent's machine user too (D1632).
                host.uninstall_retiring(&p.id)
                    .await
                    .map_err(|e| failed(&e))?;
                self.marketplace_changed(host);
                reply::<calls::UninstallListing, _>(&OkReply::OK)
            }
            methods::LIST_INSTALLED_LISTINGS => {
                let installed = host
                    .list_installed()
                    .await
                    .into_iter()
                    .map(|view| installed_of(view.installed, view.revocation.as_ref(), view.update))
                    .collect();
                reply::<calls::ListInstalledListings, _>(&wire::InstalledListingList { installed })
            }
            methods::CHECK_LISTING_UPDATES => {
                let updates = host
                    .check_updates()
                    .await
                    .into_iter()
                    .map(|u| wire::AvailableUpdate {
                        widens: !u.widens.is_empty(),
                        id: u.id,
                        from: u.from,
                        to: u.to,
                    })
                    .collect();
                reply::<calls::CheckListingUpdates, _>(&wire::AvailableUpdateList { updates })
            }
            methods::UPDATE_LISTING => {
                let p = params_of::<calls::UpdateListing>(params)?;
                let updated = host
                    .update(
                        &p.id,
                        p.version.and_then(some).as_deref(),
                        p.plan_digest.as_deref().unwrap_or_default(),
                    )
                    .await
                    .map_err(|e| failed(&e))?;
                self.marketplace_changed(host);
                let installed = self.installed_view(host, &updated.id).await?;
                reply::<calls::UpdateListing, _>(&wire::InstalledListingReply { installed })
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}
