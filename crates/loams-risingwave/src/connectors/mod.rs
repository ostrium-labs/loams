//! Typed builders for RisingWave's data-integration DDL (plan RW1, lane RW-b).
//!
//! Each builder renders one statement with [`Ddl::to_sql`] (credentials read
//! from the environment where the spec names a variable) or
//! [`Ddl::to_redacted_sql`] (every credential shown as `<redacted>`).
//! Identifiers and literals are always quoted ([`quote`]). Credentials are
//! the instance's own scoped service credentials (design 81 R5),
//! written inline; no builder needs a RisingWave licence (R4). The
//! shapes follow RisingWave's documented syntax:
//!
//! | Builder | Statement |
//! |---|---|
//! | [`kafka::KafkaIngest`] | `CREATE SOURCE` / `CREATE TABLE … WITH (connector = 'kafka')` from a Loams stream through the Iggy Kafka gateway |
//! | [`postgres_cdc::PostgresCdcSource`], [`postgres_cdc::CdcTable`] | `CREATE SOURCE … (connector = 'postgres-cdc')`, `CREATE TABLE … FROM src TABLE 'schema.table'` |
//! | [`MaterializedView`] | `CREATE MATERIALIZED VIEW … AS …` |
//! | [`kafka::KafkaSink`] | `CREATE SINK … (connector = 'kafka')` into a Loams stream |
//! | [`iceberg::IcebergSink`] | `CREATE SINK … (connector = 'iceberg', catalog.type = 'rest')` through Lakekeeper |
//! | [`postgres_sink::PostgresSink`] | `CREATE SINK … (connector = 'jdbc' \| 'postgres')` into Loams Postgres |
//! | [`clickhouse::HouseSink`] | `CREATE SINK … (connector = 'clickhouse')`: not a default path (R10) |
//! | [`secret::CreateSecret`] | `CREATE SECRET`: a RisingWave Premium feature, never used by default (R4) |
//!
//! Status: preview. The option names follow RisingWave's documentation and
//! are checked against the pinned image by `conformance/risingwave/` once the
//! managed cluster runs (not yet run end to end).

pub mod clickhouse;
pub mod format;
pub mod iceberg;
pub mod kafka;
pub mod pipeline;
pub mod postgres_cdc;
pub mod postgres_sink;
pub mod quote;
/// `CREATE SECRET` (RisingWave Premium; never a default, R4).
pub mod secret;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use quote::{quote_ident, quote_literal};

/// Why a builder could not render its statement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DdlError {
    #[error("an identifier is empty")]
    EmptyIdent,
    #[error("a {what} holds a NUL character")]
    Nul { what: &'static str },
    #[error("{0:?} is not a valid option key")]
    BadOptionKey(String),
    #[error("{0:?} is not an accepted column data type")]
    BadDataType(String),
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("environment variable {0} is not set")]
    MissingEnv(String),
}

/// How credentials are rendered.
pub struct RenderContext<'a> {
    /// Looks up an environment variable named by [`Credential::Env`].
    env: &'a dyn Fn(&str) -> Option<String>,
    /// Show every credential as `<redacted>`.
    redact: bool,
}

impl fmt::Debug for RenderContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderContext")
            .field("redact", &self.redact)
            .finish_non_exhaustive()
    }
}

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn no_env(_: &str) -> Option<String> {
    None
}

/// The text a redacted credential renders as.
pub const REDACTED: &str = "<redacted>";

impl<'a> RenderContext<'a> {
    /// Credentials resolved through `env`.
    pub fn with_env(env: &'a dyn Fn(&str) -> Option<String>) -> Self {
        Self { env, redact: false }
    }

