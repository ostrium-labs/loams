//! The ClickHouse native protocol (TCP, 9000 and 9440): a minimal subset, so
//! stock applications that speak it work unmodified (HE0, rulings R9.40
//! onward; D1575 amends D767/D768 and pulls this part of HS1 Tasks 30–31
//! forward).
//!
//! # One policy path
//!
//! Every statement runs through the HTTP interface's own path
//! ([`crate::http::run_native`]): the connection's credentials go in as
//! `X-ClickHouse-User`/`X-ClickHouse-Key`, the statement, its settings and
//! parameters as the query string's `query`, settings and `param_*`, and an
//! `INSERT`'s blocks as a `Native` body. So authentication, the settings
//! allowlist, the classifier, the deny list, refusal codes, admission, quotas,
//! the sandboxed workers, the query log and the audit are the HTTP interface's,
//! with nothing of their own here. What this module does is framing:
//!
//! * **Handshake.** `Hello` (the password checked at once, as clickhouse-server
//!   does, with [`crate::auth::identify`]), the server's `Hello` at the
//!   negotiated revision, the addendum.
//! * **Queries.** The result comes back as chDB's `Native` output; each block is
//!   sent as a `Data` packet (a header block first), then `ProfileInfo`,
//!   `Progress`, the empty block and `EndOfStream`, the oracle's order. A
//!   result with no rows still has a header: the statement is asked again for
//!   its columns (`RowBinaryWithNamesAndTypes`, R9.43).
//! * **INSERT with data from the client.** The table's columns (a `DESCRIBE`
//!   on the same path) make the header block; the client's blocks become the
//!   `INSERT … FORMAT Native` body as they arrive.
//! * **Errors** are `Exception` packets with the statement's code and message.
//! * `Ping`, `Cancel` (between statements, before the first result, R9.69, and
//!   during one) and `SET`/`USE`, which the connection remembers as
//!   clickhouse-server's session does.
//! * A top-level `DateTime` column keeps its time zone (R9.66).
//!
//! A query that asks for compression gets its `Data` blocks both ways in
//! ClickHouse's compressed frames (LZ4 written; LZ4, ZSTD or none read; R9.48).
//! An `INSERT`'s header follows `TableColumns` (R9.68). External tables,
//! `ProfileEvents` and `Totals`/`Extremes` as packets of their own are not
//! served (R9.42, R9.68).

mod head;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::Request;
use loams_house_native::packet::{self, ClientPacket};
use loams_house_native::{Block, Framing, NativeStream, revision, wire};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

use crate::errors::{ChError, HouseError};
use crate::http::{BodyPiece, ForcedFormat, HouseBody, RequestBody, Shared, StatementError};

/// How long a client may take over its `Hello`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long an idle connection stays open: clickhouse-server's
/// `idle_connection_timeout` (3 600 s).
const IDLE_TIMEOUT: Duration = Duration::from_secs(3600);
/// A partial packet is decoded again only once the buffer has grown by half,
/// or after this long without more bytes (so a large block arriving in small
/// pieces is walked a bounded number of times, and a packet that completed is
/// never left waiting).
const RETRY_AFTER: Duration = Duration::from_millis(20);
/// Bytes asked of the socket per read.
const READ_CHUNK: usize = 256 * 1024;
/// How many converted `INSERT` blocks may wait for the statement.
const INSERT_DEPTH: usize = 4;
/// `ClickHouse` is what clients check for.
const SERVER_NAME: &str = "ClickHouse";

/// Accepts native connections on `listener` (TLS when `tls` is given), within
/// the HTTP interface's connection cap.
pub(crate) fn spawn(
    listener: TcpListener,
    tls: Option<tokio_rustls::TlsAcceptor>,
    shared: Arc<Shared>,
    permits: Arc<Semaphore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                return;
            };
            let (stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let shared = Arc::clone(&shared);
            let tls = tls.clone();
            tokio::spawn(async move {
                match tls {
                    Some(acceptor) => {
                        let handshake = tokio::time::timeout(
                            shared.config.receive_timeout,
                            acceptor.accept(stream),
                        )
                        .await;
                        if let Ok(Ok(stream)) = handshake {
                            connection(stream, shared).await;
                        }
                    }
                    None => connection(stream, shared).await,
                }
                drop(permit);
            });
        }
    })
}

/// Serves one connection until the client leaves or the protocol breaks.
async fn connection<S>(stream: S, shared: Arc<Shared>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (read, write) = tokio::io::split(stream);
    let mut conn = Connection {
        shared,
        reader: PacketReader::new(read),
        writer: write,
        revision: 0,
        user: String::new(),
        password: String::new(),
        database: "default".to_string(),
        settings: Vec::new(),
        session: None,
        compressed: false,
    };
    let _ = conn.serve().await;
    let _ = conn.writer.shutdown().await;
}

