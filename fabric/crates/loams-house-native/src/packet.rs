//! Client packets in, server packets out.
//!
//! Every field a client sends is read at the **negotiated** revision (the lower
//! of the client's and [`revision::SERVER`]); the server writes its own
//! revision in `Hello` and every optional field by the negotiated one, as
//! clickhouse-server does (a v1 client at 54213 gets `Hello` with a time zone and
//! nothing after it).

use crate::block::{Block, Framing};
use crate::wire::{Reader, put_bytes, put_i32, put_str, put_varuint};
use crate::{Error, revision};

/// Client packet types.
pub mod client {
    /// `Hello`.
    pub const HELLO: u64 = 0;
    /// `Query`.
    pub const QUERY: u64 = 1;
    /// `Data`.
    pub const DATA: u64 = 2;
    /// `Cancel`.
    pub const CANCEL: u64 = 3;
    /// `Ping`.
    pub const PING: u64 = 4;
}

/// Server packet types.
pub mod server {
    /// `Hello`.
    pub const HELLO: u64 = 0;
    /// `Data`.
    pub const DATA: u64 = 1;
    /// `Exception`.
    pub const EXCEPTION: u64 = 2;
    /// `Progress`.
    pub const PROGRESS: u64 = 3;
    /// `Pong`.
    pub const PONG: u64 = 4;
    /// `EndOfStream`.
    pub const END_OF_STREAM: u64 = 5;
    /// `ProfileInfo`.
    pub const PROFILE_INFO: u64 = 6;
    /// `Totals`.
    pub const TOTALS: u64 = 7;
    /// `Extremes`.
    pub const EXTREMES: u64 = 8;
    /// `TableColumns`.
    pub const TABLE_COLUMNS: u64 = 11;
}

/// The client's `Hello`.
#[derive(Clone, PartialEq, Eq)]
pub struct Hello {
    /// `client_name`.
    pub client_name: String,
    /// The client's version.
    pub version_major: u64,
    /// The client's version.
    pub version_minor: u64,
    /// The highest revision the client speaks.
    pub revision: u64,
    /// The default database; empty for the server's.
    pub database: String,
    /// The user; empty for `default`.
    pub user: String,
    /// The password, in the clear (TLS is what protects it).
    pub password: String,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("client_name", &self.client_name)
            .field("revision", &self.revision)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// What a client sends after the server's `Hello` from revision 54458.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addendum {
    /// `quota_key`.
    pub quota_key: String,
}

/// One setting or query parameter of a `Query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// Its name.
    pub name: String,
    /// Its flags (`0x01` important, `0x02` custom, `0x0c` tier).
    pub flags: u64,
    /// Its value: the setting's text, or for a custom setting or a parameter a
    /// field dump (`'text'` for a string).
    pub value: String,
}

/// The `ClientInfo` a `Query` carries (the fields the House reads).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientInfo {
    /// 0 none, 1 initial, 2 secondary (distributed).
    pub query_kind: u8,
    /// 1 TCP, 2 HTTP.
    pub interface: u8,
    /// The client's name, from its `Hello` as it repeats it here.
    pub client_name: String,
    /// The client's OS user.
    pub os_user: String,
    /// The client's host name.
    pub client_hostname: String,
    /// `initial_query_id`.
    pub initial_query_id: String,
    /// An OpenTelemetry parent: trace id, span id, flags.
    pub trace: Option<([u8; 16], [u8; 8], u8)>,
}

/// A `Query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// `query_id`; empty for the server's.
    pub id: String,
    /// `client_info`.
    pub client_info: ClientInfo,
    /// The query's settings.
    pub settings: Vec<Setting>,
    /// `stage`: 2 (`Complete`) for every client but another server.
    pub stage: u64,
    /// Whether the client compresses its data blocks and wants ours compressed.
    pub compression: bool,
    /// The statement.
    pub body: String,
    /// Query parameters (`{name:Type}`), from revision 54459.
    pub parameters: Vec<Setting>,
}

