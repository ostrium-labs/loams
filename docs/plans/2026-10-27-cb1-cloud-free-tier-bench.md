# CB1: Free-Tier OpenTofu and Benchmarks on AWS, Google Cloud and Azure

Status: **In progress** · 2026-10-27. Track AB (app BYOC), alongside [BY1](2026-10-21-by1-byoc-clouds.md).

Source: the owner's request of 2026-10-27, "now i have azure account with billing, 180 dollars in aws and 300 in gcp, create opentofu, we will benchmark it on real service", with a free-tier mapping per cloud.

Designs: [§71](../design/71-app-byoc-free-tiers.md) (app BYOC on free tiers, D1390–D1410) and [§70](../design/70-byoc-clouds.md) (data-plane BYOC). The owner's mapping is input, not a new decision. Where it differs from §71, this plan follows §71 (§2). No decision changes, so no amendment is needed. If a run shows that one is needed, it goes in a separate amendment PR labelled `review:greptile`.

Markers: **(estimate)** for figures computed from list prices, **(verify)** for facts a real run must confirm. Prices are list prices for us-east-1, us-central1 and eastus, read on 2026-10-27.

---

## 1. Goal

1. **OpenTofu modules for §71's app targets on AWS, Google Cloud and Azure**, mapped onto each cloud's free tier, with §71's cost guards (D1400, D1402): a budget with alerts created first, hard caps where the cloud has them, and every resource tagged.
2. **Benchmarks on the real services:**
   - the §71 portable app on each cloud's serverless compute, with its `items` and `queue` stores;
   - for data-plane comparison, Loams on the existing VM modules (`aws-vm`, `gcp-vm`, `azure-vm`).
3. **Results in the repository**, with raw data, method, region, date and cost. Teardown is verified after every run.

**Not in this plan:**
- `@loams/app`, `loams app deploy` and the conformance suite (AB0 Tasks 2–5). The benchmark uses a small fixture app with the same contract (§4).
- The `schedule` and `blobs` ports' benchmarks. The modules create them, but they are not on a latency path worth measuring.
- Cloudflare and Vercel. Their rows wait for AB0.

**What the modules are for after the benchmark:**
- They are the AB0/AB1 templates that `loams app deploy` (D1406) will call from its generated root module.
- They are also the targets of the nightly free-tier runs (D1407).

## 2. The owner's mapping against §71

| Cloud | Owner's list | §71 (followed) | Note |
|---|---|---|---|
| AWS | Lambda, Step Functions, DynamoDB, SQS, CloudFront, Cognito, KMS, Budgets | Lambda + Function URL, DynamoDB **provisioned** 25/25, SQS, EventBridge Scheduler, S3, Budgets | Step Functions is an escape hatch (§6.6), not a port. CloudFront is not in AB0 (§5.1). Cognito is out of scope (apps authenticate themselves, D1409). KMS uses AWS-owned keys (free), not a customer key ($1/month). |
| Google Cloud | Cloud Run, Firestore, **Pub/Sub**, BigQuery, GCS, Cloud Build, Artifact Registry, Secret Manager, Logging, Workflows, e2-micro | Cloud Run (request-based, `min-instances` 0), Firestore `(default)`, **Cloud Tasks**, Cloud Scheduler, GCS in a free US region | The `queue` port is one consumer with per-task retry, which is Cloud Tasks (§7.3). Pub/Sub is the fan-out alternative. Artifact Registry holds the image (0.5 GB free). The image is built locally, so Cloud Build is not used. BigQuery and Workflows are not ports. The e2-micro is not used as the loadgen (0.25 vCPU sustained; §5.4). |
| Azure | Static Web Apps, Functions (1M requests), Container Apps, Cosmos DB free tier, Azure SQL free offer, Event Grid, App Configuration, budgets | Functions **Flex Consumption**, Cosmos DB free tier, Queue Storage, timer trigger, Blob Storage | 1M executions is the Consumption plan's grant. §71 chose Flex (250,000 executions and 100,000 GB-s; Q1101). Container Apps is not an app-BYOC target (D1395). Azure SQL, Event Grid and App Configuration are not ports. |

