//! `FactoryService` (SF1 amendment F2; design §50 §13): the factory's reads, and
//! configure and remove, which are Electron main's alone (F4): the renderer's
//! allowlist leaves them out (`renderer: false`) and the remote Studio role may not
//! call them (`roles::STUDIO_EXCLUDED`).

use connectrpc::{RequestContext, ServiceRequest, ServiceResult};
use loams_agentd_proto as rs;
use loams_agentd_proto::factory as rf;

use super::{Agentd, unary};
use crate::map::MapError;
use crate::proto::loams::agentd::v1 as pb;

fn health(h: rf::FactoryHealth) -> buffa::EnumValue<pb::FactoryHealth> {
    buffa::EnumValue::Known(match h {
        rf::FactoryHealth::Ok => pb::FactoryHealth::FACTORY_HEALTH_OK,
        rf::FactoryHealth::AuthFailed => pb::FactoryHealth::FACTORY_HEALTH_AUTH_FAILED,
        rf::FactoryHealth::Unreachable => pb::FactoryHealth::FACTORY_HEALTH_UNREACHABLE,
        rf::FactoryHealth::Unconfigured => pb::FactoryHealth::FACTORY_HEALTH_UNCONFIGURED,
        rf::FactoryHealth::Locked => pb::FactoryHealth::FACTORY_HEALTH_LOCKED,
    })
}

fn app(a: &rf::FactoryAppInfo) -> pb::FactoryApp {
    pb::FactoryApp {
        id: a.id.clone(),
        label: a.label.clone(),
        url: a.url.clone(),
        health: health(a.health),
        has_panels: a.has_panels,
        preview: a.preview.clone(),
        credential_fields: a
            .credential_fields
            .iter()
            .map(|f| pb::FactoryCredentialField {
                key: f.key.clone(),
                label: f.label.clone(),
                secret: f.secret,
                ..Default::default()
            })
            .collect(),
        fields: a
            .fields
            .iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        persistent: a.persistent,
        ..Default::default()
    }
}

fn error(e: &rf::FactoryError) -> pb::FactoryError {
    pb::FactoryError {
        code: e.code.clone(),
        message: e.message.clone(),
        ..Default::default()
    }
}

/// `params_json` as the op's params: empty means `{}`.
fn params(json: &str) -> Result<serde_json::Value, MapError> {
    if json.trim().is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(json).map_err(|e| MapError(format!("params_json is not JSON: {e}")))
}

impl pb::FactoryService for Agentd {
    async fn list_factory_apps(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListFactoryAppsRequest>,
    ) -> ServiceResult<pb::ListFactoryAppsResponse> {
        unary!(self, request, FactoryList, |req| rs::Empty {}, |list| {
            pb::ListFactoryAppsResponse {
                apps: list.apps.iter().map(app).collect(),
                ..Default::default()
            }
        })
    }

    async fn test_factory_app(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::TestFactoryAppRequest>,
    ) -> ServiceResult<pb::TestFactoryAppResponse> {
        unary!(
            self,
            request,
            FactoryTest,
            |req| rf::FactoryAppParams { app: req.app },
            |reply| pb::TestFactoryAppResponse {
                result: match (&reply.app, &reply.error) {
                    (_, Some(e)) => Some(error(e).into()),
                    (Some(a), None) => Some(app(a).into()),
                    (None, None) => None,
                },
                ..Default::default()
            }
        )
    }

    async fn query_factory(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::QueryFactoryRequest>,
    ) -> ServiceResult<pb::QueryFactoryResponse> {
        unary!(
            self,
            request,
            FactoryQuery,
            |req| rf::FactoryQueryParams {
                app: req.app,
                op: req.op,
                params: params(&req.params_json)?,
            },
            |reply| pb::QueryFactoryResponse {
                result: match (&reply.value, &reply.error) {
                    (_, Some(e)) => Some(error(e).into()),
                    (Some(v), None) => Some(
                        pb::__buffa::oneof::query_factory_response::Result::ValueJson(
                            v.to_string(),
                        )
                    ),
                    (None, None) => None,
                },
                ..Default::default()
            }
        )
    }

    async fn configure_factory_app(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ConfigureFactoryAppRequest>,
    ) -> ServiceResult<pb::ConfigureFactoryAppResponse> {
        unary!(
            self,
            request,
            FactoryConfigure,
            |req| rf::FactoryConfigureParams {
                app: req.app,
                url: req.url,
                fields: req.fields.into_iter().collect(),
            },
            |reply| pb::ConfigureFactoryAppResponse {
                result: match (&reply.app, &reply.error) {
                    (_, Some(e)) => Some(error(e).into()),
                    (Some(a), None) => Some(app(a).into()),
                    (None, None) => None,
                },
                ..Default::default()
            }
        )
    }

    async fn remove_factory_app(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::RemoveFactoryAppRequest>,
    ) -> ServiceResult<pb::RemoveFactoryAppResponse> {
        unary!(
            self,
            request,
            FactoryRemove,
            |req| rf::FactoryAppParams { app: req.app },
            |_done| pb::RemoveFactoryAppResponse::default()
        )
    }
}
