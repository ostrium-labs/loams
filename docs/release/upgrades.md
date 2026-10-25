# Upgrading a self-hosted stack

From v0.1.0 on, a Loams compose stack is upgraded in place and keeps its data (design
[§54](../design/54-upgrades.md)). `loams upgrade` does it, one release at a time. It takes a snapshot first, and
`loams upgrade rollback` puts the stack back. preview.1 → preview.2 kept no data. preview.2 → 0.1.0 is the
first step that keeps it.

## Quick start

From the stack's directory (the bundle's `release/`, the one with `compose.yaml` and `.env`):

```sh
loams upgrade check --to 0.1.1  # what it will do, and what would stop it
loams upgrade --to 0.1.1        # asks first; --yes to skip the question
loams upgrade status            # the stack's release, the ledger, the last attempt
```

If anything is wrong afterwards:

```sh
loams upgrade rollback                       # restores the snapshot; prints the writes it loses first
```

`--dir DIR` (or `LOAMS_STACK_DIR`) names the stack directory when you are elsewhere. The `loams` CLI is in the
release packages (`.deb`, `.rpm`, `.pkg.tar.zst`, the macOS and Windows archives).

## What an upgrade does

`loams upgrade --to V` runs these steps (§54 §10.3). Every step that touches data runs as `loams migrate …` in a
container of the **target** release's `loams` image. The CLI itself reads no store.

1. **Preflight.** `check`'s blockers must be empty, and every service healthy.
2. **Bundle.** It downloads the target's compose bundle and checks it against the release's `SHA256SUMS`.
   `--bundle DIR` takes a local bundle instead (a checkout at the target's tag); the report says it is
   unverified.
3. **Pull** the target's images. The stack still serves, and a pull failure changes nothing.
4. **Close the door.** The Tailscale sidecars are removed (their state volumes stay), and so is any other
   long-running service in another service's network namespace (`network_mode: service:…`): no client reaches
   the stack until step 12. Removed, not stopped, because Podman cannot recreate the engine or the House while
   a container, even a stopped one, shares its network namespace (see "Recreating a service by hand" below).
5. **Quiesce.** The writers stop: agentd, the console, the Postgres computes, `pg-control`, `pg-runner`, the House,
   the engine, TiDB.
6. **Snapshot** (see below).
7. **Install the bundle.** Its files replace the stack directory's; `.env`, `secrets/` and your
   `compose.override.yaml` stay. The release's new `.env` variables are added with their defaults (your values
   are kept), `LOAMS_VERSION` is set, and the old `.env` is kept as `.env.bak-<from>` (mode 0600).
8. **Third-party restarts:** PD, then TiKV.
9. **Migrate:** `loams migrate run` applies the release's migrations and records each in the ledger.
10. **Ordered start:** TiDB, the engine, the House, the Postgres stack, then the rest.
11. **Verify:** `loams migrate verify` reads every product's data, every service is healthy, and the engine
    answers `/ready`.
12. **Open the door:** the sidecars are created and started again.

A failure in steps 1–3 changes nothing. A failure in steps 4–7 restarts the old release on its unchanged data. A
failure in steps 8–11 rolls back from the snapshot automatically, with the door still closed, and exits 10
(`upgrade_failed_rolled_back`). With `--no-auto-rollback` the stack stays stopped with the door closed: rerun the
upgrade to resume it, or run `rollback`.

`--to` beyond one release walks the chain, one verified step at a time, and stops at the first failure. Every
step's snapshot is kept until the last step verifies. If the CLI is killed, run the same command again: it reads
`.loams-upgrade/history.jsonl` and resumes the attempt.

## Snapshots

The snapshot is taken with the writers stopped, by the target image:

- **TiKV** (the metastore, Live, TiDB, Durable on TiKV):
  - with BR (`br backup txn`), when the stack's `tikv` mounts the snapshot volume (0.1.0 and later);
  - otherwise, as preview.2 does, as a cold copy of `pd-data` and `tikv-data`, with PD and TiKV stopped for the
    copy.
- **The bucket:** objects are immutable. The snapshot copies the few mutable pointer objects and records a GC
  root, so nothing it needs is collected while it exists.
- **Postgres:** the safekeeper is drained first: the pageserver has uploaded everything the safekeeper flushed.
  Then every timeline is branched at its last record LSN.
