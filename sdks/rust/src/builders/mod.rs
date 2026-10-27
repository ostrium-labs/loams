//! The app builders (SDKP §2.4, §2.6): [`Schema`], [`Filter`] and [`Search`].
//!
//! Each builds the request's **proto3 JSON**, which is how the builder corpus
//! (`sdks/fixtures/builders/`) spells its expectations and what every SDK's
//! builder produces, and decodes it into the generated message with the
//! messages' own serde mapping. One JSON shape, thirteen languages, one corpus
//! checking them (`rust_builders_match_corpus`).

pub mod filter;
pub mod schema;
pub mod search;

pub use filter::{DocId, Filter};
pub use schema::{Distance, DynamicMapping, FieldOptions, Schema, TextOptions};
pub use search::{
    ConsistencyLevel, ExpandOptions, Fusion, HighlightOptions, Order, Search, SelectOptions, Sort,
    TextRetriever, VectorOptions,
};

use serde::de::DeserializeOwned;

use crate::error::{ErrorKind, LoamsError};
use connectrpc::ErrorCode as Code;

/// Decodes proto3 JSON into a generated message, or an `invalid_argument`.
pub(crate) fn decode<M: DeserializeOwned>(
    json: serde_json::Value,
    what: &str,
) -> Result<M, LoamsError> {
    serde_json::from_value(json).map_err(|error| {
        LoamsError::new(
            ErrorKind::InvalidArgument,
            Code::InvalidArgument,
            format!("the {what} the builder made does not decode: {error}"),
        )
    })
}
