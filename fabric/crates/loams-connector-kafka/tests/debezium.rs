//! Debezium change events on the Kafka source (CC1 Task 3, D1822, design
//! §80 §4.2): Debezium's own documented example events
//! (`tests/fixtures/debezium/`, Apache-2.0, see its README) decoded into
//! Loams `change` events shaped as the Postgres and MySQL CDC sources emit
//! them; Avro through the fake Schema Registry; and a malformed record
//! dead-lettered by a real source pipeline on librdkafka's mock cluster.
//! No external system is needed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use apache_avro::Schema as AvroSchema;
use apache_avro::writer::datum::GenericDatumWriter;
use common::*;
use loams_connector_kafka::config::{DebeziumConfig, RecordFormat, Tombstones};
use loams_connector_kafka::{ConfluentRegistry, DebeziumDecoder, RawRecord};
use loams_connector_sdk::schema::registry::{SchemaRegistry, frame};
use loams_connector_sdk::testing::memory::{MemoryCheckpointStore, MemoryFabric};
use loams_connector_sdk::{AllowAll, CloudEvent, EnvelopeBuilder, ErrorClass, InstanceKey};
use serde_json::{Value, json};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/debezium")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn envelope() -> EnvelopeBuilder {
    EnvelopeBuilder::new("kafka", &InstanceKey::new(1, "inventory-in"))
}

fn json_decoder(cfg: DebeziumConfig) -> DebeziumDecoder {
    DebeziumDecoder::new(envelope(), RecordFormat::DebeziumJson, cfg, None).expect("decoder")
}

fn record(topic: &str, offset: i64, key: Option<Vec<u8>>, value: Option<Vec<u8>>) -> RawRecord {
    RawRecord {
        topic: topic.to_owned(),
        partition: 0,
        offset,
        key,
        value,
        timestamp_ms: Some(1_760_000_000_000),
    }
}

fn data(ev: &CloudEvent) -> Value {
    serde_json::from_slice(&ev.data).expect("data is JSON")
}

fn ext<'a>(ev: &'a CloudEvent, name: &str) -> Option<&'a str> {
    ev.extensions.get(name).map(String::as_str)
}

const PG_TOPIC: &str = "PostgreSQL_server.public.customers";
const MY_TOPIC: &str = "mysql-server-1.inventory.customers";

#[tokio::test]
async fn decodes_pg_create_update_delete() {
    let d = json_decoder(DebeziumConfig::default());
    let key = fixture("pg-key.json");

    let c = d
        .decode(&record(
            PG_TOPIC,
            10,
            Some(key.clone()),
            Some(fixture("pg-create.json")),
        ))
        .await
        .expect("create")
        .expect("an event");
    // The Postgres source's envelope: change type, table as source and
    // subject, a stable id, loamsop and loamslsn.
    assert_eq!(c.ty, "io.loams.dev.flow.kafka.change.v1");
    assert_eq!(c.source, "/connectors/inventory-in/public.customers");
    assert_eq!(c.subject.as_deref(), Some("public.customers"));
    assert_eq!(c.id, format!("{PG_TOPIC}/0/10"));
    assert_eq!(c.datacontenttype.as_deref(), Some("application/json"));
    assert_eq!(ext(&c, "loamsop"), Some("c"));
    // source.lsn 24023128 = 0x16E9058, as the Postgres source writes it.
    assert_eq!(ext(&c, "loamslsn"), Some("00000000016E9058/0000000000"));
    assert_eq!(c.time.as_deref(), Some("2019-05-28T08:58:24.863Z"));
    assert_eq!(
        data(&c),
        json!({
            "table": "public.customers",
            "op": "c",
            "key": {"id": "1"},
            "before": null,
            "after": {"id": 1, "first_name": "Anne", "last_name": "Kretchmar", "email": "annek@noanswer.org"},
        })
    );

    // Update and delete: the payload without the converter's wrapper.
    let u = d
        .decode(&record(
            PG_TOPIC,
            11,
            Some(key.clone()),
            Some(fixture("pg-update.json")),
        ))
        .await
        .expect("update")
        .expect("an event");
    assert_eq!(ext(&u, "loamsop"), Some("u"));
    assert_eq!(data(&u)["before"], json!({"id": 1}));
    assert_eq!(data(&u)["after"]["first_name"], "Anne Marie");
    assert_eq!(u.time.as_deref(), Some("2016-06-10T18:40:25.523Z"));

    let del = d
        .decode(&record(
            PG_TOPIC,
            12,
            Some(key),
            Some(fixture("pg-delete.json")),
        ))
        .await
        .expect("delete")
        .expect("an event");
    assert_eq!(ext(&del, "loamsop"), Some("d"));
    assert_eq!(data(&del)["after"], Value::Null);
    assert_eq!(data(&del)["before"], json!({"id": 1}));
    assert_eq!(data(&del)["key"], json!({"id": "1"}));
    assert_eq!(ext(&del, "loamslsn"), Some("0000000002C5E2F8/0000000000"));

    // key_fields: the key from the row instead of the record key.
    let by_row = json_decoder(DebeziumConfig {
        key_fields: vec!["id".to_owned()],
        ..DebeziumConfig::default()
    });
    let c = by_row
        .decode(&record(PG_TOPIC, 10, None, Some(fixture("pg-create.json"))))
        .await
        .expect("create")
        .expect("an event");
    assert_eq!(data(&c)["key"], json!({"id": 1}));
    // A re-read of the same offset is the same event (ingest dedups it).
    let again = by_row
        .decode(&record(PG_TOPIC, 10, None, Some(fixture("pg-create.json"))))
        .await
        .expect("create")
        .expect("an event");
    assert_eq!(again, c);
}

