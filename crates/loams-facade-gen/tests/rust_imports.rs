//! The rendered Rust facade's imports: every type its signatures name but do
//! not define must resolve (D641).
//!
//! The renderer emitted a file whose header said it was generated and whose
//! traits named `CallOptions`, `LoamsError` and `ResponseStream` with **no
//! `use` for any of them**. That file did not compile: 15 errors, one per use,
//! and the SDK's whole test suite was unreachable behind them. Nothing in the
//! generator's tests noticed, because the golden file recorded the broken
//! output faithfully — a golden pins what the renderer does, not whether it is
//! right.
//!
//! So this asserts the property the golden cannot: that every name a signature
//! uses is either defined in the file or brought into scope by a `use`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use loams_facade_gen::{Model, Options, model_from_request, reasons, rust};

mod protoreq;

const PROTO_REV: &str = "v1";

/// Mirrors `sdks/rust/buf.gen.yaml`.
const PACKAGE_PARAMETERS: &[&str] = &[
    "loams.instance.v1=loams_proto::loams::instance::v1",
    "loams.devices.v1=loams_proto::loams::devices::v1",
    "loams.approvals.v1=loams_proto::loams::approvals::v1",
    "loams.operations.v1=loams_proto::loams::operations::v1",
    "loams.notifications.v1=loams_proto::loams::notifications::v1",
    "loams.live.v1=loams_live_proto::loams::live::v1",
];

/// The three types the generated traits name that the runtime defines. They are
/// the hand-written half of §44 §7.3's split, and the generated file has to
/// import every one of them.
const RUNTIME_TYPES: &[&str] = &["CallOptions", "LoamsError", "ResponseStream"];

/// Every runtime type a signature names is imported by a `use`.
///
/// A bare `use crate::error::LoamsError;` and a fully-qualified
/// `crate::error::LoamsError` in each signature both resolve; what does not
/// resolve is naming the type with neither. This asserts the first form is
/// emitted, and that it is emitted for all three.
#[test]
fn every_runtime_type_the_traits_name_is_imported() {
    let rendered = render();
    for name in RUNTIME_TYPES {
        assert!(
            is_imported(&rendered, name),
            "the rendered facade names {name} but imports nothing for it, so the file \
             does not compile. Its imports are:\n{}",
            rendered
                .lines()
                .filter(|line| line.trim_start().starts_with("use "))
                .collect::<Vec<_>>()
                .join("\n")
        );
        // And it is actually used, so the import is not itself dead.
        assert!(
            uses_beyond_the_import(&rendered, name),
            "{name} is imported but never named in a signature, so the import is dead \
             and the type is not the one the traits want"
        );
    }
}

/// The imports come from the crate's own modules: `CallOptions` and
/// `ResponseStream` are the runtime's, beside the file, and `LoamsError` is the
/// SDK's error type. An import of a nonexistent module is as broken as no
/// import, and the golden cannot tell the two apart.
#[test]
fn the_imports_come_from_the_sdk_crate() {
    let rendered = render();
    let uses: Vec<&str> = rendered
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("use "))
        .collect();
    assert!(
        uses.iter().any(|line| line.contains("crate::")),
        "no import comes from the SDK crate itself, so the runtime's types cannot \
         resolve. Imports: {uses:?}"
    );
    // Every imported module must be a module the SDK crate actually declares.
    // `sdks/rust/src/lib.rs` is the list; a generated import of a module that
    // does not exist is E0433 the moment the trait is implemented.
    let lib =
        std::fs::read_to_string(root().join("sdks/rust/src/lib.rs")).expect("sdks/rust/src/lib.rs");
    for line in &uses {
        let Some(rest) = line.strip_prefix("use ").map(|r| r.trim_end_matches(';')) else {
            continue;
        };
        let Some(module) = rest.strip_prefix("crate::") else {
            continue;
        };
        let module = module.split("::").next().unwrap_or(module);
        assert!(
            lib.contains(&format!("pub mod {module};")),
            "the facade imports crate::{module}, which sdks/rust/src/lib.rs does not \
             declare: {line}"
        );
    }
}

/// Anti-vacuity. The assertions above pass on a file that names the three types
/// with a `use`; they must fail on the file that shipped, which named them with
/// no `use` at all. This drives the same predicate over a planted render with
/// its imports removed.
#[test]
fn the_import_guard_rejects_a_facade_with_no_imports() {
    let rendered = render();
    assert!(
        is_imported(&rendered, "LoamsError"),
        "the real render does not import LoamsError, so this test's premise is wrong"
    );
    // Strip every `use` line: exactly the defect that shipped.
    let stripped: String = rendered
        .lines()
        .filter(|line| !line.trim_start().starts_with("use "))
        .collect::<Vec<_>>()
        .join("\n");
    for name in RUNTIME_TYPES {
        assert!(
            !is_imported(&stripped, name),
            "a facade whose imports were all removed still counts as importing {name}; \
             the guard cannot fail, so it is worth nothing"
        );
        // And the type is still *named*, which is why it did not compile.
        assert!(
            stripped.contains(name),
            "the planted defect should still name {name} in a signature; the strip removed \
             too much, so this test is not exercising the shipped failure"
        );
    }
}

