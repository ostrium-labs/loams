//! The route table and hot reload (GW1 Task 2).
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use loams_gateway::routes::{RouteError, RouteTable, generation_of};

const RANGE: std::ops::RangeInclusive<u16> = 16000..=17999;

const FULL: &str = r#"
[pg]
default_namespace = "acme"

[[pg.namespaces]]
name = "acme"
hostnames = ["Acme.EU1.pg.example.com", "acme-alias.eu1.pg.example.com"]
port = 16004
upstreams = ["pgdog.loams-pg-acme.svc.cluster.local:6432", "[::1]:6432"]
upstream_server_name = "acme.eu1.pg.example.com"

[[pg.namespaces]]
name = "beta"
hostnames = ["beta.eu1.pg.example.com"]
upstreams = ["10.0.0.7:6432"]
"#;

fn parse(text: &str) -> Result<RouteTable, RouteError> {
    RouteTable::parse(text, &RANGE, false)
}

#[test]
fn parse_minimal_and_full() {
    let empty = parse("").unwrap();
    assert!(empty.pg.namespaces.is_empty());
    let t = parse(FULL).unwrap();
    assert_eq!(t.pg.namespaces.len(), 2);
    assert_eq!(t.by_port(16004).unwrap().name, "acme");
    assert_eq!(t.pg.default_namespace.as_ref().unwrap().name, "acme");
    let beta = t.by_hostname("beta.eu1.pg.example.com").unwrap();
    assert_eq!(beta.upstream_server_name(), "beta.eu1.pg.example.com");
    assert!(t.generation.starts_with("sha256:"));
}

#[test]
fn hostname_lookup_is_case_insensitive() {
    let t = parse(FULL).unwrap();
    assert_eq!(
        t.by_hostname("ACME.eu1.PG.example.com").unwrap().name,
        "acme"
    );
    assert_eq!(
        t.by_hostname("acme-alias.eu1.pg.example.com").unwrap().name,
        "acme"
    );
    assert!(t.by_hostname("gamma.eu1.pg.example.com").is_none());
}

#[test]
fn duplicate_hostname_rejected() {
    let text = FULL.replace("beta.eu1.pg.example.com", "acme.eu1.pg.example.com");
    assert!(matches!(parse(&text), Err(RouteError::Invalid(m)) if m.contains("listed twice")));
}

#[test]
fn duplicate_port_rejected() {
    let text = FULL.replace(
        "upstreams = [\"10.0.0.7:6432\"]",
        "port = 16004\nupstreams = [\"10.0.0.7:6432\"]",
    );
    assert!(matches!(parse(&text), Err(RouteError::Invalid(m)) if m.contains("port 16004")));
}

#[test]
fn port_outside_range_rejected() {
    let text = FULL.replace("port = 16004", "port = 5432");
    assert!(matches!(parse(&text), Err(RouteError::Invalid(m)) if m.contains("outside")));
}

#[test]
fn bad_names_and_upstreams_rejected() {
    assert!(parse(&FULL.replace("name = \"beta\"", "name = \"Beta_1\"")).is_err());
    assert!(parse(&FULL.replace("10.0.0.7:6432", "10.0.0.7")).is_err());
    assert!(parse(&FULL.replace("10.0.0.7:6432", "10.0.0.7:0")).is_err());
    assert!(
        parse(&FULL.replace(
            "default_namespace = \"acme\"",
            "default_namespace = \"zeta\""
        ))
        .is_err()
    );
    assert!(parse(&FULL.replace("[\"beta.eu1.pg.example.com\"]", "[]")).is_err());
    assert!(parse(&FULL.replace("beta.eu1", "*.eu1")).is_err());
    assert!(matches!(parse("bogus = 1"), Err(RouteError::Parse(_))));
}

const RW: &str = r#"
[[risingwave.endpoints]]
name = "eng_abcdefghijklmnopqrstuvwxyz"
namespace = "dev"
hostnames = ["clicks.dev.eu1.rw.example.com"]
upstreams = ["rw-abcdefghijklmnopqrstuvwxyz-frontend.eng-abcdefghijklmnopqrstuvwxyz.svc:4566"]
upstream_user = "loams_gateway"
upstream_password_file = "/var/run/loams/risingwave/abcdefghijklmnopqrstuvwxyz/password"
port = 16566
"#;

#[test]
fn risingwave_endpoints_route_by_name_and_port() {
    let t = parse(&format!("{FULL}{RW}")).unwrap();
    let ep = t
        .risingwave
        .by_hostname("Clicks.dev.eu1.rw.example.com")
        .unwrap();
    assert_eq!(ep.namespace, "dev");
    assert!(t.risingwave.by_port(16566).is_some());
    let mut ports: Vec<u16> = t.dedicated_ports().collect();
    ports.sort_unstable();
    assert_eq!(ports, vec![16004, 16566]);
    // RisingWave routes are not Postgres namespaces.
    assert!(t.by_hostname("clicks.dev.eu1.rw.example.com").is_none());
}

