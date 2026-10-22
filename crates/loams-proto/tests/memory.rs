//! `loams.memory.v1` (MM1 Task 1; design §52 §9.1, D884): the descriptor
//! carries both services complete, every mutation that is not idempotent by
//! construction takes an `idempotency_key`, no name is a metering name, every
//! reason the design names is registered, `SearchRequest.spaces` documents its
//! 1..8 cap, the package is unstable in the proto and in `buf.yaml` alike, and
//! every RPC has a planned route.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use prost::Message;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::method_options::IdempotencyLevel;
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    MethodDescriptorProto,
};

const PACKAGE: &str = "loams.memory.v1";
const PROTO: &str = "proto/loams/memory/v1/memory.proto";

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

/// The files of `loams.memory.v1`.
fn files(set: &FileDescriptorSet) -> Vec<&FileDescriptorProto> {
    let files: Vec<_> = set.file.iter().filter(|f| f.package() == PACKAGE).collect();
    assert!(
        !files.is_empty(),
        "{PACKAGE} is not in loams-proto's descriptor set (crates/loams-proto/build.rs FILES)"
    );
    files
}

/// The methods of one service of the package.
fn methods_of(set: &FileDescriptorSet, service: &str) -> Vec<MethodDescriptorProto> {
    files(set)
        .into_iter()
        .flat_map(|f| f.service.iter())
        .find(|s| s.name() == service)
        .unwrap_or_else(|| panic!("no {service} in {PACKAGE}"))
        .method
        .clone()
}

/// Every method of both services, with its service's name.
fn methods(set: &FileDescriptorSet) -> Vec<(String, MethodDescriptorProto)> {
    SERVICES
        .iter()
        .flat_map(|(service, _)| {
            methods_of(set, service)
                .into_iter()
                .map(move |m| (service.to_string(), m))
        })
        .collect()
}

/// Every message of the descriptor set by its fully qualified name, nested
/// ones included (`.loams.memory.v1.Outer.Inner`).
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

/// The package's own messages and enums, nested ones included.
fn package_types(set: &FileDescriptorSet) -> (Vec<DescriptorProto>, Vec<EnumDescriptorProto>) {
    fn walk(
        m: &DescriptorProto,
        msgs: &mut Vec<DescriptorProto>,
        enums: &mut Vec<EnumDescriptorProto>,
    ) {
        enums.extend(m.enum_type.iter().cloned());
        for nested in &m.nested_type {
            walk(nested, msgs, enums);
        }
        msgs.push(m.clone());
    }
    let mut msgs = Vec::new();
    let mut enums = Vec::new();
    for f in files(set) {
        enums.extend(f.enum_type.iter().cloned());
        for m in &f.message_type {
            walk(m, &mut msgs, &mut enums);
        }
    }
    (msgs, enums)
}

fn is_read_only(m: &MethodDescriptorProto) -> bool {
    m.options
        .as_ref()
        .is_some_and(|o| o.idempotency_level() == IdempotencyLevel::NoSideEffects)
}

