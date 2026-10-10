//! `loams.graph.v1.Value` to and from the engine's own value, without loss (design §48 §8.2,
//! GR1 Task 2).
//!
//! The fabric-era contract carried values as `google.protobuf.Value`, whose only number is a
//! double, so every `INT64` above 2^53 came back wrong (§48 §5 finding 6). Here each Grafeo value
//! has a wire form of its own, and [`from_proto`] of [`to_proto`] is the identity for every value
//! the engine produces. The cases that are not, and why:
//!
//! * **`uint64`.** Grafeo has no unsigned integer. A `uint64` up to `i64::MAX` decodes to
//!   `INT64` (and so comes back as `int64`); a larger one is refused rather than wrapped.
//! * **`decimal`.** Grafeo has no decimal type, so a decimal is refused rather than rounded to a
//!   double. [`to_proto`] never produces one.
//! * **Datetimes are microsecond-precise** in Grafeo (`Timestamp` is microseconds since the
//!   epoch). A `local_datetime` or `zoned_datetime` whose nanosecond part is not a whole number
//!   of microseconds is refused rather than truncated. A `local_time`/`zoned_time` keeps
//!   nanoseconds, as Grafeo's `Time` does.
//! * **Nodes and relationships.** Grafeo has no node or edge value: a projected node is a map
//!   with `_id` (`INT64`) and `_labels` (a list of strings), and a relationship a map with `_id`,
//!   `_type`, `_source` and `_target` (`grafeo-core` `execution/operators/project.rs`). A map of
//!   exactly that shape is answered as a `Node` or `Relationship`, and decodes back to the same
//!   map.
//! * **Paths.** Grafeo 0.5.43 builds a path of element ids, not of projected maps, so a path's
//!   `Node`s and `Relationship`s carry only their `id` (a client that needs labels or properties
//!   returns the elements too, or `nodes(p)`). An id-only element decodes back to the id. A path
//!   of any other shape (the engine never builds one) is answered as a map `{nodes, edges}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use grafeo::Value;
use grafeo_common::types::{Date, Duration, PropertyKey, Time, Timestamp, ZonedDatetime};
use loams_proto::loams::graph::v1 as pb;
use pb::__buffa::oneof::value::Kind;

use crate::engine::GraphError;

const MICROS_PER_DAY: i64 = 86_400_000_000;
/// The largest UTC offset ISO 8601 and GQL allow: 18 hours.
const MAX_OFFSET_SECONDS: i32 = 18 * 3600;

/// A UTC offset in GQL's range.
fn offset(seconds: i32) -> Result<i32, GraphError> {
    if seconds.unsigned_abs() > MAX_OFFSET_SECONDS.unsigned_abs() {
        return Err(invalid(format!(
            "a UTC offset of {seconds} seconds is outside ±18 hours"
        )));
    }
    Ok(seconds)
}

/// The engine's reserved keys on a projected node or relationship map.
const ID: &str = "_id";
const LABELS: &str = "_labels";
const TYPE: &str = "_type";
const SOURCE: &str = "_source";
const TARGET: &str = "_target";

fn wire(kind: Kind) -> pb::Value {
    pb::Value {
        kind: Some(kind),
        ..Default::default()
    }
}

fn invalid(message: impl Into<String>) -> GraphError {
    GraphError::InvalidValue(message.into())
}