/// A client packet after the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPacket {
    /// A statement.
    Query(Query),
    /// A data block: an external table, or an `INSERT`'s data.
    Data {
        /// The external table's name; empty for the main stream.
        table: String,
        /// The block.
        block: Block,
    },
    /// Stop the running statement.
    Cancel,
    /// `Ping`.
    Ping,
}

/// Decodes a `Hello`, the first packet. Answers it and the bytes it used.
pub fn decode_hello(buf: &[u8]) -> Result<(Hello, usize), Error> {
    let mut r = Reader::new(buf);
    let kind = r.varuint()?;
    if kind != client::HELLO {
        return Err(Error::Malformed(format!(
            "the first packet is of type {kind}, not Hello"
        )));
    }
    let hello = Hello {
        client_name: r.string()?,
        version_major: r.varuint()?,
        version_minor: r.varuint()?,
        revision: r.varuint()?,
        database: r.string()?,
        user: r.string()?,
        password: r.string()?,
    };
    Ok((hello, r.position()))
}

/// Decodes the addendum (revision 54458 and above).
pub fn decode_addendum(buf: &[u8], revision: u64) -> Result<(Addendum, usize), Error> {
    let mut r = Reader::new(buf);
    let quota_key = r.string()?;
    if revision >= revision::CHUNKED_PACKETS {
        return Err(Error::Unsupported(format!(
            "revision {revision} is above the {} the House serves",
            revision::SERVER
        )));
    }
    Ok((Addendum { quota_key }, r.position()))
}

/// Decodes one client packet after the handshake, at `revision`.
pub fn decode_packet(buf: &[u8], revision: u64) -> Result<(ClientPacket, usize), Error> {
    decode_packet_with(buf, revision, false)
}

/// [`decode_packet`], with `Data` blocks in compressed frames when the running
/// query asked for compression (R9.48).
pub fn decode_packet_with(
    buf: &[u8],
    revision: u64,
    compressed: bool,
) -> Result<(ClientPacket, usize), Error> {
    let mut r = Reader::new(buf);
    let packet = match r.varuint()? {
        client::QUERY => ClientPacket::Query(read_query(&mut r, revision)?),
        client::DATA => {
            let table = r.string()?;
            let block = read_block(&mut r, revision, compressed)?;
            ClientPacket::Data { table, block }
        }
        client::CANCEL => ClientPacket::Cancel,
        client::PING => ClientPacket::Ping,
        client::HELLO => {
            return Err(Error::Malformed("a second Hello".into()));
        }
        other => return Err(Error::Malformed(format!("client packet type {other}"))),
    };
    Ok((packet, r.position()))
}

/// A block, from compressed frames when `compressed`: frames are read until
/// their bytes hold exactly one block (clients flush a frame at each block's
/// end).
fn read_block(r: &mut Reader<'_>, revision: u64, compressed: bool) -> Result<Block, Error> {
    if !compressed {
        return Block::read(r, Framing::protocol(revision));
    }
    let mut raw = Vec::new();
    loop {
        crate::compress::read_frame(r, &mut raw)?;
        let mut inner = Reader::new(&raw);
        match Block::read(&mut inner, Framing::protocol(revision)) {
            Ok(block) if inner.position() == raw.len() => return Ok(block),
            Ok(_) => {
                return Err(Error::Malformed(
                    "a compressed frame that ends inside the next block".into(),
                ));
            }
            Err(Error::Incomplete) => {}
            Err(err) => return Err(err),
        }
    }
}