- **The House's directory, `pg-control`'s records and Durable's SQLite (backup API)** are copied.
- **The stack directory** (`.env`, `secrets/`, the bundle) is copied to `.loams-upgrade/snapshots/<id>/host/`.

Snapshots live in the `upgrade-snapshots` volume, or in a host directory if you set `LOAMS_UPGRADE_SNAPSHOT_DIR`.
They are mode 0700, files 0600, and they are never uploaded. They hold your data and your secrets: treat them like
the stack itself. The latest snapshot is kept until the next upgrade verifies. `loams upgrade snapshots list`
shows the snapshots; `loams upgrade snapshots prune --id ID` removes one and its GC root.

`check` estimates a snapshot's size and needs 1.5× that free (`insufficient_space` otherwise).

## Rollback

`loams upgrade rollback` (§54 §8):
1. closes the door and stops everything;
2. puts the previous bundle, `.env` and `secrets/` back;
3. restores TiKV at the snapshot's version: the cold copy is unpacked, or BR restores into fresh volumes;
4. restores the rest with `loams migrate restore`;
5. starts the previous release, checks it and opens the door.

Writes made since the snapshot are lost. The CLI prints the window (`writes between … and … UTC will be lost`)
before it asks. `--snapshot ID` restores an older snapshot.

`rollback --keep-writes` swaps the images back and keeps the data, so writes made after the upgrade survive. The
CLI allows it only when every migration of the step is **Reversible** (the previous release reads what it wrote)
and no third-party pin changed. Otherwise it exits 6 (`rollback_needs_snapshot`). The release gate proves each
Reversible claim before a release ships.

## What is reversible in each release

| Release | Migration | Store | Class |
|---|---|---|---|
| 0.1.0 | `0000-adopt-preview-2`: records preview.2's versions (preview.2 wrote no stack record) | meta | Reversible |
| 0.1.0 | `0001-safekeeper-volume`: gives the safekeeper a volume, after a drain | host | Reversible |
| 0.1.1 | None (from any earlier 0.1.x). A 0.1.0 stack keeps its stores with `COMPOSE_PROFILES=tikv,rustfs`, Durable on SQLite included | | |
| 0.1.2 | None (from any earlier 0.1.x). Durable on TiKV's keyspace keeps its layout (FW1). Remove `LOAMS_CORS_ORIGINS=https://console.loams.dev` from `.env` by hand | | |

`loams upgrade rollback --keep-writes` back to 0.1.0 is refused: 0.1.0's startup gate does not serve data a later
release stamped (fixed after 0.1.0, design §54 §17 T14-2), so it would not start. Roll back from the snapshot
instead (the default). From 0.1.1 on, `--keep-writes` works whenever every migration of the step is reversible.

The release notes of every release that keeps data have an "Upgrading" section with its table, its third-party
pins and its expected downtime (`scripts/release/upgrade-notes.sh`).

## Rules worth knowing

- **One step at a time.** Each release upgrades from the one before it only. From v0.1.0 on, a patch release
  upgrades from any earlier patch of its minor line. `migrate` refuses any other source
  (`upgrade_step_not_adjacent`).
- **Never by hand across preview.2 → 0.1.0.** `docker compose up` with the new bundle would recreate the
  safekeeper before its WAL is drained. Use `loams upgrade`.
- **No downgrades of third-party components.** A target whose PD, TiKV or TiDB pin is older than the stack's
  is refused (`component_downgrade`). No Postgres major changes through `loams upgrade`.
- **An upgrade keeps a stack on its stores** (D1340). A stack from 0.1.0 or earlier runs PD, TiKV and RustFS
  always; the light default that follows it does not. Before upgrading such a stack, add
  `COMPOSE_PROFILES=tikv,rustfs` to its `.env`, so the new compose file keeps them (without it the engine
  starts on an empty embedded catalog beside the TiKV data, which stays unread). `loams upgrade` follows the
  profiles: TiKV's snapshot (BR or a cold copy) and its ordered start only with `tikv`.
- **A stack on the embedded stores** (no `tikv` profile) is not upgraded by `loams upgrade` yet: `loams migrate`
  cannot open the embedded metastore while the engine runs, and `check` refuses it (`stack_unrecognised`).
  Upgrade it by hand: back up the volumes, `docker compose stop`, set `LOAMS_VERSION` in `.env`, then
  `docker compose up -d`; `migrate` runs before the engine, and the engine refuses data with a migration still
  pending.
