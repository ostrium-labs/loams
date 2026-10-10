![Loams — Your data. Your bucket.](../../../../docs/assets/loams-banner.svg)

# `@loams/live`

The generated `loams.live.v1` messages and the `LiveService` descriptor that
[`@connectrpc/connect`](https://connectrpc.com) v2 turns into a client: Loams
Live, the reactive database on TiKV (design [§20](../../../../docs/design/20-reactive-database-on-tikv.md)).

It carries no hand-written runtime. `@loams/client` imports the types through
the `@loams/live/live` and `@loams/live/value` subpaths, which is why the
service's package shows up in the SDK's facade bindings.

The package is **unstable** (design [§44](../../../../docs/design/44-unified-api-and-sdks.md)
§10.3): the wire contract may still change, `buf breaking` skips it, and an SDK
built on it marks its Live modules experimental.

## Layout

| Path | What it is |
|---|---|
| `src/gen/loams/live/v1/live_pb.ts` | generated: the `LiveService` messages and descriptor |
| `src/gen/loams/live/v1/value_pb.ts` | generated: the `loams.live.v1` value messages |
| `src/gen/loams/options/v1/options_pb.ts` | generated: `loams.options.v1`, which `LiveService` annotates |
| `src/index.ts` | hand-written: the re-exports and `LIVE_API_PACKAGE` |

Everything under `src/gen` is written by `buf generate` and committed. **Do not
edit it.**

Protos generated from [`proto/`](../../../../proto):

- **public** — `loams/live/v1/live.proto` and `loams/live/v1/value.proto`, plus
  `loams/options/v1` because the service descriptor references it;
- **server-internal, not generated** — `loams/live/v1/journal.proto`,
  `loams/live/v1/catalog.proto` and `loams/live/v1/idempotency.proto`. They are
  the storage and transaction details a client must not see.

## Regenerate

Needs Node 22 and pnpm 11; `buf` and `protoc-gen-es` come from the pinned
`devDependencies`, and the plugin version is an exact pin (SDK1's global
constraint), so a bump is its own PR with the regenerated output.

```bash
pnpm install
pnpm --filter @loams/live generate
```

CI regenerates and fails on a diff, so `git status` must be clean afterwards.

## Where it fits

[`sdks/typescript/README.md`](../../README.md) covers the SDK as a whole; the app
packages use `buf.gen.apps.yaml` and the facade uses `buf.gen.yaml`, both beside
this folder.