fn read_query(r: &mut Reader<'_>, revision: u64) -> Result<Query, Error> {
    let id = r.string()?;
    let mut info = ClientInfo::default();
    if revision >= revision::CLIENT_INFO {
        info.query_kind = r.u8()?;
        if info.query_kind != 0 {
            let _initial_user = r.string()?;
            info.initial_query_id = r.string()?;
            let _initial_address = r.string()?;
            if revision >= revision::INITIAL_QUERY_START_TIME {
                r.take(8)?;
            }
            info.interface = r.u8()?;
            match info.interface {
                1 => {
                    info.os_user = r.string()?;
                    info.client_hostname = r.string()?;
                    info.client_name = r.string()?;
                    r.varuint()?;
                    r.varuint()?;
                    r.varuint()?;
                }
                2 => {
                    r.u8()?;
                    r.bytes()?;
                    if revision >= revision::X_FORWARDED_FOR_IN_CLIENT_INFO {
                        r.bytes()?;
                    }
                    if revision >= revision::REFERER_IN_CLIENT_INFO {
                        r.bytes()?;
                    }
                }
                other => {
                    return Err(Error::Malformed(format!("client interface {other}")));
                }
            }
            if revision >= revision::QUOTA_KEY_IN_CLIENT_INFO {
                r.bytes()?;
            }
            if revision >= revision::DISTRIBUTED_DEPTH {
                r.varuint()?;
            }
            if info.interface == 1 && revision >= revision::VERSION_PATCH {
                r.varuint()?;
            }
            if revision >= revision::OPENTELEMETRY && r.u8()? != 0 {
                let mut trace = [0u8; 16];
                trace.copy_from_slice(r.take(16)?);
                let mut span = [0u8; 8];
                span.copy_from_slice(r.take(8)?);
                r.bytes()?;
                let flags = r.u8()?;
                info.trace = Some((trace, span, flags));
            }
            if revision >= revision::PARALLEL_REPLICAS {
                r.varuint()?;
                r.varuint()?;
                r.varuint()?;
            }
        }
    }
    let settings = read_settings(r, revision)?;
    if revision >= revision::INTERSERVER_SECRET {
        let hash = r.bytes()?;
        if !hash.is_empty() {
            return Err(Error::Unsupported(
                "inter-server queries (a cluster secret) are not served by the House".into(),
            ));
        }
    }
    let stage = r.varuint()?;
    let compression = r.varuint()? != 0;
    let body = r.string()?;
    let parameters = if revision >= revision::PARAMETERS {
        read_settings(r, revision)?
    } else {
        Vec::new()
    };
    Ok(Query {
        id,
        client_info: info,
        settings,
        stage,
        compression,
        body,
        parameters,
    })
}

fn read_settings(r: &mut Reader<'_>, revision: u64) -> Result<Vec<Setting>, Error> {
    let mut out = Vec::new();
    loop {
        let name = r.string()?;
        if name.is_empty() {
            return Ok(out);
        }
        if revision < revision::SETTINGS_SERIALIZED_AS_STRINGS {
            // Below 54429 a value is written in its setting's own binary form, which
            // only a table of every setting's type could read.
            return Err(Error::Unsupported(format!(
                "setting {name} in the binary form of protocol revision {revision}; send settings \
                 from a client of revision 54429 or later, or in the statement's SETTINGS clause"
            )));
        }
        let flags = r.varuint()?;
        let value = r.string()?;
        out.push(Setting { name, flags, value });
    }
}

/// What the server says about itself in `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHello {
    /// `ClickHouse`, so clients that check the name accept it.
    pub name: String,
    /// The ClickHouse version served.
    pub version_major: u64,
    /// The ClickHouse version served.
    pub version_minor: u64,
    /// The ClickHouse version served.
    pub version_patch: u64,
    /// The server's time zone.
    pub timezone: String,
    /// `display_name`.
    pub display_name: String,
}

/// Appends the server's `Hello` for a client at `negotiated`.
pub fn encode_hello(out: &mut Vec<u8>, hello: &ServerHello, negotiated: u64) {
    put_varuint(out, server::HELLO);
    put_str(out, &hello.name);
    put_varuint(out, hello.version_major);
    put_varuint(out, hello.version_minor);
    put_varuint(out, revision::SERVER);
    if negotiated >= revision::SERVER_TIMEZONE {
        put_str(out, &hello.timezone);
    }
    if negotiated >= revision::SERVER_DISPLAY_NAME {
        put_str(out, &hello.display_name);
    }
    if negotiated >= revision::VERSION_PATCH {
        put_varuint(out, hello.version_patch);
    }
}

