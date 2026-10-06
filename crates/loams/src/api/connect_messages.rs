//! The `loams.collection.v1` messages built from the server's own types.
//!
//! Design §44 §4 and the API1 plan's "handlers are thin" rule: the RPCs in
//! [`super::connect_collections`] call the same `CollectionService`,
//! `MetaStore` and hot-tier entry points the REST routes call, and *this*
//! module is the only place that turns what those return into the generated
//! messages. That is what makes the REST route and the RPC provably the same
//! behaviour: there is one mapping from `loams_query::CollectionInfo` to
//! `loams.collection.v1.CollectionInfo`, not one per surface.
//!
//! ## What is carried as JSON, and why
//!
//! Three things are `google.protobuf.Struct` rather than typed messages,
//! because they are open documents today (the proto header gives the full
//! reason): a collection's **schema**, the **fields and vectors** `AddFields`
//! adds, and a **fragment's** Lance metadata. [`struct_of`] and [`json_of`]
//! are the two ends of that, and between them a schema is the REST route's
//! schema JSON unchanged, so moving a caller onto the RPC changes its
//! envelope and not its schema.
//!
//! One normalisation happens in [`json_of`]: `google.protobuf.Value` holds
//! every JSON number as a `double`, so an integral one is written back as an
//! integer rather than `3.0`. Without it a schema would gain a decimal point
//! on every `dim` and `max_fields` on the way out and back.

use buffa::Inline;
use buffa::MessageField;
use buffa::MessageView as _;
use buffa::enumeration::EnumValue;
use loams_collection::{CollectionSchema, ConsistencyToken};
use loams_proto::google::protobuf::ListValue;
use loams_proto::google::protobuf::Struct;
use loams_proto::google::protobuf::Value as ProtoValue;
use loams_proto::google::protobuf::__buffa::view::StructView;
use loams_proto::loams::collection::v1 as pb;
use loams_query::backlog::{BackpressureState, BackpressureStatus as NativeBackpressure};
use loams_query::hot::{HotState, HotStateKind, HotStatus as CatalogHotStatus};
use loams_query::json::schema;
use loams_query::{
    CollectionInfo as NativeCollectionInfo, ColumnRole, DeletionKind, ManifestInfo,
    ScanColumn as NativeScanColumn, ScanFragment, ScanPlan as NativeScanPlan,
};
use serde_json::{Map, Number, Value, json};

/// A `Struct` from a JSON object, or `None` for anything else (a missing
/// field, `null`, an array, a string): the caller asked for a document and
/// did not send one.
pub(super) fn struct_of(value: &Value) -> Option<Struct> {
    let Value::Object(fields) = value else {
        return None;
    };
    Some(Struct::from_iter(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), proto_value(value))),
    ))
}

/// A JSON object from a `Struct`; an empty object when there is none.
pub(super) fn json_of(value: &Struct) -> Value {
    Value::Object(
        value
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), json_of_proto(value)))
            .collect(),
    )
}

/// A JSON object from a request's `Struct`, which is a **view** over the
/// decoded body.
///
/// The view is materialized into the owned `Struct` and read by
/// [`json_of`], so a request document is read exactly like a response
/// document. A view that will not materialize is a decode the codec already
/// accepted, so it is not expected: it is logged and read as `{}` rather than
/// failing a call the rest of the request is fine for.
pub(super) fn json_of_view(value: &StructView<'_>) -> Value {
    match value.to_owned_message() {
        Ok(value) => json_of(&value),
        Err(err) => {
            tracing::error!(%err, "a decoded schema document did not materialize");
            Value::Object(Map::new())
        }
    }
}

/// A `Struct` field that is set, or `None` when the document is absent, which
/// is what proto3 JSON requires of a message field at its default.
fn optional(value: Option<Struct>) -> MessageField<Struct, Inline<Struct>> {
    value.map_or_else(MessageField::none, MessageField::some)
}

/// A JSON value from a `google.protobuf.Value`.
fn json_of_proto(value: &ProtoValue) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    if let Some(number) = value.as_number() {
        // `Value.number_value` is a `double` and serde_json prints a whole
        // float as `3.0`. A schema's `dim` and `max_fields` are whole numbers,
        // so they are written back as integers: the value the caller sent is
        // the value it reads. Past 2^53 the `double` has already lost the
        // integer and stays a float, which is the honest reading of it.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
            return json!(number as i64);
        }
        return Number::from_f64(number).map_or(Value::Null, Value::Number);
    }
    if let Some(text) = value.as_str() {
        return Value::String(text.to_owned());
    }
    if let Some(flag) = value.as_bool() {
        return Value::Bool(flag);
    }
    if let Some(list) = value.as_list() {
        return Value::Array(list.values.iter().map(json_of_proto).collect());
    }
    if let Some(object) = value.as_struct() {
        return json_of(object);
    }
    Value::Null
}

