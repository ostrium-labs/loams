use std::path::PathBuf;

use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::render;
use proptest::prelude::*;

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (UPDATE_GOLDEN=1 writes it)", path.display()));
    assert_eq!(
        actual,
        expected,
        "{} differs (UPDATE_GOLDEN=1 rewrites it)",
        path.display()
    );
}

fn branch() -> BranchId {
    BranchId::parse("br_0123456789abcdef").expect("valid id")
}

/// The desktop shape: the spike stack's PD and a loopback gate.
fn desktop() -> Endpoints {
    Endpoints::new(
        vec!["127.0.0.1:29379".into()],
        vec!["127.0.0.1/32".parse().expect("cidr")],
    )
    .expect("endpoints")
}

/// The Kubernetes shape: three PDs, a dual-stack gate network, cluster TLS.
fn kubernetes() -> Endpoints {
    Endpoints::new(
        vec![
            "pd-0.pd.loams.svc:2379".into(),
            "pd-1.pd.loams.svc:2379".into(),
            "pd-2.pd.loams.svc:2379".into(),
        ],
        vec![
            "10.42.0.0/16".parse().expect("v4"),
            "fd00:42::/64".parse().expect("v6"),
        ],
    )
    .expect("endpoints")
    .with_cluster_tls(true)
}

#[test]
fn tidb_config_golden_xs() {
    golden(
        "tidb-xs.toml",
        &render::tidb(&branch(), Class::Xs, &desktop()),
    );
    golden("tidb-xs.init.sql", &render::tidb_init_sql(Class::Xs));
}

#[test]
fn tidb_config_golden_xl() {
    golden(
        "tidb-xl.toml",
        &render::tidb(&branch(), Class::Xl, &kubernetes()),
    );
    golden("tidb-xl.init.sql", &render::tidb_init_sql(Class::Xl));
}

#[test]
fn rendered_config_parses_with_the_required_keys() {
    for class in Class::ALL {
        let text = render::tidb(&branch(), class, &kubernetes());
        let t: toml::Table = text.parse().expect("valid TOML");
        assert_eq!(t["store"].as_str(), Some("tikv"));
        assert_eq!(
            t["path"].as_str(),
            Some("pd-0.pd.loams.svc:2379,pd-1.pd.loams.svc:2379,pd-2.pd.loams.svc:2379")
        );
        assert_eq!(t["split-table"].as_bool(), Some(false));
        assert_eq!(
            t["server-version"].as_str(),
            Some("8.0.11-TiDB-v8.5.8-Loams")
        );
        assert_eq!(t["enable-global-kill"].as_bool(), Some(true));
        assert_eq!(
            t["initialize-sql-file"].as_str(),
            Some(render::INIT_SQL_PATH)
        );
        let perf = t["performance"].as_table().expect("[performance]");
        assert_eq!(perf["force-init-stats"].as_bool(), Some(false));
        assert_eq!(perf["lite-init-stats"].as_bool(), Some(true));
        let pp = t["proxy-protocol"].as_table().expect("[proxy-protocol]");
        assert_eq!(pp["networks"].as_str(), Some("10.42.0.0/16,fd00:42::/64"));
        assert_eq!(pp["fallbackable"].as_bool(), Some(false));
        let sec = t["security"].as_table().expect("[security]");
        for key in [
            "ssl-ca",
            "ssl-cert",
            "ssl-key",
            "cluster-ssl-ca",
            "cluster-ssl-cert",
            "cluster-ssl-key",
        ] {
            assert!(
                sec[key]
                    .as_str()
                    .is_some_and(|p| p.starts_with("/etc/tidb/")),
                "{key}"
            );
        }
        assert!(t.contains_key("log"));
    }
    // Without cluster TLS no cluster-ssl-* key is rendered.
    let t: toml::Table = render::tidb(&branch(), Class::S, &desktop())
        .parse()
        .expect("toml");
    assert!(
        !t["security"]
            .as_table()
            .expect("sec")
            .contains_key("cluster-ssl-ca")
    );
}

#[test]
fn memory_limits_follow_the_class() {
    // R2.1/R2.2: tidb_server_memory_limit is '80%' of the memory TiDB sees
    // (the container or pod limit, through cgroups), so it follows a class
    // change by itself; the per-query quota is 40 % in bytes, by class.
    for class in Class::ALL {
        let sql = render::tidb_init_sql(class);
        let mib = class.memory_mib();
        assert!(
            sql.contains("SET GLOBAL tidb_server_memory_limit = '80%';"),
            "{class}: {sql}"
        );
        assert!(!sql.contains("MB'"), "no fixed size: {sql}");
        assert!(
            sql.contains(&format!("tidb_mem_quota_query = {};", (mib * 2 / 5) << 20)),
            "{class}"
        );
        let globals = render::tidb_globals(class);
        let names: Vec<&str> = globals.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "tidb_server_memory_limit",
                "tidb_mem_quota_query",
                "tidb_redact_log"
            ]
        );
        // TiDB clamps a limit under 512 MiB up to 512 MiB (varsutil.go).
        assert!(class.server_memory_limit_mib() >= 512, "{class}");
    }
    assert_eq!(Class::Xs.memory_mib(), 768, "R2.1: xs is 0.75 GiB");
}

