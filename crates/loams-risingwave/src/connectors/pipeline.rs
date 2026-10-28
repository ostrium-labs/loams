//! A pipeline spec: statements in order, read from TOML or JSON by
//! `loams risingwave render` and `loams risingwave apply`.
//!
//! ```toml
//! [[statements]]
//! statement = "kafka"
//! kind = "source"
//! name = "clicks"
//! topic = "web.clicks"
//! endpoint = { bootstrap_servers = "kafka-gateway:9093", auth = { mode = "sasl", mechanism = "PLAIN", username = "eng_01jbzq7m3k8x2v5n9c4t6w0r1y", password = { env = "RW_KAFKA_PASSWORD" } } }
//! schema = { columns = [{ name = "url", type = "VARCHAR" }] }
//! ```
//!
//! Credential values never sit in the spec: each names the environment
//! variable that holds the instance's scoped service credential (design 81
//! R5), read when the pipeline is rendered for execution. RisingWave
//! secrets (`CREATE SECRET`) are Premium (R4), so a pipeline has no secrets
//! section.

use serde::{Deserialize, Serialize};

use super::clickhouse::HouseSink;
use super::iceberg::IcebergSink;
use super::kafka::{KafkaIngest, KafkaSink};
use super::postgres_cdc::{CdcTable, PostgresCdcSource};
use super::postgres_sink::PostgresSink;
use super::{Ddl, DdlError, DropObject, MaterializedView, RenderContext};

/// One statement of a pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "statement", rename_all = "snake_case")]
pub enum Statement {
    /// A source or table over a Loams stream.
    Kafka(KafkaIngest),
    PostgresCdcSource(PostgresCdcSource),
    CdcTable(CdcTable),
    MaterializedView(MaterializedView),
    KafkaSink(KafkaSink),
    IcebergSink(IcebergSink),
    /// Not a default path (R10).
    HouseSink(HouseSink),
    PostgresSink(PostgresSink),
    Drop(DropObject),
    /// SQL written by hand, run as is (one statement).
    Sql {
        sql: String,
    },
}

impl Statement {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        match self {
            Self::Kafka(s) => s.render(ctx),
            Self::PostgresCdcSource(s) => s.render(ctx),
            Self::CdcTable(s) => s.render(ctx),
            Self::MaterializedView(s) => s.render(ctx),
            Self::KafkaSink(s) => s.render(ctx),
            Self::IcebergSink(s) => s.render(ctx),
            Self::HouseSink(s) => s.render(ctx),
            Self::PostgresSink(s) => s.render(ctx),
            Self::Drop(s) => s.render(ctx),
            Self::Sql { sql } => Ok(format!("{};", super::quote::clean_query(sql)?)),
        }
    }
}

/// The whole spec.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    #[serde(default)]
    pub statements: Vec<Statement>,
}

/// Why a pipeline could not be read or rendered.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("reading the pipeline: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("reading the pipeline: {0}")]
    Json(#[from] serde_json::Error),
    #[error("statement {index}: {source}")]
    Statement { index: usize, source: DdlError },
}

/// One rendered statement: what is executed and what may be shown.
#[derive(Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The statement to execute (holds credential values).
    pub sql: String,
    /// The statement with every credential replaced by `<redacted>`.
    pub display: String,
}

impl std::fmt::Debug for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rendered")
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

impl Pipeline {
    /// Reads TOML.
    pub fn from_toml(text: &str) -> Result<Self, PipelineError> {
        Ok(toml::from_str(text)?)
    }

    /// Reads JSON.
    pub fn from_json(text: &str) -> Result<Self, PipelineError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Reads TOML or JSON, by the file name's extension (`.json` is JSON,
    /// anything else TOML).
    pub fn parse(file_name: &str, text: &str) -> Result<Self, PipelineError> {
        if file_name.ends_with(".json") {
            Self::from_json(text)
        } else {
            Self::from_toml(text)
        }
    }

