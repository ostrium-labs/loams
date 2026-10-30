use async_trait::async_trait;
use cassandra_protocol::frame::{
    Direction, Envelope, Flags, Opcode, Version, message_response::ResponseBody,
    message_result::ResResultBody,
};
use loams_cql::{
    query::{ColumnSpec, ColumnType, QueryError, QueryExecutor, QueryResult, SessionContext},
    query_protocol::handle_query,
};

struct Executor;

#[async_trait]
impl QueryExecutor for Executor {
    async fn execute(
        &self,
        session: &SessionContext,
        query: &str,
        values: &[Option<Vec<u8>>],
    ) -> Result<QueryResult, QueryError> {
        assert_eq!(session.namespace, "tenant");
        match query {
            "USE ks" => Ok(QueryResult::SetKeyspace("ks".into())),
            "SELECT value FROM ks.tbl WHERE id=?" => {
                assert_eq!(values, &[Some(42_i32.to_be_bytes().to_vec())]);
                Ok(QueryResult::Rows {
                    columns: vec![ColumnSpec {
                        keyspace: "ks".into(),
                        table: "tbl".into(),
                        name: "value".into(),
                        data_type: ColumnType::Int,
                    }],
                    rows: vec![vec![Some(7_i32.to_be_bytes().to_vec())]],
                    paging_state: None,
                })
            }
            _ => Err(QueryError::Invalid("bad query".into())),
        }
    }
}

fn session() -> SessionContext {
    SessionContext {
        namespace: "tenant".into(),
        principal: "user".into(),
        keyspace: None,
    }
}

fn query(text: &str, values: &[Option<Vec<u8>>]) -> Envelope {
    let mut body = Vec::new();
    body.extend_from_slice(&(text.len() as i32).to_be_bytes());
    body.extend_from_slice(text.as_bytes());
    body.extend_from_slice(&1_u16.to_be_bytes()); // consistency ONE
    body.push(if values.is_empty() { 0 } else { 1 });
    if !values.is_empty() {
        body.extend_from_slice(&(values.len() as u16).to_be_bytes());
        for v in values {
            match v {
                Some(v) => {
                    body.extend_from_slice(&(v.len() as i32).to_be_bytes());
                    body.extend_from_slice(v);
                }
                None => body.extend_from_slice(&(-1_i32).to_be_bytes()),
            }
        }
    }
    Envelope::new(
        Version::V4,
        Direction::Request,
        Flags::empty(),
        Opcode::Query,
        9,
        body,
        None,
        vec![],
    )
}

#[tokio::test]
async fn query_values_and_rows_round_trip() {
    let mut context = session();
    let response = handle_query(
        &Executor,
        &mut context,
        query(
            "SELECT value FROM ks.tbl WHERE id=?",
            &[Some(42_i32.to_be_bytes().to_vec())],
        ),
    )
    .await;
    assert_eq!(response.stream_id, 9);
    let ResponseBody::Result(ResResultBody::Rows(rows)) = response.response_body().unwrap() else {
        panic!("expected rows")
    };
    assert_eq!(rows.rows_count, 1);
    assert_eq!(
        rows.rows_content[0][0].as_slice(),
        Some(7_i32.to_be_bytes().as_slice())
    );
}

#[tokio::test]
async fn use_updates_only_the_current_connection() {
    let mut context = session();
    let response = handle_query(&Executor, &mut context, query("USE ks", &[])).await;
    assert_eq!(context.keyspace.as_deref(), Some("ks"));
    assert!(matches!(
        response.response_body().unwrap(),
        ResponseBody::Result(ResResultBody::SetKeyspace(_))
    ));
    assert!(session().keyspace.is_none());
}

#[tokio::test]
async fn malformed_bound_length_does_not_reach_executor() {
    let mut context = session();
    let mut request = query("SELECT value FROM ks.tbl WHERE id=?", &[Some(vec![1])]);
    let n = request.body.len();
    request.body[n - 5..n - 1].copy_from_slice(&i32::MAX.to_be_bytes());
    let response = handle_query(&Executor, &mut context, request).await;
    assert!(matches!(
        response.response_body().unwrap(),
        ResponseBody::Error(_)
    ));
}