/// Why a connection ends.
#[derive(Debug)]
enum Ended {
    /// The client went away, or a read or write failed.
    Gone,
    /// The protocol broke: an `Exception` went out (or could not), and the
    /// connection closes.
    Broken,
}

/// Reads whole packets from the socket.
struct PacketReader<R> {
    inner: R,
    buf: Vec<u8>,
    /// The buffer's length at the last incomplete decode; 0 after a whole one.
    tried_at: usize,
}

impl<R: AsyncRead + Unpin> PacketReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            tried_at: 0,
        }
    }

    /// The next packet `decode` makes of the bytes; `Ok(None)` when the client
    /// closed between packets. A client silent for `idle` is an error.
    async fn next<T>(
        &mut self,
        idle: Duration,
        decode: impl Fn(&[u8]) -> Result<(T, usize), loams_house_native::Error>,
    ) -> Result<Option<T>, loams_house_native::Error> {
        let mut force = false;
        let mut quiet = Duration::ZERO;
        loop {
            if !self.buf.is_empty()
                && (self.tried_at == 0 || force || self.buf.len() >= self.tried_at * 3 / 2)
            {
                force = false;
                match decode(&self.buf) {
                    Ok((value, used)) => {
                        self.buf.drain(..used);
                        self.tried_at = 0;
                        return Ok(Some(value));
                    }
                    Err(loams_house_native::Error::Incomplete) => self.tried_at = self.buf.len(),
                    Err(err) => return Err(err),
                }
            }
            let wait = if self.tried_at > 0 { RETRY_AFTER } else { idle };
            self.buf.reserve(READ_CHUNK);
            match tokio::time::timeout(wait, self.inner.read_buf(&mut self.buf)).await {
                Ok(Ok(0)) if self.buf.is_empty() => return Ok(None),
                Ok(Ok(0)) => {
                    return Err(loams_house_native::Error::Malformed(
                        "the connection closed inside a packet".into(),
                    ));
                }
                Ok(Ok(_)) => quiet = Duration::ZERO,
                Ok(Err(err)) => {
                    return Err(loams_house_native::Error::Malformed(format!(
                        "reading the connection: {err}"
                    )));
                }
                Err(_) if self.tried_at > 0 => {
                    quiet += wait;
                    if quiet >= idle {
                        return Err(loams_house_native::Error::Malformed(format!(
                            "the client sent nothing for {idle:?} inside a packet"
                        )));
                    }
                    force = true;
                }
                Err(_) => {
                    return Err(loams_house_native::Error::Malformed(format!(
                        "the client sent nothing for {idle:?}"
                    )));
                }
            }
        }
    }
}

/// One client connection: what clickhouse-server keeps in its TCP session.
struct Connection<S> {
    shared: Arc<Shared>,
    reader: PacketReader<ReadHalf<S>>,
    writer: WriteHalf<S>,
    /// The negotiated protocol revision.
    revision: u64,
    user: String,
    password: String,
    /// The current database: the `Hello`'s, then each `USE`'s.
    database: String,
    /// Each `SET`'s settings, sent with every later statement (and checked
    /// again each time, like the statement's own).
    settings: Vec<(String, String)>,
    /// The House session, from the connection's first temporary table on.
    session: Option<String>,
    /// Whether the running query's `Data` blocks are compressed (R9.48).
    compressed: bool,
}

/// What came of one statement.
struct Answer {
    response: http::Response<HouseBody>,
}

impl Answer {
    fn error(&self) -> Option<HouseError> {
        self.response
            .extensions()
            .get::<StatementError>()
            .map(|e| e.0.clone())
    }

    /// The statement's counters, from `X-ClickHouse-Summary`.
    fn progress(&self) -> packet::Progress {
        summary(
            self.response
                .headers()
                .get("X-ClickHouse-Summary")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(""),
        )
    }
}

