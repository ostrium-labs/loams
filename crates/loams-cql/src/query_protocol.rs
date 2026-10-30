//! Bounded V4 QUERY decoding and result serialization.
use crate::query::{ColumnType, QueryError, QueryExecutor, QueryResult, SessionContext};
use cassandra_protocol::frame::{
    Direction, Envelope, Flags, Opcode, Serialize, Version,
    message_error::{ErrorBody, ErrorType},
    message_result::{
        BodyResResultRows, BodyResResultSetKeyspace, ColSpec, ColType, ColTypeOption,
        ResResultBody, RowsMetadata, RowsMetadataFlags, TableSpec,
    },
};
use cassandra_protocol::types::CBytes;

const MAX_VALUES: usize = 4096;

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], QueryError> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or_else(|| QueryError::Invalid("invalid length".into()))?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| QueryError::Invalid("truncated query".into()))?;
        self.offset = end;
        Ok(result)
    }
    fn u16(&mut self) -> Result<u16, QueryError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("length checked"),
        ))
    }
    fn i32(&mut self) -> Result<i32, QueryError> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().expect("length checked"),
        ))
    }
    fn positive_len(&mut self) -> Result<usize, QueryError> {
        usize::try_from(self.i32()?).map_err(|_| QueryError::Invalid("negative length".into()))
    }
}

struct DecodedQuery {
    text: String,
    values: Vec<Option<Vec<u8>>>,
    page_size: Option<usize>,
}
fn decode(body: &[u8]) -> Result<DecodedQuery, QueryError> {
    let mut input = Reader {
        bytes: body,
        offset: 0,
    };
    let len = input.positive_len()?;
    let text = std::str::from_utf8(input.take(len)?)
        .map_err(|_| QueryError::Invalid("query is not UTF-8".into()))?
        .to_owned();
    let consistency = input.u16()?;
    if consistency > 10 {
        return Err(QueryError::Invalid("unknown consistency level".into()));
    }
    let flags = input.take(1)?[0];
    // Bound values, metadata hint and page size are supported. Reject named
    // values, paging continuation, timestamps and serial options explicitly.
    if flags & !0x07 != 0 {
        return Err(QueryError::Unsupported(
            "query options are not supported yet".into(),
        ));
    }
    let mut values = Vec::new();
    if flags & 1 != 0 {
        let count = usize::from(input.u16()?);
        if count > MAX_VALUES {
            return Err(QueryError::Invalid("too many bound values".into()));
        }
        for _ in 0..count {
            let len = input.i32()?;
            if len == -1 {
                values.push(None);
            } else {
                let len = usize::try_from(len)
                    .map_err(|_| QueryError::Unsupported("unset value".into()))?;
                values.push(Some(input.take(len)?.to_vec()));
            }
        }
    }
    let page_size = if flags & 4 != 0 {
        let size = input.positive_len()?;
        if size == 0 {
            return Err(QueryError::Invalid("page size must be positive".into()));
        }
        Some(size)
    } else {
        None
    };
    if input.offset != body.len() {
        return Err(QueryError::Invalid("trailing query data".into()));
    }
    Ok(DecodedQuery {
        text,
        values,
        page_size,
    })
}

fn col_type(value: ColumnType) -> ColType {
    match value {
        ColumnType::Ascii => ColType::Ascii,
        ColumnType::Boolean => ColType::Boolean,
        ColumnType::TinyInt => ColType::Tinyint,
        ColumnType::SmallInt => ColType::Smallint,
        ColumnType::Int => ColType::Int,
        ColumnType::BigInt => ColType::Bigint,
        ColumnType::Float => ColType::Float,
        ColumnType::Double => ColType::Double,
        ColumnType::Text | ColumnType::Varchar => ColType::Varchar,
        ColumnType::Blob => ColType::Blob,
        ColumnType::Uuid => ColType::Uuid,
        ColumnType::TimeUuid => ColType::Timeuuid,
        ColumnType::Timestamp => ColType::Timestamp,
        ColumnType::Inet => ColType::Inet,
    }
}