    /// Credentials resolved from the process environment.
    pub fn process() -> RenderContext<'static> {
        RenderContext {
            env: &process_env,
            redact: false,
        }
    }

    /// Every credential redacted; the environment is not read.
    pub fn redacted() -> RenderContext<'static> {
        RenderContext {
            env: &no_env,
            redact: true,
        }
    }

    pub fn is_redacted(&self) -> bool {
        self.redact
    }

    /// The literal a credential renders as, or the secret it references.
    fn credential(&self, credential: &Credential) -> Result<OptionValue, DdlError> {
        match credential {
            Credential::Secret(name) => Ok(OptionValue::Secret(name.clone())),
            _ if self.redact => Ok(OptionValue::Literal(REDACTED.to_owned())),
            Credential::Inline(value) => Ok(OptionValue::Literal(value.clone())),
            Credential::Env(var) => match (self.env)(var.as_str()) {
                Some(value) if !value.is_empty() => Ok(OptionValue::Literal(value)),
                _ => Err(DdlError::MissingEnv(var.clone())),
            },
        }
    }
}

/// One rendered statement.
pub trait Ddl {
    /// The statement, ending in `;`, with credentials rendered as `ctx` says.
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError>;

    /// The statement to execute: credentials named by an environment
    /// variable are read from the process environment.
    fn to_sql(&self) -> Result<String, DdlError> {
        self.render(&RenderContext::process())
    }

    /// The statement for logs and display: every credential `<redacted>`.
    fn to_redacted_sql(&self) -> Result<String, DdlError> {
        self.render(&RenderContext::redacted())
    }
}

/// A possibly schema-qualified relation or secret name.
///
/// Serialized as a string: `orders` or `analytics.orders` (split on the
/// first `.`). A name with a `.` of its own is built with
/// [`ObjectName::qualified`] in code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct ObjectName {
    pub schema: Option<String>,
    pub name: String,
}

impl ObjectName {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// `"schema"."name"` or `"name"`.
    pub fn render(&self) -> Result<String, DdlError> {
        match &self.schema {
            Some(schema) => Ok(format!(
                "{}.{}",
                quote_ident(schema)?,
                quote_ident(&self.name)?
            )),
            None => quote_ident(&self.name),
        }
    }
}

impl From<String> for ObjectName {
    fn from(text: String) -> Self {
        match text.split_once('.') {
            Some((schema, name)) if !schema.is_empty() && !name.is_empty() => {
                Self::qualified(schema, name)
            }
            _ => Self::new(text),
        }
    }
}

impl From<&str> for ObjectName {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

impl From<ObjectName> for String {
    fn from(name: ObjectName) -> Self {
        match name.schema {
            Some(schema) => format!("{schema}.{}", name.name),
            None => name.name,
        }
    }
}

impl fmt::Display for ObjectName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.schema {
            Some(schema) => write!(f, "{schema}.{}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// A password, key or token in a `WITH` clause (design 81 R5).
///
/// The credentials are the instance's own scoped service credentials, minted
/// by the engines facade: its Iggy user for the Kafka gateway, its Postgres
/// role per CDC source or sink, its Lakekeeper client. A user's Loams API key
/// is never written into engine SQL. RisingWave keeps every statement's text
/// in `rw_catalog` (the `definition` columns), which is why only those
/// scoped credentials go there.
///
/// - [`Credential::Env`] (the default in pipeline specs) names an environment
///   variable read when the statement is rendered for execution, so a spec
///   file never holds a value.
/// - [`Credential::Inline`] is the value itself.
/// - [`Credential::Secret`] references a RisingWave secret (`SECRET "name"`).
///   Secret management is a RisingWave **Premium** feature (R4): it
///   needs a licence key Loams does not ship, so no default or documented
///   path uses it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Credential {
    /// An environment variable holding the value.
    Env(String),
    /// The value itself, written into the DDL text.
    Inline(String),
    /// A RisingWave secret's name (Premium).
    Secret(ObjectName),
}

impl Credential {
    pub fn env(var: impl Into<String>) -> Self {
        Self::Env(var.into())
    }

    pub fn inline(value: impl Into<String>) -> Self {
        Self::Inline(value.into())
    }

    /// A RisingWave secret reference. Premium: see the type's docs.
    pub fn secret(name: impl Into<ObjectName>) -> Self {
        Self::Secret(name.into())
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(var) => f.debug_tuple("Env").field(var).finish(),
            Self::Inline(_) => f.write_str("Inline(<redacted>)"),
            Self::Secret(name) => f.debug_tuple("Secret").field(name).finish(),
        }
    }
}

/// One option value: a literal, a secret reference, or a credential that is
/// resolved when the statement is rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    Literal(String),
    Secret(ObjectName),
    Credential(Credential),
}