/// An RPC: its name, whether it reads only, and whether it streams.
type Rpc = (&'static str, bool, bool);

/// The RPCs of §52 §9.1, per service, each with whether it reads only
/// (`NO_SIDE_EFFECTS`) and whether it streams. `CodeMemoryService` is
/// `code.proto`'s, MM1 Task 27.
const SERVICES: &[(&str, &[Rpc])] = &[
    (
        "MemoryAdminService",
        &[
            ("CreateStore", false, false),
            ("GetStore", true, false),
            ("ListStores", true, false),
            ("UpdateStore", false, false),
            ("DeleteStore", false, false),
            ("CreateSpace", false, false),
            ("GetSpace", true, false),
            ("ListSpaces", true, false),
            ("UpdateSpace", false, false),
            ("ExportSpace", false, false),
            ("ImportSpace", false, false),
            ("ConfigureMirror", false, false),
            ("GetErasure", true, false),
        ],
    ),
    (
        "MemoryService",
        &[
            ("Add", false, false),
            ("AddBatch", false, false),
            ("Search", true, false),
            ("GetProfile", true, false),
            ("GetFact", true, false),
            ("ListFacts", true, false),
            ("UpdateFact", false, false),
            ("InvalidateFact", false, false),
            ("PinFact", false, false),
            ("History", true, false),
            ("GetEntity", true, false),
            ("ListEntities", true, false),
            ("ListEpisodes", true, false),
            ("RetryExtraction", false, false),
            ("Forget", false, false),
            ("GetBlock", true, false),
            ("ListBlocks", true, false),
            ("UpdateBlock", false, false),
            ("Consolidate", false, false),
            ("ListDreamRuns", true, false),
            ("RevertDreamRun", false, false),
            ("WatchSpace", true, true),
        ],
    ),
];

/// The mutations that take no `idempotency_key` because they are idempotent
/// by construction (API1 ruling 2.4; MM1 ruling R1.2): a create keyed by
/// name, or a call that sets state, so a repeat is the same call.
const IDEMPOTENT_BY_CONSTRUCTION: &[&str] = &[
    // Keyed by (kind, external_id), unique per store.
    "CreateSpace",
    // Field-mask updates: a repeat sets the same fields again.
    "UpdateStore",
    "UpdateSpace",
    "ConfigureMirror",
    // Set a fact's state: `invalid_at`, `pinned`.
    "InvalidateFact",
    "PinFact",
    // Compare-and-swap on `if_version`: a repeat is refused, never applied
    // twice (`memory_block_version_mismatch`).
    "UpdateBlock",
];

/// Both services are §52 §9.1's, RPC for RPC: names, read-only marks and
/// streaming.
#[test]
fn the_services_are_the_design_contract() {
    let set = descriptor_set();
    for (service, contract) in SERVICES {
        let got: BTreeMap<String, (bool, bool)> = methods_of(&set, service)
            .iter()
            .map(|m| {
                (
                    m.name().to_string(),
                    (
                        is_read_only(m),
                        m.server_streaming() || m.client_streaming(),
                    ),
                )
            })
            .collect();
        let want: BTreeMap<String, (bool, bool)> = contract
            .iter()
            .map(|(n, r, s)| (n.to_string(), (*r, *s)))
            .collect();
        assert_eq!(got, want, "{service}");
    }
    // Server streaming only (D420).
    for (_, m) in methods(&set) {
        assert!(
            !m.client_streaming(),
            "{} streams from the client",
            m.name()
        );
    }
}

/// Every RPC that is not `NO_SIDE_EFFECTS` takes `string idempotency_key =
/// 15`, except the ones idempotent by construction, which take none.
#[test]
fn every_mutation_has_idempotency_key() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let mut keyed = 0;
    let mut exempt = BTreeSet::new();
    for (_, m) in methods(&set) {
        let input = &msgs[m.input_type()];
        let field = input.field.iter().find(|f| f.number() == 15);
        if is_read_only(&m) {
            assert!(
                field.is_none(),
                "{}: a read takes no idempotency_key",
                m.name()
            );
            continue;
        }
        if IDEMPOTENT_BY_CONSTRUCTION.contains(&m.name()) {
            assert!(
                input.field.iter().all(|f| f.name() != "idempotency_key"),
                "{}: idempotent by construction, so no idempotency_key",
                m.name()
            );
            exempt.insert(m.name().to_string());
            continue;
        }
        let field =
            field.unwrap_or_else(|| panic!("{}: {} has no field 15", m.name(), input.name()));
        assert_eq!(field.name(), "idempotency_key", "{}", m.name());
        assert_eq!(field.r#type(), Type::String, "{}", m.name());
        assert_eq!(field.label(), Label::Optional, "{}: not repeated", m.name());
        assert!(
            !field.proto3_optional(),
            "{}: a plain proto3 string, not `optional`",
            m.name()
        );
        keyed += 1;
    }
    let want: BTreeSet<String> = IDEMPOTENT_BY_CONSTRUCTION
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(exempt, want, "the exempt list names RPCs that do not exist");
    // CreateStore, DeleteStore, ExportSpace, ImportSpace, Add, AddBatch,
    // UpdateFact, RetryExtraction, Forget, Consolidate, RevertDreamRun.
    assert_eq!(keyed, 11);
}

/// The words of a name: `snake_case` split on `_`, `CamelCase` on its
/// capitals.
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in name.chars() {
        if c == '_' || c == '.' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_ascii_uppercase() && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// No RPC, message, field, enum or enum value of the package is named for
/// metering (API2's rule, D220, D552; §52 §9.1): model work is
/// `ModelCallStats`.
#[test]
fn no_metering_names_in_memory_proto() {
    const BANNED: &[&str] = &["usage", "meter", "plan", "price", "credit", "invoice"];
    let set = descriptor_set();
    let mut names: Vec<String> = Vec::new();
    for (service, _) in SERVICES {
        names.push(service.to_string());
    }
    for (_, m) in methods(&set) {
        names.push(m.name().to_string());
    }
    let (msgs, enums) = package_types(&set);
    for m in &msgs {
        names.push(m.name().to_string());
        names.extend(m.field.iter().map(|f| f.name().to_string()));
        names.extend(m.oneof_decl.iter().map(|o| o.name().to_string()));
    }
    for e in &enums {
        names.push(e.name().to_string());
        names.extend(e.value.iter().map(|v| v.name().to_string()));
    }
    assert!(names.len() > 300, "only {} names parsed", names.len());
    for name in &names {
        for word in words(name) {
            for bad in BANNED {
                assert!(!word.starts_with(bad), "{name} is a metering name ({bad})");
            }
        }
    }
    // The guard can fail.
    assert!(words("ListUsageRecords").contains(&"usage".to_string()));
    assert!(words("price_per_call").contains(&"price".to_string()));
    // ...and the stats type the design names exists.
    let stats = msgs.iter().find(|m| m.name() == "ModelCallStats").unwrap();
    let fields: Vec<&str> = stats.field.iter().map(|f| f.name()).collect();
    assert_eq!(fields, ["calls", "input_tokens", "output_tokens"]);
}

/// The reasons §52 §9.1 names: the backticked `snake_case` tokens of its
/// `- **Reasons**` bullet.
fn design_reasons() -> BTreeSet<String> {
    let text = read("docs/design/52-loams-memory.md");
    let start = text.find("### 9.1").expect("§52 has a §9.1");
    let section = &text[start..start + text[start..].find("\n### ").unwrap()];
    let line = section
        .lines()
        .find(|l| l.starts_with("- **Reasons**"))
        .expect("§52 §9.1 has a Reasons bullet");
    let mut out = BTreeSet::new();
    for (i, tok) in line.split('`').enumerate() {
        let snake = !tok.is_empty()
            && tok.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            && tok.contains('_');
        if i % 2 == 1 && snake {
            out.insert(tok.to_string());
        }
    }
    out
}

/// The registry: reason to Connect code.
fn registry() -> BTreeMap<String, String> {
    read("docs/api/reasons.md")
        .lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let reason = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            Some((reason.to_string(), cells.get(2)?.to_string()))
        })
        .collect()
}