/// Appends a `Data` packet (or `Totals`, `Extremes` by `kind`) of `block`.
pub fn encode_data(out: &mut Vec<u8>, kind: u64, block: &Block, revision: u64) {
    encode_data_with(out, kind, block, revision, false);
}

/// [`encode_data`], the block in LZ4 frames when `compressed` (R9.48).
pub fn encode_data_with(
    out: &mut Vec<u8>,
    kind: u64,
    block: &Block,
    revision: u64,
    compressed: bool,
) {
    put_varuint(out, kind);
    put_str(out, "");
    if compressed {
        let mut raw = Vec::new();
        block.write(&mut raw, Framing::protocol(revision));
        crate::compress::write_frames(out, &raw);
    } else {
        block.write(out, Framing::protocol(revision));
    }
}

/// Appends an `Exception`: `code`, `DB::Exception` and the message as
/// clickhouse-server words it (`DB::Exception: <text>`), no stack trace.
pub fn encode_exception(out: &mut Vec<u8>, code: i32, message: &str) {
    put_varuint(out, server::EXCEPTION);
    put_i32(out, code);
    put_str(out, "DB::Exception");
    put_str(out, &format!("DB::Exception: {message}"));
    put_str(out, "");
    out.push(0);
}

/// A `Progress` packet's counters: increments since the last one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Rows read.
    pub rows: u64,
    /// Bytes read.
    pub bytes: u64,
    /// Rows to read in all, when known.
    pub total_rows: u64,
    /// Rows written.
    pub written_rows: u64,
    /// Bytes written.
    pub written_bytes: u64,
    /// Time so far, in nanoseconds.
    pub elapsed_ns: u64,
}

/// Appends a `Progress`.
pub fn encode_progress(out: &mut Vec<u8>, progress: &Progress, revision: u64) {
    put_varuint(out, server::PROGRESS);
    put_varuint(out, progress.rows);
    put_varuint(out, progress.bytes);
    put_varuint(out, progress.total_rows);
    if revision >= revision::CLIENT_WRITE_INFO {
        put_varuint(out, progress.written_rows);
        put_varuint(out, progress.written_bytes);
    }
    if revision >= revision::SERVER_QUERY_TIME_IN_PROGRESS {
        put_varuint(out, progress.elapsed_ns);
    }
}

/// `ProfileInfo`'s counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProfileInfo {
    /// Rows sent.
    pub rows: u64,
    /// Blocks sent.
    pub blocks: u64,
    /// Bytes sent.
    pub bytes: u64,
    /// Whether a `LIMIT` applied.
    pub applied_limit: bool,
    /// Rows before the `LIMIT`.
    pub rows_before_limit: u64,
}

/// Appends a `ProfileInfo` (the obsolete `calculated_rows_before_limit` byte is
/// written true, as clickhouse-server does).
pub fn encode_profile_info(out: &mut Vec<u8>, info: &ProfileInfo) {
    put_varuint(out, server::PROFILE_INFO);
    put_varuint(out, info.rows);
    put_varuint(out, info.blocks);
    put_varuint(out, info.bytes);
    out.push(u8::from(info.applied_limit));
    put_varuint(out, info.rows_before_limit);
    out.push(1);
}

/// One column of a table as `TableColumns` describes it (`DESCRIBE TABLE`'s
/// fields; empty ones are left out).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableColumn {
    /// The name.
    pub name: String,
    /// The type.
    pub type_name: String,
    /// `DEFAULT`, `MATERIALIZED`, `ALIAS`, `EPHEMERAL` or empty.
    pub default_kind: String,
    /// The default's expression.
    pub default_expression: String,
    /// The comment.
    pub comment: String,
    /// The codec's expression, without `CODEC(…)`.
    pub codec: String,
    /// The TTL's expression.
    pub ttl: String,
}

