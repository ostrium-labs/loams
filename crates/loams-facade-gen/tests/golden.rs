//! The generated TypeScript facade, over the real `proto/` tree (SDK1 Task 3's
//! `golden_typescript`).
//!
//! `model.rs` pins the reading of the annotations with hand-written
//! descriptors. This pins the *output*: `protoc` compiles the repository's own
//! protos, the model is built from that descriptor set exactly as buf hands it
//! to the plugin, and the rendered text must equal the committed
//! `facade.ts` byte for byte. A proto edit that changes the SDK surface
//! therefore fails here until the generated file is regenerated and committed,
//! which is the same rule CI's drift check enforces through `buf generate`.

use std::path::{Path, PathBuf};

use loams_facade_gen::{Options, model_from_request, reasons, typescript};

mod protoreq;

/// The proto packages the SDK's message types are imported from. It mirrors
/// the `package=` parameters in `sdks/typescript/buf.gen.yaml`, and
/// `the_package_map_matches_the_buf_template` below fails if the two drift.
const PACKAGE_PARAMETERS: &[&str] = &[
    "loams.instance.v1=@loams/proto/instance",
    "loams.devices.v1=@loams/proto/devices",
    "loams.approvals.v1=@loams/proto/approvals",
    "loams.operations.v1=@loams/proto/operations",
    "loams.notifications.v1=@loams/proto/notifications",
    "loams.errors.v1=@loams/proto/errors",
    "loams.live.v1=@loams/live/live",
];

const PROTO_REV: &str = "v1";

fn root() -> PathBuf {
    protoreq::root()
}

/// The request, built once: `protoc` runs for every test in this file and the
/// descriptor set does not change.
fn code_generator_request() -> &'static [u8] {
    protoreq::code_generator_request()
}

/// The rendered facade, from the repository's own protos and reason registry.
fn render() -> String {
    let reasons = reasons::read(&root().join("docs/api/reasons.md")).expect("the reason registry");
    let model = model_from_request(code_generator_request(), reasons).expect("the descriptors");
    let mut packages = Options::default().packages;
    for parameter in PACKAGE_PARAMETERS {
        packages.insert(parameter).expect("a package parameter");
    }
    typescript::render(&model, &packages, PROTO_REV).expect("the facade renders")
}

/// SDK1 Task 3's `golden_typescript`: the committed generated file is what the
/// generator writes.
#[test]
fn golden_typescript() {
    let path = root().join("sdks/typescript/packages/client/src/gen/facade.ts");
    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let rendered = render();
    assert_eq!(
        rendered,
        committed,
        "{} is stale: run `scripts/sdk/gen.sh typescript` and commit the result",
        path.display()
    );
}

/// The package map is the one thing the golden file cannot check about itself,
/// so the committed template and this test's constants are compared directly:
/// a package added to one and not the other fails generation or fails the
/// typecheck, and this says which.
#[test]
fn the_package_map_matches_the_buf_template() {
    let template = std::fs::read_to_string(root().join("sdks/typescript/buf.gen.yaml"))
        .expect("sdks/typescript/buf.gen.yaml");
    for parameter in PACKAGE_PARAMETERS {
        assert!(
            template.contains(&format!("- package={parameter}")),
            "{parameter} is not in buf.gen.yaml"
        );
    }
}

/// Every module the design's catalogue names for the live package is generated
/// from the annotations: `live` for the session half and `tables` for the
/// table half (§44 §7.2), both facade names for `loams.live.v1.LiveService`.
#[test]
fn the_live_package_splits_into_live_and_tables() {
    let rendered = render();
    for expected in [
        "export interface LiveModule {",
        "export interface TablesModule {",
        "'loams.live.v1.LiveService': LiveService,",
        "derived: true,",
    ] {
        assert!(rendered.contains(expected), "{expected} is missing");
    }
    assert!(
        !rendered.contains("export interface ErrorsModule {"),
        "loams.errors.v1 declares no service"
    );
}

/// The reason registry reaches the SDK as a type, so a caller switching on it
/// is exhaustive and a reason the registry has lost stops compiling.
#[test]
fn the_reason_union_is_generated_from_the_registry() {
    let rendered = render();
    assert!(
        rendered.contains("export type Reason = (typeof REASONS)[number];"),
        "{rendered}"
    );
    assert!(
        rendered.contains("export const FEATURE_NOT_IN_VARIANT = 'feature_not_in_variant' as const satisfies Reason;"),
        "the unavailable-service reason must be in the union"
    );
    let registry =
        std::fs::read_to_string(root().join("docs/api/reasons.md")).expect("the reason registry");
    for reason in loams_facade_gen::reasons::parse(&registry, Path::new("docs/api/reasons.md"))
        .expect("parse")
    {
        assert!(
            rendered.contains(&format!("  '{}',", reason.reason)),
            "{} is in the registry but not in the generated union",
            reason.reason
        );
    }
}