/// Every reason §52 §9.1 names, and `memory_not_yet_available` (the plan's
/// "Shared contracts"), is a row of `docs/api/reasons.md`'s registry.
#[test]
fn reasons_registered() {
    let mut reasons = design_reasons();
    assert_eq!(
        reasons.len(),
        21,
        "§52 §9.1's reasons parsed as {reasons:?}"
    );
    for must in [
        "memory_store_not_found",
        "memory_block_version_mismatch",
        "memory_cross_space_write",
        "code_scip_invalid",
    ] {
        assert!(reasons.contains(must), "§52 §9.1 no longer names {must}");
    }
    reasons.insert("memory_not_yet_available".to_string());
    let registry = registry();
    let missing: Vec<_> = reasons
        .iter()
        .filter(|r| !registry.contains_key(*r))
        .collect();
    assert!(
        missing.is_empty(),
        "not in docs/api/reasons.md: {missing:?}"
    );
}

/// Each reason is registered under the Connect code MM1 ruling R1.4 gives
/// it; §52 fixes two of them (`memory_managed_object` in §5.1,
/// `memory_block_version_mismatch` in §6.5).
#[test]
fn reasons_have_their_connect_codes() {
    let registry = registry();
    for (reason, code) in [
        ("memory_store_not_found", "not_found"),
        ("memory_space_not_found", "not_found"),
        ("memory_space_limit", "resource_exhausted"),
        ("memory_managed_object", "failed_precondition"),
        ("memory_block_version_mismatch", "aborted"),
        ("memory_block_too_large", "invalid_argument"),
        ("memory_core_budget_exceeded", "resource_exhausted"),
        ("memory_content_too_large", "invalid_argument"),
        ("memory_secret_detected", "failed_precondition"),
        ("memory_pii_refused", "failed_precondition"),
        ("memory_model_not_configured", "failed_precondition"),
        ("memory_model_not_local", "failed_precondition"),
        ("memory_token_budget_exhausted", "resource_exhausted"),
        ("memory_extraction_failed", "failed_precondition"),
        ("memory_too_many_spaces", "invalid_argument"),
        ("memory_cross_space_write", "invalid_argument"),
        ("memory_not_yet_available", "unimplemented"),
        ("code_repo_not_indexed", "failed_precondition"),
        ("code_index_building", "unavailable"),
        ("code_change_too_large", "invalid_argument"),
        ("code_language_unsupported", "failed_precondition"),
        ("code_scip_invalid", "invalid_argument"),
    ] {
        assert_eq!(
            registry.get(reason).map(String::as_str),
            Some(code),
            "{reason} in docs/api/reasons.md"
        );
    }
}

