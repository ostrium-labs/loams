//! The generated Go facade, over the real `proto/` tree (SDK1 Task 3's
//! `golden_go`).
//!
//! `sdks/go/` belongs to SDK2 Task 2, so this pins the renderer against a
//! golden file the generator owns rather than against a committed SDK file;
//! `scripts/sdk/drift.sh` takes over that comparison once the SDK lands. See
//! `golden_python.rs` for the same argument in full.
//!
//! The Go import paths in the golden file are not a guess: they were produced
//! by running `protoc-gen-go` 1.36.11 and `protoc-gen-connect-go` 1.21.0 with
//! buf's managed-mode `go_package_prefix` of `loams.dev/go/gen`, which is what
//! `sdks/go/buf.gen.proto.yaml` configures. `protoc-gen-connect-go` writes its
//! stubs into a `v1connect` sub-package beside the messages, and names the
//! client interface `<Service>Client`; the facade below is written to those
//! names.

use std::path::PathBuf;

use loams_facade_gen::{Model, Options, go, model_from_request, reasons};

mod protoreq;

const PROTO_REV: &str = "v1";

/// The proto packages' generated types, as Go import paths. It mirrors the
/// `package=` parameters in `sdks/go/buf.gen.yaml`.
const PACKAGE_PARAMETERS: &[&str] = &[
    "loams.instance.v1=loams.dev/go/gen/loams/instance/v1",
    "loams.devices.v1=loams.dev/go/gen/loams/devices/v1",
    "loams.approvals.v1=loams.dev/go/gen/loams/approvals/v1",
    "loams.operations.v1=loams.dev/go/gen/loams/operations/v1",
    "loams.notifications.v1=loams.dev/go/gen/loams/notifications/v1",
    "loams.live.v1=loams.dev/go/gen/loams/live/v1",
];

/// SDK1 Task 3's `golden_go`.
#[test]
fn golden_go() {
    let rendered = render();
    let expected = golden("go.golden");
    assert_eq!(
        rendered,
        expected,
        "{} is stale: regenerate it with UPDATE_GOLDEN=1 cargo test -p loams-facade-gen",
        expected_path().display(),
    );
}

#[test]
fn the_package_map_matches_the_buf_template() {
    let template_path = root().join("sdks/go/buf.gen.yaml");
    let template = std::fs::read_to_string(&template_path).expect("sdks/go/buf.gen.yaml");
    for parameter in PACKAGE_PARAMETERS {
        assert!(
            template.contains(&format!("- package={parameter}")),
            "{parameter} is not in {}",
            template_path.display()
        );
    }
}

/// The Go module is `loams.dev/go` (§44 §9), which is a vanity path: the import
/// path has to be the one the generated code uses and the one Q607's
/// `go-import` meta names, and `scripts/sdk/check-pins.sh` fails if they drift.
#[test]
fn the_import_paths_are_under_the_vanity_module() {
    for parameter in PACKAGE_PARAMETERS {
        let (_, path) = parameter.split_once('=').expect("pkg=path");
        assert!(
            path.starts_with("loams.dev/go/"),
            "{path} is not under the module loams.dev/go"
        );
    }
    let rendered = render();
    assert!(
        rendered.contains("// Module: loams.dev/go"),
        "the generated file does not name the module it belongs to"
    );
}

/// Method names are the proto names in Go's case (`PascalCase`, design §44
/// §7.1). `loams.instance.v1.InstanceService/GetInstance` is annotated
/// `name: "getInstance"`, so the Go name is `GetInstance` and the binding table
/// keeps `getInstance` as the name the annotation gave.
#[test]
fn method_names_are_pascal_case() {
    let rendered = render();
    assert!(
        rendered.contains(
            "GetInstance(ctx context.Context, req *connect.Request[instancev1.GetInstanceRequest], opts *CallOptions) (*connect.Response[instancev1.GetInstanceResponse], error)"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("ProtoName: \"getInstance\""),
        "{rendered}"
    );
}

/// A server stream is a `*Stream` the caller receives from, which is Go's
/// idiom for a server stream (design §44 §7.1).
#[test]
fn a_server_stream_returns_a_stream() {
    let rendered = render();
    assert!(
        rendered.contains("*connect.ServerStreamForClient[livev1.Transition]"),
        "the Watch signature is not a stream:\n{rendered}"
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
    let reasons = reasons::read(&root().join("docs/api/reasons.md")).expect("the reason registry");
    model_from_request(protoreq::code_generator_request(), reasons).expect("the descriptors")
}

fn render() -> String {
    go::render(&model(), &packages(), PROTO_REV).expect("the facade renders")
}

fn root() -> PathBuf {
    protoreq::root()
}

fn expected_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/go.golden")
}

fn golden(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let rendered = render();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("tests/golden")).expect("tests/golden");
        std::fs::write(&path, &rendered).expect("write the golden file");
    }
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}
