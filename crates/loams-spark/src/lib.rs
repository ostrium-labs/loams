//! Managed Spark through Spark Connect (design 81, plan SP1; design 26 §9,
//! design 68 §7).
//!
//! Loams runs a Spark Connect server per instance (Sail by default, Apache
//! Spark 4 on request) and puts a proxy in front of it; Loams implements no
//! Spark Connect server and parses no plans (D1263).
//!
//! - [`proxy`]: the Spark Connect gRPC proxy: auth ([`auth`]), tenant routing
//!   ([`route`]), session limits and idle timeouts ([`sessions`]), byte
//!   forwarding ([`wire`]). Served by `loams-spark-proxy` or
//!   `loams-gateway --module spark` ([`standalone`]).
//! - [`backend`]: the Sail and Apache Spark servers behind one trait.
//! - [`conf`]: cluster config rendering (Iceberg catalog `loams` at
//!   Lakekeeper, S3A on Loams object storage, event logs to S3, Sail env).
//! - [`k8s`]: an instance's Kubernetes objects.
//! - [`managed`]: the `ManagedEngine` adapter and the proxy's Kubernetes
//!   router (start-on-first-connect).
//! - [`proto`]: the Spark Connect protos (apache/spark v4.1.3).
//!
//! Everything here is **preview** (design 81 R20) and has not yet been run
//! end to end.

pub mod auth;
pub mod backend;
pub mod conf;
pub mod k8s;
pub mod managed;
pub mod proto;
pub mod proxy;
pub mod route;
pub mod sessions;
pub mod standalone;
pub mod wire;

// SP-b's modules (jobs, catalog, CLI). Whichever lane lands second keeps
// both sets of lines (engines-81.md, SP-a/SP-b):
// pub mod catalog;
// pub mod jobs;
// pub use jobs::cli;