/// `SearchRequest.spaces`'s comment states the 1..8 cap (§52 §4.2, §13.1),
/// which the facade generator reads into the SDKs' docs.
#[test]
fn search_request_caps_spaces_documented() {
    let text = read(PROTO);
    let start = text
        .find("message SearchRequest {")
        .expect("the proto has a SearchRequest");
    let body = &text[start..start + text[start..].find("\n}").unwrap()];
    let lines: Vec<&str> = body.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("repeated SpaceRef spaces = 3;"))
        .expect("SearchRequest has `repeated SpaceRef spaces = 3`");
    // The field's leading comment: the `//` lines right above it.
    let comment: Vec<&str> = lines[..at]
        .iter()
        .rev()
        .take_while(|l| l.trim_start().starts_with("//"))
        .copied()
        .collect();
    let comment = comment.join(" ");
    assert!(
        comment.contains("1..8"),
        "the spaces comment is {comment:?}"
    );
    // ...and the descriptor agrees on the shape.
    let set = descriptor_set();
    let req = &messages(&set)[".loams.memory.v1.SearchRequest"];
    let spaces = field(req, "spaces");
    assert_eq!(spaces.label(), Label::Repeated);
    assert_eq!(spaces.type_name(), ".loams.memory.v1.SpaceRef");
}

fn field<'a>(m: &'a DescriptorProto, name: &str) -> &'a prost_types::FieldDescriptorProto {
    m.field
        .iter()
        .find(|f| f.name() == name)
        .unwrap_or_else(|| panic!("{} has no {name}", m.name()))
}

