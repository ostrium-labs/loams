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
        let mut promoted = Vec::new();
        for binding in every_binding() {
            assert_eq!(
                binding.retry_class(),
                RetryClass::of_generated_retry(binding.retry),
                "{}: the class must be the generated decision, read whole table",
                binding.rpc
            );
            // Without `retry_safe` the decision is the one the descriptor
            // implies — the equivalence `dev` relied on. `retry_safe` only
            // promotes a call to `Safe`; the read-only admin calls of
            // `LiveAdminService` (LV1 Task 10) are the ones that set it.
            let from_descriptor = match binding.idempotency {
                facade::Idempotency::NoSideEffects | facade::Idempotency::Idempotent => {
                    RetryClass::Safe
                }
                facade::Idempotency::None => RetryClass::Manual,
            };
            if binding.retry_class() != from_descriptor {
                assert_eq!(binding.retry_class(), RetryClass::Safe, "{}", binding.rpc);
                promoted.push(binding.rpc);
            }
        }
        promoted.sort_unstable();
        assert_eq!(
            promoted,
            [
                // SDKP P0a: the app calls §2.2 and §2.5 mark `retry_safe`.
                "loams.collection.v1.CollectionService/AddFields",
                "loams.collection.v1.CollectionService/CreateCollection",
                "loams.collection.v1.CollectionService/DropCollection",
                "loams.collection.v1.CollectionService/SetHot",
                "loams.collection.v1.CollectionService/UpdateAliases",
                "loams.collection.v1.CollectionService/WarmCollection",
                "loams.collection.v1.NamespaceService/CreateNamespace",
                "loams.graph.v1.GraphService/Explain",
                "loams.live.v1.LiveAdminService/GetApp",
                "loams.live.v1.LiveAdminService/GetDeployment",
                "loams.live.v1.LiveAdminService/GetIndexStatus",
                "loams.live.v1.LiveAdminService/ListApps",
                "loams.live.v1.LiveAdminService/ListDeployments",
                "loams.stream.v1.StreamService/CreateStream",
            ],
            "a proto changed which calls set retry_safe: a behaviour change"
        );
    }

    #[test]
    fn only_the_messages_that_declare_a_key_are_keyed() {
        assert!(
            binding_of("tables", "mutate")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !binding_of("live.admin", "deploy")
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
            .map(|binding| binding.rpc)
            .collect();
        // `KeyService/CreateKey` (API2 Task 7) is the one admin create that is
        // not by name, so it is the one that takes a key.
        assert_eq!(
            keyed,
            vec![
                "loams.admin.v1.KeyService/CreateKey",
                // SDKP P0a: the app mutations that take a key.
                "loams.collection.v1.DocumentService/WriteDocuments",
                "loams.graph.v1.GraphAdminService/CreateGraph",
                "loams.graph.v1.GraphAdminService/DeleteGraph",
                "loams.graph.v1.GraphService/Execute",
                "loams.graph.v1.GraphService/ExecuteBatch",
                "loams.graph.v1.GraphAdminService/UpdateGraph",
                // Loams Meter's creates (D2202).
                "loams.meter.v1.CustomerService/CreateCustomer",
                "loams.meter.v1.EntitlementService/CreateEntitlement",
                "loams.meter.v1.EntitlementService/CreateGrant",
                "loams.live.v1.LiveService/Mutate",
            ]
        );
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

    /// Only `LiveAdminService.ListDeployments` and `ListApps` (LV1 Tasks 10
    /// and 13) and the lists of
    /// `loams.admin.v1` (API2 Task 7) declare paging, and the table says so: a
    /// row that claimed pagination for an RPC that does not read a page token
    /// would send one anyway. (The name predates the admin lists.)
    #[test]
    fn only_list_deployments_is_paged_and_that_is_visible() {
        let mut paged: Vec<&str> = every_binding()
            .into_iter()
            .filter(|binding| binding.pagination.is_some())
            .map(|binding| binding.rpc)
            .collect();
        paged.sort_unstable();
        assert_eq!(
            paged,
            [
                "loams.admin.v1.AgentService/ListAgentTokens",
                "loams.admin.v1.AgentService/ListAgents",
                "loams.admin.v1.AgentService/ListServiceAccounts",
                "loams.admin.v1.AgentService/ListTrustPolicies",
                "loams.admin.v1.AuditService/ListAuditEvents",
                "loams.admin.v1.KeyService/ListKeys",
                "loams.admin.v1.OrgService/ListInvitations",
                "loams.admin.v1.OrgService/ListMembers",
                "loams.admin.v1.OrgService/ListTeams",
                "loams.admin.v1.ProjectService/ListAccess",
                "loams.admin.v1.ProjectService/ListEnvironments",
                "loams.admin.v1.ProjectService/ListProjects",
                "loams.collection.v1.CollectionService/ListCollections",
                "loams.collection.v1.CollectionService/ListVersions",
                "loams.collection.v1.NamespaceService/ListNamespaces",
                "loams.graph.v1.GraphAdminService/ListGraphs",
                "loams.live.v1.LiveAdminService/ListApps",
                "loams.live.v1.LiveAdminService/ListDeployments",
                // Loams Meter's lists (D2202).
                "loams.meter.v1.CustomerService/ListCustomers",
                "loams.meter.v1.EntitlementService/ListEntitlements",
                "loams.meter.v1.EntitlementService/ListGrants",
                "loams.stream.v1.StreamService/ListStreams",
            ]
        );
    }

    /// Only the watches (`Watch`, `WatchAuditEvents`), `TailLogs` and Graph's
    /// `ExecuteStream` (SDKP P0a) stream server messages.
    #[test]
    fn only_watch_and_tail_logs_are_server_streams() {
        for binding in every_binding() {
            let expected = if ["Watch", "WatchAuditEvents", "TailLogs", "ExecuteStream"]
                .contains(&binding.method)
            {
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
            LIVE_ADMIN_SERVICE_ACTIVATE_DEPLOYMENT_SPEC, LIVE_ADMIN_SERVICE_CREATE_APP_SPEC,
            LIVE_ADMIN_SERVICE_DELETE_APP_SPEC, LIVE_ADMIN_SERVICE_DEPLOY_SPEC,
            LIVE_ADMIN_SERVICE_GET_APP_SPEC, LIVE_ADMIN_SERVICE_GET_DEPLOYMENT_SPEC,
            LIVE_ADMIN_SERVICE_GET_INDEX_STATUS_SPEC, LIVE_ADMIN_SERVICE_LIST_APPS_SPEC,
            LIVE_ADMIN_SERVICE_LIST_DEPLOYMENTS_SPEC, LIVE_ADMIN_SERVICE_TAIL_LOGS_SPEC,
            LIVE_ADMIN_SERVICE_UPDATE_APP_SPEC, LIVE_SERVICE_AUTHENTICATE_SPEC,
            LIVE_SERVICE_MODIFY_QUERY_SET_SPEC, LIVE_SERVICE_MUTATE_SPEC, LIVE_SERVICE_QUERY_SPEC,
            LIVE_SERVICE_WATCH_SPEC,
        };
        use loams_proto::loams::admin::v1::{
            AGENT_SERVICE_CREATE_AGENT_SPEC, AGENT_SERVICE_CREATE_SERVICE_ACCOUNT_SPEC,
            AGENT_SERVICE_CREATE_TRUST_POLICY_SPEC, AGENT_SERVICE_GET_AGENT_SPEC,
            AGENT_SERVICE_LIST_AGENT_TOKENS_SPEC, AGENT_SERVICE_LIST_AGENTS_SPEC,
            AGENT_SERVICE_LIST_SERVICE_ACCOUNTS_SPEC, AGENT_SERVICE_LIST_TRUST_POLICIES_SPEC,
            AGENT_SERVICE_RESUME_AGENT_SPEC, AGENT_SERVICE_SUSPEND_AGENT_SPEC,
            AGENT_SERVICE_UPDATE_AGENT_SPEC, AUDIT_SERVICE_LIST_AUDIT_EVENTS_SPEC,
            AUDIT_SERVICE_WATCH_AUDIT_EVENTS_SPEC, KEY_SERVICE_CREATE_KEY_SPEC,
            KEY_SERVICE_LIST_KEYS_SPEC, KEY_SERVICE_REVOKE_KEY_SPEC, KEY_SERVICE_REVOKE_TOKEN_SPEC,
            ORG_SERVICE_ADD_TEAM_MEMBER_SPEC, ORG_SERVICE_CREATE_INVITATION_SPEC,
            ORG_SERVICE_CREATE_TEAM_SPEC, ORG_SERVICE_GET_ORG_SPEC, ORG_SERVICE_GET_TEAM_SPEC,
            ORG_SERVICE_LIST_INVITATIONS_SPEC, ORG_SERVICE_LIST_MEMBERS_SPEC,
            ORG_SERVICE_LIST_TEAMS_SPEC, ORG_SERVICE_REMOVE_MEMBER_SPEC,
            ORG_SERVICE_UPDATE_ORG_SPEC, PROJECT_SERVICE_CREATE_ENVIRONMENT_SPEC,
            PROJECT_SERVICE_CREATE_PROJECT_SPEC, PROJECT_SERVICE_GET_ENVIRONMENT_ACTIVITY_SPEC,
            PROJECT_SERVICE_GET_ENVIRONMENT_SPEC, PROJECT_SERVICE_GET_PROJECT_SPEC,
            PROJECT_SERVICE_GRANT_ACCESS_SPEC, PROJECT_SERVICE_LIST_ACCESS_SPEC,
            PROJECT_SERVICE_LIST_ENVIRONMENTS_SPEC, PROJECT_SERVICE_LIST_PROJECTS_SPEC,
        };
        use loams_proto::loams::auth::v1::{
            AUTH_SERVICE_COMPLETE_SETUP_SPEC, AUTH_SERVICE_CREATE_SESSION_SPEC,
            AUTH_SERVICE_DECIDE_CONSENT_SPEC, AUTH_SERVICE_DELETE_SESSION_SPEC,
            AUTH_SERVICE_GET_SESSION_SPEC, AUTH_SERVICE_LIST_PROVIDERS_SPEC,
        };
        use loams_proto::loams::collection::v1::{
            COLLECTION_SERVICE_ADD_FIELDS_SPEC, COLLECTION_SERVICE_CREATE_COLLECTION_SPEC,
            COLLECTION_SERVICE_DROP_COLLECTION_SPEC, COLLECTION_SERVICE_GET_COLLECTION_SPEC,
            COLLECTION_SERVICE_LIST_COLLECTIONS_SPEC, COLLECTION_SERVICE_LIST_VERSIONS_SPEC,
            COLLECTION_SERVICE_SCAN_SPEC, COLLECTION_SERVICE_SET_HOT_SPEC,
            COLLECTION_SERVICE_UPDATE_ALIASES_SPEC, COLLECTION_SERVICE_WARM_COLLECTION_SPEC,
            DOCUMENT_SERVICE_COUNT_DOCUMENTS_SPEC, DOCUMENT_SERVICE_DELETE_BY_FILTER_SPEC,
            DOCUMENT_SERVICE_GET_DOCUMENTS_SPEC, DOCUMENT_SERVICE_PATCH_BY_FILTER_SPEC,
            DOCUMENT_SERVICE_SCROLL_DOCUMENTS_SPEC, DOCUMENT_SERVICE_WRITE_DOCUMENTS_SPEC,
            NAMESPACE_SERVICE_CREATE_NAMESPACE_SPEC, NAMESPACE_SERVICE_LIST_NAMESPACES_SPEC,
            QUERY_SERVICE_SEARCH_SPEC,
        };
        use loams_proto::loams::graph::v1::{
            GRAPH_ADMIN_SERVICE_CREATE_GRAPH_SPEC, GRAPH_ADMIN_SERVICE_DELETE_GRAPH_SPEC,
            GRAPH_ADMIN_SERVICE_GET_GRAPH_SPEC, GRAPH_ADMIN_SERVICE_GET_SCHEMA_SPEC,
            GRAPH_ADMIN_SERVICE_LIST_GRAPHS_SPEC, GRAPH_ADMIN_SERVICE_UPDATE_GRAPH_SPEC,
            GRAPH_SERVICE_EXECUTE_BATCH_SPEC, GRAPH_SERVICE_EXECUTE_SPEC,
            GRAPH_SERVICE_EXECUTE_STREAM_SPEC, GRAPH_SERVICE_EXPLAIN_SPEC,
        };
        use loams_proto::loams::instance::v1::{
            INSTANCE_SERVICE_GET_INSTANCE_SPEC, INSTANCE_SERVICE_WHO_AM_I_SPEC,
        };
        // Loams Meter's public services (D2202).
        use loams_proto::loams::meter::v1::{
            CUSTOMER_SERVICE_CREATE_CUSTOMER_SPEC, CUSTOMER_SERVICE_GET_CUSTOMER_SPEC,
            CUSTOMER_SERVICE_LIST_CUSTOMERS_SPEC, CUSTOMER_SERVICE_SET_PAYMENT_STATE_SPEC,
            CUSTOMER_SERVICE_SET_SPEND_CONTROLS_SPEC, CUSTOMER_SERVICE_UPDATE_CUSTOMER_SPEC,
            ENTITLEMENT_SERVICE_CHECK_ACCESS_SPEC, ENTITLEMENT_SERVICE_CREATE_ENTITLEMENT_SPEC,
            ENTITLEMENT_SERVICE_CREATE_GRANT_SPEC, ENTITLEMENT_SERVICE_GET_BALANCE_HISTORY_SPEC,
            ENTITLEMENT_SERVICE_GET_ENTITLEMENT_VALUE_SPEC,
            ENTITLEMENT_SERVICE_LIST_ENTITLEMENTS_SPEC, ENTITLEMENT_SERVICE_LIST_GRANTS_SPEC,
            ENTITLEMENT_SERVICE_RESET_ENTITLEMENT_SPEC, ENTITLEMENT_SERVICE_VOID_GRANT_SPEC,
            METER_SERVICE_QUERY_METER_SPEC, NOTIFICATION_SERVICE_CREATE_RULE_SPEC,
            NOTIFICATION_SERVICE_CREATE_WEBHOOK_SPEC, NOTIFICATION_SERVICE_LIST_CONSOLE_NOTIFICATIONS_SPEC,
            NOTIFICATION_SERVICE_LIST_DELIVERIES_SPEC,
            NOTIFICATION_SERVICE_LIST_RULES_SPEC, NOTIFICATION_SERVICE_ROTATE_WEBHOOK_SECRET_SPEC,
            PLAN_SERVICE_APPLY_PLAN_SPEC, PLAN_SERVICE_ASSIGN_PLAN_SPEC,
            PLAN_SERVICE_LIST_PLANS_SPEC, PLAN_SERVICE_UNASSIGN_PLAN_SPEC,
        };
        use loams_proto::loams::sql::v1::{SQL_SERVICE_QUERY_ARROW_SPEC, SQL_SERVICE_QUERY_SPEC};
        use loams_proto::loams::stream::v1::{
            STREAM_SERVICE_CREATE_STREAM_SPEC, STREAM_SERVICE_DESCRIBE_STREAM_SPEC,
            STREAM_SERVICE_FETCH_CLOUD_EVENTS_SPEC, STREAM_SERVICE_FETCH_SPEC,
            STREAM_SERVICE_LIST_STREAMS_SPEC, STREAM_SERVICE_PRODUCE_CLOUD_EVENTS_SPEC,
            STREAM_SERVICE_PRODUCE_SPEC,
        };

        let generated = [
            // SDKP P0a: the app data planes. `QueryService/Search` backs two
            // facade calls (`search.query` and `vector.search`), so it is
            // listed twice, once per row.
            NAMESPACE_SERVICE_CREATE_NAMESPACE_SPEC,
            NAMESPACE_SERVICE_LIST_NAMESPACES_SPEC,
            COLLECTION_SERVICE_CREATE_COLLECTION_SPEC,
            COLLECTION_SERVICE_LIST_COLLECTIONS_SPEC,
            COLLECTION_SERVICE_GET_COLLECTION_SPEC,
            COLLECTION_SERVICE_DROP_COLLECTION_SPEC,
            COLLECTION_SERVICE_ADD_FIELDS_SPEC,
            COLLECTION_SERVICE_LIST_VERSIONS_SPEC,
            COLLECTION_SERVICE_SCAN_SPEC,
            COLLECTION_SERVICE_UPDATE_ALIASES_SPEC,
            COLLECTION_SERVICE_SET_HOT_SPEC,
            COLLECTION_SERVICE_WARM_COLLECTION_SPEC,
            DOCUMENT_SERVICE_WRITE_DOCUMENTS_SPEC,
            DOCUMENT_SERVICE_GET_DOCUMENTS_SPEC,
            DOCUMENT_SERVICE_SCROLL_DOCUMENTS_SPEC,
            DOCUMENT_SERVICE_COUNT_DOCUMENTS_SPEC,
            DOCUMENT_SERVICE_DELETE_BY_FILTER_SPEC,
            DOCUMENT_SERVICE_PATCH_BY_FILTER_SPEC,
            QUERY_SERVICE_SEARCH_SPEC,
            QUERY_SERVICE_SEARCH_SPEC,
            GRAPH_ADMIN_SERVICE_CREATE_GRAPH_SPEC,
            GRAPH_ADMIN_SERVICE_GET_GRAPH_SPEC,
            GRAPH_ADMIN_SERVICE_LIST_GRAPHS_SPEC,
            GRAPH_ADMIN_SERVICE_UPDATE_GRAPH_SPEC,
            GRAPH_ADMIN_SERVICE_DELETE_GRAPH_SPEC,
            GRAPH_ADMIN_SERVICE_GET_SCHEMA_SPEC,
            GRAPH_SERVICE_EXECUTE_SPEC,
            GRAPH_SERVICE_EXECUTE_BATCH_SPEC,
            GRAPH_SERVICE_EXECUTE_STREAM_SPEC,
            GRAPH_SERVICE_EXPLAIN_SPEC,
            // Loams Meter (D2202): customers, entitlements, plans, notifications
            // and meters.
            CUSTOMER_SERVICE_CREATE_CUSTOMER_SPEC,
            CUSTOMER_SERVICE_GET_CUSTOMER_SPEC,
            CUSTOMER_SERVICE_LIST_CUSTOMERS_SPEC,
            CUSTOMER_SERVICE_SET_PAYMENT_STATE_SPEC,
            CUSTOMER_SERVICE_SET_SPEND_CONTROLS_SPEC,
            CUSTOMER_SERVICE_UPDATE_CUSTOMER_SPEC,
            ENTITLEMENT_SERVICE_CHECK_ACCESS_SPEC,
            ENTITLEMENT_SERVICE_CREATE_ENTITLEMENT_SPEC,
            ENTITLEMENT_SERVICE_CREATE_GRANT_SPEC,
            ENTITLEMENT_SERVICE_GET_BALANCE_HISTORY_SPEC,
            ENTITLEMENT_SERVICE_GET_ENTITLEMENT_VALUE_SPEC,
            ENTITLEMENT_SERVICE_LIST_ENTITLEMENTS_SPEC,
            ENTITLEMENT_SERVICE_LIST_GRANTS_SPEC,
            ENTITLEMENT_SERVICE_RESET_ENTITLEMENT_SPEC,
            ENTITLEMENT_SERVICE_VOID_GRANT_SPEC,
            METER_SERVICE_QUERY_METER_SPEC,
            NOTIFICATION_SERVICE_CREATE_RULE_SPEC,
            NOTIFICATION_SERVICE_CREATE_WEBHOOK_SPEC,
            NOTIFICATION_SERVICE_LIST_CONSOLE_NOTIFICATIONS_SPEC,
            NOTIFICATION_SERVICE_LIST_DELIVERIES_SPEC,
            NOTIFICATION_SERVICE_LIST_RULES_SPEC,
            NOTIFICATION_SERVICE_ROTATE_WEBHOOK_SECRET_SPEC,
            PLAN_SERVICE_APPLY_PLAN_SPEC,
            PLAN_SERVICE_ASSIGN_PLAN_SPEC,
            PLAN_SERVICE_LIST_PLANS_SPEC,
            PLAN_SERVICE_UNASSIGN_PLAN_SPEC,
            // SDKP Part C: `sql`.
            SQL_SERVICE_QUERY_SPEC,
            SQL_SERVICE_QUERY_ARROW_SPEC,
            // SDKP Part C: `streams`.
            STREAM_SERVICE_CREATE_STREAM_SPEC,
            STREAM_SERVICE_DESCRIBE_STREAM_SPEC,
            STREAM_SERVICE_LIST_STREAMS_SPEC,
            STREAM_SERVICE_PRODUCE_SPEC,
            STREAM_SERVICE_FETCH_SPEC,
            STREAM_SERVICE_PRODUCE_CLOUD_EVENTS_SPEC,
            STREAM_SERVICE_FETCH_CLOUD_EVENTS_SPEC,
            // The console API (API2 Tasks 6 and 7).
            AUTH_SERVICE_LIST_PROVIDERS_SPEC,
            AUTH_SERVICE_COMPLETE_SETUP_SPEC,
            AUTH_SERVICE_GET_SESSION_SPEC,
            AUTH_SERVICE_CREATE_SESSION_SPEC,
            AUTH_SERVICE_DELETE_SESSION_SPEC,
            AUTH_SERVICE_DECIDE_CONSENT_SPEC,
            ORG_SERVICE_GET_ORG_SPEC,
            ORG_SERVICE_UPDATE_ORG_SPEC,
            ORG_SERVICE_LIST_MEMBERS_SPEC,
            ORG_SERVICE_REMOVE_MEMBER_SPEC,
            ORG_SERVICE_LIST_INVITATIONS_SPEC,
            ORG_SERVICE_CREATE_INVITATION_SPEC,
            ORG_SERVICE_LIST_TEAMS_SPEC,
            ORG_SERVICE_CREATE_TEAM_SPEC,
            ORG_SERVICE_GET_TEAM_SPEC,
            ORG_SERVICE_ADD_TEAM_MEMBER_SPEC,
            PROJECT_SERVICE_LIST_PROJECTS_SPEC,
            PROJECT_SERVICE_CREATE_PROJECT_SPEC,
            PROJECT_SERVICE_GET_PROJECT_SPEC,
            PROJECT_SERVICE_LIST_ACCESS_SPEC,
            PROJECT_SERVICE_GRANT_ACCESS_SPEC,
            PROJECT_SERVICE_LIST_ENVIRONMENTS_SPEC,
            PROJECT_SERVICE_CREATE_ENVIRONMENT_SPEC,
            PROJECT_SERVICE_GET_ENVIRONMENT_SPEC,
            PROJECT_SERVICE_GET_ENVIRONMENT_ACTIVITY_SPEC,
            AGENT_SERVICE_LIST_AGENTS_SPEC,
            AGENT_SERVICE_CREATE_AGENT_SPEC,
            AGENT_SERVICE_GET_AGENT_SPEC,
            AGENT_SERVICE_UPDATE_AGENT_SPEC,
            AGENT_SERVICE_SUSPEND_AGENT_SPEC,
            AGENT_SERVICE_RESUME_AGENT_SPEC,
            AGENT_SERVICE_LIST_AGENT_TOKENS_SPEC,
            AGENT_SERVICE_LIST_TRUST_POLICIES_SPEC,
            AGENT_SERVICE_CREATE_TRUST_POLICY_SPEC,
            AGENT_SERVICE_LIST_SERVICE_ACCOUNTS_SPEC,
            AGENT_SERVICE_CREATE_SERVICE_ACCOUNT_SPEC,
            KEY_SERVICE_LIST_KEYS_SPEC,
            KEY_SERVICE_CREATE_KEY_SPEC,
            KEY_SERVICE_REVOKE_KEY_SPEC,
            KEY_SERVICE_REVOKE_TOKEN_SPEC,
            AUDIT_SERVICE_LIST_AUDIT_EVENTS_SPEC,
            AUDIT_SERVICE_WATCH_AUDIT_EVENTS_SPEC,
            INSTANCE_SERVICE_GET_INSTANCE_SPEC,
            INSTANCE_SERVICE_WHO_AM_I_SPEC,
            LIVE_SERVICE_WATCH_SPEC,
            LIVE_SERVICE_AUTHENTICATE_SPEC,
            LIVE_SERVICE_MODIFY_QUERY_SET_SPEC,
            LIVE_SERVICE_MUTATE_SPEC,
            LIVE_SERVICE_QUERY_SPEC,
            LIVE_ADMIN_SERVICE_DEPLOY_SPEC,
            LIVE_ADMIN_SERVICE_LIST_DEPLOYMENTS_SPEC,
            LIVE_ADMIN_SERVICE_GET_DEPLOYMENT_SPEC,
            LIVE_ADMIN_SERVICE_ACTIVATE_DEPLOYMENT_SPEC,
            LIVE_ADMIN_SERVICE_GET_INDEX_STATUS_SPEC,
            LIVE_ADMIN_SERVICE_TAIL_LOGS_SPEC,
            LIVE_ADMIN_SERVICE_CREATE_APP_SPEC,
            LIVE_ADMIN_SERVICE_LIST_APPS_SPEC,
            LIVE_ADMIN_SERVICE_GET_APP_SPEC,
            LIVE_ADMIN_SERVICE_UPDATE_APP_SPEC,
            LIVE_ADMIN_SERVICE_DELETE_APP_SPEC,
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
