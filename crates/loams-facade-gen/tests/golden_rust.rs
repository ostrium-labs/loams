//! The generated Rust facade, over the real `proto/` tree (SDK1 Task 3's
//! `golden_rust`).
//!
//! Two things are unlike the other two languages, and both come from D128 (the
//! Rust SDK is the server's stack):
//!
//! - `tonic` is not used. The stubs come from `connectrpc-build` 0.9.0 in the
//!   SDK crate's own `build.rs`, which is what `crates/loams-proto/build.rs`
//!   already does, so `sdks/rust/buf.gen.yaml` runs the facade plugin alone
//!   and there is no buf remote plugin for Rust at all.
//! - The generated types are Rust modules mirroring the proto package:
//!   `loams_proto::loams::instance::v1`, which is how
//!   `crates/loams-live-proto/tests/roundtrip.rs` imports them today.
//!
//! `sdks/rust/` belongs to SDK2 Task 3, so this pins the renderer against a
//! golden file the generator owns; `scripts/sdk/drift.sh` takes over that
//! comparison once the SDK lands. See `golden_python.rs` for the argument.

use std::path::PathBuf;

use loams_facade_gen::{Model, Options, model_from_request, naming, reasons, rust};

mod protoreq;

const PROTO_REV: &str = "v1";

/// The crate path each proto package's generated types live at. It mirrors the
/// `package=` parameters in `sdks/rust/buf.gen.yaml`.
const PACKAGE_PARAMETERS: &[&str] = &[
    "loams.instance.v1=loams_proto::loams::instance::v1",
    "loams.devices.v1=loams_proto::loams::devices::v1",
    "loams.approvals.v1=loams_proto::loams::approvals::v1",
    "loams.operations.v1=loams_proto::loams::operations::v1",
    "loams.notifications.v1=loams_proto::loams::notifications::v1",
    // `loams.live.v1` is not in `loams_proto`: R1's `loams-live-proto`
    // generates it, and a proto package's Rust types are generated exactly
    // once per workspace (API1 plan ruling 1.2). The map has to say so, which
    // is what `the_package_map_matches_the_buf_template` below checks.
    "loams.live.v1=loams_live_proto::loams::live::v1",
];

/// SDK1 Task 3's `golden_rust`.
#[test]
fn golden_rust() {
    let rendered = render();
    let expected = golden("rust.golden");
    assert_eq!(
        rendered,
        expected,
        "{} is stale: regenerate it with UPDATE_GOLDEN=1 cargo test -p loams-facade-gen",
        expected_path().display(),
    );
}

#[test]
fn the_package_map_matches_the_buf_template() {
    let template_path = root().join("sdks/rust/buf.gen.yaml");
    let template = std::fs::read_to_string(&template_path).expect("sdks/rust/buf.gen.yaml");
    for parameter in PACKAGE_PARAMETERS {
        assert!(
            template.contains(&format!("- package={parameter}")),
            "{parameter} is not in {}",
            template_path.display()
        );
    }
}

/// D128: the Rust SDK is the server's stack, so `tonic` must not appear in its
/// generated surface. `crates/loams/Cargo.toml` still carries `tonic-prost` for
/// the pre-D128 stream service, which is why this is worth a test.
#[test]
fn tonic_is_not_used() {
    let rendered = render();
    assert!(
        !rendered.contains("tonic"),
        "the Rust facade names tonic, which D128 superseded:\n{rendered}"
    );
}

/// Every module is a trait the hand-written runtime implements once, and every
/// call is a method on it with the proto's message types.
#[test]
fn the_module_catalogue_matches_the_model() {
    let model = model();
    let rendered = render();
    for module in &model.modules {
        assert!(
            rendered.contains(&format!(
                "pub trait {}Module",
                naming::type_name(&module.name)
            )),
            "loams.{} has no module trait",
            module.name
        );
        for call in &module.calls {
            let signature = rust::call_signature(call, &packages()).expect("a call signature");
            assert!(
                rendered.contains(&signature),
                "loams.{}.{} is missing: expected {signature}",
                call.module,
                call.name
            );
        }
    }
}

/// A server stream is a `Stream`, which is Rust's native async iteration
/// (design §44 §7.1).
#[test]
fn a_server_stream_is_a_stream() {
    let rendered = render();
    assert!(
        rendered.contains("-> impl std::future::Future<Output = Result<ResponseStream<"),
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
    rust::render(&model(), &packages(), PROTO_REV).expect("the facade renders")
}

fn root() -> PathBuf {
    protoreq::root()
}

fn expected_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/rust.golden")
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
