//! The ClickHouse native protocol (TCP), the minimal subset the Loams House
//! serves (design §49 §8.1, D768; HE0 rulings R9.40 onward in
//! `docs/plans/he1-rulings/t9.md`).
//!
//! Sans-I/O: functions from bytes to packets and back, no sockets and no
//! runtime. The House's server (`loams_house::native`) owns the connection and
//! runs every statement through the same path as its HTTP interface.
//!
//! What is here, at protocol revisions up to [`revision::SERVER`] (54460, what
//! clickhouse-go v2 speaks; clickhouse-go v1 at 54213, golang-migrate's driver,
//! is served too):
//!
//! * the client's `Hello`, addendum, `Query` (its `ClientInfo`, settings as
//!   strings, parameters), `Data`, `Cancel` and `Ping` ([`packet`]);
//! * the server's `Hello`, `Data`, `Exception`, `Progress`, `ProfileInfo`,
//!   `Pong` and `EndOfStream` ([`packet`]);
//! * blocks in the protocol's framing and in the `Native` format's, and the
//!   column walk that converts between them ([`block`], [`types`]);
//! * compressed blocks: LZ4, ZSTD and none read, LZ4 written, with CityHash128
//!   checksums ([`compress`], R9.48).
//!
//! Not here: chunked packets (54470), sparse columns, `Dynamic`/`Variant` columns and `JSON` other than
//! as strings (`output_format_native_write_json_as_string`), inter-server
//! queries, and settings in the pre-54429 binary form.
//!
//! Written from the protocol's documentation (clickhouse.com/docs/native-protocol)
//! and clickhouse-go's Apache-2.0 sources, checked byte for byte against the
//! pinned clickhouse-server 26.9.2.8; no ClickHouse code is copied.

pub mod block;
pub mod compress;
pub mod packet;
pub mod types;
pub mod wire;

pub use block::{Block, BlockInfo, Column, Framing, NativeStream};
pub use packet::{ClientPacket, Hello, ProfileInfo, Progress, Query, ServerHello, Setting};
pub use types::ColumnType;

/// Why bytes did not decode.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not enough bytes yet: read more and try the packet again.
    #[error("incomplete packet")]
    Incomplete,
    /// Bytes that are not the protocol: the connection cannot continue.
    #[error("malformed packet: {0}")]
    Malformed(String),
    /// The protocol, but a part of it the House does not serve.
    #[error("{0}")]
    Unsupported(String),
}

/// Protocol revisions: what each field or behaviour needs (clickhouse-go's
/// `lib/proto/const.go` and the protocol documentation's names).
pub mod revision {
    /// The highest revision the House speaks; it writes this in `Hello`.
    pub const SERVER: u64 = SERVER_QUERY_TIME_IN_PROGRESS;
    /// `Query` carries `ClientInfo`.
    pub const CLIENT_INFO: u64 = 54032;
    /// `Hello` carries the server's time zone.
    pub const SERVER_TIMEZONE: u64 = 54058;
    /// `ClientInfo` carries a quota key.
    pub const QUOTA_KEY_IN_CLIENT_INFO: u64 = 54060;
    /// `Hello` carries a display name.
    pub const SERVER_DISPLAY_NAME: u64 = 54372;
    /// `Hello` and `ClientInfo` carry the version's patch.
    pub const VERSION_PATCH: u64 = 54401;
    /// The client reads `LowCardinality` columns.
    pub const LOW_CARDINALITY_TYPE: u64 = 54405;
    /// `TableColumns` may precede an `INSERT`'s header.
    pub const COLUMN_DEFAULTS_METADATA: u64 = 54410;
    /// `Progress` carries written rows and bytes.
    pub const CLIENT_WRITE_INFO: u64 = 54420;
    /// Settings travel as strings with flags.
    pub const SETTINGS_SERIALIZED_AS_STRINGS: u64 = 54429;
    /// `Query` carries an inter-server hash.
    pub const INTERSERVER_SECRET: u64 = 54441;
    /// `ClientInfo` carries an OpenTelemetry parent.
    pub const OPENTELEMETRY: u64 = 54442;
    /// HTTP `ClientInfo` carries `X-Forwarded-For`.
    pub const X_FORWARDED_FOR_IN_CLIENT_INFO: u64 = 54443;
    /// HTTP `ClientInfo` carries the referer.
    pub const REFERER_IN_CLIENT_INFO: u64 = 54447;
    /// `ClientInfo` carries the distributed depth.
    pub const DISTRIBUTED_DEPTH: u64 = 54448;
    /// `ClientInfo` carries the initial query's start time.
    pub const INITIAL_QUERY_START_TIME: u64 = 54449;
    /// `ClientInfo` carries the parallel replicas' fields.
    pub const PARALLEL_REPLICAS: u64 = 54453;
    /// Columns carry a serialization byte.
    pub const CUSTOM_SERIALIZATION: u64 = 54454;
    /// The client sends an addendum (its quota key) after the server's `Hello`.
    pub const ADDENDUM: u64 = 54458;
    /// `Query` carries parameters.
    pub const PARAMETERS: u64 = 54459;
    /// `Progress` carries the elapsed time.
    pub const SERVER_QUERY_TIME_IN_PROGRESS: u64 = 54460;
    /// Packets may be chunked (not served).
    pub const CHUNKED_PACKETS: u64 = 54470;

    /// The revision both sides speak.
    pub fn negotiate(client: u64) -> u64 {
        client.min(SERVER)
    }
}