impl<S> Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn serve(&mut self) -> Result<(), Ended> {
        let hello = match self
            .reader
            .next(HANDSHAKE_TIMEOUT, packet::decode_hello)
            .await
        {
            Ok(Some(hello)) => hello,
            Ok(None) => return Err(Ended::Gone),
            Err(err) => return self.broken(&err).await,
        };
        self.revision = revision::negotiate(hello.revision);
        self.user = if hello.user.is_empty() {
            "default".to_string()
        } else {
            hello.user.clone()
        };
        self.password = hello.password.clone();
        if !hello.database.is_empty() {
            self.database = hello.database.clone();
        }
        // The password is checked now, as clickhouse-server checks it, and again
        // with every statement on the shared path.
        if let Err(err) =
            crate::auth::identify(&self.shared.config, &self.credential_headers(), &[])
        {
            self.exception(&err).await?;
            return Err(Ended::Broken);
        }
        let mut out = Vec::new();
        packet::encode_hello(
            &mut out,
            &packet::ServerHello {
                name: SERVER_NAME.to_string(),
                version_major: version_part(&self.shared.config.version, 0),
                version_minor: version_part(&self.shared.config.version, 1),
                version_patch: version_part(&self.shared.config.version, 2),
                timezone: "UTC".to_string(),
                display_name: crate::config::DISPLAY_NAME.to_string(),
            },
            self.revision,
        );
        self.send(&out).await?;
        if self.revision >= revision::ADDENDUM {
            let revision = self.revision;
            match self
                .reader
                .next(HANDSHAKE_TIMEOUT, |buf| {
                    packet::decode_addendum(buf, revision)
                })
                .await
            {
                Ok(Some(_)) => {}
                Ok(None) => return Err(Ended::Gone),
                Err(err) => return self.broken(&err).await,
            }
        }
        loop {
            let revision = self.revision;
            let packet = match self
                .reader
                .next(IDLE_TIMEOUT, |buf| packet::decode_packet(buf, revision))
                .await
            {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.end_session().await;
                    return Ok(());
                }
                Err(err) => {
                    self.end_session().await;
                    return self.broken(&err).await;
                }
            };
            match packet {
                ClientPacket::Ping => {
                    let mut out = Vec::new();
                    packet::encode_pong(&mut out);
                    self.send(&out).await?;
                }
                // Nothing runs between statements.
                ClientPacket::Cancel => {}
                ClientPacket::Query(query) => {
                    if let Err(ended) = Box::pin(self.query(query)).await {
                        self.end_session().await;
                        return Err(ended);
                    }
                }
                ClientPacket::Data { .. } => {
                    let err = loams_house_native::Error::Malformed(
                        "a Data packet with no INSERT waiting for it".into(),
                    );
                    self.end_session().await;
                    return self.broken(&err).await;
                }
            }
        }
    }

    /// One `Query` packet and everything that answers it; its `Data` blocks,
    /// both ways, are compressed when it asks (R9.48).
    async fn query(&mut self, query: packet::Query) -> Result<(), Ended> {
        self.compressed = query.compression;
        let answered = self.answer_query(query).await;
        self.compressed = false;
        answered
    }

    async fn answer_query(&mut self, query: packet::Query) -> Result<(), Ended> {
        // External tables, then the empty block that ends them.
        let mut external = false;
        loop {
            let (revision, compressed) = (self.revision, self.compressed);
            match self
                .reader
                .next(self.shared.config.receive_timeout, |buf| {
                    packet::decode_packet_with(buf, revision, compressed)
                })
                .await
            {
                Ok(Some(ClientPacket::Data { table, block })) => {
                    if table.is_empty() && block.is_empty() {
                        break;
                    }
                    external = true;
                }
                Ok(Some(_)) => {
                    let err = loams_house_native::Error::Malformed(
                        "a Query not followed by its data blocks".into(),
                    );
                    return self.broken(&err).await;
                }
                Ok(None) => return Err(Ended::Gone),
                Err(err) => return self.broken(&err).await,
            }
        }
        if external {
            let err = HouseError::from(ChError::not_implemented(
                "external tables over the native protocol are not supported by the House yet",
            ));
            return self.exception(&err).await;
        }
        if query.stage != 2 {
            let err = HouseError::from(ChError::not_implemented(format!(
                "query processing stage {} (only Complete, 2, is served)",
                query.stage
            )));
            return self.exception(&err).await;
        }
        let query_id = if query.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            query.id.clone()
        };
        let mut params: Vec<(String, String)> = Vec::new();
        for (name, value) in &self.settings {
            if !query.settings.iter().any(|s| &s.name == name) {
                params.push((name.clone(), value.clone()));
            }
        }
        for setting in &query.settings {
            let value = if setting.flags & 0x02 != 0 {
                packet::parameter_text(&setting.value)
            } else {
                setting.value.clone()
            };
            params.push((setting.name.clone(), value));
        }
        for parameter in &query.parameters {
            params.push((
                format!("param_{}", parameter.name),
                packet::parameter_text(&parameter.value),
            ));
        }
        let parent = query
            .client_info
            .trace
            .and_then(|(trace, span, _flags)| span_context(trace, span));
        let plan = crate::request::plan(query.body.as_bytes(), true);
        match plan.input {
            Some(spec) if plan.first_data.iter().all(u8::is_ascii_whitespace) => {
                self.insert(&spec.insert, query_id, params, parent).await
            }
            _ => self.statement(&query.body, query_id, params, parent).await,
        }
    }

    /// A statement whose answer is a result (or nothing).
    async fn statement(
        &mut self,
        sql: &str,
        query_id: String,
        params: Vec<(String, String)>,
        parent: Option<crate::trace::SpanContext>,
    ) -> Result<(), Ended> {
        let kind = crate::classify::classify(sql).ok();
        let stmt = match &kind {
            Some(crate::classify::Classified::Known { stmt, .. }) => Some(stmt),
            _ => None,
        };
        if matches!(
            stmt,
            Some(crate::classify::Stmt::CreateTemporaryTable { .. })
        ) && self.session.is_none()
        {
            self.session = Some(format!("native-{}", uuid::Uuid::new_v4()));
        }
        // A `Cancel` before the first result block (the statement still running
        // in its worker) drops the statement, as one during the result does.
        let started = self.start(
            sql,
            &query_id,
            params.clone(),
            RequestBody::empty(),
            "Native",
            parent,
        );
        let answer = tokio::select! {
            answer = started => answer,
            cancel = self.cancelled() => {
                cancel?;
                return self.end_of_stream(&packet::Progress::default()).await;
            }
        };
        if let Some(err) = answer.error() {
            return self.exception(&err).await;
        }
        match stmt {
            Some(crate::classify::Stmt::Set(pairs)) => {
                for (name, value) in pairs {
                    self.settings.retain(|(n, _)| n != name);
                    self.settings.push((name.clone(), value.clone()));
                }
            }
            Some(crate::classify::Stmt::Use(database)) => self.database = database.clone(),
            _ => {}
        }
        let progress = answer.progress();
        let mut body = answer.response.into_body();
        let mut stream = NativeStream::new();
        let mut sent = Sent::default();
        loop {
            let piece = tokio::select! {
                piece = body.next_piece() => piece,
                cancel = self.cancelled() => {
                    cancel?;
                    // Dropping the body stops the statement (its worker is
                    // killed, as for an HTTP client that leaves).
                    drop(body);
                    return self.end_of_stream(&progress).await;
                }
            };
            match piece {
                Some(BodyPiece::Data(bytes)) => {
                    stream.push(&bytes);
                    loop {
                        match stream.next_block() {
                            Ok(Some(block)) => {
                                let block = self
                                    .with_zones(block, sql, &query_id, &params, &mut sent)
                                    .await;
                                self.result_block(block, &mut sent).await?
                            }
                            Ok(None) => break,
                            Err(err) => return self.result_broken(&err).await,
                        }
                    }
                }
                Some(BodyPiece::Failed(err)) => return self.exception(&err).await,
                Some(BodyPiece::Broken) => {
                    let err = HouseError::from(ChError::network_error(
                        "the result broke off before its end",
                    ));
                    return self.exception(&err).await;
                }
                None => break,
            }
        }
        match stream.finish() {
            Ok(blocks) => {
                for block in blocks {
                    let block = self
                        .with_zones(block, sql, &query_id, &params, &mut sent)
                        .await;
                    self.result_block(block, &mut sent).await?;
                }
            }
            Err(err) => return self.result_broken(&err).await,
        }
        if !sent.header && crate::request::is_read(sql) {
            // No rows: the columns come from asking again (R9.43).
            match self.header_of(sql, &query_id, params).await {
                Ok(header) => {
                    let mut out = Vec::new();
                    packet::encode_data_with(
                        &mut out,
                        packet::server::DATA,
                        &header,
                        self.revision,
                        self.compressed,
                    );
                    self.send(&out).await?;
                    sent.header = true;
                }
                Err(err) => return self.exception(&err).await,
            }
        }
        if sent.header {
            let mut out = Vec::new();
            packet::encode_profile_info(
                &mut out,
                &packet::ProfileInfo {
                    rows: sent.rows,
                    blocks: sent.blocks,
                    bytes: sent.bytes,
                    applied_limit: false,
                    rows_before_limit: 0,
                },
            );
            packet::encode_progress(&mut out, &progress, self.revision);
            packet::encode_data_with(
                &mut out,
                packet::server::DATA,
                &Block::empty(),
                self.revision,
                self.compressed,
            );
            self.send(&out).await?;
            return self.end_of_stream(&packet::Progress::default()).await;
        }
        self.end_of_stream(&progress).await
    }

    /// An `INSERT` whose data the client sends as blocks.
    async fn insert(
        &mut self,
        head: &str,
        query_id: String,
        params: Vec<(String, String)>,
        parent: Option<crate::trace::SpanContext>,
    ) -> Result<(), Ended> {
        let (header, table) = match self.insert_header(head, &query_id, params.clone()).await {
            Ok(described) => described,
            Err(err) => return self.exception(&err).await,
        };
        let mut out = Vec::new();
        // The table's columns and their defaults first, from 54410, as
        // clickhouse-server sends them (clickhouse-client fills omitted fields
        // from them; HE1 R9.68).
        if self.revision >= revision::COLUMN_DEFAULTS_METADATA {
            packet::encode_table_columns(&mut out, &packet::columns_description(&table));
        }
        packet::encode_data_with(
            &mut out,
            packet::server::DATA,
            &header,
            self.revision,
            self.compressed,
        );
        self.send(&out).await?;

        // The blocks stream into the statement as they arrive; the statement
        // runs on the shared path meanwhile.
        let (tx, rx) = mpsc::channel::<Result<Bytes, HouseError>>(INSERT_DEPTH);
        let body = RequestBody::channel(rx, self.shared.config.receive_timeout);
        let sql = format!("{head} FORMAT Native");
        let revision = self.revision;
        let receive_timeout = self.shared.config.receive_timeout;
        let shared = Arc::clone(&self.shared);
        let parts = self.parts("Native");
        let run_params = self.statement_params(&sql, &query_id, params);
        let compressed = self.compressed;
        let reader = &mut self.reader;
        let feed = async move {
            let mut tx = Some(tx);
            loop {
                let next = reader
                    .next(receive_timeout, |buf| {
                        packet::decode_packet_with(buf, revision, compressed)
                    })
                    .await;
                match next {
                    Ok(Some(ClientPacket::Data { block, .. })) if block.is_empty() => {
                        return Ok(());
                    }
                    Ok(Some(ClientPacket::Data { block, .. })) => {
                        let mut native = Vec::new();
                        block.write(&mut native, Framing::NATIVE_FORMAT);
                        if let Some(sender) = &tx
                            && sender.send(Ok(Bytes::from(native))).await.is_err()
                        {
                            // The statement ended early (its error goes out); the
                            // rest of the client's blocks are read and dropped.
                            tx = None;
                        }
                    }
                    Ok(Some(ClientPacket::Cancel)) => {
                        if let Some(sender) = tx.take() {
                            let _ = sender
                                .send(Err(HouseError::from(ChError::query_was_cancelled(
                                    "the client cancelled the INSERT",
                                ))))
                                .await;
                        }
                        return Ok(());
                    }
                    Ok(Some(_)) => {
                        let err = loams_house_native::Error::Malformed(
                            "an INSERT's data interrupted by another packet".into(),
                        );
                        if let Some(sender) = tx.take() {
                            let _ = sender
                                .send(Err(HouseError::from(ChError::network_error(
                                    err.to_string(),
                                ))))
                                .await;
                        }
                        return Err(err);
                    }
                    Ok(None) => {
                        return Err(loams_house_native::Error::Malformed(
                            "the client left inside an INSERT's data".into(),
                        ));
                    }
                    Err(err) => {
                        if let Some(sender) = tx.take() {
                            let code = match &err {
                                loams_house_native::Error::Unsupported(_) => {
                                    ChError::not_implemented(err.to_string())
                                }
                                _ => ChError::network_error(format!(
                                    "Cannot read the INSERT's data: {err}"
                                )),
                            };
                            let _ = sender.send(Err(HouseError::from(code))).await;
                        }
                        return Err(err);
                    }
                }
            }
        };
        // Boxed: the shared path's future is large, and this task's stack is a
        // runtime worker's.
        let run = Box::pin(async move {
            crate::http::run_native(&shared, parts, body, run_params, query_id, parent).await
        });
        let (fed, response) = tokio::join!(Box::pin(feed), run);
        let answer = Answer { response };
        if let Err(err) = fed {
            if let Some(statement) = answer.error() {
                let _ = self.exception(&statement).await;
            }
            return self.broken(&err).await;
        }
        if let Some(err) = answer.error() {
            return self.exception(&err).await;
        }
        let progress = answer.progress();
        self.end_of_stream(&progress).await
    }

    /// The header an `INSERT` answers with: the listed columns, or every column
    /// an `INSERT` without a list takes, with their types, from `DESCRIBE`; and
    /// every column of the table, for `TableColumns`.
    async fn insert_header(
        &mut self,
        head: &str,
        query_id: &str,
        params: Vec<(String, String)>,
    ) -> Result<(Block, Vec<packet::TableColumn>), HouseError> {
        let target = head::parse(head).ok_or_else(|| {
            HouseError::from(ChError::not_implemented(format!(
                "INSERT over the native protocol with data from the client needs \
                 `INSERT INTO <table> [(columns)]`; '{head}' is not served yet"
            )))
        })?;
        let describe = format!("DESCRIBE TABLE {} FORMAT JSONCompactEachRow", target.table);
        let answer = self
            .run(
                &describe,
                &format!("{query_id}-describe"),
                params,
                RequestBody::empty(),
                "JSONCompactEachRow",
                None,
            )
            .await;
        if let Some(err) = answer.error() {
            return Err(err);
        }
        let text = read_all(answer.response.into_body()).await?;
        let mut columns: Vec<(String, String, String)> = Vec::new();
        let mut table: Vec<packet::TableColumn> = Vec::new();
        for line in text.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let row: Vec<serde_json::Value> = serde_json::from_slice(line).map_err(|err| {
                HouseError::from(ChError::network_error(format!(
                    "the table's description did not read: {err}"
                )))
            })?;
            let field = |i: usize| {
                row.get(i)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            columns.push((field(0), field(1), field(2)));
            table.push(packet::TableColumn {
                name: field(0),
                type_name: field(1),
                default_kind: field(2),
                default_expression: field(3),
                comment: field(4),
                codec: field(5),
                ttl: field(6),
            });
        }
        let chosen: Vec<(String, String)> = match target.columns {
            Some(list) => list
                .into_iter()
                .map(|name| {
                    columns
                        .iter()
                        .find(|(n, _, _)| *n == name)
                        .map(|(n, t, _)| (n.clone(), t.clone()))
                        .ok_or_else(|| {
                            HouseError::from(ChError {
                                code: 16,
                                name: "NO_SUCH_COLUMN_IN_TABLE",
                                message: format!("No such column {name} in table {}", target.table),
                            })
                        })
                })
                .collect::<Result<_, _>>()?,
            None => columns
                .into_iter()
                .filter(|(_, _, kind)| matches!(kind.as_str(), "" | "DEFAULT" | "EPHEMERAL"))
                .map(|(n, t, _)| (n, t))
                .collect(),
        };
        Ok((Block::header(chosen), table))
    }

    /// The columns of a statement that answered no rows: the statement once more,
    /// as `RowBinaryWithNamesAndTypes`, whose header is there even with no rows.
    async fn header_of(
        &mut self,
        sql: &str,
        query_id: &str,
        params: Vec<(String, String)>,
    ) -> Result<Block, HouseError> {
        let answer = self
            .run(
                sql,
                &format!("{query_id}-header"),
                params,
                RequestBody::empty(),
                "RowBinaryWithNamesAndTypes",
                None,
            )
            .await;
        if let Some(err) = answer.error() {
            return Err(err);
        }
        let bytes = read_all(answer.response.into_body()).await?;
        let unreadable = |what: &str| {
            HouseError::from(ChError::network_error(format!(
                "the result's columns did not read: {what}"
            )))
        };
        let mut reader = wire::Reader::new(&bytes);
        let count = reader.varuint().map_err(|e| unreadable(&e.to_string()))?;
        let mut names = Vec::new();
        for _ in 0..count {
            names.push(reader.string().map_err(|e| unreadable(&e.to_string()))?);
        }
        let mut columns = Vec::new();
        for name in names {
            let ty = reader.string().map_err(|e| unreadable(&e.to_string()))?;
            columns.push((name, ty));
        }
        Ok(Block::header(columns))
    }

    /// `block` with each top-level `DateTime` column's time zone restored. chDB's
    /// `Native` output names a `DateTime('Asia/Kolkata')` column `DateTime` (as
    /// ClickHouse does for clients before revision 54337; nested ones and
    /// `DateTime64` keep theirs), so, once per result and only when a column is
    /// named `DateTime`, the statement is described (`DESCRIBE TABLE (…)`,
    /// which does not run it) and the described types are used (HE1 R9.66).
    /// A statement that does not describe keeps the names it has.
    async fn with_zones(
        &mut self,
        mut block: Block,
        sql: &str,
        query_id: &str,
        params: &[(String, String)],
        sent: &mut Sent,
    ) -> Block {
        if !sent.zones_checked {
            sent.zones_checked = true;
            if block.columns.iter().any(|c| c.type_name == "DateTime")
                && matches!(
                    crate::request::first_keyword(sql).as_str(),
                    "SELECT" | "WITH"
                )
            {
                sent.zones = self.described_types(sql, query_id, params).await;
            }
        }
        if let Some(types) = &sent.zones
            && types.len() == block.columns.len()
        {
            for (column, described) in block.columns.iter_mut().zip(types) {
                if column.type_name == "DateTime" && described.starts_with("DateTime(") {
                    column.type_name = described.clone();
                }
            }
        }
        block
    }

    /// The result's column types, from `DESCRIBE TABLE (<statement>)`.
    async fn described_types(
        &mut self,
        sql: &str,
        query_id: &str,
        params: &[(String, String)],
    ) -> Option<Vec<String>> {
        // clickhouse-client sends its output format (`FORMAT TSV`) and formats
        // the blocks itself; the subquery takes the statement without it.
        let (body, _) = crate::request::split_format(sql);
        let body = body.trim().trim_end_matches(';');
        let describe = format!("DESCRIBE TABLE ({body}) FORMAT JSONCompactEachRow");
        let answer = self
            .run(
                &describe,
                &format!("{query_id}-zones"),
                params.to_vec(),
                RequestBody::empty(),
                "JSONCompactEachRow",
                None,
            )
            .await;
        if answer.error().is_some() {
            return None;
        }
        let text = read_all(answer.response.into_body()).await.ok()?;
        let mut types = Vec::new();
        for line in text.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let row: Vec<serde_json::Value> = serde_json::from_slice(line).ok()?;
            types.push(row.get(1)?.as_str()?.to_string());
        }
        Some(types)
    }

    /// One block of a result: the header before the first, then the rows.
    async fn result_block(&mut self, block: Block, sent: &mut Sent) -> Result<(), Ended> {
        let block = if self.revision < revision::LOW_CARDINALITY_TYPE {
            match loams_house_native::block::without_low_cardinality(block) {
                Ok(block) => block,
                Err(err) => {
                    let err = HouseError::from(ChError::not_implemented(err.to_string()));
                    self.exception(&err).await?;
                    return Err(Ended::Broken);
                }
            }
        } else {
            block
        };
        let mut out = Vec::new();
        if !sent.header {
            packet::encode_data_with(
                &mut out,
                packet::server::DATA,
                &block.to_header(),
                self.revision,
                self.compressed,
            );
            sent.header = true;
        }
        if block.rows > 0 {
            sent.rows += block.rows;
            sent.blocks += 1;
            sent.bytes += block.data_bytes();
            packet::encode_data_with(
                &mut out,
                packet::server::DATA,
                &block,
                self.revision,
                self.compressed,
            );
        }
        self.send(&out).await
    }

    /// A result whose bytes do not frame (an unsupported column type, or a
    /// codec gap): `48` or the framing error, then the connection closes.
    async fn result_broken(&mut self, err: &loams_house_native::Error) -> Result<(), Ended> {
        let code = match err {
            loams_house_native::Error::Unsupported(_) => ChError::not_implemented(err.to_string()),
            _ => ChError::network_error(format!("the result did not frame as blocks: {err}")),
        };
        self.exception(&HouseError::from(code)).await?;
        Err(Ended::Broken)
    }

    /// Waits for the client's `Cancel` while a result streams; any other packet
    /// breaks the protocol, and the client leaving ends the connection.
    async fn cancelled(&mut self) -> Result<(), Ended> {
        let (revision, compressed) = (self.revision, self.compressed);
        match self
            .reader
            .next(IDLE_TIMEOUT, |buf| {
                packet::decode_packet_with(buf, revision, compressed)
            })
            .await
        {
            Ok(Some(ClientPacket::Cancel)) => Ok(()),
            Ok(Some(_)) | Err(_) => Err(Ended::Broken),
            Ok(None) => Err(Ended::Gone),
        }
    }

    /// Runs one statement on the shared path.
    async fn run(
        &self,
        sql: &str,
        query_id: &str,
        params: Vec<(String, String)>,
        body: RequestBody,
        format: &str,
        parent: Option<crate::trace::SpanContext>,
    ) -> Answer {
        self.start(sql, query_id, params, body, format, parent)
            .await
    }

    /// One statement on the shared path, as a future that holds nothing of the
    /// connection, so the connection can read a `Cancel` while it runs.
    /// Dropping it stops the statement (its worker is killed).
    fn start(
        &self,
        sql: &str,
        query_id: &str,
        params: Vec<(String, String)>,
        body: RequestBody,
        format: &str,
        parent: Option<crate::trace::SpanContext>,
    ) -> impl std::future::Future<Output = Answer> + Send + 'static {
        let params = self.statement_params(sql, query_id, params);
        let parts = self.parts(format);
        let shared = Arc::clone(&self.shared);
        let query_id = query_id.to_string();
        async move {
            let response = Box::pin(crate::http::run_native(
                &shared, parts, body, params, query_id, parent,
            ))
            .await;
            Answer { response }
        }
    }

    /// The query string HTTP would carry for this statement.
    fn statement_params(
        &self,
        sql: &str,
        query_id: &str,
        params: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        let mut all = vec![
            ("query".to_string(), sql.to_string()),
            ("query_id".to_string(), query_id.to_string()),
            ("database".to_string(), self.database.clone()),
        ];
        if let Some(session) = &self.session {
            all.push(("session_id".to_string(), session.clone()));
            all.push((
                "session_timeout".to_string(),
                crate::session::SESSION_TIMEOUT_MAX.to_string(),
            ));
        }
        all.extend(params);
        all
    }

    /// The request head: the connection's credentials, and the result's format.
    fn parts(&self, format: &str) -> http::request::Parts {
        let mut builder = Request::builder().method(http::Method::POST).uri("/");
        for (name, value) in self.credential_headers() {
            builder = builder.header(name, value);
        }
        let (mut parts, ()) = builder
            .body(())
            .unwrap_or_else(|_| Request::new(()))
            .into_parts();
        parts.extensions.insert(ForcedFormat(format.to_string()));
        parts
    }

    fn credential_headers(&self) -> Vec<(String, String)> {
        vec![
            ("X-ClickHouse-User".to_string(), self.user.clone()),
            ("X-ClickHouse-Key".to_string(), self.password.clone()),
        ]
    }

    /// Ends the connection's House session, if it made one.
    async fn end_session(&mut self) {
        if let Some(session) = self.session.take() {
            let params = vec![
                ("query".to_string(), "SELECT 1".to_string()),
                ("session_id".to_string(), session),
                ("close_session".to_string(), "1".to_string()),
            ];
            let parts = self.parts("Native");
            let _ = Box::pin(crate::http::run_native(
                &self.shared,
                parts,
                RequestBody::empty(),
                params,
                uuid::Uuid::new_v4().to_string(),
                None,
            ))
            .await;
        }
    }

    async fn end_of_stream(&mut self, progress: &packet::Progress) -> Result<(), Ended> {
        let mut out = Vec::new();
        packet::encode_progress(&mut out, progress, self.revision);
        packet::encode_end_of_stream(&mut out);
        self.send(&out).await
    }

    async fn exception(&mut self, err: &HouseError) -> Result<(), Ended> {
        let mut out = Vec::new();
        packet::encode_exception(&mut out, err.code(), err.message());
        self.send(&out).await
    }

    /// A protocol error: its `Exception`, then the connection closes.
    async fn broken(&mut self, err: &loams_house_native::Error) -> Result<(), Ended> {
        let mut out = Vec::new();
        let code = match err {
            loams_house_native::Error::Unsupported(_) => 48,
            // UNEXPECTED_PACKET_FROM_CLIENT
            _ => 101,
        };
        packet::encode_exception(&mut out, code, &err.to_string());
        let _ = self.send(&out).await;
        Err(Ended::Broken)
    }

    async fn send(&mut self, bytes: &[u8]) -> Result<(), Ended> {
        let write = async {
            self.writer.write_all(bytes).await?;
            self.writer.flush().await
        };
        match tokio::time::timeout(self.shared.config.send_timeout, write).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(Ended::Gone),
        }
    }
}

