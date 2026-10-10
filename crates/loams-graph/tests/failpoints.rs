//! Panic containment (GR1 Task 3; R0.13; Review Focus 5). Needs the `failpoints` feature:
//! `cargo test -p loams-graph --features failpoints --test failpoints`.
#![cfg(feature = "failpoints")]

use connectrpc::ErrorCode;
use loams_graph::{Engine, service};
use loams_proto::loams::graph::v1 as pb;

fn create(engine: &Engine, name: &str) {
    service::create_graph(
        engine,
        pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: name.to_string(),
            ..Default::default()
        },
    )
    .expect("create");
}

fn execute(
    engine: &Engine,
    graph: &str,
    statement: &str,
) -> Result<pb::ExecuteResponse, connectrpc::ConnectError> {
    service::execute(
        engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            graph: graph.to_string(),
            statement: statement.to_string(),
            ..Default::default()
        },
    )
}

fn reason(err: &connectrpc::ConnectError) -> String {
    use base64::Engine as _;
    use buffa::Message as _;
    let detail = err
        .details
        .iter()
        .find(|d| d.type_url == "loams.errors.v1.ErrorInfo")
        .unwrap_or_else(|| panic!("no ErrorInfo in {err:?}"));
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(detail.value.as_deref().unwrap_or_default())
        .expect("base64");
    loams_proto::loams::errors::v1::ErrorInfo::decode_from_slice(&bytes)
        .expect("ErrorInfo")
        .reason
}

#[test]
fn engine_panic_poisons_one_graph() {
    let scenario = fail::FailScenario::setup();
    let data_dir = std::env::temp_dir().join(format!("loams-graph-panic-{}", std::process::id()));
    let engine = Engine::with_data_dir(&data_dir);
    create(&engine, "one");
    create(&engine, "two");
    execute(&engine, "one", "INSERT (:Kept {v: 1})").expect("seed one");
    execute(&engine, "two", "INSERT (:Kept {v: 2})").expect("seed two");

    // The next engine call panics, whichever graph makes it.
    fail::cfg("loams_graph::engine_call", "1*panic(injected)").expect("cfg");
    let err = execute(&engine, "one", "MATCH (k:Kept) RETURN k.v").expect_err("the call panicked");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(reason(&err), "graph_engine_panic");
    let poisoned = service::get_graph(
        &engine,
        pb::GetGraphRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("still listed");
    assert_eq!(poisoned.state.as_known(), Some(pb::GraphState::Reloading));

    // The other graph keeps answering.
    execute(&engine, "two", "MATCH (k:Kept) RETURN k.v").expect("two answers");

    // The first reopens on its next call, from its own storage, with its data.
    let response = execute(&engine, "one", "MATCH (k:Kept) RETURN k.v").expect("one reopened");
    let rows = response.rows.as_option().expect("rows");
    assert_eq!(rows.rows.len(), 1, "the committed node survived the reopen");
    let ready = service::get_graph(
        &engine,
        pb::GetGraphRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("listed");
    assert_eq!(ready.state.as_known(), Some(pb::GraphState::Ready));
    scenario.teardown();
    std::fs::remove_dir_all(&data_dir).ok();
}

/// Security review I2: an in-memory graph is never reopened empty after a panic; it fails and
/// says so, so an acknowledged write is never silently lost. Deleting it still works.
#[test]
fn in_memory_graph_fails_rather_than_reopening_empty() {
    let scenario = fail::FailScenario::setup();
    let engine = Engine::new();
    create(&engine, "mem");
    execute(&engine, "mem", "INSERT (:Kept {v: 1})").expect("an acknowledged write");
    fail::cfg("loams_graph::engine_call", "1*panic(injected)").expect("cfg");
    let err = execute(&engine, "mem", "MATCH (k:Kept) RETURN k.v").expect_err("panicked");
    assert_eq!(reason(&err), "graph_engine_panic");
    for _ in 0..2 {
        let err = execute(&engine, "mem", "MATCH (k:Kept) RETURN k.v")
            .expect_err("never answered from an empty reopen");
        assert_eq!(err.code, ErrorCode::FailedPrecondition, "{err:?}");
        assert_eq!(reason(&err), "graph_engine_panic");
    }
    let state = service::get_graph(
        &engine,
        pb::GetGraphRequest {
            namespace: "acme".to_string(),
            name: "mem".to_string(),
            ..Default::default()
        },
    )
    .expect("listed");
    assert_eq!(state.state.as_known(), Some(pb::GraphState::Failed));
    service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "mem".to_string(),
            ..Default::default()
        },
    )
    .expect("a failed graph can be deleted");
    create(&engine, "mem");
    execute(&engine, "mem", "RETURN 1 AS one").expect("and created again");
    scenario.teardown();
}

/// Security review M5: a panic in Loams's own code around the engine (the gate, and the path
/// resolution that builds rows) is contained like one inside the engine.
#[test]
fn panics_in_the_gate_and_row_building_are_contained() {
    let scenario = fail::FailScenario::setup();
    let data_dir = std::env::temp_dir().join(format!("loams-graph-m5-{}", std::process::id()));
    let engine = Engine::with_data_dir(&data_dir);
    create(&engine, "g");
    execute(&engine, "g", "INSERT (:A)-[:R]->(:B)").expect("seed");
    for failpoint in ["loams_graph::gate", "loams_graph::resolve"] {
        fail::cfg(failpoint, "1*panic(injected)").expect("cfg");
        let err = execute(&engine, "g", "MATCH p = (:A)-[:R]->(:B) RETURN p")
            .expect_err("the panic is answered, not propagated");
        assert_eq!(err.code, ErrorCode::Internal, "{failpoint}: {err:?}");
        assert_eq!(reason(&err), "graph_engine_panic", "{failpoint}");
        // The graph reopens from storage and answers.
        execute(&engine, "g", "MATCH p = (:A)-[:R]->(:B) RETURN p").expect("answers again");
    }
    scenario.teardown();
    std::fs::remove_dir_all(&data_dir).ok();
}

/// Security review M7: while another caller still holds a poisoned graph's handle, it is not
/// reopened (its file lock is held); statements answer `graph_reloading` until the handle goes.
#[test]
fn a_held_poisoned_graph_answers_reloading_until_released() {
    let scenario = fail::FailScenario::setup();
    let data_dir = std::env::temp_dir().join(format!("loams-graph-m7-{}", std::process::id()));
    let engine = Engine::with_data_dir(&data_dir);
    create(&engine, "held");
    execute(&engine, "held", "INSERT (:Kept)").expect("seed");
    let handle = loams_graph::Graph::open_or_existing(&engine, "acme", "held", || {
        unreachable!("the graph is open")
    })
    .expect("the open graph");
    fail::cfg("loams_graph::engine_call", "1*panic(injected)").expect("cfg");
    execute(&engine, "held", "MATCH (k:Kept) RETURN k").expect_err("panicked");
    for _ in 0..2 {
        let err = execute(&engine, "held", "MATCH (k:Kept) RETURN k").expect_err("still held");
        assert_eq!(err.code, ErrorCode::Unavailable, "{err:?}");
        assert_eq!(reason(&err), "graph_reloading");
    }
    drop(handle);
    let response = execute(&engine, "held", "MATCH (k:Kept) RETURN k").expect("reopened");
    assert_eq!(response.rows.as_option().expect("rows").rows.len(), 1);
    scenario.teardown();
    std::fs::remove_dir_all(&data_dir).ok();
}
