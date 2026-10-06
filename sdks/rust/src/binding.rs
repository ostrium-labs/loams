//! The runtime's view of the **generated** binding table (D641).
//!
//! [`facade`](crate::facade) is emitted by `protoc-gen-loams-facade` and holds
//! **data and traits only**: the enums, the reason constants, `MODULES`, and one
//! trait per service. It carries no lookups, and that is deliberate.
//!
//! `dev`'s `facade.rs` was a hand-written placeholder that mixed the two —
//! generated-shaped data beside hand-written `binding_of`, `module_of`,
//! `ModuleBinding::call` and `CallBinding::rpc` accessors. When the renderer
//! landed, those helpers could have gone *into* the generator, which would have
//! been the smaller diff. It would also have put hand-written code inside a
//! `@generated` file, which is the one thing §44 §10.1 point 3 and
//! `scripts/sdk/drift.sh` exist to prevent: the next regeneration would either
//! drop the helpers or silently keep them, and nobody could tell which from the
//! diff.
//!
//! So the split is by *ownership* rather than by convenience. The generated file
//! owns what the protos say; this module owns how the runtime reads them. Both
//! halves are over the same `facade::MODULES`, so there is one table and one
//! source of truth either way.
//!
//! [`CallBinding::retry_class`] is the one place the two vocabularies meet, and
//! it is where the SDK's retry semantics are pinned — read its comment before
//! changing anything about it.

use crate::facade::{self, CallBinding, ModuleBinding};
use crate::retry::RetryClass;

/// The module a module name or a proto package name identifies.
///
/// A name starting with `loams.` is read as a package, anything else as a
/// module name, so `package_of` in [`crate::system`] can hand either to it.
#[must_use]
pub fn module_of(name_or_package: &str) -> Option<&'static ModuleBinding> {
    if name_or_package.starts_with("loams.") {
        return facade::MODULES
            .iter()
            .find(|module| module.package == name_or_package);
    }
    facade::MODULES
        .iter()
        .find(|module| module.name == name_or_package)
}

/// The binding a module and call name identify, or `None` when the generator has
/// no such row.
#[must_use]
pub fn binding_of(module: &str, call: &str) -> Option<&'static CallBinding> {
    facade::MODULES
        .iter()
        .find(|entry| entry.name == module)?
        .calls
        .iter()
        .find(|binding| binding.name == call)
}

impl ModuleBinding {
    /// One of the module's calls, by its `snake_case` name.
    #[must_use]
    pub fn call(&self, name: &str) -> Option<&'static CallBinding> {
        self.calls.iter().find(|binding| binding.name == name)
    }
}

impl CallBinding {
    /// The call's retry class, in the runtime's own vocabulary.
    ///
    /// The generated table carries **two** closed sets that `dev` folded into
    /// one: `Idempotency` (`NoSideEffects` / `Idempotent` / `None`), read out of
    /// the descriptor, and `Retry` (`Safe` / `Manual`), which is the retry
    /// *decision*. `RetryClass` is what [`RetryPlan`](crate::call::RetryPlan) and
    /// [`should_retry`](crate::retry::should_retry) have always taken, and it is
    /// public API, so the mapping lives here rather than at the twelve call sites
    /// that read it.
    ///
    /// # Why this is behaviour-preserving
    ///
    /// `dev` derived the class from the descriptor alone:
    /// `RetryClass::of_idempotency_level`, which maps `NO_SIDE_EFFECTS` and
    /// `IDEMPOTENT` to `Safe` and everything else to `Manual`. The generator
    /// derives `Retry` the same way — `lib.rs` maps `NoSideEffects | Idempotent`
    /// to `Safe` and `None` to `Manual` — **unless** the proto sets
    /// `FacadeOptions.retry_safe`, which `dev` read but ignored. No proto sets it
    /// today, so for every row in the table the two agree exactly, and
    /// `binding_class_agrees_with_idempotency` asserts that rather than assuming
    /// it.
    ///
    /// The one behavioural difference is deliberate and strictly safer: a proto
    /// that later says `retry_safe: false` on an idempotent read now stops
    /// retrying it, which is what the annotation asks for and what D610's split
    /// intends. Reading `Idempotency` directly instead would pin the SDK to the
    /// old behaviour and silently ignore the annotation forever.
    #[must_use]
    pub fn retry_class(&self) -> RetryClass {
        RetryClass::of_generated_retry(self.retry)
    }