The guidance the owner pasted is all kept:
- no NAT gateways;
- no public IPv4 except the loadgen's outbound address (§5.4);
- no always-on VM in the app modules;
- no backups;
- short log retention;
- budgets alert and do not cap;
- Rust services unchanged;
- the free-tier quotas live in the product (D1401's table; the modules' defaults cite it).

## 3. The modules

`deploy/tofu/modules/{aws-free,gcp-free,azure-free}`, each with `examples/<module>/` and `tests/plan.tftest.hcl` (mock providers). They are checked by `tests/byoc/tofu/run.sh` (fmt, validate, `tofu test`, tflint), as the VM modules are.

**Common rules:**
- **Budget first.** The budget is created before anything else: every other resource `depends_on` it. It has alerts at 50%, 80% and 100% of actual spend and at 100% of forecast. It sends email, and no action stops anything.
- **Tag everything.** Every resource gets `loams-bench = <run_id>`, or `loams-app = <name>` outside a benchmark. `run_id` is lowercase letters, digits and `-`, because Google labels need it. The tests assert that every taggable resource in the plan carries the tag.
- **Hard caps where the cloud has them:**
  - AWS: DynamoDB provisioned capacity with no autoscaling; Lambda reserved concurrency.
  - Google Cloud: Cloud Run `max_instance_count`, and `min_instance_count` 0.
  - Azure: Flex Consumption `maximum_instance_count`, and Cosmos DB's fixed 1,000 RU/s on a free-tier account.
  - SQS, Firestore, Cloud Tasks and Queue Storage have no cap, so the budget covers them.
- **No NAT, no public IPv4, no VNet or VPC connector** in the app modules.
- **No backups** (Cosmos DB uses the periodic default, which is free, with no geo copy), **no point-in-time recovery**, and **logs kept 1 day** (CloudWatch) or under a daily cap (Log Analytics).
- **Free by default:**
  - AWS: DynamoDB provisioned at the free 25/25 (`items_capacity_mode = "on_demand"` is a credit-tagged opt-in for the benchmark).
  - Google Cloud: the Firestore `(default)` database and a free US region; a region outside `us-central1`, `us-east1` and `us-west1` is refused.
  - Azure: Cosmos DB `free_tier_enabled`; Flex with on-demand instances only (always-ready is 0).

| Module | Creates | Caps (defaults) |
|---|---|---|
| `aws-free` | Budget; Lambda (arm64, Node 22) with a Function URL; DynamoDB table (pk/sk, TTL); SQS queue, dead-letter queue and event-source mapping; optional EventBridge Scheduler schedule; optional S3 bucket; role; log group (1 day) | Reserved concurrency 5 (`null` when the account limit is 10); DynamoDB 25 RCU / 25 WCU; SQS mapping maximum concurrency 2 |
| `gcp-free` | Budget (project and label filtered); service account; Cloud Run v2 service (image input, request-based, public invoker optional); Firestore `(default)` (optional; delete on destroy); Cloud Tasks queue; optional Cloud Scheduler job; optional GCS bucket; Artifact Registry repository (optional) | `max_instance_count` 2; Cloud Tasks dispatch rate 10/s and 2 concurrent |
| `azure-free` | Resource group; budget on the group; storage account (host, deployment container, queue; shared keys off); Flex Consumption plan and function app (Node 22, 512 MB, system identity); Cosmos DB free-tier account, database (1,000 RU/s shared) and container; data-plane role assignments; optional Log Analytics and Application Insights with a daily cap | `maximum_instance_count` 2; 1,000 RU/s; Log Analytics daily cap 0.1 GB |

## 4. The bench app (`bench/cloud/app`)

