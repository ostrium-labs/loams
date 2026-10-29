![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# The TypeScript SDK (`@loams/client`)

Design §44 §7 and §10.2: the SDK is a `Loams` object with namespaced modules,
generated from the `loams.options.v1` annotations on the protos, over the
unified Connect API on one port. This directory is SDK2 Task 0's deliverable,
the reference the other twelve SDKs follow.

```
sdks/typescript/
  buf.gen.yaml            # the SDK facade: protoc-gen-loams-facade -> client/src/gen
  buf.gen.live.yaml       # @loams/live's stubs: protoc-gen-es
  packages/
    client/               # @loams/client: the generated facade + the runtime
    live/                 # @loams/live: the generated loams.live.v1 stubs
```

`@loams/client` and `@loams/live` are workspace packages of `web/` (see
`web/pnpm-workspace.yaml`), so they resolve `@loams/proto` — the console's
generated package — as a workspace dependency and share one lockfile. The
generator inputs stay outside the workspace, next to `proto/`.

## Install

```sh
npm i @loams/client        # and @loams/live for Live's stubs
```

`0.1.3` tracks Loams 0.1.3; in 0.x the API may change between minor versions.

## Using it

```ts
import { Loams, apiKey, FeatureNotInVariantError } from '@loams/client';

const loams = new Loams({
  endpoint: 'https://acme.loams.dev',
  auth: apiKey(process.env.LOAMS_API_KEY!),   // or envToken(), oidcExchange({...})
});

const info = await loams.instance.getInstance({});   // no auth needed
await loams.system.guard('live');                    // costs no RPC once cached

try {
  await loams.tables.mutate(request);
} catch (error) {
  if (error instanceof FeatureNotInVariantError) {
    // This build variant does not carry loams.live.v1.
    console.warn(error.variant);                    // 'standard'
  }
}
```

One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the same
object works in a browser, in Node and in Bun. The default transport is
Connect over `fetch`.

Deno is not supported (design 79, D1790): Deno's team is ending the runtime's
development. The default `fetch` transport uses only standard Web APIs, so the
SDK may still run on Deno, but we don't test it there or promise it works.
Moving off Deno? See [Moving from Deno to Bun](../../docs/guides/migrate-deno-to-bun.md). For HTTP/2 gRPC on a server:

```ts
import { Loams, createNodeTransport } from '@loams/client/node';
const loams = new Loams({ endpoint, transport: createNodeTransport() });
```

The `/node` subpath exists so a bundler never follows `@connectrpc/connect-node`
into a browser build it cannot satisfy; the conformance suite checks that
statically.

## Build an app

The app-building layer (SDKP) adds builders, a collection handle and the
runtime an app needs: a default namespace, deadlines, read-your-writes
consistency tokens, chunked bulk writes and stream subscriptions.

```ts
import { Filter, Loams, Schema, Search, Sort, rrf } from '@loams/client';

const loams = Loams.fromEnv();              // LOAMS_ENDPOINT, LOAMS_API_KEY, LOAMS_NAMESPACE
// or: new Loams({ endpoint, auth: key, namespace: 'app', sessionConsistency: true })

await loams.collections.create({
  name: 'notes',
  schema: new Schema().text('title').text('body').keyword('tags').i64('n')
    .vector('embedding', 4).toJson(),
});

const notes = loams.collection('notes');    // calls with `collection` filled in
await notes.upsert([
  { id: 1, source: { title: 'Refunds', body: 'refund policy', tags: 'policy', n: 1 },
    vectors: { embedding: [1, 0, 0, 0] } },
]);

const hits = await notes.search(
  notes.query()
    .text('refund', { fields: ['body'] })
    .vector('embedding', [1, 0, 0, 0], { k: 10 })
    .fuse(rrf(60))
    .filter(Filter.and(Filter.eq('tags', 'policy'), Filter.range('n', { gte: 1 })))
    .highlight('body')
    .select({ include: ['title'] })
    .limit(5),
);

for await (const hit of loams.search.queryAll(
  Search.on('notes').text('refund').sort(Sort.score(), Sort.pk()).limit(100),
)) { /* every hit, page by page */ }

await notes.writeBatch(rows);               // 10 000 rows / 4 MiB a chunk, one key per chunk
```

- **Default namespace (R11).** `namespace` fills every request that has a
  `namespace` field and leaves it empty; an explicit one wins, and neither is
  `InvalidArgumentError` before any network call.
- **Deadlines (R12).** Unary calls default to 30 s (`timeoutMs` on the client
  or per call; `0` disables). Running out raises `DeadlineExceededError` with
  reason `client_deadline_exceeded`, which is never retried. Streams have no
  default deadline.
- **Consistency tokens (R4).** `ConsistencyToken.parse/merge/toString`; with
  `sessionConsistency: true` every token a response carries merges into
  `loams.consistency`, and every read with a `consistency` field is sent
  `at_least` the session.
- **Bulk writes (R13).** `documents.writeBatch` sends one chunk at a time,
  each with its own idempotency key, returns the merged token, and stops at
  the first failed chunk with a `BatchError` (`chunksApplied`, `rowsApplied`).
- **Subscriptions (R14).** `streams.subscribe({ stream, partition, fromOffset })`
  is an async iterable over `Fetch` that exposes `offset` for resuming.

The modules `loams.namespaces`, `loams.collections`, `loams.documents`,
`loams.search`, `loams.vector` and `loams.graph` are generated from the protos'
facade annotations, with the hand-written conveniences above added to them.
`loams.sql` is generated from `loams.sql.v1` the same way: `sql.query` answers
`columns` and `rows`, `sql.records(statement, [$1, …])` answers one object per
row, and `sqlRecords(answer)` reads an answer already in hand. `sql.queryArrow`
answers the result as Arrow IPC bytes (`arrowIpc`), which `apache-arrow`'s
`tableFromIPC` reads; the SDK does not depend on it. `loams.streams` is
generated from `loams.stream.v1`: `create`, `get`, `list` (and `listAll`),
`produce` (pass `producerId` and `producerSeq` so a retry cannot append
twice), `fetch`, `produceEvents` and `fetchEvents` (CloudEvents as a JSON
batch), plus `streams.subscribe(...)` (R14), an async iterable over `fetch`
whose `offset` is where to resume. `examples/notes/main.ts` is the whole app
scenario as a program, type-checked in CI. The session's token is used only for
collection and SQL reads: a graph's token names one graph (SDKP-17). Durable
workflows use the Resonate SDK pointed at `LOAMS_ENDPOINT/durable/` (SDKP-1).

## Cloudflare Workers

`@loams/client` runs in Cloudflare Workers (workerd) with no `nodejs_compat` flag and no `fetch` override. The
default transport detects workerd (`navigator.userAgent === "Cloudflare-Workers"`) and sends
`redirect: "manual"`, because connect-web's default `redirect: "error"` is rejected by Workers. Loams calls never
redirect, so nothing changes for them. Other runtimes are untouched: in a browser `manual` would hide a redirect
as an opaque response.

```ts
const loams = new Loams({ endpoint: env.LOAMS_URL, token: env.LOAMS_TOKEN });
```

To control `fetch` yourself (a proxy, a service binding, a test double), pass it through the transport options;
it is used exactly as given, so set `redirect: "manual"` in it if you also run in workerd:

```ts
new Loams({ endpoint, transportOptions: { fetch: env.GATEWAY.fetch.bind(env.GATEWAY) } });
```

## What is generated and what is hand-written

Design §44 §7.3 draws the line, and this SDK follows it exactly.

**Generated**, by `buf generate` and never hand-edited:

- the messages and service descriptors (`protoc-gen-es`), in `@loams/proto`
  (the app packages) and `@loams/live` (`loams.live.v1`);
- the **facade** — `src/gen/facade.ts`, written by `protoc-gen-loams-facade`
  (`crates/loams-facade-gen`): one typed interface per service annotated with
  `loams.options.v1.module`, one method per `loams.options.v1.facade` call, the
  binding table the runtime dispatches through, and the `Reason` union read
  from `docs/api/reasons.md`.

**Hand-written**, once, in `src/runtime/`: the transport, the token sources, the
retry loop, the error mapping, the token store, the pagination iterator and the
streams.

`src/loams.ts` is the join: it reads the generated table, builds one object per
module out of it, and hands every call to the same invoker. It contains no
method names and no RPC paths, which is why annotating a proto is enough to add
an SDK method in thirteen languages.

## Adding a method to the SDK

1. Annotate the service with a module, and the method with a facade call, in the
   proto:

   ```proto
   service ThingService {
     option (loams.options.v1.module) = { name: "things" summary: "Things." };

     rpc Get(GetRequest) returns (GetResponse) {
       option idempotency_level = NO_SIDE_EFFECTS;
       option (loams.options.v1.facade) = { name: "get" };
     }
   }
   ```

   A service with no `module` option is not in the SDK, and a method with no
   `facade` option is not exposed — which is how an internal RPC stays out of
   every SDK by default.

2. `pnpm --filter @loams/client generate` (or `scripts/sdk/gen.sh typescript`).
3. `pnpm --filter @loams/client typecheck && pnpm --filter @loams/client test`.

CI regenerates and fails on a diff, and `crates/loams-facade-gen`'s
`golden_typescript` compares the committed facade against the generator's
output over the real `proto/` tree.

## Tests

```
pnpm --filter @loams/client test                              # the recorded corpus
LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 pnpm --filter @loams/client test   # a live server
sdks/conformance/run.sh typescript                            # starts a server and does both
```

Without `LOAMS_TEST_ENDPOINT` the suite replays `sdks/fixtures/recorded`, which
was captured from a real `loams dev` by `sdks/conformance/record-fixtures.mjs`.
That is deliberate: CI cannot boot a Rust server for each of the thirteen SDKs
on every change. `run.sh` runs the same suite against a live server, which is
what catches a corpus that has drifted from the API.

The suite covers the three things the design asks of a conforming SDK: a
successful call in every encoding a client might pick, a structured-reason
error, and the unavailable-service path in all three of its shapes (the guard,
the unary refusal, and the refusal on a stream). See
`docs/sdk/runtime-contract.md` for the clauses and `src/runtime/` for the code.