/// An ordered list of `key = value` options, rendered as `WITH (…)` or as a
/// `FORMAT … ENCODE … (…)` option list.
///
/// Setting a key twice keeps the first position and the last value, so a
/// caller's `extra` options can override a builder's default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    entries: Vec<(String, OptionValue)>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, key: &str) -> Option<&OptionValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn put(&mut self, key: &str, value: OptionValue) {
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.entries.push((key.to_owned(), value)),
        }
    }

    /// Sets a literal option.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.put(key, OptionValue::Literal(value.into()));
        self
    }

    /// Sets a literal option when there is a value.
    pub fn set_opt(&mut self, key: &str, value: Option<impl Into<String>>) -> &mut Self {
        if let Some(value) = value {
            self.set(key, value);
        }
        self
    }

    /// Sets a credential option, resolved (or redacted) at render time.
    pub fn set_credential(&mut self, key: &str, credential: &Credential) -> &mut Self {
        self.put(key, OptionValue::Credential(credential.clone()));
        self
    }

    /// Adds every `extra` option, overriding earlier values.
    pub fn extend(&mut self, extra: &BTreeMap<String, String>) -> &mut Self {
        for (key, value) in extra {
            self.set(key, value.clone());
        }
        self
    }

    /// `key = value` for each entry.
    fn render_entries(&self, ctx: &RenderContext<'_>) -> Result<Vec<String>, DdlError> {
        let mut out = Vec::with_capacity(self.entries.len());
        for (key, value) in &self.entries {
            quote::check_option_key(key)?;
            let resolved = match value {
                OptionValue::Credential(credential) => ctx.credential(credential)?,
                other => other.clone(),
            };
            let value = match resolved {
                OptionValue::Literal(text) => quote_literal(&text)?,
                OptionValue::Secret(name) => format!("SECRET {}", name.render()?),
                OptionValue::Credential(_) => {
                    return Err(DdlError::Invalid("an unresolved credential".to_owned()));
                }
            };
            out.push(format!("{key} = {value}"));
        }
        Ok(out)
    }

    /// `WITH (\n    key = 'value',\n    …\n)`.
    pub fn render_with(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if self.entries.is_empty() {
            return Err(DdlError::Missing("WITH options"));
        }
        let lines: Vec<String> = self
            .render_entries(ctx)?
            .into_iter()
            .map(|line| format!("    {line}"))
            .collect();
        Ok(format!("WITH (\n{}\n)", lines.join(",\n")))
    }

    /// `(key = 'value', …)` on one line, for `FORMAT … ENCODE … (…)`.
    pub fn render_inline(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        Ok(format!("({})", self.render_entries(ctx)?.join(", ")))
    }
}

/// One column of a source or table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// The RisingWave type as written: `INT`, `VARCHAR`, `TIMESTAMPTZ`,
    /// `JSONB`, `STRUCT<a INT>`.
    #[serde(rename = "type")]
    pub data_type: String,
}

impl Column {
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
        }
    }

    fn render(&self) -> Result<String, DdlError> {
        quote::check_data_type(&self.data_type)?;
        Ok(format!(
            "{} {}",
            quote_ident(&self.name)?,
            self.data_type.trim()
        ))
    }
}

/// `WATERMARK FOR <column> AS <column> - INTERVAL '<delay>'`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    pub column: String,
    /// An interval literal: `5 seconds`, `1 minute`.
    pub delay: String,
}

impl Watermark {
    pub fn new(column: impl Into<String>, delay: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            delay: delay.into(),
        }
    }

    fn render(&self) -> Result<String, DdlError> {
        let column = quote_ident(&self.column)?;
        Ok(format!(
            "WATERMARK FOR {column} AS {column} - INTERVAL {}",
            quote_literal(&self.delay)?
        ))
    }
}

