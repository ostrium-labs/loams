//! The `can_*` relation every `loams.admin.v1` RPC needs on the org (design 76 §9.2, D1611;
//! plan IA1, IA2 Task 5). Deny by default: an RPC this table does not name is refused with
//! `PERMISSION_DENIED`, whatever the caller's roles, and two tests keep the table equal to the
//! RPCs the protos define and to the role each handler asks for.
//!
//! `can_view` is any member of the org or one of its service accounts; `can_admin` is an org
//! admin or owner (`deploy/openfga/core.fga`). Owner-only steps inside a handler (removing an
//! owner, inviting one) keep their own check.

/// `(procedure, relation)`, one row per RPC.
pub(crate) const ADMIN_RPCS: &[(&str, &str)] = &[
    ("/loams.admin.v1.OrgService/GetOrg", "can_view"),
    ("/loams.admin.v1.OrgService/UpdateOrg", "can_admin"),
    ("/loams.admin.v1.OrgService/ListMembers", "can_view"),
    ("/loams.admin.v1.OrgService/RemoveMember", "can_admin"),
    ("/loams.admin.v1.OrgService/ListInvitations", "can_view"),
    ("/loams.admin.v1.OrgService/CreateInvitation", "can_admin"),
    ("/loams.admin.v1.OrgService/ListTeams", "can_view"),
    ("/loams.admin.v1.OrgService/CreateTeam", "can_admin"),
    ("/loams.admin.v1.OrgService/GetTeam", "can_view"),
    ("/loams.admin.v1.OrgService/AddTeamMember", "can_admin"),
    ("/loams.admin.v1.ProjectService/ListProjects", "can_view"),
    ("/loams.admin.v1.ProjectService/CreateProject", "can_admin"),
    ("/loams.admin.v1.ProjectService/GetProject", "can_view"),
    ("/loams.admin.v1.ProjectService/ListAccess", "can_view"),
    ("/loams.admin.v1.ProjectService/GrantAccess", "can_admin"),
    (
        "/loams.admin.v1.ProjectService/ListEnvironments",
        "can_view",
    ),
    (
        "/loams.admin.v1.ProjectService/CreateEnvironment",
        "can_admin",
    ),
    ("/loams.admin.v1.ProjectService/GetEnvironment", "can_view"),
    (
        "/loams.admin.v1.ProjectService/GetEnvironmentActivity",
        "can_view",
    ),
    ("/loams.admin.v1.AgentService/ListAgents", "can_view"),
    ("/loams.admin.v1.AgentService/CreateAgent", "can_admin"),
    ("/loams.admin.v1.AgentService/GetAgent", "can_view"),
    ("/loams.admin.v1.AgentService/UpdateAgent", "can_admin"),
    ("/loams.admin.v1.AgentService/SuspendAgent", "can_admin"),
    ("/loams.admin.v1.AgentService/ResumeAgent", "can_admin"),
    ("/loams.admin.v1.AgentService/ListAgentTokens", "can_view"),
    ("/loams.admin.v1.AgentService/ListTrustPolicies", "can_view"),
    (
        "/loams.admin.v1.AgentService/CreateTrustPolicy",
        "can_admin",
    ),
    (
        "/loams.admin.v1.AgentService/ListServiceAccounts",
        "can_view",
    ),
    (
        "/loams.admin.v1.AgentService/CreateServiceAccount",
        "can_admin",
    ),
    ("/loams.admin.v1.KeyService/ListKeys", "can_view"),
    ("/loams.admin.v1.KeyService/CreateKey", "can_admin"),
    ("/loams.admin.v1.KeyService/RevokeKey", "can_admin"),
    ("/loams.admin.v1.KeyService/RevokeToken", "can_admin"),
    ("/loams.admin.v1.AuditService/ListAuditEvents", "can_view"),
    ("/loams.admin.v1.AuditService/WatchAuditEvents", "can_view"),
];

/// The relation `procedure` (`/loams.admin.v1.OrgService/GetOrg`) needs, if it is declared.
pub(crate) fn declared(procedure: &str) -> Option<&'static str> {
    ADMIN_RPCS
        .iter()
        .find(|(p, _)| *p == procedure)
        .map(|(_, relation)| *relation)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn repo(path: &str) -> String {
        std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR")))
            .unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    /// Every RPC of every `loams.admin.v1` service, from the protos.
    fn proto_rpcs() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for file in ["agent", "audit", "common", "key", "org", "project"] {
            let text = repo(&format!("proto/loams/admin/v1/{file}.proto"));
            let mut service = None;
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("service ") {
                    service = rest.split_whitespace().next().map(str::to_owned);
                } else if let (Some(rest), Some(s)) = (line.strip_prefix("rpc "), &service) {
                    let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
                    out.insert(format!("/loams.admin.v1.{s}/{name}"));
                }
            }
        }
        out
    }

    #[test]
    fn every_admin_rpc_is_declared() {
        let names: BTreeSet<String> = ADMIN_RPCS.iter().map(|(p, _)| (*p).to_owned()).collect();
        assert_eq!(
            names.len(),
            ADMIN_RPCS.len(),
            "a procedure is declared twice"
        );
        assert_eq!(names, proto_rpcs(), "the table and the protos disagree");
        for (p, relation) in ADMIN_RPCS {
            assert!(
                matches!(*relation, "can_view" | "can_admin"),
                "{p}: {relation} is not an org can_* relation"
            );
        }
        assert_eq!(declared("/loams.admin.v1.OrgService/Nope"), None);
    }

    /// The table says what each handler's own `self.actor(&ctx, Role::…)` asks for: `Member` is
    /// `can_view`, `Admin` is `can_admin`.
    #[test]
    fn admin_rpcs_match_their_handlers() {
        let source = repo("crates/loams/src/api/connect_admin.rs");
        let mut service = String::new();
        let mut method = String::new();
        let mut seen = BTreeSet::new();
        for line in source.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("impl ")
                && let Some(name) = rest.strip_suffix(" for Admin {")
            {
                service = name.to_owned();
            } else if let Some(rest) = line.strip_prefix("async fn ") {
                method = rest
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .split('_')
                    .map(|w| {
                        let mut c = w.chars();
                        c.next()
                            .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                            .unwrap_or_default()
                    })
                    .collect();
            } else if let Some(at) = line.find("self.actor(&ctx, Role::") {
                let role = &line[at + "self.actor(&ctx, Role::".len()..];
                let relation = if role.starts_with("Member") {
                    "can_view"
                } else {
                    "can_admin"
                };
                let procedure = format!("/loams.admin.v1.{service}/{method}");
                assert_eq!(declared(&procedure), Some(relation), "{procedure}");
                seen.insert(procedure);
            }
        }
        assert_eq!(
            seen.len(),
            ADMIN_RPCS.len(),
            "a declared RPC has no handler check"
        );
    }
}