    /// Whether the call's request message declares an `idempotency_key` field.
    ///
    /// Read out of the descriptor by the generator, so a proto that adds the
    /// field is keyed here without anything in this crate changing. `dev` carried
    /// this as a hand-written `keyed: bool` literal on each row of a
    /// hand-written table, which was a second place to forget.
    #[must_use]
    pub fn takes_idempotency_key(&self) -> bool {
        self.keyed
    }
}

/// `Retry::Safe` is the generated decision "the SDK retries this on its own",
/// and `RetryClass::Safe` is what the runtime has always called it. The two
/// closed sets are the same decision; this is the naming between them.
impl RetryClass {
    /// The runtime's retry class for a generated [`facade::Retry`].
    ///
    /// Total by construction: `facade::Retry` has exactly two variants, so a
    /// proto that grows a third cannot leave this unhandled.
    #[must_use]
    pub fn of_generated_retry(retry: facade::Retry) -> Self {
        match retry {
            facade::Retry::Safe => RetryClass::Safe,
            facade::Retry::Manual => RetryClass::Manual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row of the generated table: the anti-vacuity floor for the module.
    /// A test that walks nothing passes, so the count is asserted.
    fn every_binding() -> Vec<&'static CallBinding> {
        let bindings: Vec<&'static CallBinding> = facade::MODULES
            .iter()
            .flat_map(|module| module.calls.iter())
            .collect();
        assert!(
            bindings.len() >= 7,
            "only {} bindings in the generated table; the walk is broken, not the table",
            bindings.len()
        );
        bindings
    }

    #[test]
    fn a_binding_is_found_by_its_module_and_call() {
        let binding = binding_of("instance", "get_instance").expect("generated");
        assert_eq!(binding.name, "get_instance");
        assert_eq!(binding.module, "instance");
        // And a name the generator has no row for is `None`, not a panic: the
        // runtime reports it by name (`Loams::binding`).
        assert!(binding_of("instance", "no_such_call").is_none());
        assert!(binding_of("no_such_module", "get_instance").is_none());
    }

    #[test]
    fn a_module_is_found_by_its_name_or_its_package() {
        let by_name = module_of("live").expect("generated");
        assert_eq!(by_name.name, "live");
        let by_package = module_of("loams.instance.v1").expect("generated");
        assert_eq!(by_package.package, "loams.instance.v1");
        assert!(module_of("loams.no.such.v1").is_none());
        assert!(module_of("no_such_module").is_none());
    }

    /// `Retry::Safe` and `RetryClass::Safe` are the same decision, so the class
    /// must be exactly the generated one for every row.
    ///
    /// This is the claim `retry_class` makes about the whole table, asserted
    /// rather than assumed: if a proto ever sets `FacadeOptions.retry_safe`, this
    /// fails and whoever changed it has to say out loud that the retry semantics
    /// moved.
    #[test]
    fn binding_class_agrees_with_idempotency() {
        for binding in every_binding() {
            assert_eq!(
                binding.retry_class(),
                RetryClass::of_generated_retry(binding.retry),
                "{}: the class must be the generated decision, read whole table",
                binding.rpc
            );
            // And, while no proto sets `retry_safe`, that decision is still the
            // one the descriptor implies — the equivalence `dev` relied on.
            let from_descriptor = match binding.idempotency {
                facade::Idempotency::NoSideEffects | facade::Idempotency::Idempotent => {
                    RetryClass::Safe
                }
                facade::Idempotency::None => RetryClass::Manual,
            };
            assert_eq!(
                binding.retry_class(),
                from_descriptor,
                "{}: a proto now sets retry_safe, so the generated decision and the \
                 descriptor's implication differ. That is allowed and safer, but it \
                 is a behaviour change and the test above no longer proves the \
                 equivalence `dev` had.",
                binding.rpc
            );
        }
    }

    #[test]
    fn only_the_messages_that_declare_a_key_are_keyed() {
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
        assert!(
            !binding_of("instance", "get_instance")
                .expect("generated")
                .takes_idempotency_key()
        );
        // The field is read out of the descriptor, so this is a fact about the
        // protos rather than a list: walk every call and require the flag to
        // agree with the one message that declares it.
        let keyed: Vec<&str> = every_binding()
            .into_iter()
            .filter(|binding| binding.keyed)
            .map(|binding| binding.name)
            .collect();
        assert_eq!(keyed, vec!["mutate"]);
    }

    /// Every binding's `rpc` is the path curl and grpcurl use, and it is what
    /// the retry loop reports. `dev` computed it in an accessor; it is data now.
    #[test]
    fn every_binding_names_its_own_rpc() {
        for binding in every_binding() {
            assert_eq!(
                binding.rpc,
                format!("{}/{}", binding.service, binding.method),
                "{} is not its service and method",
                binding.name
            );
        }
    }

    /// No call declares paging yet, and the table says so rather than staying
    /// silent. `ListCollections` arrives with API1 Task 2; a row that claimed
    /// pagination before the RPC existed would send a page token to an RPC that
    /// does not read one.
    #[test]
    fn no_call_is_paged_yet_and_that_is_visible() {
        for binding in every_binding() {
            assert!(
                binding.pagination.is_none(),
                "{} claims paging before the RPC exists",
                binding.rpc
            );
        }
    }

    /// Only `Watch` streams server messages.
    #[test]
    fn only_watch_is_a_server_stream() {
        for binding in every_binding() {
            let expected = if binding.method == "Watch" {
                facade::Streaming::Server
            } else {
                facade::Streaming::Unary
            };
            assert_eq!(
                binding.streaming, expected,
                "{}: the streaming shape is read off the proto, not written down",
                binding.rpc
            );
        }
    }

    /// The anti-vacuity floor for the table: it covers every RPC the proto
    /// compiler generated, and nothing more.
    ///
    /// `dev` compared `CallBinding.spec.same_method(spec)` against the
    /// `connectrpc` `Spec` constants. The generated table carries the
    /// procedure path as data rather than the `Spec` itself, so the comparison
    /// is against `Spec::procedure` directly — which is the same fact, because
    /// the generator derives `rpc` from that `Spec`.
    ///
    /// This is the test that fails if the generator ever stops emitting a row
    /// for an annotated method, or emits one for a method the protos do not
    /// have. Both halves are asserted; a count alone would not catch a row
    /// added and a row dropped in the same change.
    #[test]
    fn the_table_covers_every_generated_spec_and_nothing_more() {
        use loams_live_proto::loams::live::v1::{
            LIVE_SERVICE_DEPLOY_SPEC, LIVE_SERVICE_MODIFY_QUERY_SET_SPEC, LIVE_SERVICE_MUTATE_SPEC,
            LIVE_SERVICE_QUERY_SPEC, LIVE_SERVICE_WATCH_SPEC,
        };
        use loams_proto::loams::instance::v1::{
            INSTANCE_SERVICE_GET_INSTANCE_SPEC, INSTANCE_SERVICE_WHO_AM_I_SPEC,
        };

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
            let procedure = spec.procedure.trim_start_matches('/');
            assert!(
                table.iter().any(|call| call.rpc == procedure),
                "{procedure} is generated but not in the facade"
            );
        }
    }
}
