//! `loams.postgres.v1` (PG2 Task 1; design §46 §4): the descriptor carries
//! the contract's service complete, every mutation takes an
//! `idempotency_key`, no connection info carries a secret, every reason the
//! design names is registered, and every RPC has a planned console route.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use prost::Message;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::method_options::IdempotencyLevel;
use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet, MethodDescriptorProto};

const PACKAGE: &str = "loams.postgres.v1";
const SERVICE: &str = "PostgresService";

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = workspace().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn descriptor_set() -> FileDescriptorSet {
    FileDescriptorSet::decode(loams_proto::FILE_DESCRIPTOR_SET).expect("the descriptor set decodes")
}

/// The files of `loams.postgres.v1`.
fn files(set: &FileDescriptorSet) -> Vec<&FileDescriptorProto> {
    let files: Vec<_> = set.file.iter().filter(|f| f.package() == PACKAGE).collect();
    assert!(
        !files.is_empty(),
        "{PACKAGE} is not in loams-proto's descriptor set (crates/loams-proto/build.rs FILES)"
    );
    files
}

fn methods(set: &FileDescriptorSet) -> Vec<MethodDescriptorProto> {
    let service = files(set)
        .into_iter()
        .flat_map(|f| f.service.iter())
        .find(|s| s.name() == SERVICE)
        .unwrap_or_else(|| panic!("no {SERVICE} in {PACKAGE}"));
    service.method.clone()
}

/// Every message of the package by its fully qualified name, nested ones
/// included (`.loams.postgres.v1.Outer.Inner`).
fn messages(set: &FileDescriptorSet) -> BTreeMap<String, DescriptorProto> {
    fn walk(prefix: &str, m: &DescriptorProto, out: &mut BTreeMap<String, DescriptorProto>) {
        let name = format!("{prefix}.{}", m.name());
        for nested in &m.nested_type {
            walk(&name, nested, out);
        }
        out.insert(name, m.clone());
    }
    let mut out = BTreeMap::new();
    for f in &set.file {
        for m in &f.message_type {
            walk(&format!(".{}", f.package()), m, &mut out);
        }
    }
    out
}

/// The RPCs of the shared contract (plan "Shared contracts"), each with
/// whether it reads only (`NO_SIDE_EFFECTS`) and whether it streams.
const CONTRACT: &[(&str, bool, bool)] = &[
    ("CreateProject", false, false),
    ("GetProject", true, false),
    ("ListProjects", true, false),
    ("UpdateProject", false, false),
    ("DeleteProject", false, false),
    ("CreateBranch", false, false),
    ("GetBranch", true, false),
    ("ListBranches", true, false),
    ("UpdateBranch", false, false),
    ("DeleteBranch", false, false),
    ("RestoreBranch", false, false),
    ("SetDefaultBranch", false, false),
    ("CreateEndpoint", false, false),
    ("GetEndpoint", true, false),
    ("ListEndpoints", true, false),
    ("UpdateEndpoint", false, false),
    ("StartEndpoint", false, false),
    ("SuspendEndpoint", false, false),
    ("RestartEndpoint", false, false),
    ("DeleteEndpoint", false, false),
    ("CreateRole", false, false),
    ("ListRoles", true, false),
    ("ResetRolePassword", false, false),
    ("DeleteRole", false, false),
    ("CreateDatabase", false, false),
    ("ListDatabases", true, false),
    ("DeleteDatabase", false, false),
    ("GetConnectionInfo", true, false),
    ("IssueConnectCredential", false, false),
    ("UpgradeProject", false, false),
    ("WatchProject", true, true),
];

/// The service is the contract's, RPC for RPC: names, read-only marks and
/// streaming.
#[test]
fn the_service_is_the_shared_contract() {
    let set = descriptor_set();
    let got: BTreeMap<String, (bool, bool)> = methods(&set)
        .iter()
        .map(|m| {
            let read_only = m
                .options
                .as_ref()
                .is_some_and(|o| o.idempotency_level() == IdempotencyLevel::NoSideEffects);
            (
                m.name().to_string(),
                (read_only, m.server_streaming() || m.client_streaming()),
            )
        })
        .collect();
    let want: BTreeMap<String, (bool, bool)> = CONTRACT
        .iter()
        .map(|(n, r, s)| (n.to_string(), (*r, *s)))
        .collect();
    assert_eq!(got, want);
}

