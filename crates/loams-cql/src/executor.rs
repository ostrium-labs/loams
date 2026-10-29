//! A tenant-scoped single-partition SQL subset over transactional Loams KV.
use crate::{
    parser::{self, Column, DataType, QualifiedName, SelectColumns, Statement, Value},
    query::{ColumnSpec, ColumnType, QueryError, QueryExecutor, QueryResult, SessionContext},
    storage::{Cell, RowStore},
};
use async_trait::async_trait;
use loams_kv::{Store, TxnError, TxnOptions};
use loams_tuple::key::put_bytes;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
/// Executes statements in one pre-authorized Loams namespace and tenant-scoped store.
#[derive(Clone, Debug)]
pub struct KvExecutor {
    namespace: String,
    store: Store,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Table {
    id: u32,
    columns: Vec<Column>,
    primary: String,
}
fn invalid(s: impl Into<String>) -> QueryError {
    QueryError::Invalid(s.into())
}
fn catalog(keyspace: &str, table: Option<&str>) -> Vec<u8> {
    let mut k = vec![b'C', b's'];
    put_bytes(&mut k, keyspace.as_bytes());
    if let Some(t) = table {
        put_bytes(&mut k, t.as_bytes());
    }
    k
}
fn storage_error(e: TxnError) -> QueryError {
    match e {
        TxnError::Fatal(s) => invalid(s),
        _ => QueryError::Unavailable,
    }
}
fn name(n: QualifiedName, s: &SessionContext) -> Result<(String, String), QueryError> {
    Ok((
        n.keyspace
            .or_else(|| s.keyspace.clone())
            .ok_or_else(|| invalid("select a keyspace first"))?,
        n.table,
    ))
}
fn column_type(t: DataType) -> ColumnType {
    match t {
        DataType::Ascii => ColumnType::Ascii,
        DataType::Varchar => ColumnType::Varchar,
        DataType::TimeUuid => ColumnType::TimeUuid,
        DataType::Text => ColumnType::Text,
        DataType::Int => ColumnType::Int,
        DataType::BigInt => ColumnType::BigInt,
        DataType::SmallInt => ColumnType::SmallInt,
        DataType::TinyInt => ColumnType::TinyInt,
        DataType::Boolean => ColumnType::Boolean,
        DataType::Blob => ColumnType::Blob,
        DataType::Float => ColumnType::Float,
        DataType::Double => ColumnType::Double,
        DataType::Timestamp => ColumnType::Timestamp,
        DataType::Uuid => ColumnType::Uuid,
        DataType::Inet => ColumnType::Inet,
    }
}
fn encode(t: DataType, v: &Value, bind: &[Option<Vec<u8>>]) -> Result<Vec<u8>, QueryError> {
    let bytes = match v {
        Value::Bind(n) => bind
            .get(*n)
            .ok_or_else(|| invalid("missing bind value"))?
            .clone()
            .ok_or_else(|| {
                QueryError::Unsupported("null writes require deletion semantics".into())
            })?,
        Value::String(s) if matches!(t, DataType::Text | DataType::Varchar | DataType::Ascii) => {
            s.as_bytes().to_vec()
        }
        Value::Boolean(b) if t == DataType::Boolean => vec![u8::from(*b)],
        Value::Integer(n) => match t {
            DataType::BigInt | DataType::Timestamp => n.to_be_bytes().to_vec(),
            DataType::Int => i32::try_from(*n)
                .map_err(|_| invalid("integer overflow"))?
                .to_be_bytes()
                .to_vec(),
            DataType::SmallInt => i16::try_from(*n)
                .map_err(|_| invalid("integer overflow"))?
                .to_be_bytes()
                .to_vec(),
            DataType::TinyInt => {
                vec![i8::try_from(*n).map_err(|_| invalid("integer overflow"))? as u8]
            }
            _ => return Err(invalid("value type mismatch")),
        },
        Value::Float(n) => match t {
            DataType::Double => n.to_be_bytes().to_vec(),
            DataType::Float => (*n as f32).to_be_bytes().to_vec(),
            _ => return Err(invalid("value type mismatch")),
        },
        _ => {
            return Err(QueryError::Unsupported(
                "unsupported literal or null".into(),
            ));
        }
    };
    let valid = match t {
        DataType::TinyInt | DataType::Boolean => {
            bytes.len() == 1 && (t != DataType::Boolean || bytes[0] <= 1)
        }
        DataType::SmallInt => bytes.len() == 2,
        DataType::Int | DataType::Float => bytes.len() == 4,
        DataType::BigInt | DataType::Double | DataType::Timestamp => bytes.len() == 8,
        DataType::Uuid => bytes.len() == 16,
        DataType::TimeUuid => bytes.len() == 16 && bytes[6] >> 4 == 1,
        DataType::Inet => matches!(bytes.len(), 4 | 16),
        DataType::Ascii => bytes.is_ascii(),
        DataType::Text | DataType::Varchar => std::str::from_utf8(&bytes).is_ok(),
        DataType::Blob => true,
    };
    if !valid {
        return Err(invalid("invalid bound value encoding"));
    }
    Ok(bytes)
}
impl KvExecutor {
    /// The caller must provide a store isolated to this namespace and authenticate each session.
    pub fn new(namespace: impl Into<String>, store: Store) -> Self {
        Self {
            namespace: namespace.into(),
            store,
        }
    }
    async fn table(&self, ks: &str, t: &str) -> Result<Table, QueryError> {
        let bytes = self
            .store
            .get(&catalog(ks, Some(t)))
            .await
            .map_err(storage_error)?
            .ok_or_else(|| invalid("unknown table"))?;
        serde_json::from_slice(&bytes).map_err(|_| QueryError::Unavailable)
    }
}
#[async_trait]
impl QueryExecutor for KvExecutor {
    async fn execute(
        &self,
        s: &SessionContext,
        query: &str,
        values: &[Option<Vec<u8>>],
    ) -> Result<QueryResult, QueryError> {
        if s.namespace != self.namespace || s.principal.is_empty() {
            return Err(QueryError::Unauthorized);
        }
        let statement = parser::parse(query).map_err(|e| invalid(e.to_string()))?;
        let binds = match &statement {
            Statement::Insert { values, .. } => values
                .iter()
                .filter(|v| matches!(v, Value::Bind(_)))
                .count(),
            Statement::Select {
                key_value: Value::Bind(_),
                ..
            } => 1,
            _ => 0,
        };
        if values.len() != binds {
            return Err(invalid("incorrect number of bound values"));
        }
        match statement {
            Statement::CreateKeyspace {
                name,
                if_not_exists,
                replication,
            } => {
                if replication.replication_factor != 1 {
                    return Err(QueryError::Unsupported(
                        "this single-store profile requires replication_factor=1".into(),
                    ));
                }
                let key = catalog(&name, None);
                self.store
                    .run(TxnOptions::pessimistic("cql.create_keyspace"), move |txn| {
                        let key = key.clone();
                        Box::pin(async move {
                            if txn.get_for_update(&key).await?.is_some() {
                                if if_not_exists {
                                    return Ok(());
                                }
                                return Err(TxnError::Fatal("keyspace already exists".into()));
                            }
                            txn.put(&key, vec![1]).await
                        })
                    })
                    .await
                    .map_err(storage_error)?;
                Ok(QueryResult::Void)
            }
            Statement::Use { keyspace } => {
                if self
                    .store
                    .get(&catalog(&keyspace, None))
                    .await
                    .map_err(storage_error)?
                    .is_none()
                {
                    return Err(invalid("unknown keyspace"));
                }
                Ok(QueryResult::SetKeyspace(keyspace))
            }
            Statement::CreateTable {
                name: n,
                columns,
                primary_key,
                if_not_exists,
            } => {
                let (ks, t) = name(n, s)?;
                let key = catalog(&ks, Some(&t));
                let kskey = catalog(&ks, None);
                self.store
                    .run(TxnOptions::pessimistic("cql.create_table"), move |txn| {
                        let (key, kskey, columns, primary_key) = (
                            key.clone(),
                            kskey.clone(),
                            columns.clone(),
                            primary_key.clone(),
                        );
                        Box::pin(async move {
                            if txn.get(&kskey).await?.is_none() {
                                return Err(TxnError::Fatal("unknown keyspace".into()));
                            }
                            let counter = b"Cnext_table";
                            let old = txn.get_for_update(counter).await?;
                            let id = match old {
                                None => 1,
                                Some(v) => u32::from_be_bytes(v.try_into().map_err(|_| {
                                    TxnError::Fatal("invalid table counter".into())
                                })?)
                                .checked_add(1)
                                .ok_or_else(|| TxnError::Fatal("table IDs exhausted".into()))?,
                            };
                            if txn.get_for_update(&key).await?.is_some() {
                                if if_not_exists {
                                    return Ok(());
                                }
                                return Err(TxnError::Fatal("table already exists".into()));
                            }
                            let value = serde_json::to_vec(&Table {
                                id,
                                columns,
                                primary: primary_key,
                            })
                            .map_err(|_| TxnError::Fatal("schema encoding failed".into()))?;
                            txn.put(counter, id.to_be_bytes().to_vec()).await?;
                            txn.put(&key, value).await
                        })
                    })
                    .await
                    .map_err(storage_error)?;
                Ok(QueryResult::Void)
            }
            Statement::DropTable { name: n, if_exists } => {
                let (ks, t) = name(n, s)?;
                let key = catalog(&ks, Some(&t));
                self.store
                    .run(TxnOptions::pessimistic("cql.drop_table"), move |txn| {
                        let key = key.clone();
                        Box::pin(async move {
                            if txn.get_for_update(&key).await?.is_none() && !if_exists {
                                return Err(TxnError::Fatal("unknown table".into()));
                            }
                            txn.delete(&key).await
                        })
                    })
                    .await
                    .map_err(storage_error)?;
                Ok(QueryResult::Void)
            }
            Statement::Insert {
                table: n,
                columns,
                values: expressions,
            } => {
                let (ks, t) = name(n, s)?;
                let schema = self.table(&ks, &t).await?;
                let mut cells = BTreeMap::new();
                let mut pk = None;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| QueryError::Unavailable)?
                    .as_micros();
                let timestamp = i64::try_from(now).map_err(|_| QueryError::Unavailable)?;
                for (c, v) in columns.iter().zip(expressions.iter()) {
                    let ty = schema
                        .columns
                        .iter()
                        .find(|x| x.name == *c)
                        .ok_or_else(|| invalid("unknown column"))?
                        .data_type;
                    let value = encode(ty, v, values)?;
                    if *c == schema.primary {
                        pk = Some(value.clone());
                    }
                    cells.insert(c.clone(), Cell { timestamp, value });
                }
                RowStore::new(self.store.clone())
                    .upsert(
                        schema.id,
                        &pk.ok_or_else(|| invalid("primary key required"))?,
                        b"",
                        cells,
                    )
                    .await
                    .map_err(storage_error)?;
                Ok(QueryResult::Void)
            }
            Statement::Select {
                table: n,
                columns,
                key_column,
                key_value,
            } => {
                let (ks, t) = name(n, s)?;
                let schema = self.table(&ks, &t).await?;
                if key_column != schema.primary {
                    return Err(QueryError::Unsupported(
                        "single partition primary-key equality required".into(),
                    ));
                }
                let keytype = schema
                    .columns
                    .iter()
                    .find(|c| c.name == schema.primary)
                    .ok_or(QueryError::Unavailable)?
                    .data_type;
                let pk = encode(keytype, &key_value, values)?;
                let selected = match columns {
                    SelectColumns::All => schema.columns.clone(),
                    SelectColumns::Named(names) => names
                        .iter()
                        .map(|n| {
                            schema
                                .columns
                                .iter()
                                .find(|c| c.name == *n)
                                .cloned()
                                .ok_or_else(|| invalid("unknown column"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };
                let row = RowStore::new(self.store.clone())
                    .get(schema.id, &pk, b"")
                    .await
                    .map_err(storage_error)?;
                let rows = row
                    .map(|r| {
                        vec![
                            selected
                                .iter()
                                .map(|c| r.get(&c.name).map(|v| v.value.clone()))
                                .collect(),
                        ]
                    })
                    .unwrap_or_default();
                Ok(QueryResult::Rows {
                    columns: selected
                        .into_iter()
                        .map(|c| ColumnSpec {
                            keyspace: ks.clone(),
                            table: t.clone(),
                            name: c.name,
                            data_type: column_type(c.data_type),
                        })
                        .collect(),
                    rows,
                    paging_state: None,
                })
            }
        }
    }
}
