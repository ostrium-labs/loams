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
//! Built so far: [`frame`], RESP frames and the request decoder (KV2 Task 1).

pub mod frame;

pub use frame::{Decoder, Limits, Proto, ProtoError, Value};
