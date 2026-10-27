# loams-agentd

`loams-agentd` is Loams Desktop's per-user agent daemon. It is headless (no GUI crate on any target, D782) and runs:
- the sessions engine;
- the agent harnesses: Claude Code, Codex, opencode, Pi, and the ACP agents Devin, Grok, Hermes, Antigravity, Goose (a pinned build in a private environment, D1809) and Loams Bot;
- local preview routing;
- the MCP shim that the engine injects into every harness run.

Clients reach it over an ndjson RPC on a loopback WebSocket. When DD1 is complete, the daemon will also supervise `loams dev` and the compose stacks, run Loams' own agent loop, and keep agent turns durable across restarts. Electron is its client.

The design is [§50, the Loams Desktop agent daemon](../../docs/design/50-loams-desktop-daemon.md). The plan is [DD1](../../docs/plans/2026-10-09-dd1-desktop-daemon.md).

## Running it from a checkout

The command-line contract (§50, plan DD1 "Shared contracts") is:

```sh
mkdir -p .agentd-dev && touch .agentd-dev/config.toml
cargo run -p loams-agentd -- run --child --config .agentd-dev/config.toml
```

`config.toml` has the keys listed in the plan's Shared contracts; every key is optional, so an empty file works. The daemon keeps its other files in the config's directory, so give it a directory of its own:
- `agentd.lock`, the single-instance lock;
- `agentd.token`, the RPC token (0600, new at every start);
- `agentd.json`, the discovery file (protocol, version, pid, port, start time, mode and `connectPort`; removed on a clean exit);
- `store/`, the sessions engine's data;
- `adapters/`, the harnesses' managed ACP adapter installs;
- `runtime/`, the installed binaries (see below);
- `logs/agentd.log`, the daemon's own log (a supervisor's capture of stdout and stderr goes to `logs/agentd.stdout.log`).

The RPC listens on `127.0.0.1` on an ephemeral port, which `agentd.json` names. `--child` (Loams Desktop starts and stops the daemon) or `--service` (the per-user service) sets the mode that `Hello` reports; without either flag, `[daemon] mode` in the config decides. `LOAMS_AGENTD_CONFIG` overrides `--config`.

Every WebSocket upgrade must carry `Authorization: Bearer <agentd.token>` and no `Origin`, with `Host: 127.0.0.1:<port>`; anything else is refused (401 or 403) before the RPC is served, and `Hello` must be the first call on a connection (design §50 §7).

