//! Execute guarded ClickHouse plans and write the oracle's compact JSON format.
use datafusion::{
    arrow::{array::RecordBatch, datatypes::SchemaRef},
    prelude::SessionContext,
};
use loams_house_sql::{Catalog, Registry};
use loams_house_types::{
    ChError,
    text::{Style, TextSettings, write_value},
};

/// Execute one SELECT. Unsupported syntax, formats and settings fail explicitly.
pub async fn execute(
    sql: &str,
    params: &[(String, String)],
    database: &str,
    catalog: &dyn Catalog,
) -> Result<Vec<u8>, ChError> {
    let prepared = loams_house_sql::prepare(sql, params, catalog, &Registry::default(), database)?;
    if let Some(format) = &prepared.format
        && format != "JSONCompactEachRowWithNamesAndTypes"
    {
        return Err(ChError::unsupported(format!("Output format {format}")));
    }
    if let Some((setting, _)) = prepared.settings.first() {
        return Err(ChError::unknown_setting(format!(
            "Setting {setting} is not supported yet"
        )));
    }
    let state = datafusion::execution::session_state::SessionStateBuilder::new()
        .with_default_features()
        .with_analyzer_rules(loams_house_sql::coerce::analyzer_rules())
        .build();
    let context = SessionContext::new_with_state(state);
    let frame = context
        .execute_logical_plan(prepared.plan)
        .await
        .map_err(loams_house_sql::plan::ch_error)?;
    let schema = frame.schema().as_arrow().clone();
    let batches = frame
        .collect()
        .await
        .map_err(loams_house_sql::plan::ch_error)?;
    compact_json(&std::sync::Arc::new(schema), &batches)
}

/// Names, exact ClickHouse types, then JSON rows. Also emits headers for zero rows.
pub fn compact_json(schema: &SchemaRef, batches: &[RecordBatch]) -> Result<Vec<u8>, ChError> {
    let types = schema
        .fields()
        .iter()
        .map(|f| loams_house_types::arrow::from_arrow(f))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    let names: Vec<_> = schema.fields().iter().map(|f| f.name()).collect();
    let type_names: Vec<_> = types.iter().map(|t| t.to_compact_string()).collect();
    for header in [serde_json::to_vec(&names), serde_json::to_vec(&type_names)] {
        out.extend(header.map_err(|e| ChError::logical_error(e.to_string()))?);
        out.push(b'\n');
    }
    for batch in batches {
        for row in 0..batch.num_rows() {
            out.push(b'[');
            for (col, ty) in types.iter().enumerate() {
                if col > 0 {
                    out.push(b',');
                }
                write_value(
                    ty,
                    batch.column(col).as_ref(),
                    row,
                    Style::Json,
                    &TextSettings::default(),
                    &mut out,
                )?;
            }
            out.extend_from_slice(b"]\n");
        }
    }
    Ok(out)
}
