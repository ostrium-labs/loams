# The Rust toolchain: rustup's rustc, clippy-driver and rustdoc at the channel
# of rust-toolchain.toml, each run through toolchains/rust/wrap.sh. Adapted from the
# prelude's system_rust_toolchain (MIT OR Apache-2.0), which runs the bare
# names from PATH.

load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")

def _loams_rust_toolchain_impl(ctx):
    def tool(name):
        return RunInfo(args = cmd_args(ctx.attrs.tools[name], hidden = [ctx.attrs.wrapper, ctx.attrs.channel]))

    return [
        DefaultInfo(),
        RustToolchainInfo(
            allow_lints = ctx.attrs.allow_lints,
            clippy_driver = tool("clippy-driver"),
            clippy_toml = ctx.attrs.clippy_toml[DefaultInfo].default_outputs[0] if ctx.attrs.clippy_toml else None,
            compiler = tool("rustc"),
            default_edition = ctx.attrs.default_edition,
            panic_runtime = PanicRuntime("unwind"),
            deny_lints = ctx.attrs.deny_lints,
            doctests = ctx.attrs.doctests,
            nightly_features = False,
            report_unused_deps = False,
            rustc_binary_flags = ctx.attrs.rustc_binary_flags,
            rustc_flags = ctx.attrs.rustc_flags,
            rustc_target_triple = ctx.attrs.rustc_target_triple,
            rustc_test_flags = ctx.attrs.rustc_test_flags,
            rustdoc = tool("rustdoc"),
            rustdoc_flags = ctx.attrs.rustdoc_flags,
            warn_lints = ctx.attrs.warn_lints,
        ),
    ]

loams_rust_toolchain = rule(
    impl = _loams_rust_toolchain_impl,
    attrs = {
        "allow_lints": attrs.list(attrs.string(), default = []),
        "channel": attrs.source(),
        "clippy_toml": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "default_edition": attrs.option(attrs.string(), default = None),
        "deny_lints": attrs.list(attrs.string(), default = []),
        "doctests": attrs.bool(default = False),
        "rustc_binary_flags": attrs.list(attrs.arg(), default = []),
        "rustc_flags": attrs.list(attrs.arg(), default = []),
        "rustc_target_triple": attrs.string(),
        "rustc_test_flags": attrs.list(attrs.arg(), default = []),
        "rustdoc_flags": attrs.list(attrs.arg(), default = []),
        "tools": attrs.dict(key = attrs.string(), value = attrs.source()),
        "warn_lints": attrs.list(attrs.string(), default = []),
        "wrapper": attrs.source(),
    },
    is_toolchain_rule = True,
)