#[test]
fn risingwave_collisions_with_namespaces_rejected() {
    let same_host = RW.replace("clicks.dev.eu1.rw.example.com", "beta.eu1.pg.example.com");
    assert!(matches!(
        parse(&format!("{FULL}{same_host}")),
        Err(RouteError::Invalid(_))
    ));
    let same_port = RW.replace("16566", "16004");
    assert!(matches!(
        parse(&format!("{FULL}{same_port}")),
        Err(RouteError::Invalid(_))
    ));
    let no_secret_inline = format!("{RW}upstream_password = \"x\"\n");
    assert!(parse(&no_secret_inline).is_err());
}

#[test]
fn missing_generation_refused_when_required() {
    assert_eq!(
        RouteTable::parse(FULL, &RANGE, true).unwrap_err(),
        RouteError::MissingGeneration
    );
    let with = loams_gateway::routes::with_generation(FULL);
    assert!(RouteTable::parse(&with, &RANGE, true).is_ok());
}

#[test]
fn generation_mismatch_rejected() {
    let good = generation_of(FULL);
    let with = format!("generation = \"{good}\"\n{FULL}");
    assert_eq!(parse(&with).unwrap().generation, good);
    let bad = format!("generation = \"sha256:00\"\n{FULL}");
    assert!(matches!(parse(&bad), Err(RouteError::Generation { .. })));
    let edited = format!(
        "generation = \"{good}\"\n{}",
        FULL.replace("10.0.0.7", "10.0.0.8")
    );
    assert!(matches!(parse(&edited), Err(RouteError::Generation { .. })));
}

async fn harness(routes: &str) -> (common::Harness, common::FakePgDog) {
    let ca = common::Ca::new("pgdog ca");
    let pg = common::fake_pgdog(&ca, &[common::WILDCARD], "acme").await;
    let routes = routes.replace("UPSTREAM", &pg.addr.to_string());
    (
        common::start(&ca, &routes, common::Options::default()).await,
        pg,
    )
}

#[tokio::test]
async fn changed_file_is_swapped() {
    let (h, pg) = harness(&common::ns("acme", "127.0.0.1:1".parse().unwrap(), None)).await;
    let first = h
        .gateway
        .routes()
        .borrow()
        .as_ref()
        .unwrap()
        .generation
        .clone();
    let next = common::ns("acme", pg.addr, None) + &common::ns("beta", pg.addr, None);
    h.set_routes(&next).await;
    let rx = h.gateway.routes();
    let t = rx.borrow().clone().unwrap();
    assert_ne!(t.generation, first);
    assert!(t.by_hostname("beta.gw.pg.test").is_some());
}

/// Writes `text` over the routes file by a rename.
fn replace(h: &common::Harness, text: &str) {
    let tmp = h.dir.path().join("routes.toml.tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, &h.routes_path).unwrap();
}

#[tokio::test]
async fn invalid_file_keeps_old_table() {
    let (h, _pg) = harness(&common::ns("acme", "127.0.0.1:1".parse().unwrap(), None)).await;
    let before = h
        .gateway
        .routes()
        .borrow()
        .as_ref()
        .unwrap()
        .generation
        .clone();
    // Each file is swapped in atomically (as pg-control writes it), so the
    // watcher sees only whole files; 300 ms is 15 poll intervals.
    replace(&h, "[[pg.namespaces]]\nname = \"BAD NAME\"\n");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.gateway.routes().borrow().as_ref().unwrap().generation,
        before
    );
    // Valid, but without its generation line: refused too.
    replace(
        &h,
        &common::ns("beta", "127.0.0.1:1".parse().unwrap(), None),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = h
        .gateway
        .routes()
        .borrow()
        .as_ref()
        .unwrap()
        .generation
        .clone();
    assert_eq!(before, after);
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_reloads_at_once() {
    // A long poll interval, so only SIGHUP can explain a quick reload.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = dir.path().join("routes.toml");
    std::fs::write(&path, "").unwrap();
    let (tx, mut rx) = loams_gateway::reload::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(loams_gateway::reload::watch_file(
        path.clone(),
        Duration::from_secs(3600),
        RANGE,
        false,
        tx,
        async move {
            let _ = stop_rx.await;
        },
    ));
    tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|t| t.is_some()))
        .await
        .unwrap()
        .unwrap();
    let next = common::ns("acme", "127.0.0.1:6432".parse().unwrap(), None);
    std::fs::write(&path, &next).unwrap();
    let status = std::process::Command::new("kill")
        .args(["-HUP", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let want = generation_of(&next);
    tokio::time::timeout(
        Duration::from_secs(5),
        rx.wait_for(|t| t.as_ref().is_some_and(|t| t.generation == want)),
    )
    .await
    .unwrap()
    .unwrap();
    let _ = stop_tx.send(());
    task.await.unwrap();
}

/// The compose stack's engine-gateway files (deploy/release/engines) parse
/// as the gateway reads them.
#[test]
fn compose_engine_gateway_files_parse() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/release/engines");
    let config = loams_gateway::GatewayConfig::from_toml(
        &std::fs::read_to_string(format!("{root}/gateway.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(config.risingwave.self_signed_names.len(), 2);
    let routes = std::fs::read_to_string(format!("{root}/routes.toml")).unwrap();
    let t = RouteTable::parse(&routes, &config.pg.dedicated_ports.0, false).unwrap();
    assert!(t.risingwave.by_port(4566).is_some());
}
