//! The storage controller's tenant creation (`libs/pageserver_api/src/
//! controller_api.rs` `TenantCreateRequest`). Every other call uses the
//! pageserver's routes, which the controller serves too
//! (`storage_controller/src/http.rs`): timeline create and delete, tenant
//! config, and a proxy for the `GET /v1/tenant/{t}/...` reads.

use serde::Serialize;

use crate::TenantId;
use crate::pageserver::TenantConfig;

/// `TenantCreateRequest` for an unsharded tenant: the controller chooses the
/// generation and the placement.
#[derive(Serialize)]
pub(crate) struct TenantCreateRequest<'a> {
    pub(crate) new_tenant_id: &'a TenantId,
    #[serde(flatten)]
    pub(crate) config: &'a TenantConfig,
}