A fixture, not `@loams/app`. It is the §71 contract with the smallest `items` and `queue` adapters a benchmark needs, so the numbers are the cloud's, not a framework's.

- `src/app.ts` is the D1391 shape: `export default { fetch, queue }`. It has these routes:
  - `GET /ping`: no store; returns the instance id and whether this is the instance's first request.
  - `GET /items/:k`: a strongly consistent read.
  - `PUT /items/:k`: an unconditional write.
  - `POST /enqueue`: a CloudEvent of 256 bytes.
  - `GET /queue/stats?run=`: lists the partition that the consumer writes.
  - The `queue` handler records `received − event.time` as an item.
- **Adapters** use each cloud's REST or SDK with **no retries**: AWS SDK v3 with `maxAttempts: 1`, Firestore REST, Cosmos DB REST with an Entra ID token from the managed identity. A store throttle returns `429` and is counted apart from `5xx`, so a free-tier cap shows as a cap, not as hidden latency.
- **Shims** follow D1791:
  - Lambda: Function URL events to `fetch`, SQS batches to `queue`.
  - Azure Functions v4 on Node: HTTP trigger to `fetch`, queue trigger to `queue`.
  - Cloud Run: a Bun binary compiled ahead of time in a distroless image; Cloud Tasks push to `queue`.