/// R2.9, R2.10: Security Enhanced Mode is on, and the first bootstrap
/// creates root as `auth_socket` on the pod-local socket only.
#[test]
fn sem_and_secure_bootstrap_are_on() {
    for endpoints in [desktop(), kubernetes()] {
        for class in Class::ALL {
            let t: toml::Table = render::tidb(&branch(), class, &endpoints)
                .parse()
                .expect("toml");
            let sec = t["security"].as_table().expect("[security]");
            assert_eq!(
                sec.get("enable-sem").and_then(toml::Value::as_bool),
                Some(true)
            );
            assert_eq!(
                sec.get("secure-bootstrap").and_then(toml::Value::as_bool),
                Some(true)
            );
            assert_eq!(sec.get("skip-grant-table"), None);
            // root@localhost (auth_socket) is reachable only through this socket.
            assert_eq!(
                t["socket"].as_str(),
                Some(format!("{}/tidb-{{Port}}.sock", render::SOCKET_DIR).as_str())
            );
        }
    }
}

/// R2.8: redaction is OFF. TiDB redacts error messages when they are
/// created (pingcap/errors), so ON or MARKER would send clients
/// `Duplicate entry '?'` and break MySQL compatibility (D735). Logs are
/// protected by the log pipeline instead.
#[test]
fn redaction_is_off_for_mysql_compatibility() {
    for class in Class::ALL {
        let sql = render::tidb_init_sql(class);
        assert!(sql.contains("SET GLOBAL tidb_redact_log = 'OFF';"), "{sql}");
        let globals = render::tidb_globals(class);
        assert!(
            globals.contains(&("tidb_redact_log", "'OFF'".to_owned())),
            "{globals:?}"
        );
        for text in [sql, render::tidb(&branch(), class, &kubernetes())] {
            assert!(!text.contains("MARKER"), "{text}");
            assert!(!text.contains("tidb_redact_log = 'ON'"), "{text}");
        }
    }
}

#[test]
fn rendered_config_holds_no_secret() {
    const FORBIDDEN: [&str; 9] = [
        "password",
        "passwd",
        "secret",
        "token",
        "identified",
        "-----begin",
        "private",
        "credential",
        "auth_string",
    ];
    for endpoints in [desktop(), kubernetes()] {
        for class in Class::ALL {
            for text in [
                render::tidb(&branch(), class, &endpoints),
                render::tidb_init_sql(class),
            ] {
                let lower = text.to_ascii_lowercase();
                for word in FORBIDDEN {
                    assert!(
                        !lower.contains(word),
                        "{word:?} in rendered output:\n{text}"
                    );
                }
                // No user is created or altered by rendered config.
                for stmt in ["create user", "alter user", "grant ", "set password"] {
                    assert!(!lower.contains(stmt), "{stmt:?} in rendered output");
                }
            }
            // Key material is referenced by path only.
            let t: toml::Table = render::tidb(&branch(), class, &endpoints)
                .parse()
                .expect("toml");
            let key = t["security"]["ssl-key"].as_str().expect("ssl-key");
            assert!(key.starts_with('/') && !key.contains('\n'), "{key}");
        }
    }
}

#[test]
fn branch_ids_and_endpoints_are_validated() {
    for bad in [
        "",
        "br_",
        "br_0123456789abcde",
        "br_0123456789abcdefg",
        "db_0123456789abcdef",
        "br_0123456789ABCDEF",
        "br_0123456789abcd\"f",
        "br_0123456789abc def",
    ] {
        assert!(BranchId::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Endpoints::new(vec![], vec!["10.0.0.0/8".parse().expect("cidr")]).is_err());
    assert!(Endpoints::new(vec!["pd:2379".into()], vec![]).is_err());
    for bad_pd in [
        "pd", "pd:", ":2379", "pd:x", "a\"b:1", "a,b:1", "a b:1", "pd:70000",
    ] {
        assert!(
            Endpoints::new(
                vec![bad_pd.into()],
                vec!["10.0.0.0/8".parse().expect("cidr")]
            )
            .is_err(),
            "{bad_pd:?}"
        );
    }
    for bad_cidr in [
        "*",
        "0.0.0.0/0",
        "::/0",
        "10.0.0.0",
        "10.0.0.0/33",
        "fd00::/129",
        "x/8",
    ] {
        assert!(
            bad_cidr.parse::<loams_sqldb::model::Cidr>().is_err(),
            "{bad_cidr:?}"
        );
    }
}

fn arb_endpoints() -> impl Strategy<Value = Endpoints> {
    let pd = prop::collection::vec(("[a-z][a-z0-9.-]{0,20}", 1u16..=65535), 1..4).prop_map(|v| {
        v.into_iter()
            .map(|(h, p)| format!("{h}:{p}"))
            .collect::<Vec<_>>()
    });
    let nets = prop::collection::vec((any::<[u8; 4]>(), 1u8..=32), 1..4).prop_map(|v| {
        v.into_iter()
            .map(|(a, n)| {
                format!("{}.{}.{}.{}/{n}", a[0], a[1], a[2], a[3])
                    .parse()
                    .expect("cidr")
            })
            .collect::<Vec<_>>()
    });
    (pd, nets, any::<bool>()).prop_map(|(pd, nets, ctls)| {
        Endpoints::new(pd, nets)
            .expect("valid endpoints")
            .with_cluster_tls(ctls)
    })
}

proptest! {
    #[test]
    fn config_never_omits_keyspace_name(
        suffix in "[0-9a-z]{16}",
        class in prop::sample::select(Class::ALL.to_vec()),
        endpoints in arb_endpoints(),
    ) {
        let id = BranchId::parse(&format!("br_{suffix}")).expect("valid id");
        let text = render::tidb(&id, class, &endpoints);
        let t: toml::Table = text.parse().expect("valid TOML");
        let ks = t.get("keyspace-name").and_then(|v| v.as_str()).unwrap_or_default();
        prop_assert!(!ks.is_empty());
        prop_assert_eq!(ks, id.as_str());
        prop_assert_eq!(t["store"].as_str(), Some("tikv"));
        prop_assert_eq!(t["split-table"].as_bool(), Some(false));
    }
}