/// The three messages §52 §9.1 writes out keep its field numbers.
#[test]
fn design_messages_keep_their_numbers() {
    let set = descriptor_set();
    let msgs = messages(&set);
    for (message, want) in [
        (
            "AddRequest",
            &[
                ("namespace", 1),
                ("store", 2),
                ("space", 3),
                ("text", 4),
                ("messages", 5),
                ("document", 6),
                ("json", 7),
                ("reference_time", 8),
                ("session_ref", 9),
                ("metadata", 10),
                ("trust", 11),
                ("infer", 12),
                ("wait_ms", 13),
                ("idempotency_key", 15),
            ][..],
        ),
        (
            "SearchRequest",
            &[
                ("namespace", 1),
                ("store", 2),
                ("spaces", 3),
                ("query", 4),
                ("include", 5),
                ("as_of", 6),
                ("known_at", 7),
                ("kinds", 8),
                ("filter", 9),
                ("limit", 10),
                ("expand", 11),
                ("rerank", 12),
                ("include_history", 13),
                ("consistency_token", 14),
            ][..],
        ),
        (
            "Fact",
            &[
                ("id", 1),
                ("space_id", 2),
                ("src", 3),
                ("dst", 4),
                ("predicate", 5),
                ("text", 6),
                ("kind", 7),
                ("world", 8),
                ("system", 9),
                ("episodes", 10),
                ("relation", 11),
                ("confidence", 12),
                ("importance", 13),
                ("pinned", 14),
                ("status", 15),
                ("extractor", 16),
            ][..],
        ),
    ] {
        let m = &msgs[&format!(".{PACKAGE}.{message}")];
        for (name, number) in want {
            assert_eq!(field(m, name).number(), *number, "{message}.{name}");
        }
    }
    // The resource types the plan's Task 1 names.
    for name in [
        "Fact",
        "Entity",
        "Episode",
        "Block",
        "Profile",
        "DreamRun",
        "Erasure",
        "Interval",
        "Relation",
        "Extractor",
        "ModelCallStats",
        "Store",
        "Space",
    ] {
        assert!(
            msgs.contains_key(&format!(".{PACKAGE}.{name}")),
            "no message {name}"
        );
    }
}

