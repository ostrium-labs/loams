//! Debezium change events on the Kafka source (CC1 Task 3, D1822, design
//! §80 §4.2), with `format: debezium-json` or `debezium-avro`.
//!
//! A user's Debezium connectors already write row changes to Kafka topics;
//! reading those topics with this source gives Loams the same `change`
//! events its own Postgres and MySQL CDC sources emit, with no Loams CDC
//! connector against the database:
//!
//! * `type` `io.loams.dev.flow.<connector>.change.v1`, `source`
//!   `/connectors/<instance>/<table>`, `subject` the table, `id`
//!   `<topic>/<partition>/<offset>` (stable across re-reads, so `ingest`'s
//!   dedup makes the source effectively once);
//! * `data` JSON `{table, op, key, before, after}`, as the MySQL source writes
//!   it: `table` is `<schema>.<table>` (or `<db>.<table>`) from Debezium's
//!   `source` block, `key` the record key (Debezium's key struct) or, with
//!   `debezium.key_fields`, those fields of the row (`after`, or `before` for
//!   a delete);
//! * `loamsop` from Debezium's `op`: `c`, `u`, `d`, `r` (snapshot reads) and
//!   `t` (truncate, as the Postgres and MySQL sources write it); `loamslsn`
//!   from `source`: Postgres' `lsn` as 16 hex digits and an ordinal (the
//!   Postgres source's shape), MySQL's `file`/`pos`/`row` (the MySQL
//!   source's), Oracle's `scn` (20 digits), SQL Server's `change_lsn`;
//! * `time` from the event's `ts_ms`, else `source.ts_ms`, else the record's
//!   timestamp.
//!
//! The value may carry the JSON converter's `{schema, payload}` wrapper or
//! not; keys likewise. With `debezium-avro` the value and key are Confluent
//! wire-format Avro, decoded with the writer schema from the configured
//! Schema Registry (`registry.rs`).
//!
//! A record from Debezium's schema-change topic (a payload with `ddl` or
//! `tableChanges` and no `op`) is passed through as a `schema-change` event
//! whose `data` is Debezium's payload unchanged. A tombstone (a key and no
//! value) is a delete of that key (`debezium.tombstones: delete`, the
//! default) or nothing (`drop`). Anything else that is not a change event is
//! `Poison`: the pipeline sends it to the DLQ with the record's position and
//! moves on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apache_avro::Schema as AvroSchema;
use apache_avro::reader::datum::GenericDatumReader;
use loams_connector_sdk::schema::registry::{SchemaFormat, SchemaRegistry, unframe};
use loams_connector_sdk::{CloudEvent, ConnectorError, EnvelopeBuilder, ids};
use rdkafka::Message;
use serde_json::{Map, Value, json};

use crate::config::{DebeziumConfig, RecordFormat, Tombstones};
use crate::record::rfc3339_millis;

/// The event name of a row change.
pub const CHANGE_EVENT: &str = "change";

/// The event name of a schema change.
pub const SCHEMA_CHANGE_EVENT: &str = "schema-change";

/// A consumed record, owned, so it can be decoded across an `await` (a
/// registry lookup) and kept to be decoded again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub key: Option<Vec<u8>>,
    pub value: Option<Vec<u8>>,
    pub timestamp_ms: Option<i64>,
}

impl RawRecord {
    pub fn from_message<M: Message>(m: &M) -> Self {
        Self {
            topic: m.topic().to_owned(),
            partition: m.partition(),
            offset: m.offset(),
            key: m.key().map(<[u8]>::to_vec),
            value: m.payload().map(<[u8]>::to_vec),
            timestamp_ms: m.timestamp().to_millis(),
        }
    }

    fn at(&self) -> String {
        format!("{}/{}@{}", self.topic, self.partition, self.offset)
    }

    fn id(&self) -> String {
        ids::log_record(
            &self.topic,
            self.partition.max(0) as u32,
            self.offset.max(0) as u64,
        )
    }
}

/// Which part of a record is being decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Key,
    Value,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Key => "key",
            Part::Value => "value",
        }
    }
}

/// Decodes Debezium change events (see the module comment).
pub struct DebeziumDecoder {
    envelope: EnvelopeBuilder,
    avro: bool,
    cfg: DebeziumConfig,
    registry: Option<Arc<dyn SchemaRegistry>>,
    schemas: Mutex<HashMap<u32, Arc<AvroSchema>>>,
}

