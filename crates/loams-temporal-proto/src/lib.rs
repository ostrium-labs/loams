//! The Temporal API for the Loams Temporal gateway (design 74 §5.7, plan DX1
//! Task 1).
//!
//! - `temporal::api::*::v1`: the messages and the `WorkflowService` and
//!   `OperatorService` servers and clients, generated from
//!   [`temporalio/api`](https://github.com/temporalio/api) at
//!   [`API_TAG`], vendored unchanged under `proto/` (MIT; the licence is
//!   `LICENSE-temporalio-api`, and `VENDORED.md` lists every file with its
//!   hash). No Temporal server code is in Loams (D1536).
//! - `loams::temporal::internal::v1`: the gateway's own messages (task
//!   tokens; mutable state from Task 3), which no SDK reads.
//!
//! Every `bytes` field is [`bytes::Bytes`]: payloads are opaque and can be
//! megabytes, so they are shared rather than copied.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(missing_docs, rustdoc::all, unreachable_pub, unused_qualifications)]

include!(concat!(env!("OUT_DIR"), "/_includes.rs"));

/// The `temporalio/api` tag the protos are vendored from (plan DX1 X1.R1).
pub const API_TAG: &str = "v1.63.5";

/// The commit of [`API_TAG`].
pub const API_COMMIT: &str = "3ebdff42a9f07ac484b415fe8ff0b483b4ce3340";

/// The encoded `FileDescriptorSet` of every generated file, for gRPC
/// reflection and for tests that compare what is implemented with what the
/// API declares.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/temporal_descriptor.bin"));

/// The full gRPC name of `WorkflowService`.
pub const WORKFLOW_SERVICE: &str = "temporal.api.workflowservice.v1.WorkflowService";

/// The method names of a service in [`FILE_DESCRIPTOR_SET`], in declaration
/// order (`service` is a full name such as [`WORKFLOW_SERVICE`]). Empty if
/// there is no such service.
pub fn service_methods(service: &str) -> Vec<String> {
    use prost::Message as _;
    let set = prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET)
        .expect("the build script writes a valid descriptor set");
    for file in &set.file {
        let package = file.package();
        for svc in &file.service {
            if format!("{package}.{}", svc.name()) == service {
                return svc.method.iter().map(|m| m.name().to_string()).collect();
            }
        }
    }
    Vec::new()
}

/// [`service_methods`] for `WorkflowService`.
pub fn workflow_service_methods() -> Vec<String> {
    service_methods(WORKFLOW_SERVICE)
}