/// A `google.protobuf.Value` from a JSON value. A number a `double` cannot
/// hold exactly (`u64::MAX` as written by a caller, say) becomes its nearest
/// `double`: the alternative is dropping the schema field.
fn proto_value(value: &Value) -> ProtoValue {
    match value {
        Value::Null => ProtoValue::null(),
        Value::Bool(flag) => ProtoValue::from(*flag),
        Value::Number(number) => {
            // JSON has no NaN or infinity, so a `f64` that is not finite can
            // only come from arithmetic that overflowed; 0 keeps the document
            // readable rather than failing the whole request.
            let as_f64 = number.as_f64().filter(|n| n.is_finite()).unwrap_or(0.0);
            ProtoValue::from(as_f64)
        }
        Value::String(text) => ProtoValue::from(text.as_str()),
        Value::Array(items) => ProtoValue::from(ListValue {
            values: items.iter().map(proto_value).collect(),
            ..Default::default()
        }),
        Value::Object(fields) => ProtoValue::from(
            struct_of(&Value::Object(fields.clone())).expect("an object is a Struct"),
        ),
    }
}

/// A collection, as `GetCollection` and the collection-carrying RPCs answer
/// it. `hot` is the caller's to pass: the owner's full status for a get
/// (M1.3 rule 4), the catalog summary everywhere else.
pub(super) fn collection_info(
    info: &NativeCollectionInfo,
    hot: Option<pb::HotStatus>,
) -> pb::CollectionInfo {
    pb::CollectionInfo {
        id: info.id.0,
        name: info.name.clone(),
        namespace: info.namespace.clone(),
        schema: optional(Some(
            struct_of(&schema::to_json(&info.schema)).expect("a schema is an object"),
        )),
        partitions: info.partitions,
        aliases: info.aliases.clone(),
        stream: info.stream.0,
        manifest_version: info.manifest_version,
        live_doc_count: info.live_doc_count,
        size_bytes: info.size_bytes,
        created_at_ms: info.created_at_ms,
        link_lag_records: info.link_lag_records,
        hot: hot.map_or_else(MessageField::none, MessageField::some),
        unapplied_bytes: info.unapplied_bytes,
        backpressure: MessageField::some(backpressure(&info.backpressure)),
        ..Default::default()
    }
}

/// The collection's schema as the REST route's schema JSON, from a parsed
/// request schema.
pub(super) fn schema_struct(schema: &CollectionSchema) -> Struct {
    struct_of(&schema::to_json(schema)).expect("a schema is an object")
}

/// The write budget and admission's answer under it (M1.3 D86).
fn backpressure(status: &NativeBackpressure) -> pb::BackpressureStatus {
    let state = match status.state {
        BackpressureState::Open => pb::BackpressureState::BACKPRESSURE_STATE_OPEN,
        BackpressureState::Throttling => pb::BackpressureState::BACKPRESSURE_STATE_THROTTLING,
        BackpressureState::Disabled => pb::BackpressureState::BACKPRESSURE_STATE_DISABLED,
    };
    pb::BackpressureStatus {
        state: EnumValue::Known(state),
        unapplied_records: status.unapplied_records,
        unapplied_bytes: status.unapplied_bytes,
        max_unapplied_records: status.max_unapplied_records,
        max_unapplied_bytes: status.max_unapplied_bytes,
        ..Default::default()
    }
}

/// The catalog's hot summary for a collection: what the create and list RPCs
/// answer, the same thing the REST route's `create`/`list` answer.
///
/// The *full* status needs the owning node, which is a call per collection
/// (`hot::hot_status_value`), so it is `GetCollection`'s answer and not the
/// list's: a cluster with a thousand collections must not make a thousand
/// node-to-node status reads to answer a list.
pub(super) fn catalog_hot(status: &CatalogHotStatus) -> pb::HotStatus {
    pb::HotStatus {
        vectors: MessageField::some(structure(&status.vectors)),
        text: MessageField::some(structure(&status.text)),
        fragments: MessageField::some(structure(&status.fragments)),
        ..Default::default()
    }
}

