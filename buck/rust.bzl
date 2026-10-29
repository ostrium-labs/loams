# Buck2 targets for one Cargo package of a Loams workspace.
#
# scripts/buck/gen.py writes each crate's BUCK file as one `cargo_package`
# call from `cargo metadata`; Cargo stays the source of truth. The macro turns
# it into the targets Cargo would build:
#
#   :<package>                  the library (rust_library)
#   :<package>-bin-<bin>        each [[bin]] (rust_binary)
#   :<package>-unittest         the library's #[cfg(test)] tests (rust_test)
#   :<package>-bin-<bin>-unittest
#   :<package>-test-<test>      each tests/*.rs integration test (rust_test)
#   :<package>-example-<name>   each example, built only (rust_binary)
#   :<package>-bench-<name>     each bench, built only (rust_binary)
#   :<package>-build-script-run its build.rs, when it has one
#   :<package>-clippy           every target's clippy diagnostics (see below)
#
# Sources are laid out at their repository paths (crates/foo/src/lib.rs) in a
# symlinked directory, so an include_str!("../../../release/x.toml") that
# leaves the crate still resolves; scripts/buck/gen.py finds those files and
# passes them as `extra_srcs`.
#
# CARGO_MANIFEST_DIR is the crate's path relative to the repository root, not
# an absolute path: compile actions and tests both run from the repository
# root, so env!("CARGO_MANIFEST_DIR") reaches the real checkout, and the baked
# path is the same on every machine (a remote cache hit stays correct).
# toolchains/rust/wrap.sh sets it from LOAMS_MANIFEST_DIR; the prelude would
# otherwise make it an absolute path into buck-out.

load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
load("@prelude//rust:clippy_configuration.bzl", "ClippyConfiguration")
load("@prelude//rust:sources.bzl", "rust_filegroup")

def _manifest_dir_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.symlinked_dir("tree", ctx.attrs.srcs)
    return [DefaultInfo(default_output = out.project(ctx.attrs.manifest_dir))]

# The crate directory a build script runs in: the crate's files, plus the
# files outside it that build.rs reads (../../proto/...), at their paths.
crate_manifest_dir = rule(
    impl = _manifest_dir_impl,
    attrs = {
        "manifest_dir": attrs.string(),
        "srcs": attrs.dict(key = attrs.string(), value = attrs.source(allow_directory = False)),
    },
)

def _clippy_toml_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(default_output = ctx.attrs.src),
        ClippyConfiguration(clippy_toml = ctx.attrs.src),
    ]

# A crate's own clippy.toml, used instead of the workspace's (as Cargo does).
clippy_toml = rule(
    impl = _clippy_toml_impl,
    attrs = {"src": attrs.source()},
)

def _relpath(path, root):
    return path.removeprefix(root + "/") if root else path

def _files(globs, exclude = []):
    return glob(globs, exclude = exclude)

def _srcs_map(package_dir, local, extra):
    # `local`: paths relative to this package; `extra`: {repo path: label}.
    srcs = {}
    for f in local:
        srcs[package_dir + "/" + f if package_dir else f] = f
    for path, label in extra.items():
        srcs[path] = label
    return srcs

