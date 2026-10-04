//! The binding table the runtime dispatches through (design §44 §7.3, D606;
//! runtime contract R2, R5, R6, R7).
//!
//! One entry per facade call. What each entry says about the wire comes from
//! the **generated** `connectrpc::Spec` the proto compiler emitted, not from a
//! string written here: the procedure path, the stream shape and the
//! `idempotency_level` are read off `INSTANCE_SERVICE_GET_INSTANCE_SPEC` and
//! friends, which `connectrpc-build` generated from `proto/`. So the RPC paths
//! in this table cannot drift from the protos — a proto that renames a method
//! changes the `Spec`, and a test compares.
//!
//! What is still hand-written is the part the facade **options** carry: the
//! module a service belongs to, the facade name of each call, and the
//! pagination fields. `protoc-gen-loams-facade`'s **Rust** renderer (SDK1
//! Task 3) will emit this file; it has not landed, and the TypeScript SDK's
//! `src/gen/facade.ts` is the shape it will emit. Until it does, this is the
//! hand-written facade D606/Q604 explicitly allow, checked by the same
//! conformance suite as a generated one. `rust_conformance_all_required_fixtures`
//! pins the rows that matter today; D744 records the substitution.

use connectrpc::{IdempotencyLevel, Spec, StreamType};

use crate::retry::RetryClass;

pub use loams_live_proto::loams::live::v1::{
    LIVE_SERVICE_MODIFY_QUERY_SET_SPEC, LIVE_SERVICE_WATCH_SPEC,
};
pub use loams_proto::loams::instance::v1::{
    INSTANCE_SERVICE_GET_INSTANCE_SPEC, INSTANCE_SERVICE_WHO_AM_I_SPEC,
};

/// The request-message field a mutation is keyed by (D610, R3).
///
/// Re-exported from [`crate::request`], where the policy that fills it lives.
pub use crate::request::IDEMPOTENCY_KEY_FIELD;

/// The proto revision this SDK was generated from (`LOAMS_PROTO_REV`, design
/// §44 §10.3). [`crate::System::version`] checks it against the server's
/// `GetInstance.api_versions`.
pub const PROTO_REV: &str = "v1";

/// The proto packages this SDK speaks, as `GetInstance.api_versions` names them.
pub const PROTO_PACKAGES: &[&str] = &[
    "google.protobuf",
    "loams.approvals.v1",
    "loams.devices.v1",
    "loams.errors.v1",
    "loams.instance.v1",
    "loams.live.v1",
    "loams.notifications.v1",
    "loams.operations.v1",
    "loams.options.v1",
];

/// Whether a call takes one request or returns a stream (D420: server streams
/// only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Streaming {
    /// One request message, one response message.
    Unary,
    /// One request message, a stream of response messages.
    Server,
}

/// AIP-158 pagination, as `FacadeOptions.pagination` spells it
/// (`"<items field>:<next page token field>"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// The repeated response field holding the page's items.
    pub items: &'static str,
    /// The response field holding the token for the next page.
    pub next_page_token: &'static str,
}

/// One facade call, as the runtime dispatches it.
#[derive(Debug, Clone, Copy)]
pub struct CallBinding {
    /// The module the call is exposed on, as `loams.<module>`.
    pub module: &'static str,
    /// The call's name in `snake_case`, the Rust convention (design §44 §7.1:
    /// module and method names are the proto names in the language's case).
    pub name: &'static str,
    /// The generated spec: procedure path, stream shape and idempotency level.
    pub spec: Spec,
    /// Whether the call's **request message** declares an `idempotency_key`
    /// field, which is what makes a mutation retryable (D610, R3).
    ///
    /// Read from the message type's own generated code, not guessed from the
    /// object a caller built — see [`takes_idempotency_key`].
    pub keyed: bool,
    /// The pagination, when the call's facade options name it.
    pub pagination: Option<Pagination>,
}

impl CallBinding {
    /// The RPC, as `package.Service/Method`: the path `curl` and `grpcurl` use.
    #[must_use]
    pub fn rpc(&self) -> String {
        format!("{}/{}", self.spec.service(), self.spec.method())
    }

