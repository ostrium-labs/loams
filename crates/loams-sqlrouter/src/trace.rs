//! Spec events (§31 §11.3, D311). Every machine reports each transition it
//! takes, named as the TLA+ action it implements, so RT1's trace validation
//! can check recorded runs against `spec/tla/router/` and `loams-specview`
//! can show them.

use serde::Serialize;

/// One transition: the spec, its action and the observed fields, e.g.
/// `ShardMap / Reload {instance: "pgdog-1", gen: 4, ok: true}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpecEvent {
    /// The TLA+ module, e.g. `ShardMap`.
    pub spec: &'static str,
    /// The action in that module, e.g. `Reload`.
    pub action: &'static str,
    /// The observed values, in the order the spec's trace checker reads them.
    pub fields: Vec<(&'static str, SpecValue)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
/// A field value, serialised as the bare JSON value.
pub enum SpecValue {
    /// A TLA+ `BOOLEAN`.
    Bool(bool),
    /// A TLA+ integer.
    Int(i64),
    /// A TLA+ string (instance, shard and key names).
    Str(String),
}

impl From<bool> for SpecValue {
    fn from(v: bool) -> Self {
        SpecValue::Bool(v)
    }
}

impl From<i64> for SpecValue {
    fn from(v: i64) -> Self {
        SpecValue::Int(v)
    }
}

impl From<u32> for SpecValue {
    fn from(v: u32) -> Self {
        SpecValue::Int(i64::from(v))
    }
}

impl From<&str> for SpecValue {
    fn from(v: &str) -> Self {
        SpecValue::Str(v.to_owned())
    }
}

impl From<String> for SpecValue {
    fn from(v: String) -> Self {
        SpecValue::Str(v)
    }
}

/// Receives spec events from machines. Drivers decide where they go.
pub trait TraceSink {
    /// Record one transition. Must not block or fail.
    fn emit(&mut self, event: SpecEvent);
}

/// Keeps every event in order; for tests and the simulator's run directory.
#[derive(Debug, Default)]
pub struct VecSink(pub Vec<SpecEvent>);

impl TraceSink for VecSink {
    fn emit(&mut self, event: SpecEvent) {
        self.0.push(event);
    }
}

/// Drops every event; for drivers that do not record traces.
#[derive(Debug, Default)]
pub struct NullSink;

impl TraceSink for NullSink {
    fn emit(&mut self, _event: SpecEvent) {}
}

/// `events` as a TLA+ sequence of records, one per line, for trace specs
/// that read a trace as a TLA+ definition (`spec/tla/router/LifecycleTrace.tla`;
/// TLC 1.7.4 has no Json module): each record has `action` and the event's
/// fields, e.g. `[action |-> "Admit", conn |-> "c1"]`.
pub fn tla_sequence(events: &[SpecEvent]) -> String {
    let value = |v: &SpecValue| match v {
        SpecValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        SpecValue::Int(i) => i.to_string(),
        SpecValue::Str(s) => format!("{s:?}"),
    };
    let records: Vec<String> = events
        .iter()
        .map(|e| {
            let mut fields = vec![format!("action |-> {:?}", e.action)];
            fields.extend(
                e.fields
                    .iter()
                    .map(|(k, v)| format!("{k} |-> {}", value(v))),
            );
            format!("    [{}]", fields.join(", "))
        })
        .collect();
    format!("<<\n{}\n>>", records.join(",\n"))
}
