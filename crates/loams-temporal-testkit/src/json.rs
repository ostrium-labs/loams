//! Protobuf JSON for Temporal messages, from the vendored descriptors.

use std::sync::OnceLock;

use loams_temporal_proto::temporal::api::history::v1::History;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, SerializeOptions};
use serde_json::Value;

fn pool() -> &'static DescriptorPool {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| {
        DescriptorPool::decode(loams_temporal_proto::FILE_DESCRIPTOR_SET)
            .expect("the proto crate's descriptor set is valid")
    })
}

/// `history` as protobuf JSON (camelCase keys, 64-bit integers as strings,
/// enums by name, bytes as base64), like the CLI's `-o json`.
pub fn history_to_json(history: &History) -> Value {
    let desc = pool()
        .get_message_by_name("temporal.api.history.v1.History")
        .expect("History is in the descriptor set");
    let message = DynamicMessage::decode(desc, history.encode_to_vec().as_slice())
        .expect("a History re-decodes dynamically");
    let options = SerializeOptions::new().skip_default_fields(true);
    message
        .serialize_with_options(serde_json::value::Serializer, &options)
        .expect("protobuf JSON always serializes")
}