    /// The method that backs the call, in the proto's `PascalCase`.
    #[must_use]
    pub fn method(&self) -> &'static str {
        self.spec.method()
    }

    /// The service behind the call, as `package.Service`.
    #[must_use]
    pub fn service(&self) -> &'static str {
        self.spec.service()
    }

    /// The proto package the call lives in.
    #[must_use]
    pub fn package(&self) -> String {
        self.spec
            .service()
            .rsplit_once('.')
            .map_or_else(String::new, |(p, _)| p.to_owned())
    }

    /// Whether the call is a server stream.
    #[must_use]
    pub fn streaming(&self) -> Streaming {
        match self.spec.stream_type {
            StreamType::ServerStream => Streaming::Server,
            _ => Streaming::Unary,
        }
    }

    /// The proto's `idempotency_level`, which is what the retry class is
    /// derived from rather than guessed.
    #[must_use]
    pub fn idempotency_level(&self) -> IdempotencyLevel {
        self.spec.idempotency_level
    }

    /// The call's retry class: a read or an idempotent RPC retries on its own, a
    /// mutation does not — unless it carries an idempotency key.
    #[must_use]
    pub fn retry(&self) -> RetryClass {
        RetryClass::of_idempotency_level(self.spec.idempotency_level)
    }

    /// Whether the call's request message declares an `idempotency_key` field.
    #[must_use]
    pub fn takes_idempotency_key(&self) -> bool {
        self.keyed
    }
}

/// One SDK module and its calls.
#[derive(Debug, Clone, Copy)]
pub struct ModuleBinding {
    /// The module's name, as it appears on `loams.<name>`.
    pub name: &'static str,
    /// One line for the module's reference docs.
    pub summary: &'static str,
    /// The service behind the module.
    pub service: &'static str,
    /// Whether the package's wire contract may still change (design §44 §10.3).
    pub unstable: bool,
    /// True for a second facade name for the same RPCs (design §44 §7.2 splits
    /// `loams.live.v1` into a session half and a table half).
    pub derived: bool,
    /// The calls the module exposes.
    pub calls: &'static [CallBinding],
}

impl ModuleBinding {
    /// The proto package behind the module.
    #[must_use]
    pub fn package(&self) -> String {
        self.service
            .rsplit_once('.')
            .map_or_else(String::new, |(p, _)| p.to_owned())
    }

    /// A call the module exposes, by its `snake_case` name.
    #[must_use]
    pub fn call(&self, name: &str) -> Option<&'static CallBinding> {
        self.calls.iter().find(|call| call.name == name)
    }
}

/// Every annotated service, as the module catalogue. Order is by module name.
pub const MODULES: &[ModuleBinding] = &[
    ModuleBinding {
        name: "instance",
        summary: "What this instance is, and who the caller is on it.",
        service: "loams.instance.v1.InstanceService",
        unstable: false,
        derived: false,
        calls: &[
            CallBinding {
                module: "instance",
                name: "get_instance",
                spec: INSTANCE_SERVICE_GET_INSTANCE_SPEC,
                // `GetInstanceRequest` is empty: there is nothing to key.
                keyed: false,
                pagination: None,
            },
            CallBinding {
                module: "instance",
                name: "who_am_i",
                spec: INSTANCE_SERVICE_WHO_AM_I_SPEC,
                keyed: false,
                pagination: None,
            },
        ],
    },
    ModuleBinding {
        name: "live",
        summary: "Live sync: watch a query set over a server stream.",
        service: "loams.live.v1.LiveService",
        unstable: true,
        derived: false,
        calls: &[
            CallBinding {
                module: "live",
                name: "modify_query_set",
                spec: LIVE_SERVICE_MODIFY_QUERY_SET_SPEC,
                keyed: false,
                pagination: None,
            },
            CallBinding {
                module: "live",
                name: "watch",
                spec: LIVE_SERVICE_WATCH_SPEC,
                keyed: false,
                pagination: None,
            },
        ],
    },
    ModuleBinding {
        name: "tables",
        summary: "",
        service: "loams.live.v1.LiveService",
        unstable: true,
        derived: true,
        calls: &[
            CallBinding {
                module: "tables",
                name: "deploy",
                spec: LIVE_SERVICE_DEPLOY_SPEC,
                keyed: false,
                pagination: None,
            },
            CallBinding {
                module: "tables",
                name: "mutate",
                spec: LIVE_SERVICE_MUTATE_SPEC,
                // `MutateRequest.idempotency_key` (proto3 `optional`, live.proto:163).
                keyed: true,
                pagination: None,
            },
            CallBinding {
                module: "tables",
                name: "query",
                spec: LIVE_SERVICE_QUERY_SPEC,
                keyed: false,
                pagination: None,
            },
        ],
    },
];

// The three table-half specs, imported here rather than at the top so the
// `pub use` above reads as one block of "the generated specs this table is
// built from".
use loams_live_proto::loams::live::v1::{
    LIVE_SERVICE_DEPLOY_SPEC, LIVE_SERVICE_MUTATE_SPEC, LIVE_SERVICE_QUERY_SPEC,
};