#[tokio::test]
async fn decodes_mysql_with_schema_wrapper() {
    let d = json_decoder(DebeziumConfig::default());
    let key = fixture("mysql-key.json");
    let c = d
        .decode(&record(
            MY_TOPIC,
            3,
            Some(key.clone()),
            Some(fixture("mysql-create.json")),
        ))
        .await
        .expect("create")
        .expect("an event");
    assert_eq!(c.subject.as_deref(), Some("inventory.customers"));
    assert_eq!(ext(&c, "loamsop"), Some("c"));
    let v = data(&c);
    assert_eq!(v["table"], "inventory.customers");
    assert_eq!(v["key"], json!({"id": 1001}));
    assert_eq!(v["after"]["email"], "annek@noanswer.org");
    // The MySQL source's loamslsn: file / pos (20 digits) / row (10).
    let source: Value = serde_json::from_slice::<Value>(&fixture("mysql-create.json"))
        .expect("fixture")["payload"]["source"]
        .clone();
    let want = format!(
        "{}/{:020}/{:010}",
        source["file"].as_str().expect("file"),
        source["pos"].as_u64().expect("pos"),
        source["row"].as_u64().expect("row")
    );
    assert_eq!(ext(&c, "loamslsn"), Some(want.as_str()));

    for (name, op) in [("mysql-update.json", "u"), ("mysql-delete.json", "d")] {
        let ev = d
            .decode(&record(MY_TOPIC, 4, Some(key.clone()), Some(fixture(name))))
            .await
            .expect(name)
            .expect("an event");
        assert_eq!(ext(&ev, "loamsop"), Some(op), "{name}");
        assert_eq!(data(&ev)["key"], json!({"id": 1001}), "{name}");
    }

    // The schema-change topic: passed through as schema-change, Debezium's
    // payload as the data.
    let sc = d
        .decode(&record(
            "mysql",
            7,
            Some(br#"{"databaseName":"inventory"}"#.to_vec()),
            Some(fixture("mysql-schema-change.json")),
        ))
        .await
        .expect("schema change")
        .expect("an event");
    assert_eq!(sc.ty, "io.loams.dev.flow.kafka.schema-change.v1");
    assert_eq!(sc.subject.as_deref(), Some("inventory"));
    assert!(
        data(&sc)["ddl"]
            .as_str()
            .expect("ddl")
            .starts_with("ALTER TABLE customers")
    );
    assert!(sc.extensions.get("loamsop").is_none());
}

#[tokio::test]
async fn tombstone_delete_or_drop() {
    let key = fixture("mysql-key.json");
    let delete = json_decoder(DebeziumConfig::default());
    let ev = delete
        .decode(&record(MY_TOPIC, 9, Some(key.clone()), None))
        .await
        .expect("tombstone")
        .expect("a delete");
    assert_eq!(ext(&ev, "loamsop"), Some("d"));
    assert_eq!(ev.subject.as_deref(), Some("inventory.customers"));
    assert_eq!(
        data(&ev),
        json!({"table": "inventory.customers", "op": "d", "key": {"id": 1001}, "before": null, "after": null})
    );
    // An empty value is a tombstone too.
    assert!(
        delete
            .decode(&record(MY_TOPIC, 9, Some(key.clone()), Some(Vec::new())))
            .await
            .expect("tombstone")
            .is_some()
    );

    let drop = json_decoder(DebeziumConfig {
        tombstones: Tombstones::Drop,
        ..DebeziumConfig::default()
    });
    assert_eq!(
        drop.decode(&record(MY_TOPIC, 9, Some(key), None))
            .await
            .expect("tombstone"),
        None
    );
    // A record with neither key nor value is nothing Debezium writes.
    let err = delete
        .decode(&record(MY_TOPIC, 9, None, None))
        .await
        .expect_err("poison");
    assert_eq!(err.class, ErrorClass::Poison);
}

/// A Debezium-shaped Avro envelope for MySQL's `customers` table.
const AVRO_VALUE: &str = r#"{"type":"record","name":"Envelope","namespace":"mysql_server_1.inventory.customers","fields":[
 {"name":"before","type":["null",{"type":"record","name":"Value","fields":[
   {"name":"id","type":"int"},{"name":"first_name","type":"string"},
   {"name":"last_name","type":"string"},{"name":"email","type":"string"}]}],"default":null},
 {"name":"after","type":["null","Value"],"default":null},
 {"name":"source","type":{"type":"record","name":"Source","namespace":"io.debezium.connector.mysql","fields":[
   {"name":"version","type":"string"},{"name":"connector","type":"string"},{"name":"name","type":"string"},
   {"name":"ts_ms","type":"long"},{"name":"db","type":"string"},{"name":"table","type":["null","string"],"default":null},
   {"name":"file","type":"string"},{"name":"pos","type":"long"},{"name":"row","type":"int"}]}},
 {"name":"op","type":"string"},
 {"name":"ts_ms","type":["null","long"],"default":null}]}"#;