Beside it, a second loopback listener serves `loams.agentd.v1`, the Connect surface Loams Studio reads (design §53 §6.1–§6.2, plan SD1 Task 5; crate `loams-agentd-connect`), on the port `agentd.json` names as `connectPort`. It keeps the same rules: the main token (`Authorization: Bearer <agentd.token>`; the MCP shim's run tokens are not accepted), `Host: 127.0.0.1:<connectPort>`, and a 403 for any `Origin`. Its handlers call the same engine dispatch as the ndjson methods, which stay as they are; `Shutdown` is not on it. Loams Desktop's renderer reaches it only through Electron main's protocol proxy (`loams-app://console/loams.agentd.v1.*`), which adds the token and lets through only the methods whose ndjson twin is `renderer: true`. `Hello` and `StudioService.GetHost` list the feature `studio.v1`.

A third listener is off unless asked for: `serve --remote --listen <addr:port> --tls-cert <pem> --tls-key <pem> --issuer <url> --audience loams-agentd --subject <sub> --allow-origin <origin> [--allow-origin …] --config <path>`, or `[remote] enabled = true` in `config.toml` with the same keys (design §53 §6.3, D909; plan SD1 Task 7). It serves `loams.agentd.v1` to one person's browser console over TLS (HTTP/1.1 and HTTP/2): an OIDC access token from `--issuer` (its JWKS from discovery, cached, refetched for an unknown key at most once a minute), whose `aud` holds `--audience` and whose `sub` is `--subject`, from no `Origin` or one of the `--allow-origin`s (CORS for those only, never with credentials), in the role `studio`, which has every Studio method except the harness installers and the account logins (Q808). It never serves the ndjson wire, `Hello` or `Shutdown`. It also forwards dev-server previews under `/preview/…` to the local preview proxy, behind the same checks, with per-preview tickets for the console's frames (SD1 Task 26). Every flag but `--allow-origin` is required, and a missing one exits 4 naming it (`loams-agentd: serve --remote needs --tls-cert`); so does a certificate or key that cannot be used. The guide is [`docs/guides/studio/remote-agentd.md`](../../docs/guides/studio/remote-agentd.md).

### What Loams Studio adds

SD1 added these to the engine, on both wires (ndjson methods with role `owner` and `renderer: true`, none in the `mcp` role; and `loams.agentd.v1`). `Hello` and `StudioService.GetHost` list each as a feature when it is served:
- `git.write`: worktree listing, commit, push (never a plain force; force-with-lease only), change requests on Forgejo and GitHub, merge and rebase in a temporary worktree, and discarding paths. Each write checks the caller's diff checksum, refuses while an agent runs in the checkout, and runs one at a time.
- `checkpoints`: a hidden commit of the chat's checkout at each turn's start and end, under `refs/loams/checkpoints/<chatId>/turn/<n>/{pre,post}`, and restore to a turn's start.
- `review`: review comments stored per checkout, sent to the thread as one turn, or posted to the pull request. `content-search`: a content search over a checkout.
- `run-groups`: one prompt to several chats, each in its own worktree, with a comparison, a check per member and a resolve. Concurrency comes from `config.toml`: `[groups] max_running_per_group = 4` and `max_running = 8`.
- `notifications`: the last 200 in memory, replayed to each watcher. `usage`: per-turn token counts, and cost only when the harness reports one, in the `turn_usage` table.
- `approvals.supervised`: per-thread approval modes and the approval list (below).

`CloneRepo` accepts only remote git URLs: `https`, `ssh`, `git` and `user@host:path`, never a local path, `file:`, `ext::` or a leading `-`. Worktrees live under `~/.loams-desktop/worktrees` on `loams-desktop/<name>` branches (`LOAMS_DESKTOP_WORKTREES_DIR` moves them). The user guides are in [`docs/guides/studio`](../../docs/guides/studio/index.md).

For tests only, the Mock harness edits files and, in a supervised thread, asks for approval first: `LOAMS_DESKTOP_HARNESS=mock`, `LOAMS_DESKTOP_MOCK_EDIT=<relative path>[,<path>…]` and `LOAMS_DESKTOP_MOCK_APPROVE`. `LOAMS_AGENTD_TEST_ISSUER` (a JWKS file for the remote listener) is honoured in debug builds only.

Each thread has an approval mode (design §53 §8.1, D911; plan SD1 Task 13). In `full-access` the harness drivers answer their agents' tool permissions themselves; in `supervised` every permission waits for the person, who answers it with `ResolveApproval` (the owner token, or the remote listener's `studio` role; never a run's `mcp` token). A thread's mode is set with `SetThreadApprovalMode` (or a run group's `approvalMode`, itself `supervised` when left out); a side chat takes its parent's; any other thread takes `[approvals] default_mode`, which is `supervised` unless the config says `full-access`, on a local daemon as with the remote listener (threat model I-1, owner 2026-10-15). A client that wants full access asks for it explicitly. An approval nobody answers is denied after `[approvals] timeout_secs` (1800). Pending approvals are kept in the store, so they are listed again after a restart; "always" is remembered per thread and tool. A harness that cannot ask (Pi) does not run a supervised thread.

Exit codes: 0 stopped, 3 already running (a second `run` on the same directory prints `already running (pid …)`), 4 invalid config (the message names the key), 70 internal error.

## The per-user service and the runtime directory

A service never runs the binary from the app bundle. `install-runtime --from <resources/bin> --to <agentd>/runtime` copies `loams-agentd` and `loams` into `runtime/<VERSION>/`, checks each copy against the bundle's `SHA256SUMS`, switches `runtime/current` to it (a symlink; `current.txt` on Windows) and keeps the newest two versions. A failed check leaves `current` where it was.

`service install|uninstall|start|stop|status --config <file>` manages the service that runs `runtime/current/loams-agentd run --service --config <file>`:
- Linux: the systemd user unit `loams-agentd.service` (in `$XDG_CONFIG_HOME/systemd/user`), `daemon-reload`, then `enable --now`;
- macOS: the LaunchAgent `dev.loams.agentd` (`~/Library/LaunchAgents`), with `launchctl bootstrap gui/<uid>`; launchd writes the daemon's stdout and stderr to `logs/agentd.stdout.log`;
- Windows: the Run value `LoamsAgentd` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. There `run --service` is a parent that runs `run --worker` in a Job Object and restarts it after a crash (not after exit 0, 3 or 4), with a backoff of 1 to 30 s and at most 5 restarts in 10 minutes.

No unit captures the environment; the daemon resolves the login shell's `PATH` when it starts. `service enable-linger` and `disable-linger` run `loginctl enable-linger|disable-linger "$USER"` on Linux (linger is off by default, and the daemon never turns it on itself) and answer `unsupported` elsewhere.

The other subcommands are `-- status --config <file>`, `-- version` and `-- mcp`. The `mcp` subcommand is the stdio shim; the daemon injects it into each harness run with a token scoped to that run (`LOAMS_AGENTD_RUN_TOKEN`, `LOAMS_AGENTD_PORT`, `LOAMS_AGENTD_SESSION_ID`), so it is not useful on its own.

Build and test one crate at a time with `-p`, for example `cargo test -p loams-agentd`. Use the shared target directory (never set `CARGO_TARGET_DIR`). `cargo clippy -p 'loams-agentd*' --all-targets -- -D warnings` runs the lint gate, and `scripts/ci/agentd-deps.sh` runs the headless guard.

## Provenance

The daemon began as **zeron** (https://github.com/zeronsh/zeron), MIT, "Copyright (c) 2026 Wing". Zeron came to Loams through the Loams Desktop fork, `ostrium-labs/loams-desktop`, which keeps zeron's history:
1. The fork's tracked files were imported into `apps/desktop/native` from fork commit `c9205f8f949711c3315af551c804c56452d2fbd7`.
2. DD1 Task 1 moved the headless crates into the root workspace as `crates/loams-agentd*` (D783).
3. GPUI, the headed mode, the edge, WorkOS, Cursor, self-update and push were deleted (D781, D782).

[`import-provenance.json`](import-provenance.json) records:
- the source commit;
- the SHA-256 of every file as it was imported;
- the fork's dependency closure;
- under `moved_to_root_workspace`, where each fork crate went and its licence.

Files kept with this crate:
- [`LICENSE`](LICENSE): zeron's MIT licence, unmodified.
- [`NOTICE`](NOTICE): the attribution and licence scope.
- [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md): third-party assets bundled with the daemon (none today; zeron's GUI entries were removed with the GUI).
- [`SCOPED_NOTICE.md`](SCOPED_NOTICE.md): the original import notice.

## Licences

Every `loams-agentd*` crate is listed here. The workspace test `licence_fields_are_set` (`tests/workspace.rs`) reads `cargo metadata` and checks each crate against this table:
- each crate's `license` matches its row;
- a `zeron` crate is MIT;
- a `Loams Desktop` or `new` crate is Apache-2.0;
- a `new` crate takes `license.workspace = true`.

Only the `new` rows may name a crate that does not exist yet. A crate added to the workspace must be added to this table.

| Crate | Licence | Origin | Notes |
|---|---|---|---|
| `loams-agentd` | MIT | zeron | The binary, from the fork's `apps/loams-desktop`, rewritten headless |
| `loams-agentd-sessions` | MIT | zeron | From the fork's `engine` |
| `loams-agentd-harness` | MIT | zeron | From the fork's `harness` |
| `loams-agentd-proto` | MIT | zeron | From the fork's `proto`. It also holds the brand strings of the fork's Apache-2.0 `loams-desktop-brand`; the Loams Authors place those lines under MIT (T0-10a) |
| `loams-agentd-rpc` | MIT | zeron | From the fork's `rpc` |
| `loams-agentd-doc` | MIT | zeron | From the fork's `doc` |
| `loams-agentd-store` | MIT | zeron | From the fork's `sync` |
| `loams-agentd-mcp` | MIT | zeron | From the fork's `mcp` |
| `loams-agentd-preview` | MIT | zeron | From the fork's `preview` |
| `loams-agentd-link` | Apache-2.0 | Loams Desktop | From the fork's `loams-desktop-link`, written by the Loams Authors; its own `LICENSE` |
| `loams-agentd-llm` | Apache-2.0 | new | Model providers (DD1 Task 18) |
| `loams-agentd-loop` | Apache-2.0 | new | The native loop, tool registry, approvals and scrubbing (DD1 Tasks 19, 24, 25) |
| `loams-agentd-tools` | Apache-2.0 | new | Loams and SQL tools (DD1 Tasks 20, 21) |
| `loams-agentd-marketplace` | Apache-2.0 | new | The Seed Exchange in agentd: listing schema, signed index, install plans and installers (MK1 Tasks 1, 4–6) |
| `loams-agentd-factory` | Apache-2.0 | new | Factory ops (DD1 Task 23): the apps' credential store and read-only op allowlist (SF1 amendment F1-F6) |
| `loams-agentd-supervisor` | Apache-2.0 | new | The engine and stack supervisor (DD1 Tasks 14, 15) |
| `loams-agentd-connect` | Apache-2.0 | new | `loams.agentd.v1`, the Connect surface for Loams Studio: the loopback listener beside the ndjson one, and the remote TLS listener (SD1 Tasks 4, 5, 7) |
| `loams-agentd-cli` | Apache-2.0 | new | The `loams` CLI commands of agentd, including `loams marketplace` (MK0 Task 10) |

Changes that the Loams Authors make to the MIT crates are released under those crates' MIT licence.
