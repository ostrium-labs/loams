//! LocalRuntime against a fake container engine (tests/fixtures/fake-engine.sh):
//! locking, failure cleanup and argument shape, without Podman.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::runtime::SqlRuntime;
use loams_sqldb::runtime::local::{ContainerEngine, LocalRuntime, LocalRuntimeConfig};

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