/// The column list of a source or table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_key: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<Watermark>,
    /// `(*)`: the columns come from upstream (a CDC table's upstream
    /// schema). With a schema registry, leave `columns` empty instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub infer: bool,
}

impl Schema {
    pub fn columns(columns: impl IntoIterator<Item = Column>) -> Self {
        Self {
            columns: columns.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn inferred() -> Self {
        Self {
            infer: true,
            ..Self::default()
        }
    }

    pub fn with_primary_key<S: Into<String>>(mut self, key: impl IntoIterator<Item = S>) -> Self {
        self.primary_key = key.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_watermark(mut self, watermark: Watermark) -> Self {
        self.watermark = Some(watermark);
        self
    }

    /// ` (\n    "a" INT,\n    …\n)`, or `` when there is nothing to list,
    /// or ` (*)`.
    fn render(&self) -> Result<String, DdlError> {
        if self.infer {
            if !self.columns.is_empty() {
                return Err(DdlError::Invalid(
                    "a schema is either inferred (*) or lists its columns".to_owned(),
                ));
            }
            if self.primary_key.is_empty() && self.watermark.is_none() {
                return Ok(" (*)".to_owned());
            }
            let mut items = vec!["*".to_owned()];
            if !self.primary_key.is_empty() {
                items.push(primary_key_clause(&self.primary_key)?);
            }
            if let Some(watermark) = &self.watermark {
                items.push(watermark.render()?);
            }
            return Ok(format!(" ({})", items.join(", ")));
        }
        let mut items = Vec::new();
        for column in &self.columns {
            items.push(column.render()?);
        }
        if !self.primary_key.is_empty() {
            items.push(primary_key_clause(&self.primary_key)?);
        }
        if let Some(watermark) = &self.watermark {
            items.push(watermark.render()?);
        }
        if items.is_empty() {
            return Ok(String::new());
        }
        Ok(format!(
            " (\n{}\n)",
            items
                .iter()
                .map(|item| format!("    {item}"))
                .collect::<Vec<_>>()
                .join(",\n")
        ))
    }
}

fn primary_key_clause(key: &[String]) -> Result<String, DdlError> {
    let mut quoted = Vec::with_capacity(key.len());
    for column in key {
        quoted.push(quote_ident(column)?);
    }
    Ok(format!("PRIMARY KEY ({})", quoted.join(", ")))
}

/// What a sink reads: a relation (`FROM mv`) or a query (`AS SELECT …`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkFrom {
    Relation(ObjectName),
    Query(String),
}

impl SinkFrom {
    pub fn relation(name: impl Into<ObjectName>) -> Self {
        Self::Relation(name.into())
    }

    fn render(&self) -> Result<String, DdlError> {
        match self {
            Self::Relation(name) => Ok(format!("FROM {}", name.render()?)),
            Self::Query(query) => Ok(format!("AS {}", quote::clean_query(query)?)),
        }
    }
}

/// How a sink writes changes, for the sinks configured with `type = …`
/// (Iceberg, ClickHouse, JDBC/Postgres).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum SinkMode {
    /// `type = 'append-only'`: the upstream must be append-only.
    AppendOnly,
    /// `type = 'append-only', force_append_only = 'true'`: updates and
    /// deletes upstream are dropped.
    ForceAppendOnly,
    /// `type = 'upsert', primary_key = 'a,b'`.
    Upsert { primary_key: Vec<String> },
}

impl SinkMode {
    pub fn upsert<S: Into<String>>(key: impl IntoIterator<Item = S>) -> Self {
        Self::Upsert {
            primary_key: key.into_iter().map(Into::into).collect(),
        }
    }