/// One addressing style: every request names its namespace in a top-level
/// `string namespace = 1`.
#[test]
fn every_request_has_a_top_level_namespace() {
    let set = descriptor_set();
    let msgs = messages(&set);
    for (_, m) in methods(&set) {
        let req = &msgs[m.input_type()];
        let ns = req
            .field
            .iter()
            .find(|f| f.number() == 1)
            .unwrap_or_else(|| panic!("{}: no field 1", m.name()));
        assert_eq!(ns.name(), "namespace", "{}", m.name());
        assert_eq!(ns.r#type(), Type::String, "{}", m.name());
    }
}

/// AIP-158 pagination: every `List*` takes `int32 page_size` and `string
/// page_token`, and answers one repeated resource and `string
/// next_page_token`.
#[test]
fn list_rpcs_page_as_ap0_says() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let mut lists = 0;
    for (_, m) in methods(&set) {
        if !m.name().starts_with("List") {
            continue;
        }
        lists += 1;
        let req = &msgs[m.input_type()];
        assert_eq!(
            field(req, "page_size").r#type(),
            Type::Int32,
            "{}",
            m.name()
        );
        assert_eq!(
            field(req, "page_token").r#type(),
            Type::String,
            "{}",
            m.name()
        );
        let resp = &msgs[m.output_type()];
        assert_eq!(
            field(resp, "next_page_token").r#type(),
            Type::String,
            "{}",
            m.name()
        );
        let repeated = resp
            .field
            .iter()
            .filter(|f| f.label() == Label::Repeated && f.r#type() == Type::Message)
            .count();
        assert_eq!(repeated, 1, "{}: one repeated resource", m.name());
    }
    assert_eq!(lists, 7);
}

/// `WatchSpace` resumes from a cursor and sends a heartbeat (AP0's stream
/// rule).
#[test]
fn watch_space_has_a_cursor_and_a_heartbeat() {
    let set = descriptor_set();
    let msgs = messages(&set);
    let watch = methods_of(&set, "MemoryService")
        .into_iter()
        .find(|m| m.name() == "WatchSpace")
        .unwrap();
    assert_eq!(
        field(&msgs[watch.input_type()], "resume_cursor").r#type(),
        Type::String
    );
    let event = &msgs[watch.output_type()];
    assert_eq!(field(event, "cursor").r#type(), Type::String);
    assert_eq!(
        field(event, "heartbeat").type_name(),
        ".loams.memory.v1.Heartbeat"
    );
}

/// The proto's header lists the `Operation` kind of every RPC that answers
/// one, and those RPCs answer `loams.operations.v1.Operation`.
#[test]
fn operation_kinds_are_listed() {
    let text = read(PROTO);
    for kind in [
        "memory.store.delete",
        "memory.space.move",
        "memory.space.export",
        "memory.space.import",
        "memory.dream.run",
        "memory.dream.revert",
    ] {
        assert!(
            text.contains(&format!("`{kind}`")),
            "the proto does not list {kind}"
        );
    }
    let set = descriptor_set();
    let want: BTreeSet<&str> = [
        "DeleteStore",
        "ExportSpace",
        "ImportSpace",
        "Consolidate",
        "RevertDreamRun",
    ]
    .into();
    let got: BTreeSet<String> = methods(&set)
        .into_iter()
        .filter(|(_, m)| m.output_type() == ".loams.operations.v1.Operation")
        .map(|(_, m)| m.name().to_string())
        .collect();
    assert_eq!(got, want.iter().map(|s| s.to_string()).collect());
}

/// `ModuleOptions.unstable = true` on both services and `buf.yaml`'s
/// `breaking.ignore` lists the package: the option and the ignore line agree
/// (API2's rule, Task 12's `unstable_flag_and_buf_ignore_agree`).
#[test]
fn unstable_flag_and_buf_ignore_agree() {
    let text = read(PROTO);
    for service in ["MemoryAdminService", "MemoryService"] {
        let start = text
            .find(&format!("service {service} {{"))
            .unwrap_or_else(|| panic!("no service {service}"));
        let block = &text[start..start + text[start..].find("\n  };").unwrap()];
        assert!(
            block.contains("option (loams.options.v1.module)"),
            "{service} has no module option"
        );
        assert!(
            block.contains("name: \"memory\""),
            "{service}: module memory"
        );
        assert!(block.contains("unstable: true"), "{service}: unstable");
    }
    let buf = read("buf.yaml");
    let breaking = &buf[buf.find("\nbreaking:").expect("buf.yaml has breaking:")..];
    assert!(
        breaking.lines().any(|l| l.trim() == "- proto/loams/memory"),
        "buf.yaml's breaking.ignore does not list proto/loams/memory"
    );
}

/// The route map's planned `loams.memory.v1` section: every row names an RPC
/// of the two services, and every RPC has a row.
#[test]
fn every_rpc_has_a_planned_route() {
    let text = read("docs/api/route-map.md");
    let start = text
        .find("## Planned: `loams.memory.v1`")
        .expect("docs/api/route-map.md has the planned loams.memory.v1 section");
    let end = text[start + 1..]
        .find("\n## ")
        .map_or(text.len(), |e| start + 1 + e);
    let mut routed = BTreeSet::new();
    for line in text[start..end].lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 || !cells[1].starts_with("`/loams.memory.v1.") {
            continue;
        }
        assert_eq!(
            cells[2], "POST",
            "Connect unary and server streams are POST: {line}"
        );
        let path = cells[1].trim_matches('`');
        let rpc = path.trim_start_matches('/');
        assert_eq!(cells[3], format!("`{rpc}`"), "{line}");
        assert!(routed.insert(rpc.to_string()), "{rpc} is listed twice");
    }
    let all: BTreeSet<String> = SERVICES
        .iter()
        .flat_map(|(service, rpcs)| {
            rpcs.iter()
                .map(move |(rpc, _, _)| format!("{PACKAGE}.{service}/{rpc}"))
        })
        .collect();
    let unknown: Vec<_> = routed.difference(&all).collect();
    let unrouted: Vec<_> = all.difference(&routed).collect();
    assert!(
        unknown.is_empty(),
        "routes to RPCs the services lack: {unknown:?}"
    );
    assert!(
        unrouted.is_empty(),
        "RPCs with no planned route: {unrouted:?}"
    );
}