/// Every RPC that is not `NO_SIDE_EFFECTS` takes `string idempotency_key =
/// 15` (AP0 rules; §46 §4.1).
#[test]
fn every_mutation_has_idempotency_key() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let mut checked = 0;
    for m in methods(&set) {
        let read_only = m
            .options
            .as_ref()
            .is_some_and(|o| o.idempotency_level() == IdempotencyLevel::NoSideEffects);
        if read_only {
            continue;
        }
        let input = &msgs[m.input_type()];
        let field = input
            .field
            .iter()
            .find(|f| f.number() == 15)
            .unwrap_or_else(|| panic!("{}: {} has no field 15", m.name(), input.name()));
        assert_eq!(field.name(), "idempotency_key", "{}", m.name());
        assert_eq!(field.r#type(), Type::String, "{}", m.name());
        assert_eq!(field.label(), Label::Optional, "{}: not repeated", m.name());
        assert!(
            !field.proto3_optional(),
            "{}: a plain proto3 string, not `optional`",
            m.name()
        );
        checked += 1;
    }
    assert_eq!(checked, CONTRACT.iter().filter(|(_, r, _)| !r).count());
}

/// `GetConnectionInfoResponse`, and every message it holds (the shared
/// `ConnectionInfo`), has no field named like a secret (§46 §4.1; Review
/// Focus 3).
#[test]
fn connection_info_has_no_password_field() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let root = format!(".{PACKAGE}.GetConnectionInfoResponse");
    let mut seen = BTreeSet::new();
    let mut todo = vec![root.clone()];
    let mut fields = 0;
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let m = msgs
            .get(&name)
            .unwrap_or_else(|| panic!("no message {name}"));
        for f in &m.field {
            fields += 1;
            let lower = f.name().to_lowercase();
            for bad in ["password", "secret", "credential", "token", "private_key"] {
                assert!(!lower.contains(bad), "{name}.{} carries a {bad}", f.name());
            }
            if f.r#type() == Type::Message && f.type_name().starts_with(&format!(".{PACKAGE}.")) {
                todo.push(f.type_name().to_string());
            }
        }
    }
    assert!(fields >= 7, "{root} parsed with only {fields} fields");
}

/// The reasons §46 §4.1 names, read from the design: its Errors bullet, and
/// the two its Idempotency bullet adds.
fn design_reasons() -> BTreeSet<String> {
    let text = read("docs/design/46-loams-postgres-production.md");
    let start = text.find("### 4.1").expect("§46 has a §4.1");
    let section = &text[start..start + text[start..].find("\n### ").unwrap()];
    let mut out = BTreeSet::new();
    for line in section.lines() {
        let take = line.starts_with("- **Errors.**") || line.starts_with("- **Idempotency.**");
        if !take {
            continue;
        }
        for (i, tok) in line.split('`').enumerate() {
            let snake = !tok.is_empty()
                && tok.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && tok.contains('_');
            // Odd pieces are inside backticks; field names are not reasons.
            if i % 2 == 1 && snake && tok != "idempotency_key" {
                out.insert(tok.to_string());
            }
        }
    }
    out
}

/// Every reason §46 §4.1 names is a row of `docs/api/reasons.md`'s
/// registry.
#[test]
fn reasons_registered() {
    let reasons = design_reasons();
    for must in [
        "project_not_found",
        "branch_has_children",
        "branch_protected",
        "lsn_out_of_retention",
        "endpoint_exists_for_branch",
        "compute_start_failed",
        "quota_exceeded",
        "storage_unavailable",
        "secret_already_issued",
        "already_exists",
    ] {
        assert!(
            reasons.contains(must),
            "§46 §4.1 no longer names {must}: {reasons:?}"
        );
    }
    let registry: BTreeSet<String> = read("docs/api/reasons.md")
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split('`').next())
        .map(str::to_string)
        .collect();
    let missing: Vec<_> = reasons.difference(&registry).collect();
    assert!(
        missing.is_empty(),
        "not in docs/api/reasons.md: {missing:?}"
    );
}

