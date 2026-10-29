//! `builtin_matches_openfga` (IA2 Task 3, D1613): every check and list in
//! `deploy/openfga/core.fga.yaml` (the model tests `fga model test` runs) is asserted against
//! `BuiltinAuthorizer`, always, and against OpenFGA when `LOAMS_AUTHZ_OPENFGA_{GRPC,STORE,MODEL}`
//! are set. Both must match the file's expected answers, so they match each other.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use loams_authz::{
    Authorizer, BuiltinAuthorizer, CheckRequest, Consistency, ObjectRef, OpenFgaAuthorizer,
    Subject, SubjectKind, Tuple,
};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
struct File {
    tuples: Vec<YTuple>,
    tests: Vec<YTest>,
}

#[derive(Deserialize, Clone)]
struct YTuple {
    user: String,
    relation: String,
    object: String,
    #[serde(default)]
    condition: Option<YCondition>,
}

#[derive(Deserialize, Clone)]
struct YCondition {
    name: String,
    #[serde(default)]
    context: Map<String, Value>,
}

#[derive(Deserialize)]
struct YTest {
    name: String,
    #[serde(default)]
    check: Vec<YCheck>,
    #[serde(default)]
    list_objects: Vec<YList>,
}

#[derive(Deserialize)]
struct YCheck {
    user: String,
    object: String,
    #[serde(default)]
    context: Map<String, Value>,
    assertions: BTreeMap<String, bool>,
}

#[derive(Deserialize)]
struct YList {
    user: String,
    #[serde(rename = "type")]
    ty: String,
    assertions: BTreeMap<String, Vec<String>>,
}

fn load() -> File {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/openfga/core.fga.yaml"
    );
    serde_yaml_ng::from_str(&std::fs::read_to_string(path).expect("read core.fga.yaml"))
        .expect("parse core.fga.yaml")
}

/// Prefixes every object id with `p`, so a shared OpenFGA store keeps runs apart.
fn px(p: &str, s: &str) -> String {
    match s.split_once(':') {
        Some((ty, rest)) if !p.is_empty() => format!("{ty}:{p}{rest}"),
        _ => s.to_owned(),
    }
}

fn subject(p: &str, user: &str) -> Subject {
    let (ty, id) = user.split_once(':').unwrap();
    let kind = match ty {
        "user" => SubjectKind::User,
        "agent" => SubjectKind::Agent,
        "service_account" => SubjectKind::ServiceAccount,
        other => panic!("unexpected subject type {other}"),
    };
    Subject::new(kind, format!("{p}{id}"))
}

fn tuples(file: &File, p: &str) -> Vec<Tuple> {
    file.tuples
        .iter()
        .map(|t| {
            let user = match t.user.split_once('#') {
                Some((set, rel)) => format!("{}#{rel}", px(p, set)),
                None => px(p, &t.user),
            };
            let tuple = Tuple::new(user, &t.relation, px(p, &t.object));
            match &t.condition {
                Some(c) => tuple.with_condition(&c.name, c.context.clone()),
                None => tuple,
            }
        })
        .collect()
}

/// Runs every case; returns the failures.
async fn run(authz: &dyn Authorizer, file: &File, p: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for test in &file.tests {
        for c in &test.check {
            for (relation, want) in &c.assertions {
                let mut req = CheckRequest::new(
                    subject(p, &c.user),
                    relation,
                    px(p, &c.object).parse().unwrap(),
                )
                .consistency(Consistency::HigherConsistency);
                req.context.clone_from(&c.context);
                match authz.check(req).await {
                    Ok(d) if d.allowed == *want => {}
                    Ok(d) => failures.push(format!(
                        "{}: {}#{relation}@{} = {}, want {want}",
                        test.name, c.object, c.user, d.allowed
                    )),
                    Err(e) => failures.push(format!(
                        "{}: {}#{relation}@{}: {e}",
                        test.name, c.object, c.user
                    )),
                }
            }
        }
        for l in &test.list_objects {
            for (relation, want) in &l.assertions {
                let got: Vec<String> = authz
                    .list_objects(&subject(p, &l.user), relation, &l.ty)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|o: ObjectRef| o.to_string())
                    .collect();
                let mut want: Vec<String> = want.iter().map(|w| px(p, w)).collect();
                want.sort();
                if got != want {
                    failures.push(format!(
                        "{}: list {} {relation} for {} = {got:?}, want {want:?}",
                        test.name, l.ty, l.user
                    ));
                }
            }
        }
    }
    failures
}

#[tokio::test]
async fn builtin_matches_the_model_tests() {
    let file = load();
    let b = BuiltinAuthorizer::new();
    b.write(&tuples(&file, ""))
        .expect("the seeded tuples satisfy the type restrictions");
    let failures = run(&b, &file, "").await;
    let cases: usize = file
        .tests
        .iter()
        .map(|t| t.check.iter().map(|c| c.assertions.len()).sum::<usize>() + t.list_objects.len())
        .sum();
    assert!(cases >= 90, "the model tests shrank to {cases} cases");
    assert!(
        failures.is_empty(),
        "BuiltinAuthorizer disagrees with core.fga.yaml:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn builtin_matches_openfga() {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(url), Some(store), Some(model)) = (
        var("LOAMS_AUTHZ_OPENFGA_GRPC"),
        var("LOAMS_AUTHZ_OPENFGA_STORE"),
        var("LOAMS_AUTHZ_OPENFGA_MODEL"),
    ) else {
        eprintln!("skipped the OpenFGA half: LOAMS_AUTHZ_OPENFGA_{{GRPC,STORE,MODEL}} are not set");
        return;
    };
    let fga = OpenFgaAuthorizer::connect(
        &url,
        &store,
        &model,
        var("LOAMS_AUTHZ_OPENFGA_KEY").as_deref(),
    )
    .await
    .unwrap();
    let file = load();
    let p = format!(
        "d{}-",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    fga.write(&tuples(&file, &p), &[]).await.unwrap();
    let b = BuiltinAuthorizer::new();
    b.write(&tuples(&file, &p)).unwrap();
    let (of, bf) = (run(&fga, &file, &p).await, run(&b, &file, &p).await);
    assert!(
        of.is_empty(),
        "OpenFGA disagrees with core.fga.yaml:\n{}",
        of.join("\n")
    );
    assert!(
        bf.is_empty(),
        "BuiltinAuthorizer disagrees with core.fga.yaml:\n{}",
        bf.join("\n")
    );
}
