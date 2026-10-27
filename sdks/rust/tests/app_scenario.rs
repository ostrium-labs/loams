//! SDKP Parts B and C for Rust: the app scenario through the public facade.
//!
//! `rust_app_collections`, `rust_app_search`, `rust_app_graph`, and (Part C)
//! `rust_app_sql` and `rust_app_streams` replay
//! `sdks/fixtures/recorded/app/` against the conformance fixture server
//! (`sdks/fixtures/app-scenario.md` says how): each step's facade call is made
//! on a `Loams` client with the recorded request — built with the builders when
//! the step names a builder case — and the fixture server compares the body it
//! receives with the recording. The answer is compared with the recorded one,
//! or, for a refusal, the typed error's code and reason. `rust_app_report`
//! drives all of them and writes `sdks/fixtures/results/rust.json`, which
//! `check-languages.mjs --check rust --suite app` reads.

mod support;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use http::{HeaderName, HeaderValue};
use loams::sql_v1 as sql;
use loams::stream_v1 as st;
use loams::{
    CallOptionsOverrides, Loams, LoamsError, TransportOptions, collection_v1 as c, graph_v1 as g,
    instance_v1 as i,
};
use serde_json::{Value, json};
use support::builders::{case, filter, schema, search};

fn sdks() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The fixture server, on a port of its own, stopped on drop.
struct FixtureServer {
    child: Child,
    url: String,
}