    fn apply(&self, options: &mut Options) -> Result<(), DdlError> {
        match self {
            Self::AppendOnly => {
                options.set("type", "append-only");
            }
            Self::ForceAppendOnly => {
                options.set("type", "append-only");
                options.set("force_append_only", "true");
            }
            Self::Upsert { primary_key } => {
                options.set("type", "upsert");
                options.set("primary_key", join_list(primary_key, "primary_key")?);
            }
        }
        Ok(())
    }
}

/// `a,b` for a comma-separated option value.
fn join_list(names: &[String], what: &'static str) -> Result<String, DdlError> {
    if names.is_empty() {
        return Err(DdlError::Missing(what));
    }
    for name in names {
        quote::check_list_member(name)?;
    }
    Ok(names.join(","))
}

/// `CREATE SINK [IF NOT EXISTS] name FROM … | AS …\n` (the part before `WITH`).
fn sink_head(name: &ObjectName, if_not_exists: bool, from: &SinkFrom) -> Result<String, DdlError> {
    Ok(format!(
        "CREATE SINK {}{} {}",
        if if_not_exists { "IF NOT EXISTS " } else { "" },
        name.render()?,
        from.render()?
    ))
}

/// `CREATE MATERIALIZED VIEW [IF NOT EXISTS] name AS <query>;`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedView {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub query: String,
}

impl MaterializedView {
    pub fn new(name: impl Into<ObjectName>, query: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            query: query.into(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }
}

impl Ddl for MaterializedView {
    fn render(&self, _ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        Ok(format!(
            "CREATE MATERIALIZED VIEW {}{} AS\n{};",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name.render()?,
            quote::clean_query(&self.query)?
        ))
    }
}

/// `DROP <kind> [IF EXISTS] name [CASCADE];`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropObject {
    pub kind: ObjectKind,
    pub name: ObjectName,
    #[serde(default)]
    pub if_exists: bool,
    #[serde(default)]
    pub cascade: bool,
}

/// The kinds of object the builders create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Source,
    Table,
    MaterializedView,
    Sink,
    Secret,
}

impl ObjectKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Source => "SOURCE",
            Self::Table => "TABLE",
            Self::MaterializedView => "MATERIALIZED VIEW",
            Self::Sink => "SINK",
            Self::Secret => "SECRET",
        }
    }
}

impl Ddl for DropObject {
    fn render(&self, _ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        Ok(format!(
            "DROP {} {}{}{};",
            self.kind.keyword(),
            if self.if_exists { "IF EXISTS " } else { "" },
            self.name.render()?,
            if self.cascade { " CASCADE" } else { "" }
        ))
    }
}

/// A render context for tests: `Credential::Env("X")` renders as `'${X}'`.
#[cfg(test)]
pub(crate) mod testing {
    use super::RenderContext;

    fn env(name: &str) -> Option<String> {
        Some(format!("${{{name}}}"))
    }