/// clickhouse-server's `ColumnsDescription::toString`: the text a `TableColumns`
/// packet carries, which clickhouse-client parses for the defaults of omitted
/// fields (`input_format_defaults_for_omitted_fields`).
pub fn columns_description(columns: &[TableColumn]) -> String {
    let mut out = format!("columns format version: 1\n{} columns:\n", columns.len());
    for c in columns {
        out.push('`');
        for ch in c.name.chars() {
            match ch {
                '`' => out.push_str("\\`"),
                '\\' => out.push_str("\\\\"),
                _ => out.push(ch),
            }
        }
        out.push_str("` ");
        escape(&mut out, &c.type_name);
        if !c.default_kind.is_empty() {
            out.push('\t');
            out.push_str(&c.default_kind);
            if !c.default_expression.is_empty() {
                out.push('\t');
                escape(&mut out, &c.default_expression);
            }
        }
        if !c.comment.is_empty() {
            out.push_str("\tCOMMENT ");
            let mut quoted = String::from("'");
            for ch in c.comment.chars() {
                match ch {
                    '\'' => quoted.push_str("\\'"),
                    '\\' => quoted.push_str("\\\\"),
                    _ => quoted.push(ch),
                }
            }
            quoted.push('\'');
            escape(&mut out, &quoted);
        }
        if !c.codec.is_empty() {
            out.push_str("\tCODEC(");
            escape(&mut out, &c.codec);
            out.push(')');
        }
        if !c.ttl.is_empty() {
            out.push_str("\tTTL ");
            escape(&mut out, &c.ttl);
        }
        out.push('\n');
    }
    out
}

/// ClickHouse's `writeEscapedString`: `\t`, `\n` and `\\` escaped, so a
/// column stays one line.
fn escape(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
}

/// Appends a `TableColumns` (sent before an `INSERT`'s header from revision
/// 54410, as clickhouse-server does): no external table name, then the
/// description.
pub fn encode_table_columns(out: &mut Vec<u8>, description: &str) {
    put_varuint(out, server::TABLE_COLUMNS);
    put_str(out, "");
    put_str(out, description);
}

/// Appends a `Pong`.
pub fn encode_pong(out: &mut Vec<u8>) {
    put_varuint(out, server::PONG);
}

/// Appends an `EndOfStream`.
pub fn encode_end_of_stream(out: &mut Vec<u8>) {
    put_varuint(out, server::END_OF_STREAM);
}

/// A query parameter's value as the House's HTTP interface takes it
/// (`param_<name>=<text>`): a string field dump (`'it\'s'`) unquoted and
/// unescaped, any other dump as written.
pub fn parameter_text(dump: &str) -> String {
    let Some(inner) = dump
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return dump.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Appends a string field as clickhouse-go and clickhouse-client write a query
/// parameter (for tests and tools).
pub fn put_parameter(out: &mut Vec<u8>, name: &str, text: &str) {
    put_str(out, name);
    put_varuint(out, 2);
    let mut dump = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => dump.push_str("\\\\"),
            '\'' => dump.push_str("\\'"),
            '\n' => dump.push_str("\\n"),
            other => dump.push(other),
        }
    }
    dump.push('\'');
    put_bytes(out, dump.as_bytes());
}

/// The client's side, for tests and tools: a `Hello`.
pub fn encode_client_hello(out: &mut Vec<u8>, hello: &Hello) {
    put_varuint(out, client::HELLO);
    put_str(out, &hello.client_name);
    put_varuint(out, hello.version_major);
    put_varuint(out, hello.version_minor);
    put_varuint(out, hello.revision);
    put_str(out, &hello.database);
    put_str(out, &hello.user);
    put_str(out, &hello.password);
}