impl FixtureServer {
    fn start() -> Self {
        let mut child = Command::new("node")
            .arg(sdks().join("conformance/fixture-server.mjs"))
            .arg("--fixtures")
            .arg(sdks().join("fixtures"))
            .arg("--port")
            .arg("0")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("node runs the fixture server");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().expect("stdout"))
            .read_line(&mut line)
            .expect("the fixture server reports its url");
        let url = serde_json::from_str::<Value>(&line).expect("a JSON line")["url"]
            .as_str()
            .expect("a url")
            .to_owned();
        FixtureServer { child, url }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture(name: &str) -> Value {
    let path = sdks().join(format!("fixtures/recorded/app/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("the recording")).expect("JSON")
}

/// The request JSON a step sends, built with the builders where it names one.
fn request_json(step: &Value) -> Value {
    let mut body: Value = serde_json::from_str(step["request"]["body"].as_str().unwrap_or("{}"))
        .expect("a JSON body");
    let Some(name) = step["builder"].as_str() else {
        return body;
    };
    let entry = case(name);
    match entry["builder"].as_str().expect("a builder") {
        // The builder writes no namespace: the client's default fills it (R11).
        "search" => search(&entry["spec"]).to_json(),
        "schema" => {
            body["schema"] = schema(entry["spec"].as_array().expect("calls"));
            body
        }
        _ => {
            let spec = &entry["spec"];
            body["where"] = filter(spec).to_json();
            body
        }
    }
}

/// A message through its own JSON mapping, so two spellings of one value
/// compare equal.
fn canonical<M: serde::Serialize + serde::de::DeserializeOwned>(value: Value) -> Value {
    serde_json::to_value(serde_json::from_value::<M>(value).expect("decodes")).expect("encodes")
}

/// One facade call: decode the request, call, and encode the answer.
macro_rules! call {
    ($loams:expr, $json:expr, $options:expr, $req:ty, $resp:ty, $($path:tt)+) => {{
        let request: $req = serde_json::from_value($json).expect("the request decodes");
        let answer: Result<$resp, LoamsError> = $loams.$($path)+(request, $options).await;
        answer.map(|answer| {
            (
                serde_json::to_value(&answer).expect("encodes"),
                Box::new(|recorded: Value| canonical::<$resp>(recorded)) as Box<dyn Fn(Value) -> Value>,
            )
        })
    }};
}

async fn replay(url: &str, name: &str) -> Result<(), String> {
    let loams = Loams::connect(TransportOptions::new(url).json(true))
        .map_err(|e| e.to_string())?
        .with_namespace("app")
        .max_retries(0);
    let recording = fixture(name);
    for (at, step) in recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .enumerate()
    {
        let call_name = step["call"].as_str().expect("a call");
        let place = format!("{name} step {at} ({call_name})");
        let options = CallOptionsOverrides::new()
            .header(
                HeaderName::from_static("loams-fixture-name"),
                HeaderValue::from_str(name).expect("ascii"),
            )
            .header(
                HeaderName::from_static("loams-fixture-step"),
                HeaderValue::from_str(&at.to_string()).expect("ascii"),
            );
        let json = request_json(step);
        let outcome = match call_name {
            "instance.get_instance" => call!(
                loams,
                json,
                options,
                i::GetInstanceRequest,
                i::GetInstanceResponse,
                instance().get_instance
            ),
            "namespaces.create" => call!(
                loams,
                json,
                options,
                c::CreateNamespaceRequest,
                c::CreateNamespaceResponse,
                namespaces().create
            ),
            "collections.create" => call!(
                loams,
                json,
                options,
                c::CreateCollectionRequest,
                c::CollectionInfo,
                collections().create
            ),
            "collections.get" => call!(
                loams,
                json,
                options,
                c::GetCollectionRequest,
                c::CollectionInfo,
                collections().get
            ),
            "collections.list" => call!(
                loams,
                json,
                options,
                c::ListCollectionsRequest,
                c::ListCollectionsResponse,
                collections().list
            ),
            "collections.drop" => call!(
                loams,
                json,
                options,
                c::DropCollectionRequest,
                c::DropCollectionResponse,
                collections().drop
            ),
            "documents.write" => call!(
                loams,
                json,
                options,
                c::WriteDocumentsRequest,
                c::WriteDocumentsResponse,
                documents().write
            ),
            "documents.get" => call!(
                loams,
                json,
                options,
                c::GetDocumentsRequest,
                c::GetDocumentsResponse,
                documents().get
            ),
            "documents.count" => call!(
                loams,
                json,
                options,
                c::CountDocumentsRequest,
                c::CountDocumentsResponse,
                documents().count
            ),
            "documents.scroll" => call!(
                loams,
                json,
                options,
                c::ScrollDocumentsRequest,
                c::ScrollDocumentsResponse,
                documents().scroll
            ),
            "search.query" => call!(
                loams,
                json,
                options,
                c::SearchRequest,
                c::SearchResponse,
                search().query
            ),
            "vector.search" => call!(
                loams,
                json,
                options,
                c::SearchRequest,
                c::SearchResponse,
                vector().search
            ),
            "graph.create" => call!(
                loams,
                json,
                options,
                g::CreateGraphRequest,
                g::Graph,
                graph().create
            ),
            "graph.query" => call!(
                loams,
                json,
                options,
                g::ExecuteRequest,
                g::ExecuteResponse,
                graph().query
            ),
            "graph.delete" => call!(
                loams,
                json,
                options,
                g::DeleteGraphRequest,
                loams_proto::loams::operations::v1::Operation,
                graph().delete
            ),
            "sql.query" => call!(
                loams,
                json,
                options,
                sql::QueryRequest,
                sql::QueryResponse,
                sql().query
            ),
            "sql.query_arrow" => call!(
                loams,
                json,
                options,
                sql::QueryArrowRequest,
                sql::QueryArrowResponse,
                sql().query_arrow
            ),
            "streams.create" => call!(
                loams,
                json,
                options,
                st::CreateStreamRequest,
                st::CreateStreamResponse,
                streams().create
            ),
            "streams.get" => call!(
                loams,
                json,
                options,
                st::DescribeStreamRequest,
                st::DescribeStreamResponse,
                streams().get
            ),
            "streams.list" => call!(
                loams,
                json,
                options,
                st::ListStreamsRequest,
                st::ListStreamsResponse,
                streams().list
            ),
            "streams.produce" => call!(
                loams,
                json,
                options,
                st::ProduceRequest,
                st::ProduceResponse,
                streams().produce
            ),
            "streams.fetch" => call!(
                loams,
                json,
                options,
                st::FetchRequest,
                st::FetchResponse,
                streams().fetch
            ),
            "streams.produce_events" => call!(
                loams,
                json,
                options,
                st::ProduceCloudEventsRequest,
                st::ProduceCloudEventsResponse,
                streams().produce_events
            ),
            "streams.fetch_events" => call!(
                loams,
                json,
                options,
                st::FetchCloudEventsRequest,
                st::FetchCloudEventsResponse,
                streams().fetch_events
            ),
            other => return Err(format!("{place}: no replay for {other}")),
        };
        let status = step["expect"]["status"].as_u64().expect("a status");
        match outcome {
            Ok((answer, canonical)) if status == 200 => {
                let want = canonical(step["response"]["body"].clone());
                if answer != want {
                    return Err(format!("{place}: answered {answer}, recorded {want}"));
                }
            }
            Ok((answer, _)) => {
                return Err(format!("{place}: expected a refusal, got {answer}"));
            }
            Err(error) if status != 200 => {
                let reason = error
                    .reason()
                    .map(|r| r.as_str().to_owned())
                    .or_else(|| error.unknown_reason().map(str::to_owned));
                let want = step["expect"]["reason"].as_str().map(str::to_owned);
                if reason != want {
                    return Err(format!("{place}: reason {reason:?}, recorded {want:?}"));
                }
                let code = snake(&format!("{:?}", error.code));
                if Some(code.as_str()) != step["response"]["body"]["code"].as_str() {
                    return Err(format!(
                        "{place}: code {code}, recorded {}",
                        step["response"]["body"]["code"]
                    ));
                }
            }
            Err(error) => return Err(format!("{place}: {error}")),
        }
    }
    Ok(())
}

/// `InvalidArgument` to `invalid_argument`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

async fn replay_on_own_server(name: &str) {
    let server = FixtureServer::start();
    if let Err(problem) = replay(&server.url, name).await {
        panic!("{problem}");
    }
}

#[tokio::test]
async fn rust_app_collections() {
    replay_on_own_server("app_collections").await;
}

#[tokio::test]
async fn rust_app_search() {
    replay_on_own_server("app_search").await;
}

#[tokio::test]
async fn rust_app_graph() {
    replay_on_own_server("app_graph").await;
}

#[tokio::test]
async fn rust_app_sql() {
    replay_on_own_server("app_sql").await;
}

#[tokio::test]
async fn rust_app_streams() {
    replay_on_own_server("app_streams").await;
}

/// `Loams::subscribe` reads the recorded fetch page of `app_streams` and knows
/// where to resume.
#[tokio::test]
async fn rust_streams_subscribe_reads_the_recorded_page() {
    use futures::StreamExt;
    let server = FixtureServer::start();
    let loams = Loams::connect(TransportOptions::new(&server.url).json(true))
        .expect("a client")
        .with_namespace("app")
        .max_retries(0);
    let options = CallOptionsOverrides::new()
        .header(
            HeaderName::from_static("loams-fixture-name"),
            HeaderValue::from_static("app_streams"),
        )
        .header(
            HeaderName::from_static("loams-fixture-step"),
            HeaderValue::from_static("5"),
        );
    let mut subscription = loams.subscribe(
        st::FetchRequest {
            stream: "note-events".into(),
            max_records: Some(10),
            ..Default::default()
        },
        options,
    );
    let mut offsets = Vec::new();
    while offsets.len() < 2 {
        let record = subscription
            .next()
            .await
            .expect("a record")
            .expect("the recorded page");
        offsets.push(record.offset);
    }
    assert_eq!(offsets, vec![0, 1]);
    assert_eq!(subscription.offset(), 2);
}

/// `sql().records` sends the `$1` value and reads the rows as records, against
/// the recorded first step of `app_sql`.
#[tokio::test]
async fn rust_sql_records_reads_rows_as_objects() {
    let server = FixtureServer::start();
    let loams = Loams::connect(TransportOptions::new(&server.url).json(true))
        .expect("a client")
        .with_namespace("app")
        .max_retries(0);
    let options = CallOptionsOverrides::new()
        .header(
            HeaderName::from_static("loams-fixture-name"),
            HeaderValue::from_static("app_sql"),
        )
        .header(
            HeaderName::from_static("loams-fixture-step"),
            HeaderValue::from_static("0"),
        );
    let records = loams
        .sql()
        .records(
            "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags",
            vec![json!(1)],
            options,
        )
        .await
        .expect("the recorded answer");
    let records: Vec<Value> = records.into_iter().map(Value::Object).collect();
    assert_eq!(
        records,
        vec![
            json!({"tags": "policy", "c": 2.0}),
            json!({"tags": "shipping", "c": 1.0})
        ]
    );
}

/// Every app fixture, and the report `check-languages --suite app` reads. The
/// report claims only what this run replayed.
#[tokio::test]
async fn rust_app_report() {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(sdks().join("fixtures/manifest.json")).expect("the manifest"),
    )
    .expect("JSON");
    let names: Vec<String> = manifest["fixtures"]
        .as_array()
        .expect("fixtures")
        .iter()
        .filter_map(|f| f["name"].as_str())
        .filter(|name| name.starts_with("app_"))
        .map(str::to_owned)
        .collect();
    let server = FixtureServer::start();
    let mut ran = Vec::new();
    for name in &names {
        replay(&server.url, name)
            .await
            .unwrap_or_else(|problem| panic!("{problem}"));
        ran.push(name.clone());
    }
    ran.sort();
    let results = sdks().join("fixtures/results");
    std::fs::create_dir_all(&results).expect("results/");
    let report = json!({"tests": [], "ran": ran, "skipped": [], "transport": "connect"});
    std::fs::write(
        results.join("rust.json"),
        format!("{}\n", serde_json::to_string_pretty(&report).expect("JSON")),
    )
    .expect("the report");
}