/// What a result has sent so far.
#[derive(Debug, Default)]
struct Sent {
    header: bool,
    /// Whether [`Connection::with_zones`] has looked at the first block.
    zones_checked: bool,
    /// The described column types, when the result had a bare `DateTime`.
    zones: Option<Vec<String>>,
    rows: u64,
    blocks: u64,
    bytes: u64,
}

/// A whole body (an internal statement's small answer).
async fn read_all(mut body: HouseBody) -> Result<Vec<u8>, HouseError> {
    let mut out = Vec::new();
    while let Some(piece) = body.next_piece().await {
        match piece {
            BodyPiece::Data(bytes) => out.extend_from_slice(&bytes),
            BodyPiece::Failed(err) => return Err(err),
            BodyPiece::Broken => {
                return Err(HouseError::from(ChError::network_error(
                    "an answer broke off before its end",
                )));
            }
        }
    }
    Ok(out)
}

/// `X-ClickHouse-Summary`'s counters as a `Progress`.
fn summary(json: &str) -> packet::Progress {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let number = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    packet::Progress {
        rows: number("read_rows"),
        bytes: number("read_bytes"),
        total_rows: number("total_rows_to_read"),
        written_rows: number("written_rows"),
        written_bytes: number("written_bytes"),
        elapsed_ns: number("elapsed_ns"),
    }
}