- **The engine refuses data it cannot read.** It exits 78 (`format_unreadable`, `migration_incomplete`,
  `stack_unrecognised`) rather than start. `loams migrate status` shows why.
- **Recreating a service by hand.** A sidecar with `network_mode: service:X` (the `tailscale` sidecar is in
  `loams`'s namespace, `house-tailscale` in `house`'s) must go before `X` is recreated, so remove it first and
  let compose create it again (see below).
- **The hosted console** supports the newest engine and the one before it. An older engine gets an "Upgrade
  your engine" page.

## Troubleshooting

Every error has a code. `--output json` prints `{"error": code, "message": …}`.

| Exit | Codes | What to do |
|---|---|---|
| 3 | `confirmation_required` | Run on a terminal, or pass `--yes`. |
| 4 | `stack_not_found`, `release_not_found`, `snapshot_not_found` | Pass `--dir` with the bundle's `release/` directory; check the version; `snapshots list`. |
| 5 | `upgrade_step_not_adjacent` | Upgrade one release at a time: the message names the chain. |
| 5 | `component_downgrade` | The target pins an older PD, TiKV or TiDB than the stack runs: not supported. |
| 5 | `insufficient_space` | Free space where the snapshots go, or set `LOAMS_UPGRADE_SNAPSHOT_DIR` to a bigger disk. |
| 5 | `stack_unhealthy` | `docker compose ps`: start or fix the named services first; the upgrade does not snapshot a broken stack. |
| 5 | `migration_incomplete`, `upgrade_in_progress` | An attempt did not finish: run the same `loams upgrade --to V` to resume it, or `loams upgrade rollback`. |
| 5 | `snapshot_incomplete` | The snapshot has no manifest (it was interrupted): it cannot be restored. Use an older one, or prune it. |
| 6 | `rollback_needs_snapshot` | `--keep-writes` is not possible for this step: roll back from the snapshot. |
| 6 | `stack_unrecognised`, `platform_unsupported` | The data is not a known release's, or neither `docker compose` nor `podman compose` works here. |
| 7 | `timeout`, `network`, `registry` | Retry; a pull failure changes nothing. `timeout` during the snapshot is the Postgres drain: check the pageserver and the safekeeper. |
| 9 | `checksum_mismatch`, `signature_invalid`, `manifest_invalid` | The download does not match the release: do not use it. |
| 10 | `upgrade_failed_rolled_back` | The upgrade failed and the stack is back on the previous release, as it was. The message says why; `status` shows the attempt. |
| 11 | `rollback_failed` | The rollback failed too. The message names the snapshot, its host files and the manual steps: stop the stack, copy the host files back, restore TiKV, run `loams migrate restore --from <snapshot>` in the target's `loams` image, start the stack. |

### Recreating a service by hand

After you change `.env` (or the image) yourself, `docker compose up -d` recreates the services it affects. On
Podman, recreating `loams` (or `house`) fails while its sidecar exists, even stopped:

```text
Error response from daemon: container … has dependent containers which must be removed before it: …
```

Remove the sidecars in its network namespace first; `up` creates them again:

```sh
docker compose rm -sf tailscale && docker compose up -d loams tailscale              # the engine
docker compose rm -sf house-tailscale && docker compose up -d house house-tailscale  # the House
```

`podman compose` runs the same commands when it uses Docker Compose. `podman-compose` (the Python tool) has no
`rm`, and its `up -d` exits 0 while it keeps the old container running: remove the sidecar with
`podman rm -f <project>_tailscale_1` (`podman ps -a` shows the name), then `podman-compose up -d`. A failed
recreate can leave a container named `<id>_<project>-loams-1` beside the old one; the commands above still bring
the service up on its new settings. `loams upgrade` and `loams upgrade rollback` remove the sidecars themselves.

For a BYOC VM that `byoc/install.sh` set up, `deploy/release/byoc/upgrade.sh user@host --to V` runs the same
upgrade on the VM over SSH (`--check`, `--rollback`); see [`deploy/release/README.md`](../../deploy/release/README.md).
