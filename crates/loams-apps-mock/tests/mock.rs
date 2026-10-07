//! The mock over real sockets, through the generated connect-rust clients,
//! on every protocol (AP0 Task 5).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use connectrpc::client::{CallOptions, ClientConfig, HttpClient};
use connectrpc::{ConnectError, ErrorCode, Protocol};
use loams_apps_mock::proto::loams::approvals::v1::__buffa::oneof::watch_approvals_response::Event;
use loams_apps_mock::proto::loams::approvals::v1::{
    ApprovalServiceClient, ApprovalState, DecideApprovalRequest, DecisionKind, GetApprovalRequest,
    ListApprovalsRequest, WatchApprovalsRequest,
};
use loams_apps_mock::proto::loams::devices::v1::{
    CreatePairingRequest, DeviceServiceClient, ListDevicesRequest, PushProvider,
    RegisterPushTargetRequest, UnregisterPushTargetRequest,
};
use loams_apps_mock::proto::loams::errors::v1::ErrorInfo;
use loams_apps_mock::proto::loams::instance::v1::{
    Edition, GetInstanceRequest, InstanceServiceClient, WhoAmIRequest,
};
use loams_apps_mock::proto::loams::notifications::v1::{
    ListNotificationsRequest, MarkReadRequest, NotificationServiceClient,
};
use loams_apps_mock::proto::loams::operations::v1::{
    ListOperationsRequest, OperationsServiceClient,
};
use loams_apps_mock::{MockConfig, MockHandle, serve};

const OMAR: &str = "Bearer mock-access-usr_omar";
const OMAR_STALE: &str = "Bearer mock-stale-usr_omar";
const DANA: &str = "Bearer mock-access-usr_dana";
const DROP_DOCS: &str = "apr_01J9ZDROPDOCS";
const CREATE_KEY: &str = "apr_01J9ZCREATEKEY";

async fn start(heartbeat: Duration) -> MockHandle {
    serve(MockConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        heartbeat,
        ..MockConfig::default()
    })
    .await
    .unwrap()
}

fn config(mock: &MockHandle, protocol: Protocol) -> ClientConfig {
    ClientConfig::new(mock.url().parse().unwrap()).with_protocol(protocol)
}

fn http(protocol: Protocol) -> HttpClient {
    if protocol == Protocol::Grpc {
        HttpClient::plaintext_http2_only()
    } else {
        HttpClient::plaintext()
    }
}

fn auth(token: &str) -> CallOptions {
    CallOptions::default().with_header("authorization", token)
}

fn reason(err: &ConnectError) -> String {
    use buffa::Message as _;
    use connectrpc::ErrorDetail;
    let detail: &ErrorDetail = err.details.first().expect("an ErrorInfo detail");
    assert_eq!(detail.type_url, "loams.errors.v1.ErrorInfo");
    let bytes = base64_decode(detail.value.as_deref().unwrap());
    ErrorInfo::decode_from_slice(&bytes).unwrap().reason
}

