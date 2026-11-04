//! CEL rules against a real API server (kind). The tests are `#[ignore]`d;
//! operator.yml's kind job runs them with `--ignored` after `crdgen`'s CRDs
//! are applied. They need a kubeconfig that reaches that cluster and use the
//! `default` namespace.

use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::core::DynamicObject;
use kube::core::{ApiResource, GroupVersionKind};
use serde_json::{Value, json};

const NS: &str = "default";

async fn api(kind: &str, plural: &str) -> Api<DynamicObject> {
    let client = kube::Client::try_default().await.expect("kube client");
    let gvk = GroupVersionKind::gvk("loams.dev", "v1alpha1", kind);
    let ar = ApiResource::from_gvk_with_plural(&gvk, plural);
    Api::namespaced_with(client, NS, &ar)
}

fn object(kind: &str, name: &str, spec: Value) -> DynamicObject {
    serde_json::from_value(json!({
        "apiVersion": "loams.dev/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": NS },
        "spec": spec,
    }))
    .expect("object")
}

fn home_spec(mode: &str) -> Value {
    json!({
        "role": "home",
        "release": "0.1.4",
        "exposure": { "mode": mode },
        "marketplace": { "index": "https://example.invalid/index.v2.json" },
        "base": { "access": {
            "org": "acme",
            "identity": { "externalDomain": "id.example.internal",
                          "database": { "secretRef": { "name": "identity-db" } } },
            "openfga": { "database": { "secretRef": { "name": "openfga-db" } } }
        }}
    })
}

/// The error text of a refused request; panics when it was accepted.
fn refused<T>(r: Result<T, kube::Error>) -> String {
    match r {
        Err(kube::Error::Api(e)) => {
            assert_eq!(e.code, 422, "expected 422, got {e:?}");
            e.message
        }
        Err(other) => panic!("expected a 422, got {other}"),
        Ok(_) => panic!("the API server accepted the object"),
    }
}

#[tokio::test]
#[ignore = "needs a Kubernetes cluster with the loams.dev CRDs (kind job)"]
async fn crd_cel_refuses_public_exposure() {
    let api = api("LoamsCluster", "loamsclusters").await;
    let msg = refused(
        api.create(
            &PostParams::default(),
            &object("LoamsCluster", "public", home_spec("public")),
        )
        .await,
    );
    assert!(msg.contains("exposure"), "{msg}");
}

#[tokio::test]
#[ignore = "needs a Kubernetes cluster with the loams.dev CRDs (kind job)"]
async fn crd_cel_listing_ref_immutable() {
    let api = api("LoamsInstall", "loamsinstalls").await;
    let spec = json!({ "cluster": "home", "listing": { "id": "a/b", "version": "1.0.0" } });
    api.create(
        &PostParams::default(),
        &object("LoamsInstall", "immutable-listing", spec),
    )
    .await
    .expect("create");
    let patch = json!({ "spec": { "listing": { "id": "c/d" } } });
    let msg = refused(
        api.patch(
            "immutable-listing",
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await,
    );
    assert!(msg.contains("listing.id is immutable"), "{msg}");
}

#[tokio::test]
#[ignore = "needs a Kubernetes cluster with the loams.dev CRDs (kind job)"]
async fn crd_cel_storage_kind_immutable() {
    let api = api("StorageBinding", "storagebindings").await;
    let spec = json!({ "kind": "object", "object": { "provider": "s3", "bucket": "b" } });
    api.create(
        &PostParams::default(),
        &object("StorageBinding", "immutable-kind", spec),
    )
    .await
    .expect("create");
    let patch = json!({ "spec": { "kind": "kv" } });
    let msg = refused(
        api.patch(
            "immutable-kind",
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await,
    );
    assert!(msg.contains("kind is immutable"), "{msg}");
}

#[tokio::test]
#[ignore = "needs a Kubernetes cluster with the loams.dev CRDs (kind job)"]
async fn crd_cel_object_kind_needs_object_block() {
    let api = api("StorageBinding", "storagebindings").await;
    let msg = refused(
        api.create(
            &PostParams::default(),
            &object("StorageBinding", "no-object", json!({ "kind": "object" })),
        )
        .await,
    );
    assert!(msg.contains("kind object needs an object block"), "{msg}");
}

#[tokio::test]
#[ignore = "needs a Kubernetes cluster with the loams.dev CRDs (kind job)"]
async fn crd_cel_one_credential_source() {
    let api = api("StorageBinding", "storagebindings").await;
    let spec = json!({
        "kind": "object",
        "object": { "provider": "s3", "bucket": "b" },
        "credentials": {
            "workloadIdentity": { "awsRoleArn": "arn:aws:iam::1:role/x" },
            "secretRef": { "name": "creds" }
        }
    });
    let msg = refused(
        api.create(
            &PostParams::default(),
            &object("StorageBinding", "two-creds", spec),
        )
        .await,
    );
    assert!(
        msg.contains("set only one of workloadIdentity and secretRef"),
        "{msg}"
    );
}
