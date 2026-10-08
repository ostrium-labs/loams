//! LocalRuntime against a fake container engine (tests/fixtures/fake-engine.sh):
//! locking, failure cleanup and argument shape, without Podman.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use loams_sqldb::images::Images;
use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::runtime::local::{ContainerEngine, LocalRuntime, LocalRuntimeConfig};
use loams_sqldb::runtime::{JobSpec, SqlRuntime};

struct Harness {
    dir: PathBuf,
    rt: LocalRuntime,
}

impl Harness {
    fn new(test: &str, port_base: u16) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("sqldb-engine-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("engine-state")).expect("dir");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-engine.sh");
        let program = dir.join("engine");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nexec bash '{}' '{}' \"$@\"\n",
                fixture.display(),
                dir.join("engine-state").display()
            ),
        )
        .expect("wrapper");
        let mut perm = std::fs::metadata(&program).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&program, perm).expect("chmod");
        let tls = dir.join("tls");
        std::fs::create_dir_all(&tls).expect("tls");
        for f in ["ca.crt", "tls.crt", "tls.key"] {
            std::fs::write(tls.join(f), "not a real certificate").expect("tls file");
        }
        let endpoints = Endpoints::new(
            vec!["127.0.0.1:29379".into()],
            vec!["10.89.0.0/16".parse().expect("cidr")],
        )
        .expect("endpoints");
        let engine = ContainerEngine::custom(program.to_string_lossy().into_owned());
        let mut config = LocalRuntimeConfig::new(engine, dir.join("pools"), tls, endpoints);
        config.instance = "unit".into();
        config.mysql_port_base = port_base;
        config.status_port_base = port_base + 500;
        config.port_span = 100;
        config.stop_timeout = Duration::from_secs(0);
        Self {
            rt: LocalRuntime::new(config).expect("runtime"),
            dir,
        }
    }

    fn switch(&self, name: &str, value: &str) {
        std::fs::write(self.dir.join("engine-state").join(name), value).expect("switch");
    }

    fn containers(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = std::fs::read_dir(self.dir.join("engine-state/c"))
            .map(|d| {
                d.flatten()
                    .map(|e| {
                        let status =
                            std::fs::read_to_string(e.path().join("status")).unwrap_or_default();
                        (
                            e.file_name().to_string_lossy().into_owned(),
                            status.trim().to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("engine-state/calls.log")).unwrap_or_default()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn br(c: char) -> BranchId {
    BranchId::parse(&format!("br_{}", c.to_string().repeat(16))).expect("id")
}

/// Fix 3: a wake on branch B is not serialised behind a slow stop on A.
#[tokio::test(flavor = "multi_thread")]
async fn wake_on_b_is_not_blocked_by_stop_on_a() {
    let h = std::sync::Arc::new(Harness::new("locks", 46_000));
    let (a, b) = (br('a'), br('b'));
    h.rt.ensure_pool(&a, Class::Xs, 1).await.expect("a");
    h.rt.ensure_pool(&b, Class::Xs, 0).await.expect("b");
    h.switch("stop-delay", "3");

    let h2 = h.clone();
    let a2 = a.clone();
    let stop_a = tokio::spawn(async move { h2.rt.scale(&a2, 0).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let t = Instant::now();
    let st = h.rt.scale(&b, 1).await.expect("wake b");
    assert_eq!(st.members.len(), 1);
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "wake on B waited {:?}",
        t.elapsed()
    );
    assert!(!stop_a.is_finished(), "stop on A should still be running");
    stop_a.await.expect("join").expect("stop a");
    assert_eq!(
        h.containers(),
        vec![(format!("loams-sqldb-unit-{b}-0"), "running".to_owned())]
    );
    assert!(
        h.calls()
            .contains(&format!("stop -t 0 loams-sqldb-unit-{a}-0")),
        "{}",
        h.calls()
    );
}

/// Fix 4: a failed `run` does not leave a `created` container behind, and a
/// `created` container found later is replaced.
#[tokio::test]
async fn failed_run_leaves_no_created_container() {
    let h = Harness::new("failed-run", 46_200);
    let a = br('c');
    h.switch("fail-run", "");
    assert!(h.rt.ensure_pool(&a, Class::Xs, 1).await.is_err());
    assert_eq!(h.containers(), vec![], "the created container was removed");
    std::fs::remove_file(h.dir.join("engine-state/fail-run")).expect("clear switch");
    h.rt.ensure_pool(&a, Class::Xs, 1).await.expect("retry");
    assert_eq!(
        h.containers(),
        vec![(format!("loams-sqldb-unit-{a}-0"), "running".to_owned())]
    );
}

#[tokio::test]
async fn created_container_is_replaced_on_reconcile() {
    let h = Harness::new("created", 46_300);
    let a = br('d');
    h.rt.ensure_pool(&a, Class::Xs, 1).await.expect("ensure");
    let name = format!("loams-sqldb-unit-{a}-0");
    // As if a crash interrupted `run` after create and before start.
    std::fs::write(
        h.dir.join("engine-state/c").join(&name).join("status"),
        "created",
    )
    .expect("status");
    h.rt.scale(&a, 1).await.expect("reconcile");
    assert_eq!(h.containers(), vec![(name.clone(), "running".to_owned())]);
    assert_eq!(
        h.calls().lines().filter(|l| l.starts_with("run ")).count(),
        2,
        "{}",
        h.calls()
    );
    assert!(
        h.calls().contains(&format!("rm -f {name}")),
        "{}",
        h.calls()
    );
}

/// Minor: `pool_status` survives a member removed between `ps` and `inspect`.
#[tokio::test]
async fn pool_status_tolerates_concurrent_removal() {
    let h = Harness::new("vanish", 46_400);
    let a = br('e');
    h.rt.ensure_pool(&a, Class::Xs, 2).await.expect("ensure");
    h.switch("vanish-on-inspect", "");
    let st = h.rt.pool_status(&a).await.expect("status").expect("pool");
    assert_eq!(st.members.iter().map(|m| m.index).collect::<Vec<_>>(), [1]);
}

/// Minors: swap equals memory; the host TLS directory is not relabelled.
#[tokio::test]
async fn member_arguments_limit_swap_and_keep_tls_labels() {
    let h = Harness::new("args", 46_500);
    let a = br('f');
    h.rt.ensure_pool(&a, Class::Xs, 1).await.expect("ensure");
    let run = h
        .calls()
        .lines()
        .find(|l| l.starts_with("run "))
        .expect("run")
        .to_owned();
    let mem = Class::Xs.memory_bytes();
    assert!(
        run.contains(&format!("--memory {mem}b --memory-swap {mem}b")),
        "{run}"
    );
    let tls = format!("{}:/etc/tidb/tls:ro ", h.dir.join("tls").display());
    assert!(
        run.contains(&tls),
        "TLS mount must be read-only, not relabelled: {run}"
    );
    assert!(
        !run.contains("/var/run/tidb"),
        "no socket directory (R2.11): {run}"
    );
    assert!(
        run.contains(&Images::load().expect("pins").tidb().reference()),
        "{run}"
    );
}

/// Minor: a job container left by a crash does not block the replay.
#[tokio::test]
async fn run_job_replaces_a_stale_container() {
    let h = Harness::new("job", 46_600);
    let stale = h.dir.join("engine-state/c/loams-sqldb-unit-job-backup-1");
    std::fs::create_dir_all(&stale).expect("stale");
    std::fs::write(stale.join("labels"), "").expect("labels");
    std::fs::write(stale.join("status"), "exited").expect("status");
    h.switch("job-exit", "3");
    let spec = JobSpec {
        name: "backup-1".into(),
        image: Images::load().expect("pins").br().clone(),
        args: vec!["backup".into()],
        env: vec![],
        timeout: Duration::from_secs(30),
    };
    assert_eq!(h.rt.run_job(&spec).await.expect("job").exit_code, 3);
    assert_eq!(h.containers(), vec![]);
}

/// R2.10: bootstrap-only statements (Task 11's `ri_control`; tests' own
/// users) are appended to the rendered init.sql, after the globals.
#[tokio::test]
async fn extra_init_sql_is_appended() {
    let mut h = Harness::new("extra-sql", 46_700);
    let mut config = h.rt.config().clone();
    config.extra_init_sql = vec!["CREATE USER 'x'@'127.0.0.1'".into()];
    h.rt = LocalRuntime::new(config).expect("runtime");
    let a = br('g');
    h.rt.ensure_pool(&a, Class::Xs, 1).await.expect("ensure");
    let sql = std::fs::read_to_string(h.dir.join("pools").join(a.as_str()).join("init.sql"))
        .expect("init.sql");
    let expected = format!(
        "{}CREATE USER 'x'@'127.0.0.1';\n",
        loams_sqldb::render::tidb_init_sql(Class::Xs)
    );
    assert_eq!(sql, expected);
}
