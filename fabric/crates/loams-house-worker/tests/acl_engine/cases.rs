//! The control connection's statements that R11.11's shape check refuses before
//! the engine sees them (`serve::control_ddl_shape`), run straight on an engine
//! booted in this process with a worker's own `config.xml` and `users.xml`
//! (HS1 R-ACL.1): the grants behind that check refuse them too. Each case runs
//! on a sealed engine (`acl_engine.rs`), which must refuse it with a missing
//! grant or a pinned setting, and on an open one (`acl_engine_open.rs`, chDB's
//! own `GRANT ALL` and no profile), which must not. One engine per process, so
//! one file each.

use std::net::TcpListener;
use std::path::PathBuf;

use loams_chdb::{ChdbError, Engine, SessionId, Settings};
use loams_house_ipc::SandboxMode;
use loams_house_worker::serve::{USER_CONNECTION_ARGS, replace_default_database};
use loams_house_worker::{WorkerArgs, config, engine_config};

fn is_access_control(err: &ChdbError) -> bool {
    match err.code {
        497 => err.message.contains("Not enough privileges"),
        452 | 164 => true,
        _ => false,
    }
}

pub fn run(open: bool) {
    let tmp_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("house-acl-engine")
        .join(format!(
            "{}-{}",
            if open { "open" } else { "sealed" },
            std::process::id()
        ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let args = WorkerArgs {
        tmp_dir,
        worker_id: "acl".to_string(),
        memory_limit: config::DEFAULT_MEMORY_LIMIT,
        s3_endpoint: None,
        sandbox: SandboxMode::None,
        forwarder_port: None,
        cgroup: None,
        open_acl: open,
        max_execution_time_s: loams_house_worker::settings::MAX_EXECUTION_TIME_S,
    };
    config::write_files(&args).expect("config files");
    let mut engine_config = engine_config(&args);
    engine_config.install_signal_handlers = false;
    let engine = Engine::start(engine_config).expect("the engine starts");
    let control = engine
        .session(SessionId::new("control"), &Settings::new())
        .expect("control");
    replace_default_database(&control).expect("default is Memory");
    let user_args: Vec<String> = USER_CONNECTION_ARGS.iter().map(|a| a.to_string()).collect();
    let user = engine
        .session_with_args(SessionId::new("user"), &Settings::new(), &user_args)
        .expect("user");

    // Nothing listens here: a statement past the grants fails on the network.
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().expect("addr").port()
    );
    drop(listener);
    let fast = if open {
        " SETTINGS http_max_tries = 1, http_retry_max_backoff_ms = 10, s3_retry_attempts = 0"
    } else {
        ""
    };
    let out = args.tmp_dir.join("loams-acl.csv");
    let control_cases = [
        "SYSTEM DROP DNS CACHE".to_string(),
        "SYSTEM DROP QUERY CACHE".to_string(),
        "CREATE TABLE default.loams_acl_t (a UInt8) ENGINE = Memory".to_string(),
        format!("SELECT * FROM url('{url}', LineAsString){fast}"),
        format!("SELECT * FROM s3('{url}bkt/k', 'dummy', 'dummy', LineAsString){fast}"),
        format!(
            "INSERT INTO FUNCTION file('{}', CSV, 'a String') SELECT 'x'",
            out.display()
        ),
        "SELECT addressToLine(toUInt64(0)) SETTINGS allow_introspection_functions = 1".to_string(),
        "DROP DATABASE information_schema".to_string(),
        "SELECT 1 SETTINGS max_rows_to_group_by = 1".to_string(),
    ];
    let mut failures = Vec::new();
    for sql in &control_cases {
        let result = control.query(sql, "TSV", &[]);
        let fine = match (&result, open) {
            (Err(err), false) => is_access_control(err),
            (Ok(_), true) => true,
            (Err(err), true) => !is_access_control(err),
            (Ok(_), false) => false,
        };
        if !fine {
            failures.push(format!("control: {sql}: {result:?}"));
        }
    }
    // HE1 R14.12: the control connection makes dictionaries, but one over a
    // source reads nothing on a sealed engine (no `URL` grant), whoever asks.
    let http_dictionary = format!(
        "CREATE DICTIONARY default.loams_acl_d (k UInt64, v String) PRIMARY KEY k \
         SOURCE(HTTP(url '{url}' format 'TSV')) LAYOUT(FLAT()) LIFETIME(0)"
    );
    match control.query(&http_dictionary, "TSV", &[]) {
        Err(err) if !open && is_access_control(&err) => {}
        Err(err) => failures.push(format!("control: HTTP dictionary: {err:?}")),
        Ok(_) => {
            let read = user.query(
                "SELECT dictGet('default.loams_acl_d', 'v', toUInt64(1))",
                "TSV",
                &[],
            );
            let fine = match &read {
                Err(err) => is_access_control(err) != open,
                Ok(_) => false,
            };
            if !fine {
                failures.push(format!("user: dictGet over HTTP: {read:?}"));
            }
        }
    }
    // HE1 R14.11: the control connection makes SQL functions; the user
    // connection may not (its `readonly`), on either engine.
    let function = user.query("CREATE FUNCTION loams_acl_uf AS (x) -> x + 1", "TSV", &[]);
    if !matches!(&function, Err(err) if is_access_control(err)) {
        failures.push(format!("user: CREATE FUNCTION: {function:?}"));
    }
    let dropped = user.query("DROP FUNCTION IF EXISTS loams_acl_uf", "TSV", &[]);
    if !matches!(&dropped, Err(err) if is_access_control(err)) {
        failures.push(format!("user: DROP FUNCTION: {dropped:?}"));
    }
    if !open
        && let Err(err) = control.query(
            "CREATE OR REPLACE FUNCTION loams_acl_f AS (x) -> x + 1",
            "TSV",
            &[],
        )
    {
        failures.push(format!("control: CREATE FUNCTION: {err:?}"));
    }
    // The user connection may not make a view that would lend it a source:
    // `readonly` strips `ALLOW SQL SECURITY NONE` on both engines, and only
    // the open one has `SET DEFINER`.
    let definer = format!(
        "CREATE TEMPORARY VIEW loams_acl_tv (a String) DEFINER = default SQL SECURITY DEFINER \
         AS SELECT * FROM url('{url}', LineAsString)"
    );
    let none = user.query(
        &format!(
            "CREATE TEMPORARY VIEW loams_acl_tn (a String) SQL SECURITY NONE AS \
             SELECT * FROM url('{url}', LineAsString)"
        ),
        "TSV",
        &[],
    );
    if !matches!(&none, Err(err) if err.code == 164) {
        failures.push(format!("user: SQL SECURITY NONE view: {none:?}"));
    }
    if !open {
        // The definer is the user itself, whose grants hold no source: the view
        // may be made, but it reads nothing.
        let _ = user.query(&definer, "TSV", &[]);
        let read = user.query("SELECT * FROM loams_acl_tv", "TSV", &[]);
        if !matches!(&read, Err(err) if is_access_control(err)) {
            failures.push(format!("user: DEFINER view: {read:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