- Unit tests run the handler against an in-memory store (`node --test`). Build: esbuild per target (Vite+ comes with AB0's real package).

## 5. The benchmark

### 5.1 Targets

| Row | Compute | `items` | `queue` | Region |
|---|---|---|---|---|
| `aws-free` | Lambda arm64, 256 MB, Function URL | DynamoDB provisioned 25/25 | SQS | us-east-1 |
| `aws-ondemand` (credit) | Same | DynamoDB on-demand | SQS | us-east-1 |
| `gcp-free` | Cloud Run, 1 vCPU, 512 MiB, concurrency 80, `max` 2 | Firestore `(default)` | Cloud Tasks | us-central1 |
| `azure-free` | Functions Flex, 512 MB, `max` 2 | Cosmos DB free tier, 1,000 RU/s | Queue Storage | eastus |
| `aws-vm`, `gcp-vm`, `azure-vm` (data plane) | Loams 0.1.2 light default on the module's `small` size | A collection (`documents` write and `documents/get`) | — | as above |

### 5.2 Scenarios

Each target runs the same scenarios in the same order; nothing is dropped or rerun selectively.

| Id | What | Load |
|---|---|---|
| `cold` | 10 forced cold starts: Lambda configuration change; Cloud Run new revision; Flex restart. One request after each | Serial |
| `ping` | `GET /ping` | Closed loop, **c = 10**, n = 20,000, 3 reps |
| `read` | `GET /items/k{000..999}` (random), 1 KiB values preloaded | Closed loop, c = 10, n = 10,000, 3 reps |
| `write` | `PUT /items/k{000..999}`, 1 KiB | Closed loop, c = 10, n = 5,000, 3 reps |
| `read-rate`, `write-rate` | As `read` and `write` | Open loop at **20 requests/s for 60 s** with latency correction: what a free-tier app sees inside its capacity |
| `queue` | `POST /enqueue` | 10/s for 60 s; consumer lag from `/queue/stats` |

- Each closed-loop rep is preceded by a 1,000-request warm-up. The warm-up is kept in the raw data and excluded from the summary.
- The closed-loop counts are chosen so that one run fits Firestore's daily free reads and writes (50,000 and 20,000) and Azure's monthly execution grant (250,000).
- The data-plane rows run `ping` (as `/health`), `read`, `write` and the rate scenarios. They run no `cold` and no `queue`.

### 5.3 Metrics

- **Latency:** p50, p95, p99 and maximum, measured client-side by oha with its per-request SQLite file (`--db-url`).
- **Throughput:** successful requests per second over the rep.
- **Error rate:** split into `429` (store or platform throttle), other `4xx`, `5xx` and transport errors.
- **Cold start:**
  - client-side time to the first byte;
  - the platform's own figure where it has one: Lambda's `Init Duration`; Cloud Run's `container/startup_latencies` and its "new container instance" request log; Flex's App Insights `requests` with `cloud_RoleInstance` first seen.
  - A probe that the platform does not mark as cold is reported as such, not counted as cold.
- **Cost per million requests,** from two sources:
  - **(a) usage × list price.** Usage comes from the cloud's own figures: CloudWatch `Invocations`, `Duration` and `ConsumedRead/WriteCapacityUnits`; Cloud Monitoring `request_count` and `billable_instance_time`; Azure Monitor `OnDemandFunctionExecutionCount` and `OnDemandFunctionExecutionUnits`. This is shown both inside and beyond the free tier.
  - **(b) the billed amount** for the run, once billing settles (8–24 h): Cost Explorer by tag, the Billing report or export by label, Cost Management by resource group.
  - Both are published. If they disagree, the disagreement is reported, not reconciled away.
- **Loadgen health:** the loadgen's CPU is sampled every second. A rep whose loadgen exceeds 80% CPU is flagged as possibly loadgen-bound.

### 5.4 Method

- **The loadgen runs in the target's region**, on a 2-vCPU VM: c7a.large on Amazon Linux 2023, e2-standard-2 on Debian 12, Standard_D2as_v5 on Ubuntu 24.04. A burstable or 0.25-vCPU machine would measure itself.
- **The loadgen VM drives itself.**
  - It has no inbound rule and an outbound-only public IPv4, the VM modules' precedent; there is no NAT.
  - At boot, cloud-init downloads oha 1.16.0 (checked by SHA-256) and runs `bench/cloud/loadgen/run-scenarios.sh`.
  - It writes raw files and a `done` marker to the run's bucket, then powers off.
  - Its identity may change only the target's configuration (for `cold`) and write to the results bucket.
- **The data-plane rows** reach the engine over the tailnet: the loadgen joins it with an ephemeral key. `tailscale status` must show a direct path; a DERP-relayed run is marked invalid and repeated once.
- **Raw data and method** go to `bench/results/cloud-free-tier/<date>-<row>/`:
  - oha's JSON and SQLite per rep;
  - the cloud's metric exports;
  - the run's `tofu` outputs;
  - the module inputs;
  - the commit;
  - region, timestamps and the sweep output.
- `bench/cloud/summarize.py` builds `bench/results/cloud-free-tier/README.md` from the raw data only.

## 6. Cost model per run

All figures are **(estimate)**, at list prices. A run is one row's scenarios, about 45–60 minutes of wall time with the loadgen up. **free** means inside the always-free allowance; **credit** means billed to AWS or Google credits; **paid** means billed on Azure.

### 6.1 AWS (`aws-free`, us-east-1)

| Item | Usage | Tag | Cost |
|---|---|---|---|
| Lambda | about 115,000 invocations, about 600 GB-s | free (1M, 400,000 GB-s) | $0 |
| Function URL | billed as Lambda | free | $0 |
| DynamoDB provisioned 25/25 | — | free | $0 |
| SQS | about 2,000 requests | free (1M) | $0 |
| CloudWatch Logs | about 100 MB, 1-day retention | free (5 GB) | $0 |
| S3 (results, function package) | under 50 MB for a day | credit | < $0.01 |
| Loadgen c7a.large | 1 h, 8 GB gp3, public IPv4 | credit | about $0.11 |
| Cost Explorer API | about 10 calls | credit | $0.10 |
| Budgets | 1 budget | free | $0 |
| **Run total** | | | **about $0.25; logged as $0.50** |

- **`aws-ondemand`** adds about 35,000 read and 20,000 write request units, under $0.02 (credit).
- **`aws-vm`** costs about $0.30 per hour (credit): t3.large, 60 GB gp3, a KMS key and secrets prorated, public IPv4, and the loadgen.

### 6.2 Google Cloud (`gcp-free`, us-central1)

| Item | Usage | Tag | Cost |
|---|---|---|---|
| Cloud Run | about 115,000 requests, about 1,000 vCPU-s, 500 GiB-s | free (2M, 180,000, 360,000) | $0 |
| Firestore | about 32,000 reads, 18,000 writes, 1,000 deletes | free per day (50,000, 20,000, 20,000); a second run the same day is credit, < $0.05 | $0 |
| Cloud Tasks | 600 | free (1M) | $0 |
| Artifact Registry | one image, about 40 MB | free (0.5 GB) | $0 |
| GCS (results) | < 50 MB, us-central1 | free (5 GB-month) | $0 |
| Logging | about 150 MB | free (50 GiB) | $0 |
| Loadgen e2-standard-2 | 1 h, 10 GB balanced disk, ephemeral IPv4 | credit | about $0.08 |
| Budget | 1 | free | $0 |
| **Run total** | | | **about $0.10; logged as $0.50** |

**`gcp-vm`** costs about $0.20 per hour (credit).

### 6.3 Azure (`azure-free`, eastus)

| Item | Usage | Tag | Cost |
|---|---|---|---|
| Functions Flex, 512 MB | about 116,000 executions; at most about 58,000 GB-s, given the 1 s minimum billed per execution | free (250,000 executions, 100,000 GB-s a month); beyond: $0.40 per million and $0.000026/GB-s | $0 in the first run; up to about $1.20 in later runs in the same month |
| Cosmos DB free tier | under 1 GB, 1,000 RU/s | free | $0 |
| Storage account | host, package, queue and results; about 50,000 transactions | paid | about $0.02 |
| Log Analytics and App Insights | daily cap 0.1 GB | free (5 GB a month) | $0 |
| Loadgen D2as_v5 | 1 h, Standard SSD, Standard public IP | paid | about $0.10 |
| Budget | 1 | free | $0 |
| **Run total** | | | **about $0.15 for the first run; logged as $1.50** |

**`azure-vm`** costs about $0.35 per hour (paid).

### 6.4 The lane's limits

| Cloud | Lane cap | 25% stop line per estimate | Planned |
|---|---|---|---|
| AWS | $60 of the $180 credit | $15 | 3 app runs + 1 on-demand run + 1 VM run: about $2.50 |
| Google Cloud | $100 of the $300 credit | $25 | 3 app runs + 1 VM run: about $1.50 |
| Azure | $25, paid | $6.25 | 2 app runs + 1 VM run: about $2.50 |

**The ledger.** Every run is recorded in `.he0-shared/cloud-bench-ledger.md` before it starts, with cloud, run id, estimate, created and destroyed timestamps, and actual cost once billed.

**When the coordinator is told:**
- an estimate exceeds the 25% line;
- the lane's actual spend on a cloud passes 60% of its cap;
- in either case, nothing more runs on that cloud until the coordinator answers.

## 7. Teardown guarantee

1. **One wrapper per run.** `bench/cloud/run.sh <row> <run-id>` applies, waits for the loadgen's `done` marker, downloads the raw data, and runs `tofu destroy` in an `EXIT` trap. The trap fires on success, failure, timeout and interrupt.
2. **A hard deadline.** The wrapper has `--max-minutes` (default 90). The loadgen VM schedules `shutdown -h` at boot for the same deadline, so it stops even if the laptop disconnects. Serverless rows cost nothing when idle.
3. **A sweep after destroy.** `bench/cloud/sweep.sh <cloud> <run-id>` lists what still carries `loams-bench=<run-id>` and fails if anything does:
   - AWS: the Resource Groups Tagging API, plus Lambda, DynamoDB, SQS, logs and EC2 listed directly, because the tagging API lags.
   - Google Cloud: Cloud Asset Inventory by label, plus Cloud Run, Firestore databases, Cloud Tasks, Artifact Registry and Compute listed directly.
   - Azure: `az resource list --tag`, plus `az group exists`.
   - The sweep runs again 10 minutes later, because the listing APIs are eventually consistent.
4. **The record.** The results directory holds the sweep output, both passes, with timestamps. The ledger gets the destroy timestamp.
5. **Nothing overnight.** A run does not start after 20:00 IST. A run that fails to destroy is escalated to the coordinator immediately, with the resource list.
6. **Known leftovers that cost nothing** are listed, not hidden:
   - enabled Google APIs;
   - registered Azure resource providers;
   - Cloud Logging entries (default 30-day retention, free);
   - the AWS budget's history;
   - Firestore's `(default)` database name, which is reusable after deletion.

## 8. Tasks

Each task is one PR into `dev`. Checks are run locally and listed in the PR body.

| Task | Content | Done when |
|---|---|---|
| **0** | This plan; a row in `docs/plans/README.md` | Merged |
| **1** | `aws-free`, `gcp-free` and `azure-free` modules, examples, mock-provider tests; `tests/byoc/tofu/run.sh` and `byoc-static.yml` include them; `deploy/tofu/README.md` | fmt, validate, `tofu test` and tflint clean |
| **2** | `bench/cloud/app`: contract, three adapters, three shims, unit tests, the Cloud Run image build | `node --test` green; each target's bundle builds |
| **3** | The harness: `bench/cloud/{aws,gcp,azure}/` root modules (free module + loadgen), `loadgen/run-scenarios.sh`, `run.sh`, `sweep.sh`, `summarize.py`, ledger entry helper; a dry run against a local server | A local dry run produces a complete results directory |
| **4** | AWS runs: `aws-free` and `aws-ondemand`; results and sweep | Results merged; sweep clean |
| **5** | Google Cloud run: `gcp-free` | Same |
| **6** | Azure run: `azure-free` | Same |
| **7** | Data-plane rows on `aws-vm`, `gcp-vm`, `azure-vm` | Same; gated on the tailnet key |
| **8** | Billed costs reconciled a day after each run; final summary | The summary shows both cost sources |

## 9. Inputs needed

- **Credentials,** loaded from files and never printed:
  - `~/aws.env`: an IAM user, not root;
  - `~/gcp-sa.json` and `~/gcp.env`: project Owner, plus Billing Account Costs Manager for the budget;
  - `~/azure.env`: a service principal with Owner, or Contributor plus User Access Administrator.
  - Tasks 1–3 need none.
- **For Task 7:** a reusable, ephemeral Tailscale (or Headscale) auth key in `~/tailscale-bench.env`.
- **Prerequisites to check:**
  - the AWS Lambda account concurrency limit: if it is 10, reserved concurrency cannot be set and the row says so;
  - an unused Cosmos DB free-tier slot on the Azure subscription;
  - no existing Firestore `(default)` database in the Google project.

## 10. Risks

| Risk | Mitigation |
|---|---|
| A free-tier store throttles the closed-loop scenarios (DynamoDB 25 units) | Expected and reported. The open-loop rate scenarios show the inside-capacity latency; `aws-ondemand` shows the service without the cap |
| The loadgen is the bottleneck | A 2-vCPU VM per run; CPU sampled; flagged reps |
| A forced cold start is not cold (a Cloud Run deploy pre-starts an instance) | Use the platform's cold flag; report the count of real cold samples |
| Billing lags or misattributes | Report usage × list price and the billed amount separately (§5.3) |
| A destroy fails (Cosmos DB and Firestore deletes are slow; IAM propagation) | The trap retries `destroy` 3 times; the sweep fails loudly; the coordinator is told at once |
| Azure Flex's 1 s minimum makes GB-s larger than expected | 512 MB instances; the first run's metrics set the counts for later runs |
