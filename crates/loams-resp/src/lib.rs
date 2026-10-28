//! `loams-resp`: Loams' Redis gateway (design §77 §8; plan KV1, milestone
//! KV2). It speaks RESP2 and RESP3 to Redis and Valkey clients and stores
//! keys in `loams-kv` (the embedded backend in the light default, TiKV in
//! the `tikv` profile), with the key layout of D1653.
//!
//! The gateway ports Tidis (`tidb-incubator/tidis`, Apache-2.0), part of
//! which follows tokio's mini-redis (MIT), with `distributedio/titan`
//! (Apache-2.0) as the second reference. Ported files carry the header of
//! `docs/licences/ported-code.md`. No Redis source is used (D1649).
//!
//! - [`frame`]: RESP frames and the request decoder (KV2 Task 1).
//! - [`config`], [`server`], [`cmd`]: the module's configuration, the
//!   listener and connection loop, and the command table with the
//!   connection and server commands (KV2 Task 2).
//! - [`layout`]: the key layout and the meta record of D1653 (KV2 Task 3);
//!   `types`: the Redis types on `loams-kv` transactions (strings in
//!   Task 3).
//! - [`testing`]: a test gateway and client.

pub mod cmd;
pub mod config;
pub mod frame;
pub mod layout;
pub mod server;
pub mod testing;
pub(crate) mod types;

pub use config::RespConfig;
pub use frame::{Decoder, Limits, Proto, ProtoError, Value};
pub use server::Server;