/// The proto's comment lists the `Operation` kind of every long-running RPC
/// (§46 §4.1's table: the RPCs that return an `Operation`).
#[test]
fn operation_kinds_are_listed() {
    let text = read("proto/loams/postgres/v1/postgres.proto");
    for kind in [
        "postgres.project.create",
        "postgres.project.delete",
        "postgres.project.upgrade",
        "postgres.branch.create",
        "postgres.branch.delete",
        "postgres.branch.restore",
        "postgres.endpoint.start",
        "postgres.endpoint.suspend",
        "postgres.endpoint.restart",
        "postgres.endpoint.delete",
    ] {
        assert!(
            text.contains(&format!("`{kind}`")),
            "the proto does not list {kind}"
        );
    }
    // ...and those RPCs answer an Operation.
    let set = descriptor_set();
    let msgs = messages(&set);
    for rpc in [
        "CreateProject",
        "DeleteProject",
        "UpgradeProject",
        "CreateBranch",
        "DeleteBranch",
        "RestoreBranch",
        "StartEndpoint",
        "SuspendEndpoint",
        "RestartEndpoint",
        "DeleteEndpoint",
    ] {
        let m = methods(&set).into_iter().find(|m| m.name() == rpc).unwrap();
        let out = &msgs[m.output_type()];
        assert!(
            out.field
                .iter()
                .any(|f| f.type_name() == ".loams.operations.v1.Operation"),
            "{rpc}'s {} has no Operation",
            out.name()
        );
    }
}

/// The route map's planned `loams.postgres.v1` section (§46 §4.2): every
/// row names an RPC of the service, and every RPC has a row.
#[test]
fn every_rpc_has_a_planned_console_route() {
    let text = read("docs/api/route-map.md");
    let start = text
        .find("## Planned: `loams.postgres.v1`")
        .expect("docs/api/route-map.md has the planned loams.postgres.v1 section");
    let end = text[start + 1..]
        .find("\n## ")
        .map_or(text.len(), |e| start + 1 + e);
    let prefix = format!("`{PACKAGE}.{SERVICE}/");
    let mut routed = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    let mut rows = 0;
    for line in text[start..end].lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 || !cells[1].starts_with("`/v1/namespaces/{ns}/postgres/") {
            continue;
        }
        rows += 1;
        assert!(
            pairs.insert((cells[1].to_string(), cells[2].to_string())),
            "route {} {} is listed twice",
            cells[2],
            cells[1]
        );
        assert!(
            ["GET", "POST", "PATCH", "DELETE"].contains(&cells[2]),
            "bad method in {line}"
        );
        let rpc = cells[3]
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix('`'))
            .unwrap_or_else(|| panic!("no {prefix}…` RPC in {line}"));
        routed.insert(rpc.to_string());
    }
    let all: BTreeSet<String> = CONTRACT.iter().map(|(n, _, _)| n.to_string()).collect();
    let unknown: Vec<_> = routed.difference(&all).collect();
    let unrouted: Vec<_> = all.difference(&routed).collect();
    assert!(
        unknown.is_empty(),
        "routes to RPCs the service lacks: {unknown:?}"
    );
    assert!(
        unrouted.is_empty(),
        "RPCs with no planned route: {unrouted:?}"
    );
    assert!(rows >= all.len());
}

fn request_of(set: &FileDescriptorSet, rpc: &str) -> DescriptorProto {
    let m = methods(set).into_iter().find(|m| m.name() == rpc).unwrap();
    messages(set)[m.input_type()].clone()
}

fn response_of(set: &FileDescriptorSet, rpc: &str) -> DescriptorProto {
    let m = methods(set).into_iter().find(|m| m.name() == rpc).unwrap();
    messages(set)[m.output_type()].clone()
}