fn base64_decode(s: &str) -> Vec<u8> {
    // Unpadded standard alphabet (the Connect error-detail encoding).
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = ALPHABET.iter().position(|a| *a == c).unwrap() as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

#[tokio::test]
async fn every_protocol_serves_get_instance() {
    let mock = start(Duration::from_secs(15)).await;
    for protocol in [Protocol::Connect, Protocol::Grpc, Protocol::GrpcWeb] {
        let client = InstanceServiceClient::new(http(protocol), config(&mock, protocol));
        let info = client
            .get_instance(GetInstanceRequest::default())
            .await
            .unwrap_or_else(|e| panic!("{protocol:?}: {e}"))
            .into_owned();
        assert_eq!(info.edition.as_known(), Some(Edition::EDITION_OSS));
        assert!(info.api_versions.iter().any(|v| v == "loams.approvals.v1"));
    }
    // JSON over Connect too.
    let client = InstanceServiceClient::new(
        HttpClient::plaintext(),
        config(&mock, Protocol::Connect).json(),
    );
    let info = client
        .get_instance(GetInstanceRequest::default())
        .await
        .unwrap()
        .into_owned();
    assert_eq!(info.name, "Loams (mock)");
    mock.stop().await;
}

#[tokio::test]
async fn who_am_i_needs_a_token_and_reports_the_actor_chain() {
    let mock = start(Duration::from_secs(15)).await;
    let client =
        InstanceServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let err = client.who_am_i(WhoAmIRequest::default()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let me = client
        .who_am_i_with_options(
            WhoAmIRequest::default(),
            auth("Bearer mock-access-agt_claude"),
        )
        .await
        .unwrap()
        .into_owned();
    assert_eq!(me.principal.as_option().unwrap().id, "usr_dana");
    assert_eq!(me.actor_chain[0].id, "agt_claude");
    mock.stop().await;
}

#[tokio::test]
async fn approval_decides_once_with_a_fresh_session() {
    let mock = start(Duration::from_secs(15)).await;
    let client =
        ApprovalServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let pending = client
        .list_approvals_with_options(ListApprovalsRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(
        pending.approvals.len(),
        2,
        "expired approvals are not pending"
    );

    let request = DecideApprovalRequest {
        approval_id: CREATE_KEY.into(),
        revision: 1,
        decision: DecisionKind::DECISION_KIND_APPROVE.into(),
        idempotency_key: "01J9ZIDEMPOTENT".into(),
        ..Default::default()
    };
    let decided = client
        .decide_approval_with_options(request.clone(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    let approval = decided.approval.as_option().unwrap();
    assert_eq!(
        approval.state.as_known(),
        Some(ApprovalState::APPROVAL_STATE_APPROVED)
    );
    assert_eq!(approval.revision, 2);

    // A retry with the same idempotency key returns the same answer.
    let again = client
        .decide_approval_with_options(request.clone(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(again.approval.as_option().unwrap().revision, 2);

    // A new decision is refused.
    let err = client
        .decide_approval_with_options(
            DecideApprovalRequest {
                idempotency_key: "01J9ZOTHER".into(),
                revision: 2,
                ..request
            },
            auth(OMAR),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::FailedPrecondition);
    assert_eq!(reason(&err), "approval_already_decided");
    mock.stop().await;
}

#[tokio::test]
async fn stale_session_requires_step_up_and_requester_cannot_approve() {
    let mock = start(Duration::from_secs(15)).await;
    let client =
        ApprovalServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let request = DecideApprovalRequest {
        approval_id: DROP_DOCS.into(),
        revision: 1,
        decision: DecisionKind::DECISION_KIND_APPROVE.into(),
        reason: "ticket 42".into(),
        ..Default::default()
    };
    let err = client
        .decide_approval_with_options(request.clone(), auth(OMAR_STALE))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    assert_eq!(reason(&err), "step_up_required");

    let err = client
        .decide_approval_with_options(request.clone(), auth(DANA))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&err), "requester_cannot_approve");

    // An agent acting for Dana cannot approve Dana's own request either.
    let err = client
        .decide_approval_with_options(
            DecideApprovalRequest {
                approval_id: CREATE_KEY.into(),
                revision: 1,
                decision: DecisionKind::DECISION_KIND_APPROVE.into(),
                ..Default::default()
            },
            auth("Bearer mock-access-agt_claude"),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(reason(&err), "requester_cannot_approve");
    let key = client
        .get_approval_with_options(
            GetApprovalRequest {
                approval_id: CREATE_KEY.into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    let key = key.approval.as_option().unwrap();
    assert_eq!(key.requested_by.as_option().unwrap().id, "usr_dana");
    assert_eq!(
        key.state.as_known(),
        Some(ApprovalState::APPROVAL_STATE_PENDING)
    );

    let err = client
        .decide_approval_with_options(
            DecideApprovalRequest {
                decision_proof: "eyJhbGciOiJFUzI1NiJ9.e30.sig".into(),
                ..request.clone()
            },
            auth(OMAR_STALE),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&err),
        "not_implemented",
        "proofs are refused, never trusted"
    );

    let still = client
        .get_approval_with_options(
            GetApprovalRequest {
                approval_id: DROP_DOCS.into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    assert_eq!(
        still.approval.as_option().unwrap().state.as_known(),
        Some(ApprovalState::APPROVAL_STATE_PENDING)
    );
    mock.stop().await;
}

/// The next stream message, owned, within 5 s.
macro_rules! next {
    ($stream:expr) => {
        tokio::time::timeout(Duration::from_secs(5), $stream.message())
            .await
            .expect("a stream message within 5 s")
            .unwrap()
            .expect("the stream is open")
            .to_owned_message()
    };
}

#[tokio::test]
async fn watch_sends_snapshot_changes_and_heartbeats() {
    let mock = start(Duration::from_millis(200)).await;
    let client =
        ApprovalServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let mut stream = client
        .watch_approvals_with_options(WatchApprovalsRequest::default(), auth(OMAR))
        .await
        .unwrap();
    let first = next!(stream);
    let Some(Event::Snapshot(snapshot)) = first.event else {
        panic!("expected a snapshot first, got {first:?}");
    };
    assert_eq!(snapshot.approvals.len(), 2);
    let cursor = first.cursor.clone();

    client
        .decide_approval_with_options(
            DecideApprovalRequest {
                approval_id: CREATE_KEY.into(),
                revision: 1,
                decision: DecisionKind::DECISION_KIND_REJECT.into(),
                reason: "not now".into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap();
    // Heartbeats may come first; the decision arrives as a remove (it left
    // the PENDING filter).
    let removed = loop {
        let msg = next!(stream);
        match msg.event {
            Some(Event::Heartbeat(_)) => continue,
            Some(Event::Remove(id)) => break (id, msg.cursor),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(removed.0, CREATE_KEY);
    assert_ne!(removed.1, cursor);
    let beat = next!(stream);
    assert!(matches!(beat.event, Some(Event::Heartbeat(_))));
    drop(stream);

    // Resuming from the snapshot's cursor replays the missed change only.
    let mut resumed = client
        .watch_approvals_with_options(
            WatchApprovalsRequest {
                resume_cursor: cursor,
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap();
    let replay = next!(resumed);
    assert!(!replay.snapshot_reset);
    assert!(matches!(replay.event, Some(Event::Remove(ref id)) if id == CREATE_KEY));

    // An unknown cursor resets to a fresh snapshot.
    let mut reset = client
        .watch_approvals_with_options(
            WatchApprovalsRequest {
                resume_cursor: "c999".into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap();
    let fresh = next!(reset);
    assert!(fresh.snapshot_reset);
    assert!(matches!(fresh.event, Some(Event::Snapshot(ref s)) if s.approvals.len() == 1));
    mock.stop().await;
}

#[tokio::test]
async fn devices_operations_and_inbox() {
    let mock = start(Duration::from_secs(15)).await;
    let devices =
        DeviceServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let pairing = devices
        .create_pairing_with_options(CreatePairingRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    let qr: serde_json::Value = serde_json::from_str(&pairing.qr_payload).unwrap();
    assert_eq!(qr["v"], 1);
    assert_eq!(qr["user_code"], pairing.user_code);
    let mine = devices
        .list_devices_with_options(ListDevicesRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(mine.devices.len(), 1, "the seeded phone");

    // Registering a push target needs a device and a known provider, and
    // refuses without either.
    let err = devices
        .register_push_target_with_options(RegisterPushTargetRequest::default(), auth(OMAR))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let registered = devices
        .register_push_target_with_options(
            RegisterPushTargetRequest {
                device_id: mine.devices[0].id.clone(),
                provider: PushProvider::PUSH_PROVIDER_FCM.into(),
                token_or_endpoint: "fcm-test-token".into(),
                app_id: "dev.loams.app".into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    let target = registered.target.as_option().expect("a target ref");
    let after = devices
        .list_devices_with_options(ListDevicesRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(
        after.devices[0].push_targets.as_slice(),
        std::slice::from_ref(target),
        "the target is listed on the device"
    );

    let err = devices
        .register_push_target_with_options(
            RegisterPushTargetRequest {
                device_id: "dev_nosuch".into(),
                provider: PushProvider::PUSH_PROVIDER_FCM.into(),
                token_or_endpoint: "x".into(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    devices
        .unregister_push_target_with_options(
            UnregisterPushTargetRequest {
                target_id: target.id.clone(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap();
    let after = devices
        .list_devices_with_options(ListDevicesRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert!(after.devices[0].push_targets.is_empty());

    let operations =
        OperationsServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let ops = operations
        .list_operations_with_options(ListOperationsRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(ops.operations.len(), 2);

    let inbox =
        NotificationServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let marked = inbox
        .mark_read_with_options(
            MarkReadRequest {
                all: true,
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    assert_eq!(marked.marked, 1);
    let unread = inbox
        .list_notifications_with_options(
            ListNotificationsRequest {
                unread_only: true,
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    assert!(unread.notifications.is_empty());
    mock.stop().await;
}

#[tokio::test]
async fn a_pairing_code_from_the_rpc_redeems_at_the_token_endpoint() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let devices =
        DeviceServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));

    // A signed-in session creates the pairing, exactly as the desktop does.
    let pairing = devices
        .create_pairing_with_options(CreatePairingRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert_eq!(pairing.user_code.len(), 8);

    // The phone exchanges it. The answer is a token this mock's RPCs accept.
    let rest = reqwest::Client::new();
    let form = [
        ("grant_type", "urn:loams:params:oauth:grant-type:pairing"),
        ("user_code", pairing.user_code.as_str()),
    ];
    let answer: serde_json::Value = rest
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&form)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_id = answer["device_id"].as_str().unwrap();
    let issued = answer["access_token"].as_str().unwrap();
    // Issued tokens are per-device, so only the prefix is fixed.
    assert!(
        issued.starts_with("mock-access-usr_omar-"),
        "unexpected access token {issued}"
    );

    // The paired device is listed for its owner, and the issued token works.
    let mine = devices
        .list_devices_with_options(ListDevicesRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    assert!(
        mine.devices.iter().any(|d| d.id == device_id),
        "the phone registered itself"
    );
    let who = InstanceServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect))
        .who_am_i_with_options(WhoAmIRequest::default(), auth(&format!("Bearer {issued}")))
        .await
        .unwrap_or_else(|e| panic!("the issued token must authenticate: {e}"))
        .into_owned();
    assert_eq!(who.principal.id, "usr_omar");
    assert_eq!(
        who.device.as_option().map(|d| d.id.as_str()),
        Some(device_id),
        "the device the token was issued to"
    );

    // Replaying the code is refused.
    let replay = rest
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 400);
    let refused: serde_json::Value = replay.json().await.unwrap();
    assert_eq!(refused["loams_reason"], "pairing_used");
    mock.stop().await;
}

#[tokio::test]
async fn the_fake_authentik_verifies_pkce_and_the_exchange_accepts_its_token() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Discovery, addressed the way a phone would.
    let discovery: serde_json::Value = rest
        .get(format!(
            "{base}/mock/authentik/application/o/loams/.well-known/openid-configuration"
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(discovery["code_challenge_methods_supported"][0], "S256");

    // Authorize redirects back with a code, no login page in between.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    let redirect = format!("{base}/callback");
    let authorize = rest
        .get(format!(
            "{base}/mock/authentik/application/o/loams/authorize/"
        ))
        .query(&[
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(authorize.status(), 302);
    let location = authorize.headers()["location"].to_str().unwrap().to_owned();
    let code = location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();
    assert!(location.ends_with("state=xyz"), "{location}");

    // A wrong verifier is refused, the right one is not.
    let wrong = rest
        .post(format!("{base}/mock/authentik/application/o/loams/token/"))
        .form(&[
            ("code", code.as_str()),
            ("code_verifier", "not-the-verifier"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400);

    let idp: serde_json::Value = rest
        .post(format!("{base}/mock/authentik/application/o/loams/token/"))
        .form(&[("code", code.as_str()), ("code_verifier", verifier)])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let subject = idp["access_token"].as_str().unwrap();
    assert!(subject.starts_with("mock-authentik-"), "{subject}");

    // The gateway exchanges it for a Loams token, the RFC 8693 path.
    let exchanged: serde_json::Value = rest
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token", subject),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:id_token",
            ),
        ])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        exchanged["access_token"]
            .as_str()
            .unwrap()
            .starts_with("mock-access-")
    );
    mock.stop().await;
}

#[tokio::test]
async fn the_test_controls_drive_the_state_the_rpcs_read() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::new();

    // A pending approval, as an agent asking for one would create.
    let created: serde_json::Value = rest
        .post(format!("{base}/mock/approvals"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["approval_id"].as_str().unwrap();
    let approvals =
        ApprovalServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let pending = approvals
        .list_approvals_with_options(
            ListApprovalsRequest {
                states: vec![ApprovalState::APPROVAL_STATE_PENDING.into()],
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned();
    assert!(
        pending.approvals.iter().any(|a| a.id == id),
        "the RPC list sees what the control created"
    );

    // Advancing operations moves the seeded import along and finishes it.
    let operations =
        OperationsServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let before = operations
        .list_operations_with_options(ListOperationsRequest::default(), auth(OMAR))
        .await
        .unwrap()
        .into_owned();
    let import = before
        .operations
        .iter()
        .find(|o| o.progress.as_option().is_some())
        .expect("a running import")
        .clone();
    for _ in 0..10 {
        rest.post(format!("{base}/mock/tick")).send().await.unwrap();
    }
    let after = operations
        .get_operation_with_options(
            loams_apps_mock::proto::loams::operations::v1::GetOperationRequest {
                operation_id: import.id.clone(),
                ..Default::default()
            },
            auth(OMAR),
        )
        .await
        .unwrap()
        .into_owned()
        .operation;
    assert_eq!(
        after.progress.as_option().map(|p| p.done),
        Some(100_000),
        "the import ran to its total"
    );
    mock.stop().await;
}

/// A phone has to be able to follow what it is handed. The seed's URLs are
/// `.invalid` placeholders and the fake Authentik used to answer on the host
/// alone, dropping the port, so both are pinned here.
#[tokio::test]
async fn discovery_and_instance_hand_out_addresses_a_client_can_follow() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::new();

    let instance =
        InstanceServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));
    let instance = instance
        .get_instance_with_options(GetInstanceRequest::default(), Default::default())
        .await
        .unwrap()
        .into_owned();
    assert_eq!(
        instance.issuer.trim_end_matches('/'),
        base.trim_end_matches('/'),
        "the issuer is the address the client dialled, not the seed's placeholder"
    );
    assert!(
        instance.jwks_uri.starts_with(&base),
        "the advertised JWKS is served by this mock, got {}",
        instance.jwks_uri
    );
    assert!(
        rest.get(&instance.jwks_uri)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .is_ok(),
        "the advertised JWKS resolves"
    );
    assert!(
        instance.sign_in_methods[0].issuer.starts_with(&base),
        "the sign-in issuer is reachable, got {}",
        instance.sign_in_methods[0].issuer
    );

    let discovery: serde_json::Value = rest
        .get(format!(
            "{base}/mock/authentik/application/o/loams/.well-known/openid-configuration"
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    // The port has to survive, or a client follows the endpoints to :80.
    let port = base.rsplit(':').next().unwrap();
    for field in [
        "issuer",
        "authorization_endpoint",
        "token_endpoint",
        "jwks_uri",
    ] {
        let url = discovery[field].as_str().unwrap();
        assert!(
            url.contains(&format!(":{port}")),
            "{field} keeps the port, got {url}"
        );
    }
    assert!(
        rest.get(discovery["jwks_uri"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .is_ok(),
        "the advertised Authentik JWKS resolves"
    );
    mock.stop().await;
}

/// `/mock/pairing` stands in for scanning the console's QR, so the code it
/// prints has to be the one the token endpoint accepts.
#[tokio::test]
async fn the_paired_qr_payload_carries_a_redeemable_code() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::new();

    let payload: serde_json::Value = rest
        .get(format!("{base}/mock/pairing"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = payload["code"].as_str().unwrap();
    let user_code = payload["user_code"].as_str().unwrap();
    assert_ne!(code, user_code, "the payload names both forms of the code");

    // The scanned `code` redeems.
    let token: serde_json::Value = rest
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            ("grant_type", "urn:loams:params:oauth:grant-type:pairing"),
            ("code", code),
            ("client_id", "loams-android"),
        ])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        token["access_token"]
            .as_str()
            .unwrap()
            .starts_with("mock-access-"),
        "the scanned code issues a device token"
    );

    // And so does the typed `user_code`, against a second pairing.
    let second: serde_json::Value = rest
        .get(format!("{base}/mock/pairing"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let typed: serde_json::Value = rest
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            ("grant_type", "urn:loams:params:oauth:grant-type:pairing"),
            ("user_code", second["user_code"].as_str().unwrap()),
            ("client_id", "loams-android"),
        ])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        typed["access_token"].as_str().is_some(),
        "the typed user code issues a device token too"
    );
    mock.stop().await;
}

#[tokio::test]
async fn drop_streams_ends_open_watch_streams() {
    let mock = start(Duration::from_secs(3600)).await;
    let base = mock.url();
    let approvals =
        ApprovalServiceClient::new(HttpClient::plaintext(), config(&mock, Protocol::Connect));

    let mut stream = approvals
        .watch_approvals_with_options(WatchApprovalsRequest::default(), auth(OMAR))
        .await
        .unwrap();
    // The snapshot arrives, so the stream is genuinely open.
    let first = next!(stream);
    assert!(
        matches!(first.event, Some(Event::Snapshot(ref s)) if !s.approvals.is_empty()),
        "a snapshot first, got {first:?}"
    );

    reqwest::Client::new()
        .post(format!("{base}/mock/drop-streams"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // The stream ends rather than hanging for the next heartbeat.
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.message()).await;
    match ended {
        Ok(Ok(None)) => {}
        Ok(Ok(Some(message))) => panic!("the stream should end, not yield {message:?}"),
        Ok(Err(err)) => panic!("a dropped stream ends cleanly, not with {err}"),
        Err(_) => panic!("the stream must end promptly, not hang for the next heartbeat"),
    }
    mock.stop().await;
}

#[tokio::test]
async fn console_rest_is_served_beside_the_app_protos() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::new();

    // A console route (the signed-in session) answers REST JSON.
    let session: serde_json::Value = rest
        .get(format!("{base}/api/v1/session"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["user"]["email"], "priya@acme.dev");

    // The engine's liveness probe: an empty 204, matching the mock.
    let health = rest.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(health.status(), 204);
    assert!(health.bytes().await.unwrap().is_empty());

    // A seeded console read.
    let projects: serde_json::Value = rest
        .get(format!("{base}/api/v1/projects"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        projects["projects"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
    );

    // The app protos still answer on the same port, over Connect.
    let client =
        InstanceServiceClient::new(http(Protocol::Connect), config(&mock, Protocol::Connect));
    let info = client
        .get_instance(GetInstanceRequest::default())
        .await
        .unwrap()
        .into_owned();
    assert_eq!(info.edition.as_known(), Some(Edition::EDITION_OSS));
    mock.stop().await;
}

#[tokio::test]
async fn console_query_constraints_pick_the_right_answer() {
    let mock = start(Duration::from_secs(15)).await;
    let base = mock.url();
    let rest = reqwest::Client::new();

    let unfiltered: serde_json::Value = rest
        .get(format!("{base}/api/v1/audit"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    // The agent id the seed gives three events, so a filtered read is shorter.
    let filtered: serde_json::Value = rest
        .get(format!("{base}/api/v1/audit?actor=agt_claude_code"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unfiltered["events"].as_array().unwrap().len(), 20);
    assert_eq!(filtered["events"].as_array().unwrap().len(), 3);
    mock.stop().await;
}

#[tokio::test]
async fn console_signed_out_answers_401_on_session() {
    let mock = serve(MockConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        console_signed_in: false,
        ..MockConfig::default()
    })
    .await
    .unwrap();
    let status = reqwest::Client::new()
        .get(format!("{}/api/v1/session", mock.url()))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401, "the sign-in screen needs a signed-out session");
    mock.stop().await;
}

#[tokio::test]
async fn non_loopback_is_refused() {
    let err = serve(MockConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        ..MockConfig::default()
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("loopback"), "{err}");
}