impl std::fmt::Debug for DebeziumDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebeziumDecoder")
            .field("avro", &self.avro)
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl DebeziumDecoder {
    /// A decoder for `format` (`debezium-json` or `debezium-avro`; Avro needs
    /// `registry`).
    pub fn new(
        envelope: EnvelopeBuilder,
        format: RecordFormat,
        cfg: DebeziumConfig,
        registry: Option<Arc<dyn SchemaRegistry>>,
    ) -> Result<Self, ConnectorError> {
        let avro = match format {
            RecordFormat::DebeziumJson => false,
            RecordFormat::DebeziumAvro => true,
            RecordFormat::Plain => {
                return Err(ConnectorError::fatal(
                    "the Debezium decoder is for format debezium-json or debezium-avro",
                ));
            }
        };
        if avro && registry.is_none() {
            return Err(ConnectorError::fatal(
                "kafka config: format debezium-avro needs a schema_registry block",
            ));
        }
        Ok(Self {
            envelope,
            avro,
            cfg,
            registry,
            schemas: Mutex::new(HashMap::new()),
        })
    }

    /// The event `rec` carries, `None` for a dropped tombstone. A record that
    /// is not a Debezium change event is `Poison`; a registry failure keeps
    /// its class (a retry decodes the same record again).
    pub async fn decode(&self, rec: &RawRecord) -> Result<Option<CloudEvent>, ConnectorError> {
        let key = match rec.key.as_deref() {
            Some(k) if !k.is_empty() => Some(unwrap_converter(self.read(rec, Part::Key, k).await?)),
            _ => None,
        };
        let payload = match rec.value.as_deref() {
            Some(v) if !v.is_empty() => unwrap_converter(self.read(rec, Part::Value, v).await?),
            _ => Value::Null,
        };
        let payload = match payload {
            Value::Null => return self.tombstone(rec, key),
            Value::Object(p) => p,
            _ => return Err(poison(rec, "the value is not an object")),
        };
        match payload.get("op") {
            Some(Value::String(op)) => self.change(rec, key, op.clone(), payload).map(Some),
            Some(_) => Err(poison(rec, "op is not a string")),
            None if payload.contains_key("ddl") || payload.contains_key("tableChanges") => {
                Ok(Some(self.schema_change(rec, payload)))
            }
            None => Err(poison(rec, "the payload has no op")),
        }
    }

    /// One part of the record as JSON.
    async fn read(
        &self,
        rec: &RawRecord,
        part: Part,
        bytes: &[u8],
    ) -> Result<Value, ConnectorError> {
        if self.avro && bytes.first() == Some(&0) {
            return self.read_avro(rec, part, bytes).await;
        }
        if self.avro && part == Part::Value {
            return Err(poison(
                rec,
                "the value is not in the Confluent Avro wire format (magic byte 0)",
            ));
        }
        match serde_json::from_slice::<Value>(bytes) {
            Ok(v) => Ok(v),
            // A key written by a string converter.
            Err(_) if part == Part::Key => std::str::from_utf8(bytes)
                .map(|s| Value::String(s.to_owned()))
                .map_err(|_| poison(rec, "the key is neither JSON nor UTF-8 text")),
            Err(e) => Err(poison(rec, &format!("the value is not JSON ({e})"))),
        }
    }

    async fn read_avro(
        &self,
        rec: &RawRecord,
        part: Part,
        bytes: &[u8],
    ) -> Result<Value, ConnectorError> {
        let frame =
            unframe(bytes).map_err(|e| poison(rec, &format!("the {}: {e}", part.name())))?;
        let schema = self.avro_schema(rec, frame.schema_id).await?;
        let mut body = frame.payload;
        let value = GenericDatumReader::builder(&schema)
            .build()
            .and_then(|r| r.read_value(&mut body))
            .map_err(|e| {
                poison(
                    rec,
                    &format!(
                        "the {} does not decode with schema {}: {e}",
                        part.name(),
                        frame.schema_id
                    ),
                )
            })?;
        Value::try_from(value)
            .map_err(|e| poison(rec, &format!("the {} has no JSON form: {e}", part.name())))
    }

    /// The writer schema `id`, from the registry once.
    async fn avro_schema(
        &self,
        rec: &RawRecord,
        id: u32,
    ) -> Result<Arc<AvroSchema>, ConnectorError> {
        if let Some(s) = self
            .schemas
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
        {
            return Ok(s.clone());
        }
        let registry = self
            .registry
            .as_ref()
            .ok_or_else(|| ConnectorError::fatal("debezium-avro without a schema registry"))?;
        let registered = registry.by_id(id).await?;
        if registered.format != SchemaFormat::Avro {
            return Err(poison(rec, &format!("schema {id} is not an Avro schema")));
        }
        let text = std::str::from_utf8(&registered.schema)
            .map_err(|_| poison(rec, &format!("schema {id} is not UTF-8")))?;
        let schema = Arc::new(
            AvroSchema::parse_str(text)
                .map_err(|e| poison(rec, &format!("schema {id} does not parse: {e}")))?,
        );
        self.schemas
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, schema.clone());
        Ok(schema)
    }

    fn change(
        &self,
        rec: &RawRecord,
        key: Option<Value>,
        op: String,
        p: Map<String, Value>,
    ) -> Result<CloudEvent, ConnectorError> {
        if !matches!(op.as_str(), "c" | "u" | "d" | "r" | "t") {
            return Err(poison(rec, &format!("op {op:?} is not a row change")));
        }
        let before = row(rec, &p, "before")?;
        let after = row(rec, &p, "after")?;
        match op.as_str() {
            "c" | "u" | "r" if after.is_null() => {
                return Err(poison(rec, &format!("op {op} has no after row")));
            }
            "d" if before.is_null() && key.is_none() => {
                return Err(poison(rec, "a delete with neither a before row nor a key"));
            }
            _ => {}
        }
        let source = match p.get("source") {
            Some(Value::Object(s)) => s,
            _ => return Err(poison(rec, "the payload has no source block")),
        };
        let table = table_of(source).ok_or_else(|| poison(rec, "source names no table"))?;
        let row_for_key = if op == "d" { &before } else { &after };
        let key = self.key(rec, key, row_for_key)?;
        let data = json!({
            "table": table,
            "op": op,
            "key": key,
            "before": before,
            "after": after,
        });
        let mut ev = self
            .envelope
            .event(CHANGE_EVENT, &table, rec.id())
            .with_subject(table.clone())
            .with_data("application/json", data.to_string())
            .with_extension("loamsop", op);
        if let Some(lsn) = position_of(source) {
            ev = ev.with_extension("loamslsn", lsn);
        }
        Ok(with_time(ev, rec, &p, Some(source)))
    }

    /// The change's key: `debezium.key_fields` of the row when set (the
    /// record key when there is no row), else the record key.
    fn key(
        &self,
        rec: &RawRecord,
        key: Option<Value>,
        row: &Value,
    ) -> Result<Value, ConnectorError> {
        if self.cfg.key_fields.is_empty() || row.is_null() {
            return Ok(key.unwrap_or(Value::Null));
        }
        let mut out = Map::new();
        for f in &self.cfg.key_fields {
            let v = row
                .get(f)
                .ok_or_else(|| poison(rec, &format!("key field {f} is not in the row")))?;
            out.insert(f.clone(), v.clone());
        }
        Ok(Value::Object(out))
    }

    fn tombstone(
        &self,
        rec: &RawRecord,
        key: Option<Value>,
    ) -> Result<Option<CloudEvent>, ConnectorError> {
        if self.cfg.tombstones == Tombstones::Drop {
            return Ok(None);
        }
        let Some(key) = key.filter(|k| !k.is_null()) else {
            return Err(poison(rec, "a record with neither a value nor a key"));
        };
        let table = table_of_topic(&rec.topic);
        let data = json!({
            "table": table,
            "op": "d",
            "key": key,
            "before": Value::Null,
            "after": Value::Null,
        });
        let ev = self
            .envelope
            .event(CHANGE_EVENT, &table, rec.id())
            .with_subject(table.clone())
            .with_data("application/json", data.to_string())
            .with_extension("loamsop", "d");
        Ok(Some(with_time(ev, rec, &Map::new(), None)))
    }

    fn schema_change(&self, rec: &RawRecord, p: Map<String, Value>) -> CloudEvent {
        let db = p
            .get("databaseName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let schema = p
            .get("schemaName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let resource = match (db, schema) {
            (Some(d), Some(s)) => format!("{d}.{s}"),
            (Some(d), None) => d.to_owned(),
            (None, Some(s)) => s.to_owned(),
            (None, None) => rec.topic.clone(),
        };
        let source = match p.get("source") {
            Some(Value::Object(s)) => Some(s.clone()),
            _ => None,
        };
        let mut ev = self
            .envelope
            .event(SCHEMA_CHANGE_EVENT, &resource, rec.id())
            .with_subject(resource);
        if let Some(lsn) = source.as_ref().and_then(position_of) {
            ev = ev.with_extension("loamslsn", lsn);
        }
        let ev = with_time(ev, rec, &p, source.as_ref());
        ev.with_data("application/json", Value::Object(p).to_string())
    }
}