/// The client's side: a `Query` at `revision`, as clickhouse-go writes one
/// (an initial TCP query; settings as strings, so from revision 54429), and
/// the empty block that ends its external tables.
pub fn encode_query(out: &mut Vec<u8>, query: &Query, revision: u64) {
    put_varuint(out, client::QUERY);
    put_str(out, &query.id);
    if revision >= revision::CLIENT_INFO {
        out.push(1);
        put_str(out, "");
        put_str(out, &query.client_info.initial_query_id);
        put_str(out, "127.0.0.1:0");
        if revision >= revision::INITIAL_QUERY_START_TIME {
            out.extend_from_slice(&[0; 8]);
        }
        out.push(1);
        put_str(out, &query.client_info.os_user);
        put_str(out, &query.client_info.client_hostname);
        put_str(out, &query.client_info.client_name);
        put_varuint(out, 1);
        put_varuint(out, 0);
        put_varuint(out, revision);
        if revision >= revision::QUOTA_KEY_IN_CLIENT_INFO {
            put_str(out, "");
        }
        if revision >= revision::DISTRIBUTED_DEPTH {
            put_varuint(out, 0);
        }
        if revision >= revision::VERSION_PATCH {
            put_varuint(out, 0);
        }
        if revision >= revision::OPENTELEMETRY {
            out.push(0);
        }
        if revision >= revision::PARALLEL_REPLICAS {
            put_varuint(out, 0);
            put_varuint(out, 0);
            put_varuint(out, 0);
        }
    }
    for setting in &query.settings {
        put_str(out, &setting.name);
        put_varuint(out, setting.flags);
        put_str(out, &setting.value);
    }
    put_str(out, "");
    if revision >= revision::INTERSERVER_SECRET {
        put_str(out, "");
    }
    put_varuint(out, query.stage);
    put_varuint(out, u64::from(query.compression));
    put_str(out, &query.body);
    if revision >= revision::PARAMETERS {
        for parameter in &query.parameters {
            put_str(out, &parameter.name);
            put_varuint(out, parameter.flags);
            put_str(out, &parameter.value);
        }
        put_str(out, "");
    }
    encode_client_data_with(out, &Block::empty(), revision, query.compression);
}

/// The client's side: a `Data` packet of the main stream.
pub fn encode_client_data(out: &mut Vec<u8>, block: &Block, revision: u64) {
    encode_client_data_with(out, block, revision, false);
}

/// [`encode_client_data`], the block in LZ4 frames when `compressed`.
pub fn encode_client_data_with(out: &mut Vec<u8>, block: &Block, revision: u64, compressed: bool) {
    put_varuint(out, client::DATA);
    put_str(out, "");
    if compressed {
        let mut raw = Vec::new();
        block.write(&mut raw, Framing::protocol(revision));
        crate::compress::write_frames(out, &raw);
    } else {
        block.write(out, Framing::protocol(revision));
    }
}

/// A server packet, for tests and tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerPacket {
    /// `Hello`: name and revision.
    Hello {
        /// The server's name.
        name: String,
        /// The server's revision.
        revision: u64,
    },
    /// `Data`, `Totals` or `Extremes` (by `kind`).
    Data {
        /// The packet type.
        kind: u64,
        /// The block.
        block: Block,
    },
    /// `Exception`: code and message.
    Exception {
        /// The code.
        code: i32,
        /// The message (`DB::Exception: …`).
        message: String,
    },
    /// `Progress`.
    Progress(Progress),
    /// `ProfileInfo`.
    ProfileInfo(ProfileInfo),
    /// `Pong`.
    Pong,
    /// `EndOfStream`.
    EndOfStream,
    /// `TableColumns`: the description.
    TableColumns(String),
}

/// Decodes one server packet at `revision` (`Hello` by the client's
/// `revision`, as a client reads it).
pub fn decode_server_packet(buf: &[u8], revision: u64) -> Result<(ServerPacket, usize), Error> {
    decode_server_packet_with(buf, revision, false)
}