/// The module a module name or a proto package name identifies.
#[must_use]
pub fn module_of(name_or_package: &str) -> Option<&'static ModuleBinding> {
    if name_or_package.starts_with("loams.") {
        return MODULES
            .iter()
            .find(|module| module.package() == name_or_package);
    }
    MODULES.iter().find(|module| module.name == name_or_package)
}

/// Every call of a module, by module name, or `None` when there is no such
/// module.
#[must_use]
pub fn binding_of(module: &str, call: &str) -> Option<&'static CallBinding> {
    MODULES
        .iter()
        .find(|entry| entry.name == module)
        .and_then(|entry| entry.call(call))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spec this table is built from, paired with the binding that names
    /// it. A proto that adds, removes or renames an RPC has to be reflected
    /// here, and a forgotten row fails this test rather than a call at runtime.
    fn every_binding() -> Vec<&'static CallBinding> {
        MODULES
            .iter()
            .flat_map(|module| module.calls.iter())
            .collect()
    }

    #[test]
    fn every_binding_names_its_own_module_and_a_generated_rpc_path() {
        for module in MODULES {
            for call in module.calls {
                assert_eq!(call.module, module.name);
                assert_eq!(call.spec.service(), module.service);
                let rpc = call.rpc();
                assert!(
                    rpc.starts_with(&format!("{}/", module.service)),
                    "{}.{}: {rpc}",
                    module.name,
                    call.name
                );
                assert_eq!(
                    rpc.matches('/').count(),
                    1,
                    "the rpc path is <service>/<method>: {rpc}"
                );
            }
        }
    }

    #[test]
    fn the_table_covers_every_generated_spec_and_nothing_more() {
        // The generated specs, straight from the proto build.
        let generated = [
            INSTANCE_SERVICE_GET_INSTANCE_SPEC,
            INSTANCE_SERVICE_WHO_AM_I_SPEC,
            LIVE_SERVICE_WATCH_SPEC,
            LIVE_SERVICE_MODIFY_QUERY_SET_SPEC,
            LIVE_SERVICE_DEPLOY_SPEC,
            LIVE_SERVICE_MUTATE_SPEC,
            LIVE_SERVICE_QUERY_SPEC,
        ];
        let table = every_binding();
        assert_eq!(
            table.len(),
            generated.len(),
            "a facade row is missing or extra"
        );
        for spec in generated {
            assert!(
                table.iter().any(|call| call.spec.same_method(spec)),
                "{spec:?} is generated but not in the facade"
            );
        }
    }

    #[test]
    fn the_retry_class_comes_from_the_protos_idempotency_level() {
        let get_instance = binding_of("instance", "get_instance").expect("generated");
        assert_eq!(
            get_instance.idempotency_level(),
            IdempotencyLevel::NoSideEffects
        );
        assert_eq!(get_instance.retry(), RetryClass::Safe);
        // A live RPC's proto declares no idempotency level, so the class is
        // `Manual` until the call carries a key.
        let mutate = binding_of("tables", "mutate").expect("generated");
        assert_eq!(mutate.idempotency_level(), IdempotencyLevel::Unknown);
        assert_eq!(mutate.retry(), RetryClass::Manual);
    }

    #[test]
    fn only_watch_is_a_server_stream() {
        for call in every_binding() {
            let expected = if call.method() == "Watch" {
                Streaming::Server
            } else {
                Streaming::Unary
            };
            assert_eq!(call.streaming(), expected, "{}", call.rpc());
        }
    }

    #[test]
    fn only_mutate_is_keyed() {
        // `MutateRequest` declares `idempotency_key`; `DeployRequest` and
        // `QueryRequest` do not, and a key would be a field their schema does
        // not know.
        assert!(
            binding_of("tables", "mutate")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !binding_of("tables", "deploy")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !binding_of("tables", "query")
                .expect("generated")
                .takes_idempotency_key()
        );
    }

    #[test]
    fn no_call_is_paged_yet_and_that_is_visible() {
        // `ListCollections` arrives with API1 Task 2. A facade row that claims
        // pagination before the RPC exists would send a page token to an RPC
        // that does not read one.
        assert!(every_binding().iter().all(|call| call.pagination.is_none()));
    }

    #[test]
    fn a_module_name_or_a_package_name_finds_the_same_module() {
        assert_eq!(module_of("live").map(|m| m.name), Some("live"));
        assert_eq!(module_of("loams.live.v1").map(|m| m.name), Some("live"));
        assert!(module_of("collections").is_none(), "API1 Task 2");
    }
}
