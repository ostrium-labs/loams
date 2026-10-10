# loams-agentd

`loams-agentd` is Loams Desktop's per-user agent daemon. It is headless (no GUI crate on any target, D782) and runs:
- the sessions engine;
- the agent harnesses: Claude Code, Codex, opencode, Pi, and the ACP agents Devin, Grok, Hermes, Antigravity and Loams Bot;
- local preview routing;
- the MCP shim that the engine injects into every harness run.

Clients reach it over an ndjson RPC on a loopback WebSocket. When DD1 is complete, the daemon will also supervise `loams dev` and the compose stacks, run Loams' own agent loop, and keep agent turns durable across restarts. Electron is its client.

The design is [§50, the Loams Desktop agent daemon](../../docs/design/50-loams-desktop-daemon.md). The plan is [DD1](../../docs/plans/2026-10-09-dd1-desktop-daemon.md).

## Running it from a checkout

The command-line contract (§50, plan DD1 "Shared contracts") is:

```sh
cargo run -p loams-agentd -- run --child --config <file>
```

Here `<file>` is a `config.toml` like the one in the plan's Shared contracts. `--child` ties the daemon's life to its parent; `--service` is for the per-user service.

`--child`, `--service` and `--config` arrive in DD1 Task 6. Until then `run` takes no flags and is configured through the environment:

```sh
LOAMS_DESKTOP_DATA_DIR="$PWD/.agentd-dev" LOAMS_DESKTOP_IPC_PORT=27654 \
  cargo run -p loams-agentd -- run
```

The other subcommands are `cargo run -p loams-agentd -- status`, `-- version` and `-- mcp`. The `mcp` subcommand is the stdio shim; the engine starts it for harness runs, so you rarely need to run it yourself.

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
- [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md): zeron's third-party notices, as imported.
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
| `loams-agentd-factory` | Apache-2.0 | new | Factory ops (DD1 Task 23) |
| `loams-agentd-supervisor` | Apache-2.0 | new | The engine and stack supervisor (DD1 Tasks 14, 15) |

Changes that the Loams Authors make to the MIT crates are released under those crates' MIT licence.