/// [`decode_server_packet`], with `Data` blocks in compressed frames.
pub fn decode_server_packet_with(
    buf: &[u8],
    revision: u64,
    compressed: bool,
) -> Result<(ServerPacket, usize), Error> {
    let mut r = Reader::new(buf);
    let packet = match r.varuint()? {
        server::HELLO => {
            let name = r.string()?;
            r.varuint()?;
            r.varuint()?;
            let server_revision = r.varuint()?;
            let negotiated = revision.min(server_revision);
            if negotiated >= revision::SERVER_TIMEZONE {
                r.bytes()?;
            }
            if negotiated >= revision::SERVER_DISPLAY_NAME {
                r.bytes()?;
            }
            if negotiated >= revision::VERSION_PATCH {
                r.varuint()?;
            }
            ServerPacket::Hello {
                name,
                revision: server_revision,
            }
        }
        kind @ (server::DATA | server::TOTALS | server::EXTREMES) => {
            r.bytes()?;
            ServerPacket::Data {
                kind,
                block: read_block(&mut r, revision, compressed)?,
            }
        }
        server::EXCEPTION => {
            let code = r.i32()?;
            r.bytes()?;
            let message = r.string()?;
            r.bytes()?;
            if r.bool()? {
                return Err(Error::Unsupported("nested exceptions".into()));
            }
            ServerPacket::Exception { code, message }
        }
        server::PROGRESS => {
            let mut p = Progress {
                rows: r.varuint()?,
                bytes: r.varuint()?,
                total_rows: r.varuint()?,
                ..Progress::default()
            };
            if revision >= revision::CLIENT_WRITE_INFO {
                p.written_rows = r.varuint()?;
                p.written_bytes = r.varuint()?;
            }
            if revision >= revision::SERVER_QUERY_TIME_IN_PROGRESS {
                p.elapsed_ns = r.varuint()?;
            }
            ServerPacket::Progress(p)
        }
        server::PROFILE_INFO => {
            let info = ProfileInfo {
                rows: r.varuint()?,
                blocks: r.varuint()?,
                bytes: r.varuint()?,
                applied_limit: r.bool()?,
                rows_before_limit: r.varuint()?,
            };
            r.u8()?;
            ServerPacket::ProfileInfo(info)
        }
        server::PONG => ServerPacket::Pong,
        server::END_OF_STREAM => ServerPacket::EndOfStream,
        server::TABLE_COLUMNS => {
            r.bytes()?;
            ServerPacket::TableColumns(r.string()?)
        }
        other => return Err(Error::Malformed(format!("server packet type {other}"))),
    };
    Ok((packet, r.position()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tests::hex;

    #[test]
    fn hello_as_the_oracle_writes_it() {
        let hello = ServerHello {
            name: "ClickHouse".into(),
            version_major: 26,
            version_minor: 9,
            version_patch: 2,
            timezone: "UTC".into(),
            display_name: "ae32eb9b3603".into(),
        };
        // To clickhouse-go v1 (54213): the time zone and nothing after it. The
        // oracle writes its own revision (54492); the House writes 54460.
        let mut out = Vec::new();
        encode_hello(&mut out, &hello, 54213);
        let mut want = hex("000a436c69636b486f7573651a09");
        put_varuint(&mut want, revision::SERVER);
        want.extend(hex("03555443"));
        assert_eq!(out, want);
        // To clickhouse-go v2 (54460): display name and patch too.
        let mut out = Vec::new();
        encode_hello(&mut out, &hello, 54460);
        assert!(out.ends_with(&hex("0c61653332656239623336303302")));
    }

    #[test]
    fn table_columns_as_clickhouse_server_writes_them() {
        let text = columns_description(&[
            TableColumn {
                name: "id".into(),
                type_name: "UInt64".into(),
                ..TableColumn::default()
            },
            TableColumn {
                name: "s".into(),
                type_name: "String".into(),
                default_kind: "DEFAULT".into(),
                default_expression: "'x'".into(),
                comment: "it's".into(),
                codec: "ZSTD(3)".into(),
                ..TableColumn::default()
            },
        ]);
        assert_eq!(
            text,
            "columns format version: 1\n2 columns:\n`id` UInt64\n`s` String\tDEFAULT\t'x'\tCOMMENT 'it\\\\'s'\tCODEC(ZSTD(3))\n"
        );
        let mut out = Vec::new();
        encode_table_columns(&mut out, &text);
        let (packet, used) = decode_server_packet(&out, 54460).expect("decodes");
        assert_eq!(used, out.len());
        assert_eq!(packet, ServerPacket::TableColumns(text));
    }

    #[test]
    fn v1_query_decodes() {
        // golang-migrate's first statement, from the capture (revision 54213).
        let mut buf = hex("0100010000145b3a3a666666663a3132372e302e302e315d3a3001");
        put_str(&mut buf, "host");
        put_str(&mut buf, "host");
        put_str(&mut buf, "Golang SQLDriver");
        buf.extend(hex("0101c5a703"));
        put_str(&mut buf, ""); // quota key
        put_str(&mut buf, ""); // end of settings
        buf.extend(hex("0200"));
        put_str(&mut buf, "SELECT 1");
        let (packet, used) = decode_packet(&buf, 54213).expect("decodes");
        assert_eq!(used, buf.len());
        let ClientPacket::Query(query) = packet else {
            panic!("not a query");
        };
        assert_eq!(query.body, "SELECT 1");
        assert_eq!(query.client_info.client_name, "Golang SQLDriver");
        assert!(!query.compression);
        assert!(matches!(
            decode_packet(&buf[..buf.len() - 1], 54213),
            Err(Error::Incomplete)
        ));
    }

    #[test]
    fn exception_and_trailer_bytes() {
        let mut out = Vec::new();
        encode_progress(
            &mut out,
            &Progress {
                rows: 1,
                bytes: 1,
                total_rows: 1,
                elapsed_ns: 1_258_180,
                ..Progress::default()
            },
            54460,
        );
        assert_eq!(out, hex("03010101 0000c4e54c".replace(' ', "").as_str()));
        let mut out = Vec::new();
        encode_profile_info(
            &mut out,
            &ProfileInfo {
                rows: 1,
                blocks: 1,
                bytes: 4104,
                ..ProfileInfo::default()
            },
        );
        assert_eq!(
            out,
            hex("06010188200000" /* then the obsolete byte */)
                .into_iter()
                .chain([1])
                .collect::<Vec<_>>()
        );
        let mut out = Vec::new();
        encode_exception(&mut out, 60, "Unknown table");
        let mut r = Reader::new(&out);
        assert_eq!(r.varuint().expect("type"), server::EXCEPTION);
        assert_eq!(r.i32().expect("code"), 60);
        assert_eq!(r.string().expect("name"), "DB::Exception");
        assert_eq!(r.string().expect("text"), "DB::Exception: Unknown table");
    }

    #[test]
    fn client_packets_roundtrip() {
        for revision in [54213, 54460] {
            let query = Query {
                id: "q".into(),
                client_info: ClientInfo::default(),
                settings: vec![Setting {
                    name: "max_threads".into(),
                    flags: 0,
                    value: "1".into(),
                }],
                stage: 2,
                compression: false,
                body: "SELECT 1".into(),
                parameters: Vec::new(),
            };
            let mut buf = Vec::new();
            encode_query(&mut buf, &query, revision);
            let (packet, used) = if revision < revision::SETTINGS_SERIALIZED_AS_STRINGS {
                assert!(matches!(
                    decode_packet(&buf, revision),
                    Err(Error::Unsupported(_))
                ));
                continue;
            } else {
                decode_packet(&buf, revision).expect("decodes")
            };
            let ClientPacket::Query(back) = packet else {
                panic!("a query");
            };
            assert_eq!(back.body, "SELECT 1");
            assert_eq!(back.settings, query.settings);
            let (end, rest) = decode_packet(&buf[used..], revision).expect("the end block");
            assert_eq!(used + rest, buf.len());
            assert!(matches!(end, ClientPacket::Data { block, .. } if block.is_empty()));
        }
    }

    #[test]
    fn parameters_unquote() {
        assert_eq!(parameter_text("'it\\'s'"), "it's");
        assert_eq!(parameter_text("'a\\\\b\\n'"), "a\\b\n");
        assert_eq!(parameter_text("42"), "42");
        let mut buf = Vec::new();
        put_parameter(&mut buf, "s", "it's");
        let mut r = Reader::new(&buf);
        assert_eq!(r.string().expect("name"), "s");
        assert_eq!(r.varuint().expect("flags"), 2);
        assert_eq!(parameter_text(&r.string().expect("value")), "it's");
    }
}