/// The client's OpenTelemetry parent. clickhouse-go and clickhouse-client write
/// the trace id's two 8-byte halves byte-swapped (ClickHouse issue 34369), and
/// the span id swapped too; this undoes it.
fn span_context(mut trace: [u8; 16], mut span: [u8; 8]) -> Option<crate::trace::SpanContext> {
    trace[..8].reverse();
    trace[8..].reverse();
    span.reverse();
    (trace != [0; 16] && span != [0; 8]).then_some(crate::trace::SpanContext {
        trace_id: trace,
        span_id: span,
    })
}

/// One part of a `major.minor.patch.build` version.
fn version_part(version: &str, index: usize) -> u64 {
    version
        .split('.')
        .nth(index)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reads_the_http_header() {
        let p = summary(&crate::http::progress_json(&loams_house_ipc::Progress {
            rows_read: 5,
            bytes_read: 40,
            written_rows: 2,
            elapsed_ns: 9,
            ..loams_house_ipc::Progress::default()
        }));
        assert_eq!(
            (p.rows, p.bytes, p.written_rows, p.elapsed_ns),
            (5, 40, 2, 9)
        );
        assert_eq!(summary("not json"), packet::Progress::default());
    }

    #[test]
    fn version_parts() {
        assert_eq!(version_part("26.9.2.1", 0), 26);
        assert_eq!(version_part("26.9.2.1", 2), 2);
        assert_eq!(version_part("x", 1), 0);
    }
}
