//! Loams Flow's connector registry: the manifests of design §33 D352, the schema
//! and rules that check them (D353), and the 200-connector catalog CN1 Task 1 and
//! Task 2 generate and drift-check.
//!
//! * [`manifest`] — §33 §4's YAML as typed Rust, and the protobuf form that mirrors
//!   it one to one. Every enum carries its exact YAML slug.
//! * [`validate`] — `connectors/schema/connector.schema.json`, CN1 Task 1's six
//!   semantic rules, §33 §4 rule 1's refusal ([`validate::check_use`]) and D359's
//!   licence gate.
//! * [`registry`] — every manifest under `connectors/registry/`, indexed by id and by
//!   category, with [`registry::Filter`] for the catalog page's queries.
//!
//! The YAML is the source and the protobuf form mirrors it key for key, so
//! [`manifest::ConnectorSpec::to_proto`] and [`manifest::ConnectorSpec::from_proto`]
//! translate every enum slug to the proto name `fabric/proto/loams/flow/v1/connector.proto`
//! gives it and back; `proto_and_yaml_agree` drives that over all 21 ★ manifests.
//!
//! The crate is unpublished: it reads `connectors/registry/*.yaml`,
//! `connectors/schemas/*.config.json`, `connectors/schema/connector.schema.json`,
//! `connectors/registry/catalog.csv` and `connectors/licences.toml` from the
//! repository at run time (D352, D359, CN1 Ruling 5), and a crate archive would
//! carry none of them.
//!
//! ```
//! use loams_flow::manifest::repository_root;
//! use loams_flow::registry::Registry;
//!
//! // The crate is unpublished because it reads `connectors/` from the repository
//! // (D352, D359), so a caller names that directory rather than the current one.
//! let registry = Registry::load(&repository_root().join("connectors/registry"))
//!     .expect("the registry loads");
//! assert!(!registry.is_empty());
//! ```
//!
//! Names: Loams, `loams-flow`, `loams.flow.v1`, `io.loams.dev.*` (owner rulings,
//! 2026-10-01).

pub mod manifest;
pub mod registry;
pub mod validate;

pub use manifest::{
    API_VERSION, AuthMethod, Backpressure, BulkCaps, Capabilities, Category, ConfigRef,
    ConnectorSpec, Delivery, DeliveryCaps, EnvelopeDecl, Evolution, Format, KIND, Licence, Limits,
    ManifestError, ManifestErrors, Ordering, Priority, ProtoError, RegistryRequirement,
    RuntimeKind, RuntimeSpec, SchemaCaps, SinkCaps, SourceCaps, Status, Suite, load_manifest,
    load_manifest_value, load_manifest_with_schema, pb,
};
pub use registry::{Filter, Registry, RegistryError};
pub use validate::{
    CapabilityError, Direction, LicenceError, LicenceGate, ManifestSchema, Mode, SchemaError,
    SinkRequirement, Uses, check_use, validate_manifest, validate_manifest_with_config_schema,
};
