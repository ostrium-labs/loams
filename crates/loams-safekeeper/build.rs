//! The storage broker's gRPC client (feature `server`; PG2 Task 32), from
//! the vendored `proto/storage_broker/broker.proto`. The server side is
//! generated too, for the tests' stand-in broker.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/storage_broker/broker.proto");
    // `tonic-prost-build` is an optional build dependency, so the call must
    // be compiled out (not just skipped) when `server` is off.
    #[cfg(feature = "server")]
    tonic_prost_build::configure()
        .compile_protos(&["proto/storage_broker/broker.proto"], &["proto"])?;
    Ok(())
}
