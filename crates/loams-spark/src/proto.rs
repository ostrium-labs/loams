//! The Spark Connect API (`spark.connect`), generated from
//! [`apache/spark`](https://github.com/apache/spark) at [`SPARK_TAG`],
//! vendored unchanged under `proto/` (Apache-2.0; `VENDORED.md` lists every
//! file with its hash).
//!
//! The proxy itself forwards bytes (D1994); these types serve the tests' fake
//! backend, the proxy's own `ReleaseSession` calls and the conformance
//! checks of `tests/proto.rs`. Every `bytes` field is [`bytes::Bytes`].

#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    missing_docs,
    rustdoc::all,
    unreachable_pub,
    unused_qualifications,
    missing_debug_implementations
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/_includes.rs"));
}

pub use generated::spark::connect;

/// The `apache/spark` tag the protos are vendored from.
pub const SPARK_TAG: &str = "v4.1.3";

/// The commit of [`SPARK_TAG`].
pub const SPARK_COMMIT: &str = "77bbf77e86ad48f58b5dfbc6ac882b3e70cf1989";

/// The encoded `FileDescriptorSet` of every generated file.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/spark_connect_descriptor.bin"));

/// The full gRPC name of the service.
pub const SERVICE: &str = "spark.connect.SparkConnectService";

/// The path prefix every Spark Connect call has: `/spark.connect.SparkConnectService/`.
pub const PATH_PREFIX: &str = "/spark.connect.SparkConnectService/";

/// The service's methods at [`SPARK_TAG`], in declaration order.
pub const METHODS: &[&str] = &[
    "ExecutePlan",
    "AnalyzePlan",
    "Config",
    "AddArtifacts",
    "ArtifactStatus",
    "Interrupt",
    "ReattachExecute",
    "ReleaseExecute",
    "ReleaseSession",
    "FetchErrorDetails",
    "CloneSession",
];

/// The method names of a service in [`FILE_DESCRIPTOR_SET`], with their
/// input type's full name, in declaration order. Empty if there is no such
/// service.
pub fn service_methods(service: &str) -> Vec<(String, String)> {
    use prost::Message as _;
    let set = prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET)
        .expect("the build script writes a valid descriptor set");
    for file in &set.file {
        let package = file.package();
        for svc in &file.service {
            if format!("{package}.{}", svc.name()) == service {
                return svc
                    .method
                    .iter()
                    .map(|m| (m.name().to_string(), m.input_type().to_string()))
                    .collect();
            }
        }
    }
    Vec::new()
}