/// The wire form of one engine value.
#[must_use]
pub fn to_proto(value: &Value) -> pb::Value {
    let kind = match value {
        Value::Null => Kind::Null(buffa_types::google::protobuf::NullValue::NULL_VALUE.into()),
        Value::Bool(b) => Kind::Boolean(*b),
        Value::Int64(i) => Kind::Int64(*i),
        Value::Float64(f) => Kind::Float64(*f),
        Value::String(s) => Kind::String(s.to_string()),
        Value::Bytes(bytes) => Kind::Bytes(bytes.to_vec()),
        Value::Date(d) => Kind::Date(Box::new(date_to_proto(*d))),
        Value::Time(t) => match t.offset_seconds() {
            None => Kind::LocalTime(Box::new(time_to_proto(t))),
            Some(offset_seconds) => Kind::ZonedTime(Box::new(pb::ZonedTime {
                time: time_to_proto(t).into(),
                offset_seconds,
                ..Default::default()
            })),
        },
        Value::Timestamp(ts) => Kind::LocalDatetime(Box::new(datetime_to_proto(ts.as_micros()))),
        Value::ZonedDatetime(z) => {
            // Saturating: an engine value at the edge of Grafeo's range is still answered.
            let local = z
                .as_timestamp()
                .as_micros()
                .saturating_add(i64::from(z.offset_seconds()) * 1_000_000);
            Kind::ZonedDatetime(Box::new(pb::ZonedDateTime {
                local: datetime_to_proto(local).into(),
                offset_seconds: z.offset_seconds(),
                ..Default::default()
            }))
        }
        Value::Duration(d) => Kind::Duration(Box::new(pb::Duration {
            months: d.months(),
            days: d.days(),
            nanos: d.nanos(),
            ..Default::default()
        })),
        Value::List(items) => Kind::List(Box::new(pb::ListValue {
            values: items.iter().map(to_proto).collect(),
            ..Default::default()
        })),
        Value::Map(entries) => {
            if let Some(node) = as_node(entries) {
                Kind::Node(Box::new(node))
            } else if let Some(rel) = as_relationship(entries) {
                Kind::Relationship(Box::new(rel))
            } else {
                let mut out = pb::MapValue::default();
                for (key, value) in entries.iter() {
                    out.entries
                        .insert(key.as_str().to_string(), to_proto(value));
                }
                Kind::Map(Box::new(out))
            }
        }
        Value::Path { nodes, edges } => {
            let typed_nodes: Option<Vec<pb::Node>> = nodes.iter().map(path_node).collect();
            let typed_edges: Option<Vec<pb::Relationship>> =
                edges.iter().map(path_relationship).collect();
            match (typed_nodes, typed_edges) {
                (Some(nodes), Some(relationships)) => Kind::Path(Box::new(pb::Path {
                    nodes,
                    relationships,
                    ..Default::default()
                })),
                // Not a shape the engine builds; kept whole rather than dropped.
                _ => {
                    let mut out = pb::MapValue::default();
                    out.entries
                        .insert("nodes".into(), to_proto(&Value::List(Arc::clone(nodes))));
                    out.entries
                        .insert("edges".into(), to_proto(&Value::List(Arc::clone(edges))));
                    Kind::Map(Box::new(out))
                }
            }
        }
        Value::Vector(values) => Kind::Vector(Box::new(pb::Vector {
            values: values.to_vec(),
            ..Default::default()
        })),
        Value::GCounter(counts) => Kind::Counter(Box::new(counter(counts, None))),
        Value::OnCounter { pos, neg } => Kind::Counter(Box::new(counter(pos, Some(neg)))),
        // `grafeo::Value` is `#[non_exhaustive]`. A variant added after 0.5.43 crosses as its
        // serde form in a string rather than vanishing; the pin (D759) means a bump re-reads this.
        other => Kind::String(serde_json::to_string(other).unwrap_or_default()),
    };
    wire(kind)
}