/// Whether a `use` brings `name` into scope: `use a::b::Name;`,
/// `use a::b::Name as N;` and `use a::b::{Name, Other};` all count.
fn is_imported(rendered: &str, name: &str) -> bool {
    rendered.lines().any(|line| {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("use ") else {
            return false;
        };
        let rest = rest
            .trim_end_matches(';')
            .trim()
            .trim_start_matches("pub ")
            .trim();
        rest.split([',', '{', '}'])
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .any(|item| {
                let leaf = item.rsplit("::").next().unwrap_or(item).trim();
                leaf == name || leaf.split(" as ").next().is_some_and(|n| n.trim() == name)
            })
    })
}

/// Whether `name` is used somewhere other than in a `use` line.
fn uses_beyond_the_import(rendered: &str, name: &str) -> bool {
    rendered
        .lines()
        .filter(|line| !line.trim_start().starts_with("use "))
        .any(|line| line.contains(name))
}

/// Every type named **bare** in a trait signature is either defined in this file
/// or imported. A name written as `a::b::Name` resolves through its own path and
/// needs no import; a bare `Name` needs one.
///
/// Stated over the whole file rather than over the three names in
/// `RUNTIME_TYPES`, so a fourth runtime type the renderer starts naming without
/// importing fails here without anyone adding it to a list.
#[test]
fn no_signature_names_an_unresolved_type() {
    let rendered = render();
    let bare = bare_names_in_traits(&rendered);
    assert!(
        bare.len() >= 3,
        "only {} bare type names were found in the traits ({bare:?}); the scan is \
         broken, and a scan that finds nothing passes vacuously",
        bare.len()
    );
    let defined = defined_types(&rendered);
    for name in &bare {
        if defined.contains(name) || PRELUDE.contains(&name.as_str()) {
            continue;
        }
        assert!(
            is_imported(&rendered, name),
            "the facade's traits name {name} bare, which it neither defines nor imports, \
             so the file does not compile"
        );
    }
}

/// Names a trait body may use bare because they resolve without an import.
///
/// `Option` and `Result` are in the prelude. `Send` is an auto trait the
/// renderer writes into every return type (`-> impl Future<…> + Send`); it is
/// prelude too, and needs no `use` any more than `Option` does. `Self` is the
/// implementing type. Everything else bare must come from a `use` or from this
/// file.
const PRELUDE: &[&str] = &["Option", "Result", "Send", "Sync", "Self"];

/// The capitalised names a trait body uses **without** a `::` qualifier.
///
/// The scan is over trait bodies rather than over lines beginning with `fn`,
/// because the renderer wraps a signature across several lines — `fn name(`,
/// then its parameters, then `) -> impl Future<…>` — and a line-oriented scan
/// that looks for `fn ` alone finds the name only in the return type. That bug
/// made this test pass on the very file it was written for, which is why the
/// name count is asserted too.
fn bare_names_in_traits(rendered: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_trait = false;
    for line in rendered.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub trait ") {
            in_trait = true;
            continue;
        }
        if trimmed == "}" {
            in_trait = false;
            continue;
        }
        if !in_trait || trimmed.starts_with("///") || trimmed.starts_with("//") {
            continue;
        }
        // An associated const is a value, not a type: `const MODULE: &str`
        // names nothing this file has to import. Skip this line and stay in the
        // trait, because the `fn`s the test is actually about follow it.
        if trimmed.starts_with("const ") {
            continue;
        }
        // A name is bare only when nothing qualifies it. `::` is what
        // qualifies, so a word counts as bare unless the two characters before
        // it are `::`.
        //
        // The earlier scan split the line on `::` and treated every segment but
        // the last as qualified, which flagged the **final** segment of every
        // qualified path: `a::b::Name` reported `Name` as bare even though it
        // resolves through its own path. That made this test fail on the very
        // shape the doc comment above says is fine.
        //
        // Text inside a string literal is not a type either: the renderer
        // writes `const SERVICE: &str = "loams.instance.v1.InstanceService"`,
        // and that names the service in a value, not a path this file resolves.
        // Strip the literals before scanning, or every fully-qualified rpc name
        // the facade carries is reported as a bare type.
        let code = strip_string_literals(trimmed);
        for (start, word) in identifier_spans(&code) {
            if code[..start].ends_with("::") {
                continue;
            }
            // An associated-type binding is a name on the left of `=`, not a
            // type: `Future<Output = Result<T>>` names no type `Output`, and
            // the facade neither defines nor imports one.
            if code[start + word.len()..].trim_start().starts_with('=') {
                continue;
            }
            if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                names.insert(word);
            }
        }
    }
    names
}

