# Desktop import validation — 2026-10-06

Changes are scoped to `apps/desktop/**`. The source repository and monorepo root configuration/workflows/docs are not changed by this import.

## Integrated Nx follow-up

From the monorepo root with the working pnpm/Nx installation (all commands use `NX_DAEMON=false`):

- `pnpm exec nx run loams-desktop:format-check` — passed in 8 seconds.
- `pnpm exec nx run loams-desktop:check-minimal` — passed in 86 seconds after two initial 120-second cold-build timeouts. Uses the explicit desktop-local `--target-dir target`; the entire workspace and all targets are checked without default features. Existing warnings remain.
- `pnpm exec nx run loams-desktop:test-focused` — passed in 72 seconds after an initial 120-second cold compilation timeout; covers the same six packages as the 144-test focused baseline below.
- `pnpm exec nx run loams-desktop:verify-import` — passed, including two new Python regression tests. Default checks read only desktop files and the committed provenance record; no sibling source checkout or Git command is needed. `--source` is opt-in only.

`test-core` now explicitly uses `--no-default-features` in addition to excluding the app/UI. It remains a comprehensive non-GUI compilation/test target and was not run again in this bounded follow-up. `test-focused` avoids engine/GPUI compilation. Browser-enabled `check` remains available for the root CI/hosts with WebKit development packages. No deployment, release/update policy, root config, or CI workflow was changed by this follow-up.

Rebrand audit: no remaining Zeron product/window/menu/CLI labels were found. Remaining references are preserved legal/theme/dependency provenance, synthetic test URLs, explicit clipboard/settings compatibility, and the internal document tokens documented in `native/LOAMS.md`. MIT logos/attribution and pinned functional dependency URLs were not removed. Remaining upstream-compatible remote feature contracts (WorkOS edge auth, registry/chat/device synchronization, blob sidecars, preview signaling) are documented there; they require explicit backend configuration and are not a migration to core Loams Connect/Authentik APIs.

## Original import baseline — passed

Run from `apps/desktop/native` with the installed Rust **1.97.1** toolchain and the host's existing shared Cargo build cache:

- `cargo check --offline --locked --workspace --all-targets --no-default-features` — entire imported workspace and all targets type-check, including the app and UI tests/examples. The Linux embedded-browser feature is off in this check; it stays on by default in normal builds.
- `cargo test --offline --locked -p loams-desktop-brand -p loams-desktop-link -p loams-desktop-proto -p loams-desktop-update -p loams-desktop-theme -p loams-desktop-mcp` — **144 tests passed**; six inherited doctests are ignored. Includes production-library updater policy coverage, independent of the unit-test-only transport allowance.
- `cargo test --offline --locked -p loams-desktop-harness --lib acp::loams_bot::tests` — **1 test passed** (fixture/example binaries rejected; shipping executable recognized). Windows-only executable-suffix coverage is not run on Linux.
- `rustc --edition=2024 --test apps/loams-desktop/src/paths.rs -o target/validation/paths-tests && target/validation/paths-tests` — **2 tests passed**, covering explicit data override and rejection of upstream env/data defaults.
- `cargo fmt --all -- --check` — passed.
- `bash scripts/test-linux-desktop-entry.sh` — both retained fixture and packaged-tarball installers passed offline, including XDG paths, quoting, idempotence and application identity.
- `bash -n` on every active `scripts/**/*.sh` — **12 scripts passed**.
- Optional original-source audit, `python3 ../verify_import.py --source ../../../../loams-desktop` — **17 private local crates**, independent Cargo workspace/lock, pinned git revisions, consistent OS identities and Nx wrappers, preserved legal text hashes; **all 2,600 source tracked files unchanged**.

The import copied **1,073 tracked files** using Python `git ls-files -z` and `shutil.copy2`. Removed mobile/client/text crates were outside the verified desktop dependency closure. Source hashes and closure are recorded in `import-provenance.json`. No `.git`, caches, untracked files, source workflows, or cloud worker package were copied.

## Original import baseline — limits / not passed

- `cargo check --offline --workspace --all-targets` (default browser-enabled build) reached the UI build script and failed because `pkg-config` cannot find **webkit2gtk-4.1**. No system packages were installed. Install WebKitGTK 4.1 and JSON-GLib development packages to validate the default embedded browser. The explicit reduced-browser configuration passes; browser opening in that configuration reports its unavailability.
- `cargo test --offline --locked --workspace --exclude loams-desktop --exclude loams-desktop-ui` exceeded a **240-second** compile bound, with subsequent **180-second** incremental attempts also timing out. No full-core-suite success is claimed.
- `cargo test --offline --locked -p loams-desktop-engine --lib harness_updates::tests` exceeded its **120-second** compile bound. No engine-update-suite success is claimed. All its targets do type-check in the workspace check.
- No linked GUI build, GUI launch/rendering test, actual installed-user daemon test, or native macOS/Windows package build was completed. Those require platform prerequisites and/or more build time. Existing compiler warnings and `proc-macro-error2` future-incompatibility warnings remain; unrelated warning cleanup was not attempted.
- At the original import validation, root Nx setup was absent, so wrappers were only structurally checked. This limitation is superseded by the successful integrated Nx follow-up above; root setup was provided separately and remains outside desktop ownership.

No automatic updates, release feeds, Cargo publication, root release workflows, deployments, or release operations were enabled or executed. Local packaging is manual-only; macOS signing/notarization is opt-in.
