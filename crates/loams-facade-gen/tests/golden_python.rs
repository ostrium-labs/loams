//! The generated Python facade, over the real `proto/` tree (SDK1 Task 3's
//! `golden_python`).
//!
//! `golden.rs` pins the TypeScript output against the committed
//! `facade.ts`. Python has no committed output yet: `sdks/python/` belongs to
//! SDK2 Task 1, so this file pins the *renderer* against a golden file the
//! generator owns. That is the same assertion one directory over — the
//! generator's bytes for the real protos, compared exactly — and it is what
//! makes `golden_python` a test of the renderer rather than of a file nobody
//! has written yet. `scripts/sdk/drift.sh` compares the committed SDK file
//! against the generator once that file exists.
//!
//! The Python import layout in the golden file is not a guess. It was produced
//! by running the pinned plugins (`protoc-gen-python`, i.e.
//! `buf.build/protocolbuffers/python`, and `protoc-gen-connectrpc` 0.12.1 with
//! `protobuf=google`) over `proto/`, which writes
//! `loams/<pkg path>/<file>_pb2.py` and `loams/<pkg path>/<file>_connect.py`:
//! see `sdks/python/buf.gen.proto.yaml`, which explains why the parameter is
//! load-bearing.

use std::path::{Path, PathBuf};

use loams_facade_gen::{Model, Options, model_from_request, naming, python, reasons};

mod protoreq;

const PROTO_REV: &str = "v1";

/// The proto packages the SDK's message types are imported from. It mirrors the
/// `package=` parameters in `sdks/python/buf.gen.yaml`, and
/// `the_package_map_matches_the_buf_template` fails if the two drift.
const PACKAGE_PARAMETERS: &[&str] = &[
    "loams.instance.v1=loams.instance.v1.instance_pb2",
    "loams.devices.v1=loams.devices.v1.devices_pb2",
    "loams.approvals.v1=loams.approvals.v1.approvals_pb2",
    "loams.operations.v1=loams.operations.v1.operations_pb2",
    "loams.notifications.v1=loams.notifications.v1.notifications_pb2",
    "loams.live.v1=loams.live.v1.live_pb2",
];

/// SDK1 Task 3's `golden_python`.
#[test]
fn golden_python() {
    let rendered = render();
    let expected = golden("python.golden");
    assert_eq!(
        rendered,
        expected,
        "{expected_path} is stale: regenerate it with UPDATE_GOLDEN=1 cargo test -p loams-facade-gen",
        expected_path = expected_path().display(),
    );
}

#[test]
fn the_package_map_matches_the_buf_template() {
    let template_path = root().join("sdks/python/buf.gen.yaml");
    let template = std::fs::read_to_string(&template_path).expect("sdks/python/buf.gen.yaml");
    for parameter in PACKAGE_PARAMETERS {
        assert!(
            template.contains(&format!("- package={parameter}")),
            "{parameter} is not in {}",
            template_path.display()
        );
    }
}

/// Every reason in the registry reaches Python as a `Literal`, so a caller
/// branching on a reason is exhaustive (design §44 §7.4, D611).
#[test]
fn the_reason_literal_is_generated_from_the_registry() {
    let rendered = render();
    assert!(
        rendered.contains("Reason = Literal["),
        "there is no Reason literal in the rendered facade"
    );
    let registry = std::fs::read_to_string(root().join("docs/api/reasons.md")).expect("reasons.md");
    for reason in reasons::parse(&registry, Path::new("docs/api/reasons.md")).expect("parse") {
        assert!(
            rendered.contains(&format!("\"{}\",", reason.reason)),
            "{} is in the registry but not in the generated literal",
            reason.reason
        );
    }
}

/// The Python surface is generated from the same model as the TypeScript one,
/// so the module split and the retry classes cannot differ between them.
#[test]
fn the_module_catalogue_matches_the_model() {
    let model = model();
    let rendered = render();
    for module in &model.modules {
        assert!(
            rendered.contains(&format!("class {}Module(Protocol):", naming::type_name(&module.name))),
            "loams.{} has no module protocol",
            module.name
        );
        for call in &module.calls {
            let signature = python::call_signature(call, &packages()).expect("a call signature");
            assert!(
                rendered.contains(&signature),
                "loams.{}.{} is missing: expected {signature}",
                call.module,
                call.name
            );
        }
    }
}

/// A server stream is an `AsyncIterator` on the async client and an
/// `Iterator` on the sync one (design §44 §7.1: the language's native async
/// iteration).
#[test]
fn a_server_stream_is_an_iterator_in_both_surfaces() {
    let rendered = render();
    assert!(
        rendered.contains("def watch(")
            && rendered.contains("-> AsyncIterator[loams.live.v1.live_pb2.Transition]:"),
        "the async watch signature is wrong:\n{rendered}"
    );
    assert!(
        rendered.contains("-> Iterator[loams.live.v1.live_pb2.Transition]:"),
        "the sync watch signature is wrong:\n{rendered}"
    );
    // A unary call is awaited on the async client and is not on the sync one.
    assert!(
        rendered.contains("async def get_instance("),
        "the async getInstance signature is wrong:\n{rendered}"
    );
}

/// The package map the renderers are given, which the tests also assert against
/// the buf template.
fn packages() -> loams_facade_gen::PackageMap {
    let mut map = Options::default().packages;
    for parameter in PACKAGE_PARAMETERS {
        map.insert(parameter).expect("a package parameter");
    }
    map
}

fn model() -> Model {
    let reasons =
        reasons::read(&root().join("docs/api/reasons.md")).expect("the reason registry");
    model_from_request(protoreq::code_generator_request(), reasons).expect("the descriptors")
}

fn render() -> String {
    python::render(&model(), &packages(), PROTO_REV).expect("the facade renders")
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn expected_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/python.golden")
}

fn golden(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let rendered = render();
    // UPDATE_GOLDEN=1 rewrites the file, which is how a deliberate proto edit
    // is reviewed: the diff in the golden file *is* the SDK surface change.
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("tests/golden")).expect("tests/golden");
        std::fs::write(&path, &rendered).expect("write the golden file");
    }
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}