/// A record that is not a Debezium change event: the DLQ.
fn poison(rec: &RawRecord, why: &str) -> ConnectorError {
    ConnectorError::poison(format!("{}: not a Debezium change event: {why}", rec.at()))
}

/// The JSON converter's `{schema, payload}` wrapper removed, when present.
fn unwrap_converter(v: Value) -> Value {
    match v {
        Value::Object(mut m)
            if m.len() == 2 && m.contains_key("schema") && m.contains_key("payload") =>
        {
            m.remove("payload").unwrap_or(Value::Null)
        }
        other => other,
    }
}

/// `before` or `after`: an object or null.
fn row(rec: &RawRecord, p: &Map<String, Value>, name: &str) -> Result<Value, ConnectorError> {
    match p.get(name) {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(v @ Value::Object(_)) => Ok(v.clone()),
        Some(_) => Err(poison(rec, &format!("{name} is not an object"))),
    }
}

fn str_of<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn u64_of(m: &Map<String, Value>, k: &str) -> Option<u64> {
    match m.get(k)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// `<schema>.<table>` (Postgres, SQL Server, Oracle), else `<db>.<table>`
/// (MySQL), else `<db>.<collection>` (MongoDB).
fn table_of(source: &Map<String, Value>) -> Option<String> {
    let table = str_of(source, "table").or_else(|| str_of(source, "collection"))?;
    match str_of(source, "schema").or_else(|| str_of(source, "db")) {
        Some(ns) => Some(format!("{ns}.{table}")),
        None => Some(table.to_owned()),
    }
}

/// A tombstone carries no `source`: Debezium's topic is
/// `<prefix>.<schema-or-db>.<table>`, so a three-part topic name gives
/// `<schema-or-db>.<table>`; any other topic name is used as it is.
fn table_of_topic(topic: &str) -> String {
    let parts: Vec<&str> = topic.split('.').collect();
    if parts.len() == 3 && parts.iter().all(|p| !p.is_empty()) {
        format!("{}.{}", parts[1], parts[2])
    } else {
        topic.to_owned()
    }
}

/// `loamslsn` from Debezium's `source` block (see the module comment).
fn position_of(source: &Map<String, Value>) -> Option<String> {
    if let Some(lsn) = u64_of(source, "lsn") {
        return Some(format!("{lsn:016X}/{:010}", 0));
    }
    if let Some(lsn) = str_of(source, "change_lsn") {
        return Some(lsn.to_owned());
    }
    if let (Some(file), Some(pos)) = (str_of(source, "file"), u64_of(source, "pos")) {
        let row = u64_of(source, "row").unwrap_or(0);
        return Some(format!("{file}/{pos:020}/{row:010}"));
    }
    if let Some(scn) = u64_of(source, "scn") {
        return Some(format!("{scn:020}"));
    }
    None
}

/// `time`: the payload's `ts_ms`, else `source.ts_ms`, else the record's
/// timestamp.
fn with_time(
    ev: CloudEvent,
    rec: &RawRecord,
    p: &Map<String, Value>,
    source: Option<&Map<String, Value>>,
) -> CloudEvent {
    let ms = p
        .get("ts_ms")
        .and_then(Value::as_i64)
        .or_else(|| source.and_then(|s| s.get("ts_ms")).and_then(Value::as_i64))
        .or(rec.timestamp_ms);
    match ms {
        Some(ms) => ev.with_time(rfc3339_millis(ms)),
        None => ev,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions() {
        let pg = json!({"connector": "postgresql", "lsn": 24023128u64});
        assert_eq!(
            position_of(pg.as_object().expect("o")).as_deref(),
            Some("00000000016E9058/0000000000")
        );
        let my = json!({"connector": "mysql", "file": "mysql-bin.000003", "pos": 154, "row": 0});
        assert_eq!(
            position_of(my.as_object().expect("o")).as_deref(),
            Some("mysql-bin.000003/00000000000000000154/0000000000")
        );
        let ora = json!({"connector": "oracle", "scn": "2868546"});
        assert_eq!(
            position_of(ora.as_object().expect("o")).as_deref(),
            Some("00000000000002868546")
        );
        let ss = json!({"connector": "sqlserver", "change_lsn": "00000027:00000758:0003"});
        assert_eq!(
            position_of(ss.as_object().expect("o")).as_deref(),
            Some("00000027:00000758:0003")
        );
        assert_eq!(position_of(&Map::new()), None);
    }

    #[test]
    fn tables() {
        let pg = json!({"schema": "public", "table": "customers", "db": "postgres"});
        assert_eq!(
            table_of(pg.as_object().expect("o")).as_deref(),
            Some("public.customers")
        );
        let my = json!({"db": "inventory", "table": "customers"});
        assert_eq!(
            table_of(my.as_object().expect("o")).as_deref(),
            Some("inventory.customers")
        );
        assert_eq!(table_of(&Map::new()), None);
        assert_eq!(
            table_of_topic("dbserver1.inventory.customers"),
            "inventory.customers"
        );
        assert_eq!(table_of_topic("customers"), "customers");
    }

    #[test]
    fn converter_wrapper() {
        assert_eq!(
            unwrap_converter(json!({"schema": {"type": "int32"}, "payload": {"id": 1}})),
            json!({"id": 1})
        );
        assert_eq!(unwrap_converter(json!({"id": 1})), json!({"id": 1}));
    }
}
