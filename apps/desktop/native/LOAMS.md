# Loams Desktop native import

This is a standalone Cargo workspace imported from the tracked files of `loams-desktop`, source commit `c9205f8f949711c3315af551c804c56452d2fbd7`. It is not a member of the monorepo's core Rust workspace. See `../import-provenance.json` for the source hashes and verified desktop dependency closure.

## Build and run

Rust is pinned to **1.97.1**. From this directory:

```sh
cargo build --locked --target-dir target -p loams-desktop
cargo run --locked --target-dir target -p loams-desktop
LOAMS_MOCK=1 cargo run --locked --target-dir target -p loams-desktop -- loams status
```

Linux requires the GPUI platform libraries (including ALSA, X11/Wayland, fontconfig, OpenSSL and Vulkan), a C compiler and `pkg-config`. The default embedded browser additionally requires **WebKitGTK 4.1** and **JSON-GLib** headers/pkg-config metadata. Distribution package names vary. A build without the embedded Linux browser is supported explicitly:

```sh
cargo build --locked --target-dir target -p loams-desktop --no-default-features
```

In that build the browser reports that it is unavailable; it is not replaced with a fake helper. macOS requires Xcode command-line tools; Windows requires an MSVC toolchain and Windows SDK. Native packaging scripts must run on their corresponding OS.

## Identities and configuration

- Product: **Loams Desktop**; binary: `loams-desktop`; all local packages: `loams-desktop-*`.
- The existing identity implementation is `crates/loams-desktop-brand`; the desktop integration is `crates/loams-desktop-link`, distinct from core `loams-link`.
- Application/keyring ID: `dev.loams.desktop`; macOS dev bundle: `dev.loams.desktop.dev`.
- New OS deep links: `loams://open/chat/<id>?workspace=<locator>`. Old `zeron://` links are rejected, not registered as aliases.
- Runtime overrides use `LOAMS_DESKTOP_*`, for example `LOAMS_DESKTOP_DATA_DIR`, `LOAMS_DESKTOP_IPC_PORT`, `LOAMS_DESKTOP_EDGE_URL`. No `ZERON_*` desktop env aliases. The pinned upstream GPUI dependency retains its own `ZERON_GPU_STATS` diagnostic flag; it is not a desktop configuration alias.
- Unix data: `~/.loams-desktop`; Windows data: `%LOCALAPPDATA%\Loams Desktop`. No automatic adoption, rename, or deletion of upstream data. An explicit data-dir override is the user's choice.
- User services: `loams-desktop.service` (Linux) and `dev.loams.desktop` (launchd). Loams Bot self-discovery accepts only the `loams-desktop` executable stem, not test/example binaries.
- Loams server configuration keeps `LOAMS_URL`, the existing explicit `LOAMS_URL` fallback, `LOAMS_MOCK`, `LOAMS_BOT_URL`, `LOAMS_OIDC_ISSUER`, and `LOAMS_BOT_EXECUTABLE`.

## Disabled defaults and compatibility

**Self-updates, scheduled update polling, download/install-on-quit, and release-feed transport are disabled in shipping code**, independent of env overrides. Unit tests can exercise inherited updater transport against local fixtures; that test-only allowance is not present in a shipping dependency build. The GUI updater is not initialized. The updater menu item is disabled, and the CLI update command fails with a clear disabled-policy error. No release-feed config or release manifest is packaged. Cargo publishing is disabled for every local crate. Agent-CLI update preferences default to `Off`; explicit user changes to third-party agent update policy remain supported.

Workspace cloud sync remains local-only unless explicitly configured/signed in; the inherited edge default is `https://edge.loams.invalid` and the WorkOS tenant is an unconfigured placeholder, not upstream's tenant. Loams sign-in is `loams-desktop loams login`, using the existing Authentik integration.

ACP, MCP, CRDT/document and generated Connect-RPC schemas are unchanged. For explicit synced-content compatibility, `zeron-file:` and `zeron-invoke:` remain **internal Markdown tokens**, not OS URL handlers, and the renamed Rust enum `ManagedByLoamsDesktop` serializes as the existing `managed-by-zeron` wire value. Clipboard writes use `loamsDesktopComposerV1`; reads also accept `zeronComposerV1`. Settings accept the old `openWebLinksInZeron` JSON field as an explicit deserialize alias, but write the new field.

## Scope and tests