/// The owning node's full hot status, as `hot::hot_status_value` reports it.
///
/// It is read as JSON because that is what the node-to-node internal route
/// answers (`hot::to_value`), and this is then the only reader of that
/// answer on the Connect path: a local tier and a remote owner produce the
/// same message from the same bytes, and a node one release behind its owner
/// still answers a usable status because every read falls back to a default
/// instead of failing the get.
pub(super) fn hot_status(value: &Value) -> pb::HotStatus {
    let flag = |value: &Value, key: &str| get(value, key).as_bool().unwrap_or(false);
    let count = |value: &Value, key: &str| get(value, key).as_u64();
    let config = get(value, "config");
    let owner = get(value, "owner");
    pb::HotStatus {
        enabled: flag(value, "enabled"),
        config: MessageField::some(pb::HotConfig {
            vectors: flag(config, "vectors"),
            text: flag(config, "text"),
            fragments: flag(config, "fragments"),
            ..Default::default()
        }),
        pin_all: flag(value, "pin_all"),
        promoted: flag(value, "promoted"),
        owner: MessageField::some(pb::HotOwner {
            node_id: count(owner, "node_id").unwrap_or_default(),
            local: flag(owner, "local"),
            ..Default::default()
        }),
        vectors: MessageField::some(vectors(get(value, "vectors"))),
        text: MessageField::some(pb::HotStructureStatus {
            pinned_splits: count(get(value, "text"), "pinned_splits").unwrap_or_default(),
            splits: count(get(value, "text"), "splits").unwrap_or_default(),
            ..simple_structure(get(value, "text"))
        }),
        fragments: MessageField::some(pb::HotStructureStatus {
            prefetched_bytes: count(get(value, "fragments"), "prefetched_bytes")
                .unwrap_or_default(),
            bytes: count(get(value, "fragments"), "bytes").unwrap_or_default(),
            ..simple_structure(get(value, "fragments"))
        }),
        ..Default::default()
    }
}

/// `value[key]`, or a shared JSON `null` when the key is absent — so every
/// reader of a status the reporting node sent can fall back to its default
/// without a local `Value::Null` borrow that outlives nothing.
fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    static ABSENT: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| Value::Null);
    value.get(key).unwrap_or(&ABSENT)
}

/// A structure with the keys every structure has.
fn simple_structure(value: &Value) -> pb::HotStructureStatus {
    pb::HotStructureStatus {
        state: EnumValue::Known(hot_state(json_hot_state(value.get("state")))),
        source_version: value.get("source_version").and_then(Value::as_u64),
        over_budget: value
            .get("over_budget")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..Default::default()
    }
}

