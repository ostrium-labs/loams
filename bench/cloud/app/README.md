# CB1 bench app

The fixture app for the CB1 cloud free-tier benchmark
([plan](../../../docs/plans/2026-10-27-cb1-cloud-free-tier-bench.md) §4). It is
design 71's portable app contract (D1391, `export default { fetch, queue }`)
with only the `items` and `queue` ports a benchmark needs, so the numbers are
the cloud's and not a framework's. It is not `@loams/app` (AB0).

Status: **preview, not yet run on a cloud.** The unit tests and the bundles are
checked in CI (`.github/workflows/cb1-bench.yml`); no cloud run has happened.

## Routes

| Route | What |
|---|---|
| `GET /ping` | No store. The instance id, whether this is the instance's first request, its uptime |
| `GET /items/:k` | A strongly consistent read |
| `PUT /items/:k` | An unconditional write of the request body |
| `POST /enqueue?run=` | Sends a 256-byte CloudEvent to the queue port |
| `GET /queue/stats?run=` | The consumer's recorded delivery lags for the run: count, p50, p95, p99, max |

A store throttle is `429` (`{"error":"throttled"}`), never hidden behind
retries: every adapter makes one attempt.

## Layout

| Path | What |
|---|---|
| `src/app.mjs` | The contract: `fetch` and `queue` |
| `src/adapters/aws.mjs` | DynamoDB and SQS (AWS SDK v3 from the Lambda runtime, `maxAttempts: 1`) |
| `src/adapters/gcp.mjs` | Firestore and Cloud Tasks over REST with the metadata server's token |
| `src/adapters/azure.mjs` | Cosmos DB and Queue Storage over REST with the managed identity's Entra ID token |
| `src/adapters/memory.mjs` | In-memory ports for tests and the harness's local dry run |
| `src/shims/lambda.mjs` | Function URL events to `fetch`, SQS batches to `queue` (partial batch failures) |
| `src/shims/azure.mjs` | Functions v4 HTTP trigger to `fetch`, storage queue trigger to `queue` |
| `src/shims/cloudrun.mjs` | `Bun.serve`; Cloud Tasks pushes to `/_loams/queue` go to `queue` |
| `src/shims/node.mjs` | `node:http` with the in-memory ports (`node src/shims/node.mjs`, port 8787) |
| `Containerfile.cloudrun` | The Cloud Run image: a Bun binary compiled ahead of time on distroless (design 79 D1791) |

The environment variables the adapters read are the ones the free-tier modules
set (`deploy/tofu/modules/{aws,gcp,azure}-free`): `LOAMS_ITEMS`,
`LOAMS_QUEUE_URL`, `LOAMS_PROJECT`, `LOAMS_QUEUE`, `LOAMS_COSMOS_*` and so on.

## Build and test

```sh
npm ci
npm test                 # node --test, against the in-memory ports and fake cloud APIs
node build.mjs           # dist/aws.zip (Lambda) and dist/azure.zip (Flex Consumption)
podman build -f Containerfile.cloudrun -t <region>-docker.pkg.dev/<project>/<repo>/bench-app:<run-id> .
```

The zips are built with fixed timestamps, so the same source gives the same hash.
`bench/cloud/run.sh` builds what a row needs before it applies.

Deviation from the plan's wording: the plan names `src/app.ts`. The fixture is
plain ES modules (`.mjs`) so that `node --test` runs it without a TypeScript
step; esbuild bundles it per target as the plan says.
