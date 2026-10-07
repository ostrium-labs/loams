//! `loams.devices.v1.DeviceService`: pairing, the device list, rename, revoke
//! and push targets. Preferences and test notifications are stubs until AP4.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use rand::Rng as _;

use crate::auth::caller;
use crate::not_implemented;
use crate::oauth::{PushRecord, new_pairing};
use crate::proto::loams::devices::v1::{
    CreatePairingRequest, CreatePairingResponse, DeviceService, GetNotificationPreferencesRequest,
    GetNotificationPreferencesResponse, ListDevicesRequest, ListDevicesResponse, PushTargetRef,
    RegisterPushTargetRequest, RegisterPushTargetResponse, RenameDeviceRequest,
    RenameDeviceResponse, RevokeDeviceRequest, RevokeDeviceResponse, SendTestNotificationRequest,
    SendTestNotificationResponse, SetNotificationPreferencesRequest,
    SetNotificationPreferencesResponse, UnregisterPushTargetRequest, UnregisterPushTargetResponse,
};
use crate::seed::ts;
use crate::store::Store;
use buffa::MessageField;

pub(crate) struct Devices(pub(crate) Arc<Store>);

/// The QR payload of §37 §7.2.1, version 1.
///
/// The pairing's code is not in the payload: the QR carries the user code, and
/// `POST /api/v1/oauth/token` exchanges it. That is the safer of the two (a
/// photographed QR only leaks the short, rate-limited code) and it is what the
/// Go phone mock's `user_code` grant path already does.
pub(crate) fn qr_payload(issuer: &str, instance_id: &str, user_code: &str, exp: u64) -> String {
    serde_json::json!({
        "v": 1,
        "kind": "loams-pair",
        "issuer": issuer,
        "instance_id": instance_id,
        // Public CAs in the mock: no pinned SPKI (§37 §7.2.1).
        "spki": null,
        "jkt": "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs",
        "user_code": user_code,
        "exp": exp,
    })
    .to_string()
}

impl DeviceService for Devices {
    async fn create_pairing(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, CreatePairingRequest>,
    ) -> ServiceResult<CreatePairingResponse> {
        let me = caller(&self.0, &ctx)?;
        // The pairing is recorded, so `POST /api/v1/oauth/token` with this
        // code or user code redeems it for real (§37 §7.2.2).
        let instance = &self.0.seed.instance;
        let issuer = crate::oauth::issuer_for(&self.0.public_url, &instance.issuer);
        let (pairing, _code, user_code) = new_pairing(&self.0, &me.principal.id, &issuer);
        let mut rng = rand::rng();
        let exp = pairing
            .expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Response::ok(CreatePairingResponse {
            pairing_id: format!("pair_{}", ulid::Ulid::from(rng.random::<u128>())),
            qr_payload: qr_payload(&issuer, &instance.instance_id, &user_code, exp),
            user_code,
            expires_at: ts(pairing.expires_at),
            ..Default::default()
        })
    }

    async fn list_devices(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListDevicesRequest>,
    ) -> ServiceResult<ListDevicesResponse> {
        let me = caller(&self.0, &ctx)?;
        let include_revoked = request.include_revoked;
        let owner = if request.user_id.is_empty() {
            me.principal.id
        } else {
            request.user_id.to_owned()
        };
        let devices = self
            .0
            .lock()
            .devices
            .values()
            .filter(|(o, d)| *o == owner && (include_revoked || !d.revoked_at.is_set()))
            .map(|(_, d)| d.clone())
            .collect();
        Response::ok(ListDevicesResponse {
            devices,
            ..Default::default()
        })
    }

    async fn rename_device(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RenameDeviceRequest>,
    ) -> ServiceResult<RenameDeviceResponse> {
        caller(&self.0, &ctx)?;
        let request = request.to_owned_message();
        let mut state = self.0.lock();
        let (_, device) = state
            .devices
            .get_mut(&request.device_id)
            .ok_or_else(|| ConnectError::not_found(format!("no device `{}`", request.device_id)))?;
        device.name = request.name;
        Response::ok(RenameDeviceResponse {
            device: device.clone().into(),
            ..Default::default()
        })
    }

