//! The host side of `ctx.db` (LV1 plan Tasks 3 and 4): each operation a
//! function's JavaScript asks for, run on the call's [`LiveTxn`] so reads
//! land in the read set.
//!
//! The arguments have the `_system:*` shapes ([`system`]). A query also
//! names the index fields its range uses, which are checked against the
//! index; a page also carries `cursor` (a string or null) and `numItems`.
//! A page is `{ page, continueCursor, isDone }` ([`LiveTxn::paginate`]).

use std::collections::BTreeMap;

use loams_live::catalog::{BY_CREATION_TIME, BY_ID, CREATION_TIME_FIELD, ID_FIELD};
use loams_live::query::{QueryArgs, doc_value};
use loams_live::{IndexRange, LiveError, LiveTxn, LiveValue, Page, system};

/// A `ctx.db` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostOp {
    Get,
    Query,
    Paginate,
    Insert,
    Patch,
    Replace,
    Delete,
}

impl HostOp {
    pub(crate) fn parse(op: &str) -> Option<Self> {
        Some(match op {
            "get" => HostOp::Get,
            "query" => HostOp::Query,
            "paginate" => HostOp::Paginate,
            "insert" => HostOp::Insert,
            "patch" => HostOp::Patch,
            "replace" => HostOp::Replace,
            "delete" => HostOp::Delete,
            _ => return None,
        })
    }
}

/// Runs a host operation on the call's transaction.
pub(crate) async fn host(
    txn: &mut LiveTxn<'_>,
    op: HostOp,
    args: LiveValue,
) -> Result<LiveValue, LiveError> {
    let name = match op {
        HostOp::Get => system::GET,
        HostOp::Query => return query(txn, args).await,
        HostOp::Paginate => return paginate(txn, args).await,
        HostOp::Insert => system::INSERT,
        HostOp::Patch => system::PATCH,
        HostOp::Replace => system::REPLACE,
        HostOp::Delete => system::DELETE,
    };
    let f = system::lookup(name)
        .ok_or_else(|| LiveError::Internal(format!("no system function {name}")))?;
    f.call(txn, args).await
}

fn object(args: LiveValue) -> Result<BTreeMap<String, LiveValue>, LiveError> {
    match args {
        LiveValue::Object(args) => Ok(args),
        _ => Err(LiveError::Internal(
            "a query's arguments are an object".into(),
        )),
    }
}

async fn query(txn: &mut LiveTxn<'_>, args: LiveValue) -> Result<LiveValue, LiveError> {
    let Some(range) = range(txn, object(args)?).await? else {
        return Ok(LiveValue::Array(Vec::new()));
    };
    let docs = txn.query(range).await?;
    Ok(LiveValue::Array(docs.iter().map(doc_value).collect()))
}

async fn paginate(txn: &mut LiveTxn<'_>, args: LiveValue) -> Result<LiveValue, LiveError> {
    let mut args = object(args)?;
    let cursor = match args.remove("cursor") {
        None | Some(LiveValue::Null) => None,
        Some(LiveValue::Str(c)) => Some(c),
        Some(other) => {
            return Err(LiveError::InvalidArgument(format!(
                "paginate: cursor is a string or null, not {}",
                other.type_name()
            )));
        }
    };
    let num_items = match args.remove("numItems") {
        Some(LiveValue::I64(n)) => u32::try_from(n).ok().filter(|&n| n > 0),
        _ => None,
    }
    .ok_or_else(|| LiveError::InvalidArgument("paginate: numItems is 1 to 2^32 − 1".into()))?;
    let page = match range(txn, args).await? {
        Some(range) => txn.paginate(range, cursor.as_deref(), num_items).await?,
        None => txn.empty_page(cursor.as_deref()).await?,
    };
    Ok(page_value(&page))
}

fn page_value(page: &Page) -> LiveValue {
    LiveValue::Object(BTreeMap::from([
        (
            "page".to_string(),
            LiveValue::Array(page.docs.iter().map(doc_value).collect()),
        ),
        (
            "continueCursor".to_string(),
            LiveValue::Str(page.continue_cursor.clone()),
        ),
        ("isDone".to_string(), LiveValue::Bool(page.is_done)),
    ]))
}

/// The range a `ctx.db.query` names, its index fields checked; `None` when
/// the table does not exist (the read set then depends on its creation).
async fn range(
    txn: &mut LiveTxn<'_>,
    mut args: BTreeMap<String, LiveValue>,
) -> Result<Option<IndexRange>, LiveError> {
    let named: Vec<String> = match args.remove("fields") {
        Some(LiveValue::Array(fields)) => fields
            .into_iter()
            .map(|f| match f {
                LiveValue::Str(s) => Ok(s),
                other => Err(LiveError::InvalidArgument(format!(
                    "withIndex: a field name is a string, not {}",
                    other.type_name()
                ))),
            })
            .collect::<Result<_, _>>()?,
        _ => Vec::new(),
    };
    let query = QueryArgs::parse("db.query", LiveValue::Object(args))?;
    let Some(table) = txn.table(&query.table).await? else {
        return Ok(None);
    };
    let fields: Vec<String> = match query.index.as_str() {
        BY_CREATION_TIME => vec![CREATION_TIME_FIELD.to_string()],
        BY_ID => vec![ID_FIELD.to_string()],
        name => table
            .indexes
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.fields.clone())
            .unwrap_or_default(),
    };
    if table.index_id(&query.index).is_some()
        && (named.len() > fields.len() || named.iter().zip(&fields).any(|(a, b)| a != b))
    {
        return Err(LiveError::InvalidArgument(format!(
            "withIndex: index '{}' of table '{}' has the fields [{}], in that order; \
             the range names [{}]",
            query.index,
            table.name,
            fields.join(", "),
            named.join(", ")
        )));
    }
    Ok(Some(query.range(&table)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_ops_parse() {
        assert_eq!(HostOp::parse("get"), Some(HostOp::Get));
        assert_eq!(HostOp::parse("paginate"), Some(HostOp::Paginate));
        assert_eq!(HostOp::parse("tables"), None);
    }
}
