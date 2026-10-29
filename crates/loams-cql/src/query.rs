//! Independent query-execution seam for the CQL protocol listener.

use async_trait::async_trait;

/// Authenticated identity and selected Cassandra keyspace for a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionContext {
    /// Loams namespace to which this connection is confined.
    pub namespace: String,
    /// Authenticated principal within the Loams namespace.
    pub principal: String,
    /// Current Cassandra keyspace, if one has been selected.
    pub keyspace: Option<String>,
}

/// Supported scalar Cassandra column types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    /// ASCII string.
    Ascii,
    /// Boolean value.
    Boolean,
    /// Signed 8-bit integer.
    TinyInt,
    /// Signed 16-bit integer.
    SmallInt,
    /// Signed 32-bit integer.
    Int,
    /// Signed 64-bit integer.
    BigInt,
    /// IEEE 754 single precision.
    Float,
    /// IEEE 754 double precision.
    Double,
    /// UTF-8 string.
    Text,
    /// Cassandra varchar (UTF-8 string).
    Varchar,
    /// Arbitrary bytes.
    Blob,
    /// UUID value.
    Uuid,
    /// Version-one UUID with Cassandra time ordering.
    TimeUuid,
    /// Milliseconds since the Unix epoch.
    Timestamp,
    /// IPv4 or IPv6 address.
    Inet,
}

/// One result column's fully qualified Cassandra metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSpec {
    /// Cassandra keyspace name.
    pub keyspace: String,
    /// Cassandra table name.
    pub table: String,
    /// Column name.
    pub name: String,
    /// Cassandra scalar type.
    pub data_type: ColumnType,
}

/// Query outcome, independent of protocol framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryResult {
    /// A statement with no rows.
    Void,
    /// A page of rows. Values are encoded in Cassandra's binary format.
    Rows {
        /// Result column metadata.
        columns: Vec<ColumnSpec>,
        /// Rows; `None` represents CQL null.
        rows: Vec<Vec<Option<Vec<u8>>>>,
        /// An opaque continuation token when more rows remain.
        paging_state: Option<Vec<u8>>,
    },
    /// The connection must select the named keyspace after this statement.
    SetKeyspace(String),
}

/// Errors returned by a query executor without exposing backend internals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// Invalid syntax or query parameters.
    Invalid(String),
    /// The authenticated principal lacks permission.
    Unauthorized,
    /// The requested CQL feature is not implemented.
    Unsupported(String),
    /// The backend cannot currently serve the request.
    Unavailable,
}

/// Executes a CQL statement on behalf of the authenticated connection.
#[async_trait]
pub trait QueryExecutor: Send + Sync {
    /// Execute the statement and its positional bound values.
    async fn execute(
        &self,
        session: &SessionContext,
        query: &str,
        values: &[Option<Vec<u8>>],
    ) -> Result<QueryResult, QueryError>;
}
