# SO2 Task 0: spike results and reconciliation

Plan: [SO2](2026-10-02-so2-systemone-desktop.md), Task 0. Design: [§40](../design/40-loams-systemone.md) (D528, D538), [§50](../design/50-loams-desktop-daemon.md), [§53](../design/53-studio-ade.md). Run on **2026-10-19** against `dev` at `8e1ac6ef` (the merge of the SO2 re-target, #770, which already contains SO1 Task 0, #772). The rulings that come out of this page are R0.1 to R0.21 in the plan's "Rulings made during execution" table, and owner questions SO2-Q6 to SO2-Q8 (accepted 2026-10-19, O6–O8) in its "Owner questions from Task 0"; this page holds the evidence.

This was a docs-only task on a Linux build machine. Nothing third-party was built or run. Every Swift, Core ML, signing and Gatekeeper check is listed in §6 with the exact commands for a macOS arm64 machine, and **no result is recorded for any of them**.

## Verdicts

| Check | Verdict |
|---|---|
| SO1 as built | **Not built.** There is no `crates/loams-systemone` and no `proto/loams/systemone/`; only SO1 Task 0 (docs) has merged. SO2 Tasks 2 to 7 wait for SO1 (R0.1) |
| `loams-systemone` inside the daemon's tree (`cargo tree -d`) | **Cannot be measured yet** (no crate). Baseline recorded; one predictable duplicate: `reqwest` 0.12 in the daemon against 0.13 in the workspace (R0.2) |
| `scripts/ci/agentd-deps.sh` | **Passes** on `dev` today (exit 0); re-run in Task 2 with the new crate |
| R7: does the Loams Bot process receive a scoped run token? | **Yes, already**, but not in its environment: it arrives in ACP `session/new` `mcpServers[]`, and the shim ignores it (R0.3) |
| Does `a2a::Message` carry `metadata`? | **No** (R0.4) |
| Does ACP `_meta` on a tool call reach `@loams/studio-ui`? | **No.** The harness drops it while normalising into `ToolCall`; reaching the card needs an additive persisted field (R0.5, owner question SO2-Q6) |
| `needs_person` notification priority | **No priority exists** on the daemon's `Notification`, and Electron main, not the daemon, raises the OS notification (R0.6, owner question SO2-Q7) |
| FluidUse revision | **`cfba4d3fa4b996d0b680549416caf7aba6be770c`** (2026-10-14), unchanged since SO1 Task 0 |
| `FluidUse` product alone | **No**: it depends on `FluidAudio` and copies `Resources`. **Vendor the five Laya files**, with `LayaModelStore`'s download code removed (R0.8) |
| `LayaManager` loading from our directory | **Yes, by source**: `LayaManager.load(from:configuration:)` takes a local directory and "never downloads" (R0.8) |
| `e8` (int8-embedding) buckets | **Fail our 0.02 parity gate on upstream's own benchmark** (max probability delta 0.80 on one suite). Default is `fp16` (R0.9) |
| Core ML weights against SO1's pin | **Byte-identical** multilingual checkpoint (same LFS SHA-256), so the comparison with `laya-serve` is like for like (R0.10) |
| Socket path on macOS | 82 bytes plus the user name: fits under 100 for short names up to 18 bytes; the `$TMPDIR` fallback is needed above that (R0.13) |
| Signing | **Not run.** The shared entitlements file would give `loams-coreml` JIT and disabled library validation; owner question SO2-Q8 (R0.14) |
| Apple silicon latency, memory, parity | **Not run** (Linux machine). Upstream figures recorded for orientation only; commands in §6 |

## 1. The daemon as built (`dev` at `8e1ac6ef`)

Commands: `ls crates | grep agentd`, `grep -rn` over `crates/loams-agentd*`, `apps/desktop-electron`, `web/`.

| Area | As built | File |
|---|---|---|
| Crates | Twelve: `loams-agentd` plus `-connect`, `-doc`, `-harness`, `-link`, `-mcp`, `-preview`, `-proto`, `-rpc`, `-sessions`, `-store`. Not built: `-supervisor` (DD1 Task 14), `-loop` (DD1 Tasks 19 and 22), `-llm`, `-tools`, `-factory`. R2's "sixteen" counts design §50's planned crates; as built, the new crate is the twelfth `loams-agentd-*` crate | `crates/` |
| Data directory | `Paths` under `<userData>/agentd/`: `config.toml`, `agentd.json`, `agentd.token`, `agentd.lock`, `store/`, `adapters/`, `runtime/`, `logs/`. No `run/` and no `models/` yet | `crates/loams-agentd/src/paths.rs` |
| macOS `userData` | `~/Library/Application Support/Loams Desktop` (packaged; `productName`), recorded by DD1 ruling T0-7 | `docs/plans/2026-10-09-dd1-desktop-daemon.md`, `apps/desktop-electron/electron-builder.config.cjs` |
| Config | `Config` with `[paths] [engine] [daemon] [tool_outputs] [groups] [approvals] [remote]`, `#[serde(default)]`, no `deny_unknown_fields`, so `[systemone]` is additive. There is no `SetConfig` method on the wire | `crates/loams-agentd/src/config.rs` |
| Runtime install | `BINARIES = ["loams-agentd", "loams"]`, checked against `SHA256SUMS`, `current` symlink (Unix) or `current.txt` (Windows), two versions kept | `crates/loams-agentd/src/runtime_install.rs` |
| Bundle sums and signing | `bin-sums.cjs` writes `SHA256SUMS` for a fixed name list (`loams-agentd`, `loams`); `mac-sign.cjs` signs the whole app with `@electron/osx-sign`, rewrites the sums, and reseals | `apps/desktop-electron/scripts/{bin-sums,mac-sign}.cjs` |
| Entitlements | One file for the app and, through `entitlementsInherit`, every nested Mach-O: `allow-jit`, `allow-unsigned-executable-memory`, `disable-library-validation` | `apps/desktop-electron/build/entitlements.mac.plist` |
| Keyring | No `SecretStore` (DD1 Task 22 not built). Two direct `keyring` users with **two service names**: `dev.loams.agentd` (forge credentials) and `dev.loams.desktop` (`brand::KEYRING_SERVICE`, the link's refresh token). The link crate pins `keyring = "4.2"` rather than the workspace's `=4.2.0` | `crates/loams-agentd-sessions/src/change_request_create.rs:35`, `crates/loams-agentd-proto/src/brand.rs:25`, `crates/loams-agentd-link/src/auth/store.rs`, `crates/loams-agentd-link/Cargo.toml` |
| Roles | ndjson roles `Owner` and `Mcp` (least role per method) and `renderer: bool`; `ResolveApproval` is `Owner` | `crates/loams-agentd-proto/src/rpc.rs:138,188,472` |
| Remote `studio` role | A **deny list** over the Connect twins table: every `loams.agentd.v1` method is a Studio method unless listed in `STUDIO_EXCLUDED` | `crates/loams-agentd-connect/src/roles.rs` |
| Connect services | `approvals`, `attachments`, `diff`, `files`, `git`, `groups`, `harness`, `notifications`, `projects`, `queue`, `review`, `studio`, `terminals`, `threads` | `crates/loams-agentd-connect/src/services/`, `proto/loams/agentd/v1/` |
| Electron proxy | `loams-app://console/loams.agentd.v1.*` to the daemon; `allowlist.gen.ts` | `apps/desktop-electron/src/main/protocol/{agentd,route,proxy}.ts`, `src/main/agentd/allowlist.gen.ts` |
| MCP shim | `loams-agentd mcp`, a **static** catalogue (`catalog()`: `whoami`, `list_projects`, `list_harnesses`, `list_models`, `list_chats`, `get_chat`, `create_chat`, `read_chat`, `send_message`, `wait_for_turn`, `interrupt_chat`, `respond_to_input`, `archive_chat`) | `crates/loams-agentd-mcp/src/tools.rs:59` |
| Tool registry | None (DD1 Task 19 not built) | — |
| Engine supervisor | Still Electron main's TypeScript supervisor; DD1 Task 14 not built | `apps/desktop-electron/src/main/engine/supervisor.ts` |
| Notifications | In-memory buffer of 200; `NotificationKind` `turn_done`, `approval`, `question`, `error`, `group_ready`, `check_failed`; no priority. The OS notification is shown by Electron main (`new Notification(...)`, gated by `shouldNotify`) | `crates/loams-agentd-proto/src/notify.rs`, `apps/desktop-electron/src/main/index.ts:438` |
| Plugin manifest | `modes` is supported (defaults to `["console"]`); `console.settings.section` is used by `desktop-settings`, `desktop-agentd`, `desktop-servers`, `agent` | `web/packages/console-host/src/manifest.ts:140`, `web/plugins/desktop-settings/package.json` |
| Studio tool presentation | `presentation.ts` and `commandLabel.ts` map agentd's `ToolCall` kinds; no `_meta` | `web/packages/studio-ui/src/chat/tools/` |

### 1.1 The Loams Bot path (R7, A2A metadata, ACP `_meta`)

- **Run token.** `Inner::loams_agentd_mcp` issues a scoped run token for every run, bound to the chat (`ScopedTokens::issue(chat_id)`), and builds the `loams-desktop` MCP server with exactly three env pairs: `LOAMS_AGENTD_RUN_TOKEN`, `LOAMS_AGENTD_PORT`, `LOAMS_AGENTD_SESSION_ID` (`crates/loams-agentd-sessions/src/sessions.rs:1390-1420`). The run task stamps it on every run unless one is already set (`sessions.rs:1951-1958`) and revokes it when the run ends (`RunTokenLease`). The ACP harness spawns the agent process per run (`acp/mod.rs`, `run` → `spawn_agent`) and sends the server in `session/new` `mcpServers` as `{name, command, args, env: [{name, value}]}` (`acp_mcp_servers`, `acp/mod.rs:2493`, used at `:3582`). The Loams Bot harness is an ordinary ACP spec (`crates/loams-agentd-harness/src/acp/loams_bot.rs`), so it gets the same token. The shim (`crates/loams-agentd-link/src/acp.rs`) answers `session/new` without reading its params.
- **A2A.** `a2a::Message` has `message_id`, `context_id`, `task_id`, `role`, `parts`; no `metadata` and no routing hint (`crates/loams-agentd-link/src/a2a/types.rs:67-80`). The shim builds one `Message` per prompt (`acp.rs`, `prompt`).
- **ACP `_meta`.** The shim's `tool_call` update carries `toolCallId`, `title`, `kind`, `status` and no `_meta`. On the daemon side the harness reads `_meta` only for vendor-specific tool names (`acp/normalize.rs:94,282`) and reduces every call to `ToolCall` (`crates/loams-agentd-proto/src/agent.rs:363`), which is persisted inside `MessagePart::Tool` (`crates/loams-agentd-proto/src/chat.rs:98`). Nothing carries an arbitrary `_meta` object to the renderer.
- There is no `bot/` module in `loams-agentd-link`; Task 6 creates `bot/{mod,decisions}.rs`.

### 1.2 Dependencies (`cargo tree`, no build)

```sh
PATH=$HOME/.cargo/bin:$PATH bash scripts/ci/agentd-deps.sh          # exit 0
cargo tree -d -p loams-agentd -e normal --depth 0                    # 24 crates appear twice or more
cargo tree -p loams-agentd -e normal -i reqwest                      # only reqwest v0.12.28
```

Duplicated today (baseline, before `loams-systemone`): `hashbrown` (4), `syn`, `heapless`, `getrandom` (3 each), `winnow`, `webpki-roots`, `untrusted`, `toml_edit`, `toml_datetime`, `thiserror` and `-impl` (1 and 2), `sha2` (0.10.9 and 0.11.0), `serde_core`, `serde`, `itertools`, `heck`, `hash32`, `enum-as-inner`, `digest`, `crypto-common`, `cpufeatures`, `block-buffer`, `bitflags`, `base64` (2 each).

The daemon crates declare `reqwest = "0.12"` directly (`-harness`, `-link`, `-sessions`, `-preview`, and `-connect`'s `jwks-http`), while the workspace dependency is `reqwest = "0.13"` and SO1 R0.13 puts `loams-systemone` on it. Both `reqwest` 0.12.28 and 0.13.5 have `ClientBuilder::unix_socket` (read in the cargo registry sources), so the Core ML transport needs no extra crate on either.

### 1.3 Plans since 2026-10-18

`git log --since=2026-10-18 origin/dev -- crates/loams-agentd* apps/desktop-electron web/packages/studio-ui`: release and UI fixes only. No DD1 task (11 bundle step aside, already built) and no SD1 task landed since the re-target; DD1 Tasks 14, 19 and 22 are not built; SO1 Tasks 1 to 13 are not built.

## 2. FluidUse (read 2026-10-19, `gh api`)

```sh
gh api repos/FluidInference/FluidUse --jq '.license.spdx_id,.pushed_at'          # Apache-2.0, 2026-10-14
gh api repos/FluidInference/FluidUse/commits/HEAD --jq .sha                     # cfba4d3f…
gh api 'repos/FluidInference/FluidUse/contents/Sources/FluidUse/Laya?ref=cfba4d3f…'
```

- **Revision:** `cfba4d3fa4b996d0b680549416caf7aba6be770c` (2026-10-14), the same as SO1 Task 0 read.
- **`Package.swift`:** `swift-tools-version: 6.0`, `platforms: [.macOS(.v14)]`, one library product `FluidUse` whose target depends on `FluidAudio` (`from: "0.15.8"`) and has `resources: [.copy("Resources")]`; about forty executable targets beside it. Consuming the product builds `FluidAudio` and copies the resource bundle. **Not acceptable** for a single-purpose sidecar.
- **`Sources/FluidUse/Laya/`** (five files, 57 647 bytes): `LayaManager.swift` (16 773), `LayaModelStore.swift` (6 122), `LayaSequenceBuilder.swift` (4 731), `LayaTokenizer.swift` (19 841), `LayaTypes.swift` (10 180). Imports: `Foundation`, `Darwin`, `os`, `CoreML` only. No reference to `FluidAudio`, `Bundle` or the package's resources.
- **Loading from our directory:** `LayaManager.load(from directory: URL, configuration:)` reads `tokenizer.json` and `laya_multilingual_<precision>_L<length>_options32.mlmodelc` from a local directory, compiles an `.mlpackage` if that is what is there, and "never downloads". `LayaManager.load(cacheDirectory:configuration:)` is the downloading path; it calls `LayaModelStore.ensure`, which uses `URLSession.shared.download` from `huggingface.co/<repo>/resolve/main/…` (an unpinned `main`).
- **Configuration:** `lengths` (default `[128, 512]`), `computeUnits` per length (default `.cpuAndNeuralEngine` at 128, `.all` above), `precision` (`fp16` or `e8`).
- **Behaviour the sidecar inherits:** `maximumOptions = 32`; a `choice` or `score` outside 2…32 throws `LayaError.invalidOptionCount(n)` (Task 1 maps it to `422 options_over_32`, and below 2 to `laya-serve`'s code); the smallest bucket that holds the prompt is used, otherwise the largest with the state cut on the right (`stateWasTruncated`); a question head that does not fit throws `promptTooLong`; `LayaManager` is an `actor`, so calls are serialised; several questions are answered with **one encoder pass each** (`answer(state:questions:)` maps over them), so 10 questions cost about ten single calls.

## 3. `FluidInference/laya-coreml` (read 2026-10-19)

```sh
curl -s https://huggingface.co/api/models/FluidInference/laya-coreml          # sha, siblings
curl -sL https://huggingface.co/FluidInference/laya-coreml/resolve/7b8d7a2b…/config.json
curl -s 'https://huggingface.co/api/models/convaiinnovations/laya/revision/<rev>?blobs=true'
```

- Revision `7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc` (2026-09-22), Apache-2.0, files: `LICENSE`, `NOTICE.md`, `README.md`, `config.json`, `tokenizer.json`, eight `.mlmodelc` bundles (four files each: `analytics/coremldata.bin`, `coremldata.bin`, `model.mil`, `weights/weight.bin`), and `reports/`.
- `config.json`: source `convaiinnovations/laya` `multilingual` at `1c5edc17a7acd8701df6fc341c0d179f1c62c982`, encoder `jhu-clsp/mmBERT-base`; `max_options` 32; `head_max_len` 256; buckets 128, 256, 512, 1024; precisions `fp16` (614 MB per package) and `e8` (448 MB); inputs `input_ids`, `attention_mask`, `marker_map [1, 32, L]`, `question_type [1, 3]`; outputs `logits`, `probabilities`, `action_probabilities`; minimum deployment macOS 14.
- **Source weights:** `multilingual/model.safetensors` has LFS SHA-256 `9d628fd971b700382ac6f65920a86f149777b2e748e0c955fb3b19695aa8f204` and `multilingual/tokenizer/tokenizer.json` `609d8f4c067cd3950f88594c5a802616cea245823836ef5848ee4fc40aab5b6f` at **both** `1c5edc17` (the conversion's source) and `7b928d82` (SO1 R0.4's pin).
- **Upstream's own benchmark** (`reports/benchmark-coreml.json` and `-e8.json`, Apple M5 Pro, 3 899 questions over ten suites, all four buckets; **upstream's figures, not ours**):

| | `fp16` | `e8` |
|---|---|---|
| p50 / p95 latency per question | 5.3 / 18.1 ms | 5.3 / 18.1 ms |
| load time, four buckets | 2.8 s | 30.5 s |
| max probability delta against the PyTorch reference, worst suite | 0.0206 (`jev.emotion`) | **0.8010** (`app.guardrails_jailbreak`) |
| suites over 0.02 | 1 of 10 (0.0206) | 9 of 10 |
| lowest argmax agreement | 0.9975 | 0.9749 |

  The 16-case verification report for the `fp16` L512 bucket shows a max probability error of 0.0021 and 16 of 16 argmax agreements, load 2.4 s, p50 9.0 ms (compute units `ALL`).

## 4. The socket path (R10)

`<userData>/agentd/run/systemone/coreml.sock` on a default macOS profile is `/Users/<name>/Library/Application Support/Loams Desktop/agentd/run/systemone/coreml.sock`: **82 bytes plus the length of the short user name**. Computed with `python3 -c 'len(…)'`:

| Short name | Bytes | Under R10's 100-byte limit |
|---|---|---|
| `a` | 83 | yes |
| `keshav` | 88 | yes |
| 18 characters | 100 | yes (the limit) |
| 19 characters or more | 101+ | no: the `$TMPDIR/loams-<uid>/` fallback is taken |

macOS `sun_path` is 104 bytes including the terminating NUL. The `$TMPDIR` fallback's own length depends on the per-user `/var/folders/<2>/<random>/T/` path, which this machine cannot produce; a typical form gives about 70 bytes. The real `$TMPDIR` length is measured on the Mac (§6.4).

## 5. What the plan's file list misses

- There is no `THIRD_PARTY_NOTICES.md` (SO1 R0.22): vendored FluidUse files go in the root `NOTICE` and `apps/desktop-electron/NOTICE` (shipped as `Contents/Resources/NOTICE`); FluidAudio is not used once vendored.
- Task 1 must also change `apps/desktop-electron/scripts/bin-sums.cjs` (the fixed name list) and probably `mac-sign.cjs` (per-file entitlements, SO2-Q8), and their Vitest files.
- Task 2 adds `run()` and `models()` to `crates/loams-agentd/src/paths.rs`.
- Task 3 adds the `studio` exclusions to `crates/loams-agentd-connect/src/roles.rs`.
- O3 (the supervised `loams dev` with its SystemOne route off) goes into `apps/desktop-electron/src/main/engine/supervisor.ts` until DD1 Task 14 moves supervision into the daemon.

## 6. Checks for a macOS arm64 machine (not run)

Run on a **physical** Apple-silicon Mac on macOS 14 or later for §6.2 (the labelled runner or the owner's machine). Whether the GitHub-hosted `macos-15` arm64 VM (`MACOS_ARM64_RUNNER`, `desktop-electron.yml`) exposes the Neural Engine to Core ML was not checked; record `MLComputeUnits` actually used (§6.2) before trusting its latency. Record the machine, macOS and Xcode versions, the date and the `dev` commit with each result, and add the rows to this page.

### 6.1 Swift toolchain and the vendored build

```sh
xcodebuild -version; swift --version            # tools-version 6.0 needs Swift 6.0+ (Xcode 16+)
# in a scratch SwiftPM package with the five vendored files (LayaModelStore stripped, R0.8):
swift build -c release --arch arm64
otool -L .build/release/loams-coreml            # system frameworks and /usr/lib/swift/libswift*.dylib only;
                                                # fail on FluidAudio, @rpath bundles or anything outside /System and /usr/lib
nm -u .build/release/loams-coreml | grep -i urlsession   # expect nothing (no download path)
```

### 6.2 Core ML on Apple silicon

Download the buckets once, by hand, at revision `7b8d7a2b…` into a scratch directory, and check each file against the LFS SHA-256 from the Hugging Face API. Then, with a small harness over `LayaManager.load(from:)`:

- first-load time per bucket and for `[128, 512]`, `fp16` and `e8`;
- p50 and p95 for 1, 5 and 10 questions at buckets 128, 512 and 1024 (`fp16` and `e8`), 200 calls each after warm-up;
- peak and idle RSS (`footprint <pid>` or `/usr/bin/time -l`), with `[128, 512]` loaded and after idle;
- compute units: default (`.cpuAndNeuralEngine` at 128, `.all` above) against `.all` and `.cpuOnly`;
- a `choice` with 33 options: expect `invalidOptionCount(33)`;
- parity: the 16-case self-test, and 200 decisions against `laya-serve` 0.4.11 (`multilingual`, revision `7b928d82…`) on the same inputs (SO1 spike §6 commands). The 200 are 20 cases from each of the ten suites of upstream's `reports/benchmark-coreml.json` (`app.email_spam`, `app.guardrails_jailbreak`, `app.model_routing_domain`, `app.moderation_toxicity`, `app.phishing`, `app.rag_relevance`, `app.support_triage`, `jev.ag_news`, `jev.emotion`, `massive_intent.en`), picked from those suites' public datasets with the selection fixed and recorded before measuring, so the gate covers `app.guardrails_jailbreak`, where `e8` failed. Record the case list with the results, and the max abs probability difference and argmax agreement per suite and precision. Gate: 0.02.

### 6.3 Signing, notarisation and the runtime copy

```sh
# after `pnpm nx run loams-desktop-electron:package-macos` (the macos-arm64 CI row) with Developer ID credentials:
codesign -dv --entitlements - "Loams Desktop.app/Contents/Resources/bin/loams-coreml"
codesign --verify --strict --deep "Loams Desktop.app"
spctl -a -vvv -t exec "Loams Desktop.app"
xcrun stapler validate "Loams Desktop.app"
# install-runtime's copy, launched by the daemon:
ls -l@ ~/Library/Application\ Support/Loams\ Desktop/agentd/runtime/current/   # quarantine xattr?
codesign --verify --strict ~/Library/Application\ Support/Loams\ Desktop/agentd/runtime/current/loams-coreml
spctl -a -vvv -t exec ~/Library/Application\ Support/Loams\ Desktop/agentd/runtime/current/loams-coreml
```

Then have the daemon start the copy, load `.mlmodelc` bundles from `<userData>/agentd/models/…`, and answer one decision; check Console.app for Gatekeeper, library-validation or `amfid` messages. Do it twice: with the shared entitlements file, and with `loams-coreml` signed under the hardened runtime and no entitlements (SO2-Q8).

### 6.4 Socket path

```sh
python3 -c 'import os,sys; p=os.path.expanduser("~/Library/Application Support/Loams Desktop/agentd/run/systemone/coreml.sock"); print(len(p.encode()))'
python3 -c 'import os; p=os.path.join(os.environ["TMPDIR"], f"loams-{os.getuid()}", "coreml.sock"); print(len(p.encode()), p)'
```