fn encode_result(
    result: QueryResult,
    version: Version,
    page_size: Option<usize>,
) -> Result<Vec<u8>, QueryError> {
    let body = match result {
        QueryResult::Void => ResResultBody::Void,
        QueryResult::SetKeyspace(name) => {
            if name.len() > i16::MAX as usize {
                return Err(QueryError::Invalid("keyspace too long".into()));
            }
            ResResultBody::SetKeyspace(BodyResResultSetKeyspace { body: name })
        }
        QueryResult::Rows {
            columns,
            rows,
            paging_state,
        } => {
            if paging_state.is_some() || page_size.is_some_and(|limit| rows.len() > limit) {
                return Err(QueryError::Unsupported(
                    "paging is not supported yet".into(),
                ));
            }
            if rows.iter().any(|row| row.len() != columns.len()) {
                return Err(QueryError::Unavailable);
            }
            let columns_count =
                i32::try_from(columns.len()).map_err(|_| QueryError::Unavailable)?;
            let rows_count = i32::try_from(rows.len()).map_err(|_| QueryError::Unavailable)?;
            let col_specs = columns
                .into_iter()
                .map(|column| {
                    if [column.keyspace.len(), column.table.len(), column.name.len()]
                        .into_iter()
                        .any(|n| n > i16::MAX as usize)
                    {
                        return Err(QueryError::Unavailable);
                    }
                    Ok(ColSpec {
                        table_spec: Some(TableSpec {
                            ks_name: column.keyspace,
                            table_name: column.table,
                        }),
                        name: column.name,
                        col_type: ColTypeOption {
                            id: col_type(column.data_type),
                            value: None,
                        },
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let flags = if col_specs.is_empty() {
                RowsMetadataFlags::NO_METADATA
            } else {
                RowsMetadataFlags::empty()
            };
            let rows_content = rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|value| match value {
                            Some(value) => CBytes::new(value),
                            None => CBytes::new_null(),
                        })
                        .collect()
                })
                .collect();
            ResResultBody::Rows(BodyResResultRows {
                metadata: RowsMetadata {
                    flags,
                    columns_count,
                    paging_state: None,
                    new_metadata_id: None,
                    global_table_spec: None,
                    col_specs,
                },
                rows_count,
                rows_content,
                protocol_version: version,
            })
        }
    };
    Ok(body.serialize_to_vec(version))
}

/// Execute one authenticated V4 QUERY and return a correlated response.
pub async fn handle_query(
    executor: &dyn QueryExecutor,
    session: &mut SessionContext,
    request: Envelope,
) -> Envelope {
    let version = request.version;
    let decoded = if version == Version::V4
        && request.direction == Direction::Request
        && request.opcode == Opcode::Query
        && request.flags.is_empty()
    {
        decode(&request.body)
    } else {
        Err(QueryError::Invalid("invalid query envelope".into()))
    };
    let result = match decoded {
        Ok(decoded) => match executor
            .execute(session, &decoded.text, &decoded.values)
            .await
        {
            Ok(result) => {
                // Select a keyspace only after serialization succeeds.
                let new_keyspace = match &result {
                    QueryResult::SetKeyspace(name) => Some(name.clone()),
                    _ => None,
                };
                match encode_result(result, version, decoded.page_size) {
                    Ok(body) => {
                        if let Some(name) = new_keyspace {
                            session.keyspace = Some(name);
                        }
                        Ok(body)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    let (opcode, body) = match result {
        Ok(body) => (Opcode::Result, body),
        Err(error) => {
            let (ty, message) = match error {
                QueryError::Invalid(message) => (ErrorType::Invalid, message),
                QueryError::Unauthorized => (ErrorType::Unauthorized, "not authorized".into()),
                QueryError::Unsupported(message) => (ErrorType::Invalid, message),
                QueryError::Unavailable => (ErrorType::Server, "query backend unavailable".into()),
            };
            (
                Opcode::Error,
                ErrorBody { message, ty }.serialize_to_vec(version),
            )
        }
    };
    Envelope::new(
        version,
        Direction::Response,
        Flags::empty(),
        opcode,
        request.stream_id,
        body,
        None,
        vec![],
    )
}