fn field<'a>(m: &'a DescriptorProto, name: &str) -> &'a prost_types::FieldDescriptorProto {
    m.field
        .iter()
        .find(|f| f.name() == name)
        .unwrap_or_else(|| panic!("{} has no {name}", m.name()))
}

/// AP0 pagination: every `List*` takes `int32 page_size` and `string
/// page_token`, and answers one repeated resource and `string
/// next_page_token`.
#[test]
fn list_rpcs_page_as_ap0_says() {
    let set = descriptor_set();
    let lists: Vec<String> = CONTRACT
        .iter()
        .filter(|(n, _, _)| n.starts_with("List"))
        .map(|(n, _, _)| n.to_string())
        .collect();
    assert_eq!(lists.len(), 5);
    for rpc in &lists {
        let req = request_of(&set, rpc);
        assert_eq!(field(&req, "page_size").r#type(), Type::Int32, "{rpc}");
        assert_eq!(field(&req, "page_token").r#type(), Type::String, "{rpc}");
        let resp = response_of(&set, rpc);
        assert_eq!(
            field(&resp, "next_page_token").r#type(),
            Type::String,
            "{rpc}"
        );
        let repeated: Vec<_> = resp
            .field
            .iter()
            .filter(|f| f.label() == Label::Repeated && f.r#type() == Type::Message)
            .collect();
        assert_eq!(repeated.len(), 1, "{rpc}: one repeated resource");
    }
}

/// `optional uint64 expected_version`.
fn assert_expected_version(m: &DescriptorProto, rpc: &str) {
    let v = field(m, "expected_version");
    assert_eq!(v.r#type(), Type::Uint64, "{rpc}");
    assert!(
        v.proto3_optional(),
        "{rpc}: `optional`, so 0 is not a version"
    );
}

/// AP0 updates: every `Update*` takes the resource, a `FieldMask
/// update_mask` and an `optional uint64 expected_version`.
#[test]
fn update_rpcs_take_a_mask_and_a_version() {
    let set = descriptor_set();
    for (rpc, resource) in [
        ("UpdateProject", ".loams.postgres.v1.Project"),
        ("UpdateBranch", ".loams.postgres.v1.Branch"),
        ("UpdateEndpoint", ".loams.postgres.v1.Endpoint"),
    ] {
        let req = request_of(&set, rpc);
        assert!(
            req.field.iter().any(|f| f.type_name() == resource),
            "{rpc} carries no {resource}"
        );
        assert_eq!(
            field(&req, "update_mask").type_name(),
            ".google.protobuf.FieldMask",
            "{rpc}"
        );
        assert_expected_version(&req, rpc);
    }
}

/// The deletes of versioned resources, and `SetDefaultBranch` (the
/// project's version), can be made conditional.
#[test]
fn deletes_and_set_default_take_an_expected_version() {
    let set = descriptor_set();
    for rpc in [
        "DeleteProject",
        "DeleteBranch",
        "DeleteEndpoint",
        "SetDefaultBranch",
    ] {
        assert_expected_version(&request_of(&set, rpc), rpc);
    }
}

/// One addressing style: every request names its namespace in a top-level
/// `string namespace = 1`.
#[test]
fn every_request_has_a_top_level_namespace() {
    let set = descriptor_set();
    for (rpc, _, _) in CONTRACT {
        let req = request_of(&set, rpc);
        let ns = req
            .field
            .iter()
            .find(|f| f.number() == 1)
            .unwrap_or_else(|| panic!("{rpc}: no field 1"));
        assert_eq!(ns.name(), "namespace", "{rpc}");
        assert_eq!(ns.r#type(), Type::String, "{rpc}");
    }
}

/// `Removed.kind` is an enum, not a free string.
#[test]
fn removed_kind_is_an_enum() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let kind = field(&msgs[".loams.postgres.v1.Removed"], "kind");
    assert_eq!(kind.r#type(), Type::Enum);
    assert_eq!(kind.type_name(), ".loams.postgres.v1.ResourceKind");
}
