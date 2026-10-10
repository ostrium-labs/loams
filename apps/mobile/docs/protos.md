# Protos and generated code

Both apps and the mock speak the app protos of design §37 §8 (`loams.instance.v1`, `loams.devices.v1`, `loams.approvals.v1`, `loams.operations.v1`, `loams.notifications.v1`, `loams.errors.v1`). They are defined in the main repository's `proto/` and **vendored** here.

## Where they come from

`conformance/proto-ref.lock` records the upstream repository, the commit `proto/` was last synced from, the packages the phones use, which of them are still **placeholders**, and a hash of `proto/`.

> **Today every package is a placeholder.** AP0 (the upstream protos and `loams-apps-mock`) has not merged yet, so the files under `proto/loams/` were written here from the AP0 plan and carry a `PLACEHOLDER` header. When AP0 merges, run `scripts/sync-protos.sh <merge-commit>`: it replaces each placeholder that now exists upstream, empties the `placeholder` line, updates the hash and regenerates. Then fix the apps against the real contract in the same PR. The one field the placeholders add beyond the plan, `GetInstanceResponse.identity_provider` (the Authentik issuer and client ids for browser sign-in), is a request to AP0.

Package names are `loams.*` (Q422, answered by the rename PR); Connect URL paths contain them, for example `/loams.approvals.v1.ApprovalService/WatchApprovals`.

## Generated code is committed

| Output | Generator (buf remote plugins) | Used by |
|---|---|---|
| `android/proto/src/generated/{java,kotlin}` | `buf.build/protocolbuffers/java` (`lite`), `buf.build/connectrpc/kotlin` | `:proto` |
| `ios/Packages/LoamsProto/Sources/LoamsProto/Generated` | `buf.build/apple/swift`, `buf.build/connectrpc/swift` (`GenerateAsyncMethods`) | `LoamsProto` |
| `mock/gen` | `buf.build/protocolbuffers/go`, `buf.build/connectrpc/go` | the mock |

**Decision: vendor the generated code rather than generate at build time.** The AP2 and AP3 plans proposed generating in a Gradle task and a script. Committing it instead means:

- building the Android app needs only a JDK and the Android SDK, on Linux or Windows, with no `buf` and no network access to buf.build;
- Xcode builds need no build-phase script;
- a contract change shows up as a reviewable diff.

The cost is generated files in the tree (marked `linguist-generated` in `.gitattributes`, so GitHub collapses them). CI's `protos` job runs `scripts/check-drift.sh`, which fails when `proto/` does not match the lock's hash or when regenerating changes anything.

## Commands

```sh
scripts/generate.sh                  # regenerate everything from proto/ (needs buf 1.73+)
scripts/check-drift.sh               # what CI runs
scripts/sync-protos.sh <ref>         # re-vendor from upstream at <ref>, then regenerate
scripts/update-lock.sh               # only after editing a placeholder by hand
```

The upstream repository is private until the move to `ostrium-labs/loams`, so `sync-protos.sh` needs your own git access and CI cannot yet check that `proto/` equals upstream at `ref`. Once upstream is public, the `protos` job adds that check.

## Wire choices (from §37 §8.3)

Connect protocol, binary codec, unary and server-streaming only. Reads are marked `NO_SIDE_EFFECTS` (Connect clients may send them as GET); every mutation takes an `idempotency_key`; every `Watch*` stream sends a snapshot, then changes, then a heartbeat every 15 s, and resumes from a cursor; errors carry a `loams.errors.v1.ErrorInfo` with a stable `reason`.
