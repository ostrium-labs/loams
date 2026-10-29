//! `MarketplaceService` (MK1 Task 4b): each method's ndjson twin is named in
//! `twins.rs` (`ListSources` runs `ListMarketplaceSources`, `Install` runs
//! `InstallListing`, …).

use connectrpc::{RequestContext, ServiceRequest, ServiceResult};

use super::{Agentd, unary};
use crate::map::Wire;
use crate::map::marketplace;
use crate::proto::loams::agentd::v1 as pb;

impl pb::MarketplaceService for Agentd {
    async fn list_sources(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListSourcesRequest>,
    ) -> ServiceResult<pb::ListSourcesResponse> {
        unary!(
            self,
            request,
            ListMarketplaceSources,
            |_req| marketplace::empty(),
            |list| list.to_pb()
        )
    }

    async fn add_source(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::AddSourceRequest>,
    ) -> ServiceResult<pb::AddSourceResponse> {
        unary!(
            self,
            request,
            AddMarketplaceSource,
            |req| Wire::from_pb(req)?,
            |source| pb::AddSourceResponse {
                source: crate::map::msg(&source),
                ..Default::default()
            }
        )
    }

    async fn remove_source(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::RemoveSourceRequest>,
    ) -> ServiceResult<pb::RemoveSourceResponse> {
        unary!(
            self,
            request,
            RemoveMarketplaceSource,
            |req| Wire::from_pb(req)?,
            |_ok| pb::RemoveSourceResponse::default()
        )
    }

    async fn search_listings(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::SearchListingsRequest>,
    ) -> ServiceResult<pb::SearchListingsResponse> {
        unary!(
            self,
            request,
            SearchListings,
            |req| Wire::from_pb(req)?,
            |results| results.to_pb()
        )
    }

    async fn get_listing(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetListingRequest>,
    ) -> ServiceResult<pb::GetListingResponse> {
        unary!(
            self,
            request,
            GetListing,
            |req| Wire::from_pb(req)?,
            |detail| detail.to_pb()
        )
    }

    async fn plan_install(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::PlanInstallRequest>,
    ) -> ServiceResult<pb::PlanInstallResponse> {
        unary!(
            self,
            request,
            PlanInstall,
            |req| marketplace::listing_ref_from_plan(req),
            |plan| plan.to_pb()
        )
    }

    async fn install(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::InstallRequest>,
    ) -> ServiceResult<pb::InstallResponse> {
        unary!(
            self,
            request,
            InstallListing,
            |req| Wire::from_pb(req)?,
            |installed| installed.to_pb()
        )
    }

    async fn uninstall(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::UninstallRequest>,
    ) -> ServiceResult<pb::UninstallResponse> {
        unary!(
            self,
            request,
            UninstallListing,
            |req| Wire::from_pb(req)?,
            |_ok| pb::UninstallResponse::default()
        )
    }

    async fn list_installed(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListInstalledRequest>,
    ) -> ServiceResult<pb::ListInstalledResponse> {
        unary!(
            self,
            request,
            ListInstalledListings,
            |_req| marketplace::empty(),
            |list| list.to_pb()
        )
    }

    async fn check_updates(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::CheckUpdatesRequest>,
    ) -> ServiceResult<pb::CheckUpdatesResponse> {
        unary!(
            self,
            request,
            CheckListingUpdates,
            |_req| marketplace::empty(),
            |updates| updates.to_pb()
        )
    }

    async fn update(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateRequest>,
    ) -> ServiceResult<pb::UpdateResponse> {
        unary!(
            self,
            request,
            UpdateListing,
            |req| Wire::from_pb(req)?,
            |updated| marketplace::update_response_of(&updated)
        )
    }
}