    /// The statements for display only: credentials redacted, the
    /// environment not read.
    pub fn render_redacted(&self) -> Result<Vec<String>, PipelineError> {
        let ctx = RenderContext::redacted();
        self.statements
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                statement
                    .render(&ctx)
                    .map_err(|source| PipelineError::Statement { index, source })
            })
            .collect()
    }

    /// The statements to execute, with credentials read through `env` (the
    /// process environment in the CLI), each with its redacted display text.
    pub fn render(
        &self,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<Rendered>, PipelineError> {
        let ctx = RenderContext::with_env(env);
        let redacted = RenderContext::redacted();
        self.statements
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                let wrap = |source| PipelineError::Statement { index, source };
                Ok(Rendered {
                    sql: statement.render(&ctx).map_err(wrap)?,
                    display: statement.render(&redacted).map_err(wrap)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"
[[statements]]
statement = "kafka"
kind = "source"
name = "clicks"
topic = "web.clicks"
endpoint = { bootstrap_servers = "kafka-gateway:9093", auth = { mode = "sasl", mechanism = "PLAIN", username = "eng_01jbzq7m3k8x2v5n9c4t6w0r1y", password = { env = "RW_KAFKA_PASSWORD" } } }
schema = { columns = [{ name = "url", type = "VARCHAR" }, { name = "ts", type = "TIMESTAMPTZ" }], watermark = { column = "ts", delay = "5 seconds" } }

[[statements]]
statement = "materialized_view"
name = "clicks_per_minute"
query = """
SELECT window_start, url, count(*) AS clicks
FROM TUMBLE(clicks, ts, INTERVAL '1 minute')
GROUP BY window_start, url
"""

[[statements]]
statement = "iceberg_sink"
name = "clicks_lake"
from = { relation = "clicks_per_minute" }
database = "ns_default"
table = "clicks_per_minute"
mode = { type = "upsert", primary_key = ["window_start", "url"] }
catalog = { uri = "http://lakekeeper:8181/catalog", warehouse = "loams", name = "loams", auth = { mode = "none" } }

[[statements]]
statement = "sql"
sql = "SET STREAMING_PARALLELISM = 2;"
"#;

    #[test]
    fn a_toml_pipeline_renders_in_order() {
        let pipeline = Pipeline::parse("clicks.toml", SPEC).unwrap();
        assert_eq!(pipeline.statements.len(), 4);
        let env = |name: &str| (name == "RW_KAFKA_PASSWORD").then(|| "iggy-pw".to_owned());
        let rendered = pipeline.render(&env).unwrap();
        assert_eq!(rendered.len(), 4);
        assert!(
            rendered[0].sql.starts_with("CREATE SOURCE \"clicks\" (\n"),
            "{}",
            rendered[0].sql
        );
        assert!(
            rendered[0]
                .sql
                .contains("properties.sasl.password = 'iggy-pw'")
        );
        assert!(
            rendered[0]
                .display
                .contains("properties.sasl.password = '<redacted>'")
        );
        assert!(!format!("{:?}", rendered[0]).contains("iggy-pw"));
        assert!(
            rendered[1]
                .sql
                .starts_with("CREATE MATERIALIZED VIEW \"clicks_per_minute\" AS\nSELECT")
        );
        assert!(
            rendered[2]
                .sql
                .contains("catalog.header = 'X-Iceberg-Access-Delegation=vended-credentials'")
        );
        assert!(
            rendered[2]
                .sql
                .contains("commit_checkpoint_interval = '10'")
        );
        assert!(!rendered[2].sql.contains("catalog.credential"));
        assert_eq!(rendered[3].sql, "SET STREAMING_PARALLELISM = 2;");
    }

    #[test]
    fn a_missing_credential_names_its_variable() {
        let pipeline = Pipeline::from_toml(SPEC).unwrap();
        let err = pipeline
            .render(&|_: &str| -> Option<String> { None })
            .unwrap_err();
        assert!(err.to_string().contains("RW_KAFKA_PASSWORD"), "{err}");
        let redacted = pipeline.render_redacted().unwrap();
        assert!(redacted[0].contains("'<redacted>'"));
    }

    #[test]
    fn json_round_trips() {
        let pipeline = Pipeline::from_toml(SPEC).unwrap();
        let json = serde_json::to_string(&pipeline).unwrap();
        let back = Pipeline::parse("p.json", &json).unwrap();
        assert_eq!(back, pipeline);
    }

    #[test]
    fn a_bad_statement_reports_its_index() {
        let spec = r#"
[[statements]]
statement = "sql"
sql = "SELECT 1; SELECT 2"
"#;
        let err = Pipeline::from_toml(spec)
            .unwrap()
            .render_redacted()
            .unwrap_err();
        assert!(err.to_string().starts_with("statement 0:"), "{err}");
    }
}