The desktop dependency closure excludes `apps/ios`, `crates/mobile`, `crates/client`, `crates/text`, and mobile scripts. No `.github` files, CI/deploy/release workflows, cloud worker package, or source docs were imported. `edge/src/install.sh` is retained for the daemon's compile-time installer fixture and offline launcher tests; it has no default remote download host. Cloud-worker-dependent smoke/transport scripts are disabled artifacts, not desktop test targets.

```sh
cargo check --locked --workspace --all-targets
cargo check --locked --workspace --all-targets --no-default-features
cargo test --locked --workspace --no-default-features --exclude loams-desktop --exclude loams-desktop-ui
cargo test --locked -p loams-desktop-brand -p loams-desktop-link -p loams-desktop-proto -p loams-desktop-update -p loams-desktop-theme -p loams-desktop-mcp
cargo fmt --all -- --check
bash scripts/test-linux-desktop-entry.sh
python3 ../verify_import.py
```

`../project.json` exposes scoped Nx build/check/test/format/import-verification/local-packaging wrappers with explicit working directories and output paths. From the monorepo root:

```sh
NX_DAEMON=false pnpm exec nx run loams-desktop:format-check
NX_DAEMON=false pnpm exec nx run loams-desktop:check-minimal
NX_DAEMON=false pnpm exec nx run loams-desktop:test-focused
NX_DAEMON=false pnpm exec nx run loams-desktop:verify-import
```

`test-focused` runs the identity, Loams integration, protocol, updater, theme, and MCP suites without GPUI or engine-suite compilation. `test-core` still compiles the broader non-GUI core, but explicitly disables default features; `test` and `check` retain the full browser-enabled configuration for adequately provisioned hosts/CI. Cold builds can exceed short execution bounds; retry incrementally or use the focused target. No root configuration, deployment wiring, or nested Nx project is added here. Cargo commands also work independently of Nx.

Default verification reads only this imported monorepo subtree and the committed provenance record. It does **not** require Git or a sibling source repository. The verification target also runs regression tests proving no out-of-subtree file reads or Git calls occur by default. An original-source hash audit is a separate, explicit maintainer operation: `python3 ../verify_import.py --source /path/to/original/loams-desktop`; it is not part of the default Nx/CI target.

## Remaining inherited remote-feature coupling

No shipping product labels still say Zeron, and no hard-coded runtime `zeron.sh`/`edge.zeron.sh` endpoint remains. Upstream dependency repositories, MIT/theme attribution, test fixtures, and documented compatibility tokens are deliberately retained.

The inherited remote workspace stack is **not** automatically translated into the Loams Connect/Authentik stack:

- Optional workspace account sign-in still uses WorkOS authorization and a compatible edge's `/auth/exchange`, `/auth/refresh`, `/auth/orgs`, and hosted `/auth/cli/callback` routes (`crates/engine/src/auth.rs`). `loams-desktop loams login` is the separate existing Authentik integration.
- Registry/chat synchronization still expects the upstream Worker-compatible `/registry/{org}/ws` and `/chat2/{chat}/ws`, rows/checkpoint/tail contracts, device relay/status/nudge behavior, and CRDT frames (`workspace_host.rs`, `doc_host.rs`, `chat2_host.rs`).
- Remote tool-output sidecars still use the compatible edge's `/blob/{chat}/{part}` storage contract (`doc_host.rs`); preview signaling uses `/preview/{org}/ws` (`crates/preview/src/signaling.rs`).

These features need an explicitly configured compatible backend and credentials. Their default edge host remains `edge.loams.invalid`; they are not connected to Zeron's production service. No protocol migration or broader remote-feature rewrite was undertaken. Self-update network/apply paths remain disabled even with a configured edge or release URL.

## Legal and packaging

Loams Desktop is derived from [Zeron](https://github.com/zeronsh/zeron), MIT, Copyright (c) 2026 Wing. Inherited `LICENSE`, `NOTICE`, `THIRD_PARTY_NOTICES.md`, and all bundled legal texts remain unmodified. Original `NOTICE` references historical source paths: `crates/loams-brand` and `crates/loams-link` now map to their `loams-desktop-*` directories; inherited workflow references are provenance only, not imported automation. Loams-added code remains Apache-2.0 under the preserved crate licenses. See `SCOPED_NOTICE.md`.

Local packaging scripts include upstream/Loams legal texts and the Loams placeholder icon. They generate installable local artifacts, never deploy, upload, or enable automatic updates. macOS signing/notarization remains explicitly opt-in via credentials and can contact Apple when requested. Linux installs are per-user; Windows uses a separate installer AppId and URL handler rather than modifying an upstream installation.