def cargo_package(
        name,
        version,
        edition,
        manifest_dir,
        env = {},
        features = [],
        lib = None,
        bins = [],
        tests = [],
        examples = [],
        benches = [],
        deps = [],
        named_deps = {},
        dev_deps = [],
        dev_named_deps = {},
        build_deps = [],
        build_named_deps = {},
        build_script = None,
        build_script_extra_srcs = {},
        extra_srcs = {},
        rustc_flags = [],
        clippy_config = None,
        test_labels = [],
        test_env = {},
        prefix = "",
        visibility = ["PUBLIC"]):
    """One Cargo package. `manifest_dir` is its repository path.

    `prefix` starts every target name: another workspace's build of this
    package as a path dependency (`fabric--loams-cache`), library only.
    """
    pkg = name
    name = prefix + pkg

    base_env = dict(env)
    base_env["LOAMS_MANIFEST_DIR"] = manifest_dir
    base_env["CARGO_PKG_NAME"] = pkg
    base_env["CARGO_PKG_VERSION"] = version
    parts = version.split("-", 1)[0].split(".")
    base_env["CARGO_PKG_VERSION_MAJOR"] = parts[0]
    base_env["CARGO_PKG_VERSION_MINOR"] = parts[1]
    base_env["CARGO_PKG_VERSION_PATCH"] = parts[2]
    base_env["CARGO_PKG_VERSION_PRE"] = version.split("-", 1)[1] if "-" in version else ""

    # Library and binary sources: src/ (and build.rs's output via OUT_DIR).
    src_files = _files(["src/**"])
    test_files = _files(["tests/**"])
    example_files = _files(["examples/**"])
    bench_files = _files(["benches/**"])

    common = {
        "edition": edition,
        "features": features,
        "rustc_flags": rustc_flags,
        "clippy_configuration": clippy_config,
    }

    # build.rs: compiled, run in a copy of the crate directory, and its
    # OUT_DIR, cfgs and env passed to every target of the package.
    if build_script:
        rule_env = dict(base_env)
        native.rust_binary(
            name = name + "-build-script-build",
            crate = "build_script_build",
            crate_root = build_script,
            srcs = [build_script],
            edition = edition,
            features = features,
            deps = build_deps,
            named_deps = build_named_deps,
            env = rule_env,
            visibility = [],
        )
        crate_manifest_dir(
            name = name + "-manifest-dir",
            manifest_dir = manifest_dir,
            srcs = _srcs_map(manifest_dir, _files(["**"], exclude = ["BUCK", "target/**"]), build_script_extra_srcs),
            visibility = [],
        )
        buildscript_run(
            name = name + "-build-script-run",
            package_name = pkg,
            buildscript_rule = ":" + name + "-build-script-build",
            manifest_dir = ":" + name + "-manifest-dir",
            features = features,
            version = version,
            visibility = [],
        )
        base_env["OUT_DIR"] = "$(location :{}-build-script-run[out_dir])".format(name)
        common["rustc_flags"] = common["rustc_flags"] + ["@$(location :{}-build-script-run[rustc_flags])".format(name)]

    clippy_targets = []

    def srcs_rule(suffix, files):
        # The crate's sources at their repository paths.
        rust_filegroup(
            name = name + "--srcs-" + suffix,
            mapped_srcs = {src: path for path, src in _srcs_map(manifest_dir, files, extra_srcs).items()},
            visibility = [],
        )
        return ":" + name + "--srcs-" + suffix

    lib_deps = []
    lib_named = {}
    if lib:
        lib_srcs = srcs_rule("lib", src_files)
        env_lib = dict(base_env)
        env_lib["CARGO_CRATE_NAME"] = lib["crate"]
        native.rust_library(
            name = name,
            crate = lib["crate"],
            crate_root = manifest_dir + "/" + lib["root"],
            srcs_filegroup = lib_srcs,
            proc_macro = lib.get("proc_macro", False),
            deps = deps,
            named_deps = named_deps,
            env = env_lib,
            visibility = visibility,
            **common
        )
        clippy_targets.append(":" + name)
        lib_deps = [":" + name]
        if lib.get("test", True) and not lib.get("proc_macro", False):
            native.rust_test(
                name = name + "-unittest",
                crate = lib["crate"],
                crate_root = manifest_dir + "/" + lib["root"],
                srcs_filegroup = lib_srcs,
                deps = deps + dev_deps,
                named_deps = named_deps | dev_named_deps,
                env = env_lib,
                run_env = test_env | {"CARGO_MANIFEST_DIR": manifest_dir},
                labels = test_labels,
                visibility = visibility,
                **common
            )
            clippy_targets.append(":" + name + "-unittest")

    for b in bins:
        bin_srcs = srcs_rule("bin-" + b["name"], src_files)
        env_bin = dict(base_env)
        env_bin["CARGO_CRATE_NAME"] = b["crate"]
        env_bin["CARGO_BIN_NAME"] = b["name"]
        native.rust_binary(
            name = name + "-bin-" + b["name"],
            crate = b["crate"],
            crate_root = manifest_dir + "/" + b["root"],
            srcs_filegroup = bin_srcs,
            deps = deps + lib_deps,
            named_deps = named_deps,
            env = env_bin,
            visibility = visibility,
            **common
        )
        clippy_targets.append(":" + name + "-bin-" + b["name"])
        if b.get("test", True):
            native.rust_test(
                name = name + "-bin-" + b["name"] + "-unittest",
                crate = b["crate"],
                crate_root = manifest_dir + "/" + b["root"],
                srcs_filegroup = bin_srcs,
                deps = deps + dev_deps + lib_deps,
                named_deps = named_deps | dev_named_deps,
                env = env_bin,
                run_env = test_env | {"CARGO_MANIFEST_DIR": manifest_dir},
                labels = test_labels,
                visibility = visibility,
                **common
            )
            clippy_targets.append(":" + name + "-bin-" + b["name"] + "-unittest")

    # Integration tests, examples and benches see the library, the dev
    # dependencies, CARGO_BIN_EXE_<bin> for each binary and
    # CARGO_TARGET_TMPDIR (a directory under buck-out, made relative by
    # toolchains/rust/wrap.sh).
    bin_exe_env = {
        "CARGO_BIN_EXE_" + b["name"]: "$(location :{}-bin-{})".format(name, b["name"])
        for b in bins
    }
    tmpdir_env = {"CARGO_TARGET_TMPDIR": "$(location //buck:cargo-target-tmpdir)"}
    for kind, items, files in [("test", tests, test_files), ("example", examples, example_files), ("bench", benches, bench_files)]:
        for t in items:
            tname = name + "-" + kind + "-" + t["name"]
            t_srcs = srcs_rule(kind + "-" + t["name"], src_files + files if t["root"].startswith("src/") else files)
            env_t = dict(base_env)
            env_t["CARGO_CRATE_NAME"] = t["crate"]
            env_t.update(bin_exe_env)
            env_t.update(tmpdir_env)
            if kind == "test" and t.get("harness", True):
                native.rust_test(
                    name = tname,
                    crate = t["crate"],
                    crate_root = manifest_dir + "/" + t["root"],
                    srcs_filegroup = t_srcs,
                    deps = deps + dev_deps + lib_deps,
                    named_deps = named_deps | dev_named_deps,
                    env = env_t,
                    run_env = test_env | {"CARGO_MANIFEST_DIR": manifest_dir} | bin_exe_env | tmpdir_env,
                    labels = test_labels,
                    visibility = visibility,
                    **common
                )
            else:
                # A harness = false test is run by its own main (as a test
                # with framework = False); examples and benches are built.
                rule_fn = native.rust_test if kind == "test" else native.rust_binary
                extra = {"framework": False, "run_env": test_env | {"CARGO_MANIFEST_DIR": manifest_dir} | bin_exe_env | tmpdir_env, "labels": test_labels} if kind == "test" else {}
                rule_fn(
                    name = tname,
                    crate = t["crate"],
                    crate_root = manifest_dir + "/" + t["root"],
                    srcs_filegroup = t_srcs,
                    deps = deps + dev_deps + lib_deps,
                    named_deps = named_deps | dev_named_deps,
                    env = env_t,
                    visibility = visibility,
                    **(common | extra)
                )
            clippy_targets.append(":" + tname)

    # `buck2 build :<package>-clippy` runs clippy over every target of the
    # package (cargo clippy --all-targets); scripts/buck/clippy.sh fails on
    # any diagnostic, as `-D warnings` does.
    native.filegroup(
        name = name + "-clippy",
        srcs = {t.removeprefix(":") + ".clippy.txt": t + "[clippy.txt]" for t in clippy_targets},
        visibility = visibility,
    )