const AVRO_KEY: &str = r#"{"type":"record","name":"Key","namespace":"mysql_server_1.inventory.customers","fields":[{"name":"id","type":"int"}]}"#;

fn avro(schema: &str, id: u32, value: &Value) -> Vec<u8> {
    let schema = AvroSchema::parse_str(schema).expect("schema");
    let v = apache_avro::types::Value::try_from(value.clone())
        .expect("avro value")
        .resolve(&schema)
        .expect("resolves");
    let body = GenericDatumWriter::builder(&schema)
        .build()
        .expect("writer")
        .write_value_to_vec(v)
        .expect("encoded");
    frame(id, &body)
}

#[tokio::test]
async fn avro_via_registry() {
    let (base, seen) = fake_registry_with(vec![
        ("GET", "/schemas/ids/21", 200, json!({ "schema": AVRO_VALUE })),
        ("GET", "/schemas/ids/22", 200, json!({ "schema": AVRO_KEY })),
        (
            "GET",
            "/schemas/ids/23",
            200,
            json!({ "schema": "syntax = \"proto3\"; message M { int64 a = 1; }", "schemaType": "PROTOBUF" }),
        ),
    ])
    .await;
    let registry: Arc<dyn SchemaRegistry> =
        Arc::new(ConfluentRegistry::new(&base, Arc::new(AllowAll)));
    let d = DebeziumDecoder::new(
        envelope(),
        RecordFormat::DebeziumAvro,
        DebeziumConfig::default(),
        Some(registry),
    )
    .expect("decoder");

    // The documented MySQL create event's payload, in Avro.
    let doc: Value = serde_json::from_slice(&fixture("mysql-create.json")).expect("fixture");
    let p = &doc["payload"];
    let s = &p["source"];
    let value = json!({
        "before": null,
        "after": p["after"],
        "source": {
            "version": s["version"], "connector": s["connector"], "name": s["name"],
            "ts_ms": s["ts_ms"], "db": s["db"], "table": s["table"],
            "file": s["file"], "pos": s["pos"], "row": s["row"],
        },
        "op": "c",
        "ts_ms": p["ts_ms"],
    });
    let rec = record(
        MY_TOPIC,
        5,
        Some(avro(AVRO_KEY, 22, &json!({"id": 1001}))),
        Some(avro(AVRO_VALUE, 21, &value)),
    );
    let ev = d.decode(&rec).await.expect("avro").expect("an event");
    assert_eq!(ext(&ev, "loamsop"), Some("c"));
    assert_eq!(ev.subject.as_deref(), Some("inventory.customers"));
    let v = data(&ev);
    assert_eq!(v["key"], json!({"id": 1001}));
    assert_eq!(v["after"], p["after"]);
    assert_eq!(v["before"], Value::Null);
    assert!(ext(&ev, "loamslsn").is_some());

    // The same output as the JSON converter's, for the same event.
    let as_json = json_decoder(DebeziumConfig::default())
        .decode(&record(
            MY_TOPIC,
            5,
            Some(fixture("mysql-key.json")),
            Some(fixture("mysql-create.json")),
        ))
        .await
        .expect("json")
        .expect("an event");
    assert_eq!(data(&as_json), v);

    // Each schema is fetched once.
    d.decode(&rec).await.expect("again");
    let gets = seen
        .lock()
        .expect("log")
        .iter()
        .filter(|s| s.path == "/schemas/ids/21")
        .count();
    assert_eq!(gets, 1);

    // Unknown id, a non-Avro schema, bytes that do not decode, a value
    // that is not framed: poison.
    for bad in [
        frame(99, b"\x00"),
        frame(23, b"\x00"),
        frame(21, b"\x02\x07"),
        br#"{"op":"c"}"#.to_vec(),
    ] {
        let err = d
            .decode(&record(MY_TOPIC, 6, None, Some(bad)))
            .await
            .expect_err("poison");
        assert_eq!(err.class, ErrorClass::Poison, "{err}");
    }
    // Without a registry, debezium-avro is refused at start.
    assert!(
        DebeziumDecoder::new(
            envelope(),
            RecordFormat::DebeziumAvro,
            DebeziumConfig::default(),
            None
        )
        .is_err()
    );
}