/// The engine value of one wire value. An absent `kind` is null.
///
/// # Errors
///
/// [`GraphError::InvalidValue`] for a value Grafeo cannot hold exactly: a `uint64` above
/// `i64::MAX`, a decimal, an impossible date or time, a datetime finer than a microsecond, or a
/// node, relationship or path whose ids do not fit in `INT64`.
pub fn from_proto(value: &pb::Value) -> Result<Value, GraphError> {
    let Some(kind) = &value.kind else {
        return Ok(Value::Null);
    };
    Ok(match kind {
        Kind::Null(_) => Value::Null,
        Kind::Boolean(b) => Value::Bool(*b),
        Kind::Int64(i) => Value::Int64(*i),
        Kind::Uint64(u) => Value::Int64(i64::try_from(*u).map_err(|_| {
            invalid(format!(
                "uint64 {u} does not fit in INT64, and Grafeo has no unsigned integer"
            ))
        })?),
        Kind::Float64(f) => Value::Float64(*f),
        Kind::String(s) => Value::String(s.as_str().into()),
        Kind::Bytes(bytes) => Value::Bytes(Arc::from(bytes.as_slice())),
        Kind::Decimal(d) => {
            return Err(invalid(format!(
                "decimal {} is not supported: Grafeo has no decimal type",
                d.value
            )));
        }
        Kind::Date(d) => Value::Date(date_from_proto(d)?),
        Kind::LocalTime(t) => Value::Time(time_from_proto(t)?),
        Kind::ZonedTime(z) => {
            let time = z
                .time
                .as_option()
                .ok_or_else(|| invalid("zoned_time has no time"))?;
            Value::Time(time_from_proto(time)?.with_offset(offset(z.offset_seconds)?))
        }
        Kind::LocalDatetime(dt) => {
            Value::Timestamp(Timestamp::from_micros(datetime_from_proto(dt)?))
        }
        Kind::ZonedDatetime(z) => {
            let local = z
                .local
                .as_option()
                .ok_or_else(|| invalid("zoned_datetime has no local datetime"))?;
            let offset_seconds = offset(z.offset_seconds)?;
            let utc = datetime_from_proto(local)?
                .checked_sub(i64::from(offset_seconds) * 1_000_000)
                .ok_or_else(|| invalid("the zoned datetime is out of Grafeo's range"))?;
            Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
                Timestamp::from_micros(utc),
                offset_seconds,
            ))
        }
        Kind::Duration(d) => Value::Duration(Duration::new(d.months, d.days, d.nanos)),
        Kind::List(list) => Value::List(Arc::from(
            list.values
                .iter()
                .map(from_proto)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Kind::Map(m) => {
            let mut out = BTreeMap::new();
            for (key, value) in &m.entries {
                out.insert(PropertyKey::new(key.as_str()), from_proto(value)?);
            }
            Value::Map(Arc::new(out))
        }
        Kind::Node(node) => node_from_proto(node)?,
        Kind::Relationship(rel) => relationship_from_proto(rel)?,
        Kind::Path(path) => Value::Path {
            nodes: Arc::from(
                path.nodes
                    .iter()
                    .map(path_node_from_proto)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            edges: Arc::from(
                path.relationships
                    .iter()
                    .map(path_relationship_from_proto)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        },
        Kind::Vector(v) => Value::Vector(Arc::from(v.values.as_slice())),
        Kind::Counter(c) => {
            let positive: HashMap<String, u64> =
                c.positive.iter().map(|(k, v)| (k.clone(), *v)).collect();
            if c.grow_only {
                if !c.negative.is_empty() {
                    return Err(invalid("a grow-only counter has negative entries"));
                }
                Value::GCounter(Arc::new(positive))
            } else {
                Value::OnCounter {
                    pos: Arc::new(positive),
                    neg: Arc::new(c.negative.iter().map(|(k, v)| (k.clone(), *v)).collect()),
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Temporal parts
// ---------------------------------------------------------------------------------------------

fn date_to_proto(date: Date) -> pb::Date {
    let (year, month, day) = date.to_ymd();
    pb::Date {
        year,
        month,
        day,
        ..Default::default()
    }
}

fn date_from_proto(date: &pb::Date) -> Result<Date, GraphError> {
    let parsed = Date::from_ymd(date.year, date.month, date.day).ok_or_else(|| {
        invalid(format!(
            "{}-{}-{} is not a date",
            date.year, date.month, date.day
        ))
    })?;
    // `from_ymd` normalises some out-of-range days; a date that does not read back the same is
    // not the date the caller sent.
    if parsed.to_ymd() != (date.year, date.month, date.day) {
        return Err(invalid(format!(
            "{}-{}-{} is not a date",
            date.year, date.month, date.day
        )));
    }
    Ok(parsed)
}

fn time_to_proto(time: &Time) -> pb::LocalTime {
    pb::LocalTime {
        hour: time.hour(),
        minute: time.minute(),
        second: time.second(),
        nanosecond: time.nanosecond(),
        ..Default::default()
    }
}

fn time_from_proto(time: &pb::LocalTime) -> Result<Time, GraphError> {
    Time::from_hms_nano(time.hour, time.minute, time.second, time.nanosecond).ok_or_else(|| {
        invalid(format!(
            "{}:{}:{}.{:09} is not a time of day",
            time.hour, time.minute, time.second, time.nanosecond
        ))
    })
}

/// A count of microseconds since the epoch, as a wall-clock date and time.
fn datetime_to_proto(micros: i64) -> pb::LocalDateTime {
    let days = micros.div_euclid(MICROS_PER_DAY);
    let of_day = micros.rem_euclid(MICROS_PER_DAY) as u64 * 1000;
    // `days` fits in i32 for every Timestamp (±290 000 years is ±1.06e8 days).
    let date = Date::from_days(days as i32);
    let time = Time::from_nanos(of_day).unwrap_or_default();
    pb::LocalDateTime {
        date: date_to_proto(date).into(),
        time: time_to_proto(&time).into(),
        ..Default::default()
    }
}

fn datetime_from_proto(dt: &pb::LocalDateTime) -> Result<i64, GraphError> {
    let date = dt
        .date
        .as_option()
        .ok_or_else(|| invalid("a datetime has no date"))?;
    let time = dt
        .time
        .as_option()
        .ok_or_else(|| invalid("a datetime has no time"))?;
    if time.nanosecond % 1000 != 0 {
        return Err(invalid(format!(
            "a datetime is microsecond-precise in Grafeo; {} nanoseconds is not a whole number of microseconds",
            time.nanosecond
        )));
    }
    let date = date_from_proto(date)?;
    let time = time_from_proto(time)?;
    let of_day = (time.as_nanos() / 1000) as i64;
    i64::from(date.as_days())
        .checked_mul(MICROS_PER_DAY)
        .and_then(|day| day.checked_add(of_day))
        .ok_or_else(|| invalid("the datetime is out of Grafeo's range"))
}

// ---------------------------------------------------------------------------------------------
// Nodes, relationships, counters
// ---------------------------------------------------------------------------------------------

fn properties(map: &BTreeMap<PropertyKey, Value>, reserved: &[&str]) -> pb_map::Properties {
    let mut out = pb_map::Properties::default();
    for (key, value) in map {
        if !reserved.contains(&key.as_str()) {
            out.insert(key.as_str().to_string(), to_proto(value));
        }
    }
    out
}

/// The generated map type, named once.
mod pb_map {
    pub(super) type Properties =
        ::buffa::__private::HashMap<::std::string::String, loams_proto::loams::graph::v1::Value>;
}

fn get<'a>(map: &'a BTreeMap<PropertyKey, Value>, key: &str) -> Option<&'a Value> {
    map.get(&PropertyKey::new(key))
}

/// A projected node: `_id` is an `INT64` and `_labels` a list of strings, and neither of a
/// relationship's own keys is present.
fn as_node(map: &BTreeMap<PropertyKey, Value>) -> Option<pb::Node> {
    let Some(Value::Int64(id)) = get(map, ID) else {
        return None;
    };
    let Some(Value::List(labels)) = get(map, LABELS) else {
        return None;
    };
    if get(map, TYPE).is_some() {
        return None;
    }
    let labels = labels
        .iter()
        .map(|label| match label {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(pb::Node {
        // The engine stores a `NodeId(u64)` as `i64` (`as` cast); this is the reverse cast.
        id: *id as u64,
        labels,
        properties: properties(map, &[ID, LABELS]),
        ..Default::default()
    })
}

/// A projected relationship: `_id`, `_source` and `_target` are `INT64`s and `_type` a string.
fn as_relationship(map: &BTreeMap<PropertyKey, Value>) -> Option<pb::Relationship> {
    let (
        Some(Value::Int64(id)),
        Some(Value::String(ty)),
        Some(Value::Int64(src)),
        Some(Value::Int64(dst)),
    ) = (
        get(map, ID),
        get(map, TYPE),
        get(map, SOURCE),
        get(map, TARGET),
    )
    else {
        return None;
    };
    if get(map, LABELS).is_some() {
        return None;
    }
    Some(pb::Relationship {
        id: *id as u64,
        r#type: ty.to_string(),
        src: *src as u64,
        dst: *dst as u64,
        properties: properties(map, &[ID, TYPE, SOURCE, TARGET]),
        ..Default::default()
    })
}

fn decode_properties(
    out: &mut BTreeMap<PropertyKey, Value>,
    properties: &pb_map::Properties,
    reserved: &[&str],
) -> Result<(), GraphError> {
    for (key, value) in properties {
        if reserved.contains(&key.as_str()) {
            return Err(invalid(format!(
                "property {key} is reserved by the engine on a node or relationship"
            )));
        }
        out.insert(PropertyKey::new(key.as_str()), from_proto(value)?);
    }
    Ok(())
}

fn node_from_proto(node: &pb::Node) -> Result<Value, GraphError> {
    let mut out = BTreeMap::new();
    out.insert(PropertyKey::new(ID), Value::Int64(node.id as i64));
    out.insert(
        PropertyKey::new(LABELS),
        Value::List(Arc::from(
            node.labels
                .iter()
                .map(|l| Value::String(l.as_str().into()))
                .collect::<Vec<_>>(),
        )),
    );
    decode_properties(&mut out, &node.properties, &[ID, LABELS])?;
    Ok(Value::Map(Arc::new(out)))
}

fn relationship_from_proto(rel: &pb::Relationship) -> Result<Value, GraphError> {
    let mut out = BTreeMap::new();
    out.insert(PropertyKey::new(ID), Value::Int64(rel.id as i64));
    out.insert(
        PropertyKey::new(TYPE),
        Value::String(rel.r#type.as_str().into()),
    );
    out.insert(PropertyKey::new(SOURCE), Value::Int64(rel.src as i64));
    out.insert(PropertyKey::new(TARGET), Value::Int64(rel.dst as i64));
    decode_properties(&mut out, &rel.properties, &[ID, TYPE, SOURCE, TARGET])?;
    Ok(Value::Map(Arc::new(out)))
}

/// One node of a path. Grafeo 0.5.43 builds a path of element **ids** (`Value::Int64`), not of
/// projected maps (measured: `MATCH p = ()-[]->() RETURN p`), so an id becomes a `Node` with only
/// its id; a projected node map, should the engine ever build one, becomes the full `Node`.
fn path_node(value: &Value) -> Option<pb::Node> {
    match value {
        Value::Int64(id) => Some(pb::Node {
            id: *id as u64,
            ..Default::default()
        }),
        Value::Map(m) => as_node(m),
        _ => None,
    }
}

/// One relationship of a path; see [`path_node`].
fn path_relationship(value: &Value) -> Option<pb::Relationship> {
    match value {
        Value::Int64(id) => Some(pb::Relationship {
            id: *id as u64,
            ..Default::default()
        }),
        Value::Map(m) => as_relationship(m),
        _ => None,
    }
}

/// The reverse of [`path_node`]: a node with no labels and no properties is an id-only element.
fn path_node_from_proto(node: &pb::Node) -> Result<Value, GraphError> {
    if node.labels.is_empty() && node.properties.is_empty() {
        Ok(Value::Int64(node.id as i64))
    } else {
        node_from_proto(node)
    }
}

/// The reverse of [`path_relationship`]: a relationship with no type is an id-only element (a
/// stored relationship always has a type).
fn path_relationship_from_proto(rel: &pb::Relationship) -> Result<Value, GraphError> {
    if rel.r#type.is_empty() && rel.properties.is_empty() {
        Ok(Value::Int64(rel.id as i64))
    } else {
        relationship_from_proto(rel)
    }
}

fn counter(
    positive: &HashMap<String, u64>,
    negative: Option<&Arc<HashMap<String, u64>>>,
) -> pb::Counter {
    let mut out = pb::Counter {
        grow_only: negative.is_none(),
        ..Default::default()
    };
    for (replica, n) in positive {
        out.positive.insert(replica.clone(), *n);
    }
    for (replica, n) in negative.into_iter().flat_map(|m| m.iter()) {
        out.negative.insert(replica.clone(), *n);
    }
    out
}