    async fn revoke_device(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RevokeDeviceRequest>,
    ) -> ServiceResult<RevokeDeviceResponse> {
        caller(&self.0, &ctx)?;
        let id = request.device_id.to_owned();
        let mut state = self.0.lock();
        let (_, device) = state
            .devices
            .get_mut(&id)
            .ok_or_else(|| ConnectError::not_found(format!("no device `{id}`")))?;
        if !device.revoked_at.is_set() {
            device.revoked_at = ts(SystemTime::now());
            device.push_targets.clear();
        }
        Response::ok(RevokeDeviceResponse {
            device: device.clone().into(),
            ..Default::default()
        })
    }

    async fn register_push_target(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RegisterPushTargetRequest>,
    ) -> ServiceResult<RegisterPushTargetResponse> {
        caller(&self.0, &ctx)?;
        let request = request.to_owned_message();
        if request.device_id.is_empty() {
            return Err(ConnectError::invalid_argument("device_id is required"));
        }
        let mut rng = rand::rng();
        let id = format!("pt_{}", ulid::Ulid::from(rng.random::<u128>()));
        let now = SystemTime::now();
        let provider = request.provider.as_known().ok_or_else(|| {
            ConnectError::invalid_argument("provider must be a known PushProvider")
        })?;
        let target = PushTargetRef {
            id: id.clone(),
            provider: request.provider,
            registered_at: ts(now),
            ..Default::default()
        };
        {
            let mut state = self.0.lock();
            let Some((_, device)) = state.devices.get_mut(&request.device_id) else {
                return Err(ConnectError::not_found(format!(
                    "no device `{}`",
                    request.device_id
                )));
            };
            // A device has one target per provider: re-registering replaces
            // it, as a real gateway would when a phone re-registers after a
            // reboot.
            device
                .push_targets
                .retain(|t| t.provider.as_known() != Some(provider));
            device.push_targets.push(target.clone());
            state
                .push_tokens
                .insert(id.clone(), request.token_or_endpoint.clone());
        }
        // The mock has no push gateway to deliver to, so a delivery is
        // recorded for `GET /mock/push-log` instead. What is recorded is the
        // routing a gateway would use, not a plaintext payload (§37 §7.4).
        self.0.record_push(PushRecord {
            seq: 0,
            target_id: id.clone(),
            device_id: request.device_id.to_owned(),
            provider: format!("{provider:?}").to_lowercase(),
            token: request.token_or_endpoint.to_string(),
            app_id: request.app_id.to_string(),
            sealed: !request.hpke_public_key.is_empty(),
        });
        Response::ok(RegisterPushTargetResponse {
            target: MessageField::some(target),
            ..Default::default()
        })
    }

    async fn unregister_push_target(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, UnregisterPushTargetRequest>,
    ) -> ServiceResult<UnregisterPushTargetResponse> {
        caller(&self.0, &ctx)?;
        let request = request.to_owned_message();
        let mut state = self.0.lock();
        if state.push_tokens.remove(&request.target_id).is_none() {
            return Err(ConnectError::not_found(format!(
                "no push target `{}`",
                request.target_id
            )));
        }
        for (_, device) in state.devices.values_mut() {
            device.push_targets.retain(|t| t.id != request.target_id);
        }
        Response::ok(UnregisterPushTargetResponse::default())
    }

    async fn get_notification_preferences(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, GetNotificationPreferencesRequest>,
    ) -> ServiceResult<GetNotificationPreferencesResponse> {
        caller(&self.0, &ctx)?;
        Err(not_implemented("GetNotificationPreferences"))
    }

    async fn set_notification_preferences(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, SetNotificationPreferencesRequest>,
    ) -> ServiceResult<SetNotificationPreferencesResponse> {
        caller(&self.0, &ctx)?;
        Err(not_implemented("SetNotificationPreferences"))
    }

    async fn send_test_notification(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, SendTestNotificationRequest>,
    ) -> ServiceResult<SendTestNotificationResponse> {
        caller(&self.0, &ctx)?;
        Err(not_implemented("SendTestNotification"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_payload_is_v1() {
        let payload = qr_payload("https://loams.example", "01J9", "12345678", 1_790_899_500);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["kind"], "loams-pair");
        assert!(v["issuer"].as_str().unwrap().starts_with("https://"));
        assert_eq!(v["user_code"].as_str().unwrap().len(), 8);
        assert!(v["spki"].is_null(), "a plain HTTP mock pins nothing");
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "v",
                "kind",
                "issuer",
                "instance_id",
                "spki",
                "jkt",
                "user_code",
                "exp"
            ]
        );
    }
}
