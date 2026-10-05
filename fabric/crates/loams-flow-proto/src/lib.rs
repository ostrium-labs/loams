//! The `loams.flow.v1` protocol of Loams Flow (design §33 D352 and D353; CN1
//! plan Task 1): the connector manifest (`connector.proto`) — what a connector
//! is, which runtime runs it (D354), what it can do (D353) and what it is
//! licensed under (D359) — and the connector instance (`instance.proto`), one
//! namespace's credentialed use of one.
//!
//! Everything is generated at build time from `fabric/proto/loams/flow/v1/` by
//! `connectrpc-build`: buffa message types with their borrowed views and the
//! proto3 JSON mapping, in which each field is named the way its YAML manifest
//! key in `connectors/registry/*.yaml` is (`specVersion` is `spec_version` on
//! the wire). D352 makes that YAML canonical, so the mapping is what CN1 Task
//! 1's `proto_and_yaml_agree` test reads. Oneof enums (`FabricBinding`'s
//! target) live under `loams::flow::v1::__buffa::oneof`.
//!
//! `fabric/proto` is a sibling of the engine workspace's root `proto/` tree,
//! not a subtree of it: the `fabric/` workspace is separate (FL1 plan Task 1),
//! and the two trees share only the `loams.<area>.v1` package convention.
//!
//! `connector.proto` and `instance.proto` declare **no service**, so this crate
//! generates no connect-rust service trait and does not depend on `connectrpc`:
//! `connectrpc::include_generated!()` is only
//! `include!(concat!(env!("OUT_DIR"), "/_connectrpc.rs"))`, the `include!`
//! below, so the macro would be the crate's sole use of `connectrpc`.
//!
//! CN1 Task 3's `flow.proto` brings `FlowService` — D352's serving API, with
//! `ListConnectors` and `DescribeConnector` — and its client behind the
//! `client` feature, exactly as `loams-live-proto` does for `LiveService`; that
//! is where the `connectrpc` dependency and the `client` feature's body arrive.

mod generated {
    include!(concat!(env!("OUT_DIR"), "/_connectrpc.rs"));
}

pub use generated::*;