    pub(crate) fn ctx() -> RenderContext<'static> {
        RenderContext::with_env(&env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_names_parse_and_render() {
        let plain = ObjectName::from("orders");
        assert_eq!(plain.render().unwrap(), "\"orders\"");
        let qualified = ObjectName::from("analytics.Orders");
        assert_eq!(qualified.render().unwrap(), "\"analytics\".\"Orders\"");
        assert_eq!(String::from(qualified), "analytics.Orders");
        assert_eq!(ObjectName::from(".x").render().unwrap(), "\".x\"");
        let json: ObjectName = serde_json::from_str("\"a.b\"").unwrap();
        assert_eq!(json, ObjectName::qualified("a", "b"));
    }

    #[test]
    fn options_keep_order_override_values_and_resolve_credentials() {
        let mut options = Options::new();
        options.set("connector", "kafka").set("topic", "a");
        options.set_credential(
            "properties.sasl.password",
            &Credential::env("RW_IGGY_PASSWORD"),
        );
        let mut extra = BTreeMap::new();
        extra.insert("topic".to_owned(), "b".to_owned());
        options.extend(&extra);
        let env = |name: &str| (name == "RW_IGGY_PASSWORD").then(|| "it's".to_owned());
        let ctx = RenderContext::with_env(&env);
        assert_eq!(
            options.render_with(&ctx).unwrap(),
            "WITH (\n    connector = 'kafka',\n    topic = 'b',\n    properties.sasl.password = 'it''s'\n)"
        );
        assert_eq!(
            options.render_inline(&RenderContext::redacted()).unwrap(),
            "(connector = 'kafka', topic = 'b', properties.sasl.password = '<redacted>')"
        );
        let unset = |_: &str| -> Option<String> { None };
        assert_eq!(
            options.render_with(&RenderContext::with_env(&unset)),
            Err(DdlError::MissingEnv("RW_IGGY_PASSWORD".to_owned()))
        );
        let mut bad = Options::new();
        bad.set("Bad Key", "x");
        assert!(bad.render_with(&ctx).is_err());
        assert!(Options::new().render_with(&ctx).is_err());
    }

    #[test]
    fn credentials_render_inline_redacted_or_as_premium_secrets() {
        let mut options = Options::new();
        options.set_credential("password", &Credential::inline("pw"));
        options.set_credential("token", &Credential::secret("tok"));
        let none = |_: &str| -> Option<String> { None };
        assert_eq!(
            options
                .render_inline(&RenderContext::with_env(&none))
                .unwrap(),
            "(password = 'pw', token = SECRET \"tok\")"
        );
        assert_eq!(
            options.render_inline(&RenderContext::redacted()).unwrap(),
            "(password = '<redacted>', token = SECRET \"tok\")"
        );
    }

    #[test]
    fn inline_credentials_are_redacted_in_debug() {
        let shown = format!("{:?}", Credential::Inline("lk_live_abc".to_owned()));
        assert!(!shown.contains("lk_live_abc"), "{shown}");
    }

    #[test]
    fn schemas_render_columns_keys_and_watermarks() {
        let schema = Schema::columns([
            Column::new("id", "BIGINT"),
            Column::new("ts", "TIMESTAMPTZ"),
        ])
        .with_primary_key(["id"])
        .with_watermark(Watermark::new("ts", "5 seconds"));
        assert_eq!(
            schema.render().unwrap(),
            " (\n    \"id\" BIGINT,\n    \"ts\" TIMESTAMPTZ,\n    PRIMARY KEY (\"id\"),\n    WATERMARK FOR \"ts\" AS \"ts\" - INTERVAL '5 seconds'\n)"
        );
        assert_eq!(Schema::default().render().unwrap(), "");
        assert_eq!(Schema::inferred().render().unwrap(), " (*)");
        assert_eq!(
            Schema::inferred()
                .with_primary_key(["id"])
                .render()
                .unwrap(),
            " (*, PRIMARY KEY (\"id\"))"
        );
        let mut both = Schema::inferred();
        both.columns.push(Column::new("a", "INT"));
        assert!(both.render().is_err());
        assert!(
            Schema::columns([Column::new("a", "INT; DROP")])
                .render()
                .is_err()
        );
    }

    #[test]
    fn materialized_views_and_drops() {
        let mv = MaterializedView::new("clicks_per_minute", "SELECT 1;\n").if_not_exists();
        assert_eq!(
            mv.to_sql().unwrap(),
            "CREATE MATERIALIZED VIEW IF NOT EXISTS \"clicks_per_minute\" AS\nSELECT 1;"
        );
        let drop = DropObject {
            kind: ObjectKind::MaterializedView,
            name: "a.b".into(),
            if_exists: true,
            cascade: true,
        };
        assert_eq!(
            drop.to_sql().unwrap(),
            "DROP MATERIALIZED VIEW IF EXISTS \"a\".\"b\" CASCADE;"
        );
    }

    #[test]
    fn sink_modes_set_type_and_keys() {
        let mut options = Options::new();
        SinkMode::upsert(["id", "region"])
            .apply(&mut options)
            .unwrap();
        assert_eq!(
            options.get("type"),
            Some(&OptionValue::Literal("upsert".to_owned()))
        );
        assert_eq!(
            options.get("primary_key"),
            Some(&OptionValue::Literal("id,region".to_owned()))
        );
        let mut empty = Options::new();
        assert!(
            SinkMode::upsert(Vec::<String>::new())
                .apply(&mut empty)
                .is_err()
        );
        let mut forced = Options::new();
        SinkMode::ForceAppendOnly.apply(&mut forced).unwrap();
        assert_eq!(
            forced.get("force_append_only"),
            Some(&OptionValue::Literal("true".to_owned()))
        );
    }
}