/// The scan finds exactly the names it is meant to: the ones a trait body
/// spells bare that neither resolve nor are defined here.
///
/// Each rule below was added because the scan reported a false positive without
/// it, and each of those names is one the renderer legitimately emits — so a
/// scan that ignored them could not tell a broken facade from a good one. They
/// are listed here so the reasoning survives the next edit:
///
/// - `a::b::Name` is **qualified**, including the last segment. Only the
///   characters immediately before a word decide, not its position in a split.
/// - `"..."` is a **value**. The renderer writes fully-qualified rpc and
///   service names as strings; they are not paths this file resolves.
/// - `const NAME: T` is a **binding**, not a type. The facade declares
///   `MODULE` and `SERVICE` per module and imports neither.
/// - `Future<Output = …>` is an **associated-type binding**. `Output` is a name
///   on the left of `=`, not a type, and no facade defines or imports one.
/// - `Send` is an **auto trait** in the prelude, written into every return
///   type. It resolves without an import, exactly as `Option` and `Result` do.
///
/// The negative case is what matters: a trait body that really does name an
/// unimported, undefined type is still reported, and `CallOptions`,
/// `LoamsError` and `ResponseStream` — the three the renderer once emitted
/// without a `use` — are still caught. `the_scan_still_finds_a_genuine_bare_type`
/// below asserts that directly, because a scan that finds nothing passes
/// vacuously and that is how the original defect reached a release.
#[test]
fn the_scan_still_finds_a_genuine_bare_type() {
    let facade = "\
pub trait DemoModule {
    const MODULE: &'static str = \"demo\";
    fn call(
        &self,
        request: DemoRequest,
    ) -> impl std::future::Future<Output = Result<DemoResponse, LoamsError>> + Send;
}
";
    let bare = bare_names_in_traits(facade);
    // The genuine defect: `DemoRequest` and `DemoResponse` are bare, undefined
    // and unimported. Everything else on the line is one of the exclusions.
    assert!(bare.contains("DemoRequest"), "{bare:?}");
    assert!(bare.contains("DemoResponse"), "{bare:?}");
    // `MODULE`, `Output` and `Future` are excluded by the scan itself: the first
    // is an associated const, the second an associated-type binding, the third
    // qualified. `Send`, `Result` and `LoamsError` are still *found* — they are
    // filtered by `PRELUDE` at the call site, which is what this next line
    // reproduces for the three that resolve without an import.
    for excluded in ["Output", "Future", "MODULE"] {
        assert!(
            !bare.contains(excluded),
            "{excluded} should not be bare: {bare:?}"
        );
    }
    for present in ["Result", "Send", "LoamsError"] {
        assert!(
            bare.contains(present),
            "{present} appears bare in the fixture, so the scan must find it \
             for the call site to filter: {bare:?}"
        );
    }
}

/// The line with every `"..."` literal blanked out, so a word inside one is
/// never read as code. Escaped quotes do not end the literal, and an unclosed
/// quote runs to the end of the line rather than panicking on a hand-written
/// fixture that is not valid Rust.
fn strip_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push('"');
            continue;
        }
        out.push(c);
    }
    out
}

/// The identifier-shaped words in a string, each with the byte offset it starts
/// at, so the caller can look at the characters before it. `identifier_words`
/// alone cannot tell `Name` from the `Name` of `a::b::Name`.
fn identifier_spans(text: &str) -> Vec<(usize, String)> {
    let mut spans = Vec::new();
    let mut word = String::new();
    let mut start = 0;
    for (offset, c) in text.char_indices() {
        if c.is_ascii_alphanumeric() || c == '_' {
            if word.is_empty() {
                start = offset;
            }
            word.push(c);
        } else if !word.is_empty() {
            spans.push((start, std::mem::take(&mut word)));
        }
    }
    if !word.is_empty() {
        spans.push((start, word));
    }
    spans
}

/// The identifier-shaped words in a string.
#[allow(dead_code)]
fn identifier_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    words
}

/// The types the file declares itself: every `enum`, `struct`, `type` and `trait`
/// it defines.
fn defined_types(rendered: &str) -> BTreeSet<String> {
    let mut defined = BTreeSet::new();
    for line in rendered.lines() {
        let line = line.trim();
        for keyword in ["pub enum ", "pub struct ", "pub trait ", "pub type "] {
            if let Some(rest) = line.strip_prefix(keyword) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    defined.insert(name);
                }
            }
        }
    }
    defined
}

fn packages() -> loams_facade_gen::PackageMap {
    let mut map = Options::default().packages;
    for parameter in PACKAGE_PARAMETERS {
        map.insert(parameter).expect("a package parameter");
    }
    map
}

fn model() -> Model {
    let reasons = reasons::read(&root().join("docs/api/reasons.md")).expect("the reason registry");
    model_from_request(protoreq::code_generator_request(), reasons).expect("the descriptors")
}

fn render() -> String {
    rust::render(&model(), &packages(), PROTO_REV).expect("the facade renders")
}

fn root() -> PathBuf {
    protoreq::root()
}