/// Malformed records through a real source pipeline on the mock cluster:
/// each goes to the DLQ with its position, and the change events around
/// them are ingested.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_goes_to_dlq() {
    let target = Target::mock();
    let topic = unique("dbz");
    target.create_topic(&topic, 1, false).await;
    let key = String::from_utf8(fixture("mysql-key.json")).expect("utf-8");
    let create = fixture("mysql-create.json");
    produce_raw(
        &target,
        &topic,
        Some(0),
        vec![
            (Some(key.clone()), create.clone(), vec![]),
            (Some(key.clone()), b"not json".to_vec(), vec![]),
            (
                Some(key.clone()),
                br#"{"before":null,"after":{"id":1},"source":{"db":"d","table":"t"}}"#.to_vec(),
                vec![],
            ),
            (
                Some(key.clone()),
                br#"{"op":"x","source":{"db":"d","table":"t"}}"#.to_vec(),
                vec![],
            ),
            (Some(key.clone()), fixture("mysql-update.json"), vec![]),
        ],
    )
    .await;

    let inst = InstanceKey::new(1, unique("dbz-src"));
    let cfg = target.config(
        &[&topic],
        json!({ "auto_offset_reset": "earliest", "group_id_prefix": unique("grp"), "format": "debezium-json" }),
    );
    let fabric = MemoryFabric::new();
    let store = MemoryCheckpointStore::new();
    let source = source_pipeline(
        &target,
        &inst,
        cfg,
        &fabric,
        &store,
        Duration::from_secs(3600),
    )
    .await;
    eventually("two changes ingested and three dead-lettered", 60, || {
        fabric.len() == 2 && fabric.dead_letters().len() == 3
    })
    .await;
    source.stop().await;

    let ops: Vec<String> = fabric
        .events()
        .iter()
        .map(|e| e.extensions["loamsop"].clone())
        .collect();
    assert_eq!(ops, ["c", "u"]);
    let ids: Vec<String> = fabric.events().iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids, [format!("{topic}/0/0"), format!("{topic}/0/4")]);
    let dead = fabric.dead_letters();
    for (d, offset) in dead.iter().zip([1, 2, 3]) {
        assert_eq!(d.class, ErrorClass::Poison);
        assert!(
            d.error.contains(&format!("{topic}/0@{offset}")),
            "the DLQ record names the position: {}",
            d.error
        );
        assert_eq!(
            d.split.as_ref().map(|s| s.as_str()),
            Some(format!("{topic}/0").as_str())
        );
    }
}