/// The `vectors` structure, whose only extra key is the per-column map.
fn vectors(value: &Value) -> pb::HotStructureStatus {
    let columns: buffa::__private::HashMap<String, pb::HotColumnStatus> = value
        .get("columns")
        .and_then(Value::as_object)
        .map(|columns| {
            columns
                .iter()
                .map(|(name, column)| {
                    (
                        name.clone(),
                        pb::HotColumnStatus {
                            state: EnumValue::Known(hot_state(json_hot_state(
                                column.get("state"),
                            ))),
                            source_version: column.get("source_version").and_then(Value::as_u64),
                            artifact_source_version: column
                                .get("artifact_source_version")
                                .and_then(Value::as_u64),
                            delta_rows: column
                                .get("delta_rows")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                            error: column
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pb::HotStructureStatus {
        columns,
        ..simple_structure(value)
    }
}

/// A structure from the catalog's summary, whose keys are `state` and
/// `source_version` only.
fn structure(state: &HotState) -> pb::HotStructureStatus {
    pb::HotStructureStatus {
        state: EnumValue::Known(hot_state(state.state)),
        source_version: state.source_version,
        ..Default::default()
    }
}

/// The proto spelling of a hot state. `HotStateKind` is a closed set of three,
/// so the mapping is total.
fn hot_state(state: HotStateKind) -> pb::HotStateKind {
    match state {
        HotStateKind::Off => pb::HotStateKind::HOT_STATE_KIND_OFF,
        HotStateKind::Building => pb::HotStateKind::HOT_STATE_KIND_BUILDING,
        HotStateKind::Ready => pb::HotStateKind::HOT_STATE_KIND_READY,
    }
}

/// The hot state as the reporting node's JSON spells it: the same three
/// names, `snake_case`. Anything unreadable reads as `off`, which is what a
/// reporting node with no tier says.
fn json_hot_state(value: Option<&Value>) -> HotStateKind {
    match value.and_then(Value::as_str) {
        Some("building") => HotStateKind::Building,
        Some("ready") => HotStateKind::Ready,
        _ => HotStateKind::Off,
    }
}

/// One retained manifest, as `ListVersions` answers it.
pub(super) fn manifest_version(info: &ManifestInfo) -> pb::ManifestVersion {
    pb::ManifestVersion {
        version: info.version,
        created_at_ms: info.created_at_ms,
        size_bytes: info.size_bytes,
        live_doc_count: info.live_doc_count,
        lance_version: info.lance_version,
        ..Default::default()
    }
}

/// A collection resolved into what an external reader needs to read one state
/// of it (plan M1.2 Task 14, D53).
pub(super) fn scan_plan(plan: &NativeScanPlan) -> pb::ScanPlan {
    pb::ScanPlan {
        namespace: plan.namespace.clone(),
        collection: plan.collection.clone(),
        collection_id: plan.collection_id.0,
        manifest_version: plan.manifest_version,
        schema_version: plan.schema_version,
        lance: plan.lance.as_ref().map_or_else(MessageField::none, |lance| {
            MessageField::some(pb::LanceVersionRef {
                uri: lance.uri.clone(),
                version: lance.version,
                manifest_path: lance.manifest_path.clone(),
                storage_format: lance.storage_format.clone(),
                stable_row_ids: lance.stable_row_ids,
                ..Default::default()
            })
        }),
        fragments: plan.fragments.iter().map(fragment).collect(),
        live_rows: plan.live_rows,
        columns: plan.columns.iter().map(scan_column).collect(),
        pk_encoding: plan.pk_encoding.clone(),
        tail: plan.tail,
        tail_records: plan.tail_records,
        offsets: plan
            .offsets
            .iter()
            .map(|offsets| pb::ScanOffsets {
                partition: offsets.partition,
                applied: offsets.applied,
                target: offsets.target,
                ..Default::default()
            })
            .collect(),
        durable_token: token_of(&plan.durable_token),
        pin: MessageField::some(pb::ScanPin {
            manifest_version: plan.pin.manifest_version,
            token: token_of(&plan.pin.token),
            ..Default::default()
        }),
        planned_at_ms: plan.planned_at_ms,
        expires_at_ms: plan.expires_at_ms,
        ..Default::default()
    }
}

/// A consistency token as the wire carries it, in a `ScanRequest` and a
/// `ScanPlan` alike.
pub(super) fn token_of(token: &ConsistencyToken) -> String {
    token.to_string()
}

/// One Lance fragment of the planned version. `lance` is Lance's own
/// `Fragment` JSON, carried verbatim so a reader needs no second copy of
/// Lance's fragment model.
fn fragment(fragment: &ScanFragment) -> pb::ScanFragment {
    pb::ScanFragment {
        id: fragment.id,
        physical_rows: fragment.physical_rows,
        deleted_rows: fragment.deleted_rows,
        live_rows: fragment.live_rows,
        files: fragment
            .files
            .iter()
            .map(|file| pb::ScanFile {
                path: file.path.clone(),
                size_bytes: file.size_bytes,
                ..Default::default()
            })
            .collect(),
        deletion_file: fragment.deletion_file.as_ref().map_or_else(
            MessageField::none,
            |deletion| {
                MessageField::some(pb::ScanDeletionFile {
                    path: deletion.path.clone(),
                    kind: EnumValue::Known(match deletion.kind {
                        DeletionKind::Array => pb::DeletionKind::DELETION_KIND_ARRAY,
                        DeletionKind::Bitmap => pb::DeletionKind::DELETION_KIND_BITMAP,
                    }),
                    deleted_rows: deletion.deleted_rows,
                    ..Default::default()
                })
            },
        ),
        lance: optional(struct_of(&fragment.lance)),
        ..Default::default()
    }
}

/// One readable column. `role`, `distance` and `modifier` keep the REST
/// route's `snake_case` spellings (`pk`, `cosine`, `idf`), because a reader
/// switches on them verbatim.
fn scan_column(column: &NativeScanColumn) -> pb::ScanColumn {
    pb::ScanColumn {
        name: column.name.clone(),
        data_type: column.data_type.clone(),
        role: match column.role {
            ColumnRole::Pk => "pk",
            ColumnRole::Source => "source",
            ColumnRole::IngestPartition => "ingest_partition",
            ColumnRole::IngestOffset => "ingest_offset",
            ColumnRole::Vector => "vector",
            ColumnRole::SparseVector => "sparse_vector",
        }
        .to_owned(),
        vector: column.vector.clone(),
        dim: column.dim,
        distance: column.distance.clone(),
        modifier: column.modifier.clone(),
        ..Default::default()
    }
}