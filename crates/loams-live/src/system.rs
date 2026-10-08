//! The built-in system functions (R1 plan Task 10): `_system:get`,
//! `_system:query`, `_system:insert`, `_system:patch`, `_system:replace` and
//! `_system:delete`. They run through [`LiveTxn`] like deployed functions,
//! so R1 works end to end before any bundle is deployed.
//!
//! | Function | Kind | Arguments | Result |
//! |---|---|---|---|
//! | `_system:get` | query | `{ id }` | the document, or `null` |
//! | `_system:query` | query | see [`QueryArgs`] | an array of documents |
//! | `_system:tables` | query | `{}` | `[{ name, id, indexes: [{ name, fields }] }]` |
//! | `_system:insert` | mutation | `{ table, fields }` | the new id |
//! | `_system:patch` | mutation | `{ id, fields }` | `null` |
//! | `_system:replace` | mutation | `{ id, fields }` | `null` |
//! | `_system:delete` | mutation | `{ id }` | `null` |
//!
//! Documents are values as [`doc_value`] builds them.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::catalog::{BY_CREATION_TIME, BY_ID, TableDef};
use crate::query::{QueryArgs, doc_value, fields_arg, id_arg, object_args, str_arg};
use crate::txn::{FnKind, Function, LiveTxn};
use crate::{LiveError, LiveValue};

/// `_system:get`.
pub const GET: &str = "_system:get";
/// `_system:query`.
pub const QUERY: &str = "_system:query";
/// `_system:tables`: the tables and their indexes, built-ins included.
pub const TABLES: &str = "_system:tables";
/// `_system:insert`.
pub const INSERT: &str = "_system:insert";
/// `_system:patch`.
pub const PATCH: &str = "_system:patch";
/// `_system:replace`.
pub const REPLACE: &str = "_system:replace";
/// `_system:delete`.
pub const DELETE: &str = "_system:delete";

/// Every system function's name.
pub const NAMES: [&str; 7] = [GET, QUERY, TABLES, INSERT, PATCH, REPLACE, DELETE];

/// One system function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct System {
    name: &'static str,
}

/// The system function `name`, or `None`.
pub fn lookup(name: &str) -> Option<Arc<dyn Function>> {
    NAMES
        .iter()
        .find(|n| **n == name)
        .map(|&name| Arc::new(System { name }) as Arc<dyn Function>)
}

/// A table as `_system:tables` reports it: the implicit `by_id` and
/// `by_creation_time` indexes first, then the user indexes.
pub fn table_value(t: &TableDef) -> LiveValue {
    let index = |name: &str, fields: &[String]| {
        LiveValue::Object(BTreeMap::from([
            ("name".to_string(), LiveValue::Str(name.to_string())),
            (
                "fields".to_string(),
                LiveValue::Array(fields.iter().cloned().map(LiveValue::Str).collect()),
            ),
        ]))
    };
    let mut indexes = vec![index(BY_ID, &[]), index(BY_CREATION_TIME, &[])];
    indexes.extend(t.indexes.iter().map(|i| index(&i.name, &i.fields)));
    LiveValue::Object(BTreeMap::from([
        ("name".to_string(), LiveValue::Str(t.name.clone())),
        ("id".to_string(), LiveValue::I64(i64::from(t.id.0))),
        ("indexes".to_string(), LiveValue::Array(indexes)),
    ]))
}

impl Function for System {
    fn name(&self) -> &str {
        self.name
    }

    fn kind(&self) -> FnKind {
        match self.name {
            GET | QUERY | TABLES => FnKind::Query,
            _ => FnKind::Mutation,
        }
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let name = self.name;
            match name {
                GET => {
                    let mut args = object_args(name, args, &["id"])?;
                    let id = id_arg(name, &mut args, "id")?;
                    Ok(txn
                        .get(id)
                        .await?
                        .map_or(LiveValue::Null, |doc| doc_value(&doc)))
                }
                QUERY => {
                    let args = QueryArgs::parse(name, args)?;
                    let Some(table) = txn.table(&args.table).await? else {
                        return Ok(LiveValue::Array(Vec::new()));
                    };
                    let docs = txn.query(args.range(&table)?).await?;
                    Ok(LiveValue::Array(docs.iter().map(doc_value).collect()))
                }
                TABLES => {
                    object_args(name, args, &[])?;
                    let tables = txn.tables().await?;
                    Ok(LiveValue::Array(tables.iter().map(table_value).collect()))
                }
                INSERT => {
                    let mut args = object_args(name, args, &["table", "fields"])?;
                    let table = str_arg(name, &mut args, "table")?;
                    let fields = fields_arg(name, &mut args, "fields")?;
                    let id = txn.insert(&table, fields).await?;
                    Ok(LiveValue::Str(id.to_string()))
                }
                PATCH | REPLACE => {
                    let mut args = object_args(name, args, &["id", "fields"])?;
                    let id = id_arg(name, &mut args, "id")?;
                    let fields = fields_arg(name, &mut args, "fields")?;
                    if name == PATCH {
                        txn.patch(id, fields).await?;
                    } else {
                        txn.replace(id, fields).await?;
                    }
                    Ok(LiveValue::Null)
                }
                DELETE => {
                    let mut args = object_args(name, args, &["id"])?;
                    let id = id_arg(name, &mut args, "id")?;
                    txn.delete(id).await?;
                    Ok(LiveValue::Null)
                }
                other => Err(LiveError::Internal(format!("no system function {other}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::IndexDef;
    use crate::{IndexId, TableId};

    #[test]
    fn tables_is_a_query_and_listed() {
        let f = lookup(TABLES).expect("_system:tables exists");
        assert_eq!(f.kind(), FnKind::Query);
        assert!(NAMES.contains(&TABLES));
    }

    #[test]
    fn table_value_lists_builtin_then_user_indexes() {
        let t = TableDef {
            id: TableId(3),
            name: "people".into(),
            indexes: vec![IndexDef {
                id: IndexId(IndexId::FIRST_USER),
                name: "by_name".into(),
                fields: vec!["name".into()],
            }],
            next_index_id: IndexId::FIRST_USER + 1,
        };
        let LiveValue::Object(o) = table_value(&t) else {
            panic!("an object")
        };
        assert_eq!(o["name"], LiveValue::Str("people".into()));
        assert_eq!(o["id"], LiveValue::I64(3));
        let LiveValue::Array(ix) = &o["indexes"] else {
            panic!("an array")
        };
        let names: Vec<_> = ix
            .iter()
            .map(|i| match i {
                LiveValue::Object(m) => m["name"].clone(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            names,
            ["by_id", "by_creation_time", "by_name"].map(|n| LiveValue::Str(n.into()))
        );
    }
}
