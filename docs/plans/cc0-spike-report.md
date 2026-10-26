# CC0: Confluent Cloud spikes (report)

Phase CC0 of [plan CC1](2026-10-25-cc1-confluent-byoc.md) checks design [§80](../design/80-confluent-byoc.md) against Confluent Cloud before anything is built. The owner approved the design with its defaults on 2026-10-26 and gave CC0 a budget of $100 of Ostrium's Confluent credits. CC0 needed only Basic and Standard clusters. The rulings themselves are recorded in the plan's "Rulings made during execution" (CC0 R0.1 to R0.13). This report has the evidence for them.

All work was done on 2026-10-25/26 (UTC) from the owner's machine in India. Every cluster was ephemeral and lived in a new environment, `loams-ci`, which D1831 names. The provisioning identity was a spike service account with **EnvironmentAdmin on `loams-ci` only**, so the same runs also answered ruling 6. The oracle cluster `lkc-120zwvj` and its service account were read but never changed.

## Summary

| # | Question (plan Task 0) | Answer | Evidence |
|---|---|---|---|
| 1 | Exactly-once on Basic | **Works.** The transactional sink and the `__loams_checkpoints` pattern pass on Basic and on Standard. **The profile narrows nothing.** | `--test confluent`: 3 of 3 passed 3 times on Basic `us-east-1`, 2 times on Standard `us-east-1` and on Basic `ap-south-1` ([§2](#2-exactly-once-and-the-acl-set)) |
| 2 | Latency | The owner's link adds about 0.3 to 0.7 s per round trip (TCP connect alone). Produce p50 was 289 ms to `ap-south-1` and 546 ms to `us-east-1`; end to end, p50 was 406 ms and 819 ms. **The same-region number has to come from the CC1 nightly.** | [§3](#3-latency) |
| 3 | TLS roots | `ssl.ca.location=probe` verifies Confluent's certificate inside `ghcr.io/ostrium-labs/loams-fabric:0.1.0` (Debian 13). With no CA bundle the same binary fails. **No CA path in the profile.** | [§4](#4-tls-roots-in-the-fabric-image) |
| 4 | Region listing | `GET https://confluent.cloud/api/env_metadata` (what `confluent kafka region list` calls) accepts a Cloud API key and lists the **cluster types per region**. It is undocumented. | [§5](#5-regions) |
| 5 | OAuth per type | **Not on Basic.** Confluent's docs say OAuth for Kafka is "supported for Standard, Enterprise, Dedicated, and Freight clusters only". AWS IAM federation works through STS `GetWebIdentityToken` (verify with an EC2 role in CC3). | Docs ([§6](#6-oauth)) |
| 6 | EnvironmentAdmin | It can create clusters, **service accounts**, Kafka API keys (for itself and for other accounts), topics, ACLs, and environment- or cluster-scoped role bindings. It **cannot delete service accounts** (even its own), create Cloud API keys for others, bind organisation-scope roles, read billing, create environments or see other environments. So CI needs one persistent `loams-ci-data` account. | [§7](#7-what-environmentadmin-can-do) |
| 7 | Metrics API | `received_bytes`/`sent_bytes` break down by topic and partition, **not by principal**. **`request_bytes`/`response_bytes` break down by `principal_id` on Basic and Standard**, within 2 to 6 % of the cluster totals. Data is about 1 to 3 minutes behind. The limit is 300 requests per minute per IP, and data is kept for 7 days. | [§8](#8-metrics-api) |
| 8 | Storage billing | Contradictory on Confluent's own pages. The billing docs say storage is billed on post-replication volume ("about 3x"). The pricing page says "Prices shown are pre-replication". `retained_bytes` is pre-replication. **The ceiling assumes 3x** until a Costs API line settles it. | [§9](#9-storage-billing-and-the-costs-api) |
| 9 | Cloud-side transfer | AWS: in-region traffic to a public IP is $0.01/GB each way (verify against the price list), NAT gateway $0.045/GB. Google Cloud: free to Google services' external IPs in-region, Cloud NAT $0.045/GiB. Azure: NAT Gateway price not shown on the page. | [§10](#10-cloud-side-data-transfer) |
| 10 | IP filtering | IP filters cover the Kafka data operation group on public networks. No page names the cluster types. **Keep it an optional input and verify per type in CC1 Task 7.** | Docs |
| 11 | OpenTofu and the provider | `confluentinc/confluent` **2.90.0** (the newest release at least 14 days old today) from `registry.opentofu.org`, OpenTofu **1.13.2** (the repository's CI pin): `init`, `validate`, `fmt` and an encrypted-state `apply`/`destroy` of three clusters all worked. `tofu test` with `mock_provider "confluent"` passed **with no network** (in an empty network namespace). | [§11](#11-opentofu-and-the-provider) |
| + | Partitions | **A Basic cluster at `max_ecku = 1` holds 30 partitions in total.** Confluent refuses more with a policy error. The oracle cluster is full (30 of 30). | [§12](#12-findings-the-plan-did-not-ask-for) |
| + | Bootstrap format | `bootstrap_endpoint` is `SASL_SSL://pkc-….us-east-1.aws.confluent.cloud:9092` on some clusters and `lkc-….ap-south-1.aws.public.confluent.cloud:9092`, with no scheme, on others | §12 |
| + | Costs API | Daily lines with `original_amount` and `amount`. `PROMO_CREDIT` lines carry no product or resource. Usage lines for 2026-10-24/25 were not there at 23:50 UTC on 2026-10-25. The window is at most one month. | §9 |

**Spend:** about **$1 of credits, estimated** (list prices; the Costs API will show the reconciled amount within 72 hours, see [§13](#13-resources-and-spend)).

## 1. Method

- **Clusters.** Three clusters, each made by one OpenTofu root (local state, encrypted): Basic `us-east-1` (`lkc-v75g0r5`), Basic `ap-south-1` (`lkc-do8mr8o`) and Standard `us-east-1` (`lkc-rrkd9x7`). All three were `SINGLE_ZONE` with `max_ecku = 1`. The provider authenticated as `loams-ci-cc0-provisioner` (EnvironmentAdmin on `loams-ci`).
- **Per cluster, the root created:**
  - a data service account with the D1826 grant set, plus three test-only grants: topic CREATE on `loams-cc0-`, and group READ on `grp-` and `suite-reader-`;
  - two topics with `retention.bytes`;
  - a Kafka API key for the provisioner and one for the data account.
- **The connector suite.** `fabric/crates/loams-connector-kafka/tests/confluent.rs` (new, in this PR) runs the existing roundtrip, kill-restart and transactions cases (`tests/common`) against a cluster. It is gated on `LOAMS_CONFLUENT_BOOTSTRAP`, so CI skips it. `Target::confluent()` uses `sasl_ssl`, PLAIN, `verify_full` and the probed roots, as the planned profile will.
- **Latency.** Apache Kafka 4.3.1's `kafka-producer-perf-test.sh` and `kafka-e2e-latency.sh` in the pinned `apache/kafka:4.3.1` image.
- **APIs.** `curl` against the Cloud, Metrics and Costs APIs.
- **Docs.** Read on 2026-10-26.
- **Secrets.** Keys and the state passphrase stayed in 0600 files outside the repository and were never printed.

## 2. Exactly-once and the ACL set

```
LOAMS_CONFLUENT_BOOTSTRAP=… LOAMS_CONFLUENT_API_KEY=… LOAMS_CONFLUENT_API_SECRET=… \
  cargo test -p loams-connector-kafka --test confluent -- --test-threads 1
```

| Cluster | Runs | `confluent_suite` | `confluent_txn_sink_exactly_once_under_crash` | `confluent_txn_recover_reads_checkpoint` | Time per run |
|---|---|---|---|---|---|
| Basic, AWS `us-east-1` | 3 | ok ×3 | ok ×3 | ok ×3 | 222–237 s |
| Standard, AWS `us-east-1` | 2 | ok ×2 | ok ×2 | ok ×2 | 231 s |
| Basic, AWS `ap-south-1` | 2 (+1) | 1 timeout, then ok on rerun | ok | ok ×3 (one inside the fabric image) | 22 s (rerun, suite only) |

- **What passed:**
  - A crash after `write`, after `prepare` and after `commit` (before the Fabric commit), with each restart recovering the right checkpoint.
  - `read_committed` reads that hold every event exactly once, while `read_uncommitted` shows the aborted records.
  - Transactional ids `loams-<ns>-<instance>-<split>` under a PREFIXED `loams-` transactional-id ACL.
  - The checkpoint topic as a compacted, one-partition topic that the module creates (`loams-cc0-checkpoints` here). `__loams_checkpoints` is also a legal name on Confluent Cloud: the admin key created it.
- **The data account cannot widen its own limits.** Its key was refused with `TopicAuthorizationException` or `ClusterAuthorizationException` on each of these:
  - raising `retention.bytes`;
  - adding partitions;
  - deleting a topic;
  - creating a topic outside its prefix;
  - creating an ACL.

  That is D1826's claim, checked live.
- **Replication factor.** Confluent refuses the suite's `replication_factor = 1`. `Target::confluent()` asks for the broker default (`-1`), which is 3. `txn.rs` already uses `-1` for the checkpoint topic.

## 3. Latency

| Target | Producer (`acks=all`, 1 KiB, 20 msg/s, 600 msgs) p50 / p95 / p99 | End to end (200 msgs) p50 / p99 |
|---|---|---|
| Basic `ap-south-1` (Mumbai) | 289 / 1,994 / 2,844 ms | 406 / 946 ms |
| Basic `us-east-1` | 546 / 4,121 / 4,397 ms | 819 / 1,887 ms |

From the same host, a TCP connect to the brokers took 290–680 ms (Mumbai) and 400–610 ms (`us-east-1`), and TLS setup added about 200 ms more. **These numbers measure the owner's access link (wireless, carrier NAT), not Confluent.** They are the baseline §80 §6.3 asked for, labelled as such. The guide's same-region numbers come from the CC1 nightly (the AWS VM in `us-east-1` to Basic `us-east-1`, Task 10).

## 4. TLS roots in the fabric image

The suite binary (built on the host, glibc 2.39 symbols) ran inside `ghcr.io/ostrium-labs/loams-fabric:0.1.0` (Debian 13, `ca-certificates` 20250419, bundle at `/etc/ssl/certs/ca-certificates.crt`):
- `confluent_txn_recover_reads_checkpoint` passed against Basic `ap-south-1` with `ssl.ca.location=probe` and full hostname verification.
- The same binary in `debian:trixie-slim`, which has no `/etc/ssl/certs`, failed.

librdkafka 2.12.1's probe list includes `/etc/ssl/certs/ca-certificates.crt` (`src/rdkafka_ssl.c`). **The profile needs no `ca_pem`.** The image must keep `ca-certificates`.

## 5. Regions

`confluent kafka region list` calls `kafka.ListRegions`, which reads `EnvironmentMetadata` from Confluent's v1 client (the CLI's source, `pkg/kafka/region.go`). On the wire that is **`GET https://confluent.cloud/api/env_metadata`**, and it answers a Cloud API key with HTTP Basic auth.
- **Shape.** `clouds[]` (`aws`, `gcp`, `azure`). Each cloud has `regions[]`, and each region has these fields:
  - `id` (the cloud's own region code);
  - `name`;
  - `is_schedulable`;
  - `zones[]`;
  - `type[]`, the cluster types offered, each with its `durability` (`LOW`/`HIGH`) and `networking`;
  - `networking_type[]`.

  A top-level `schema_registry_locations[]` lists the Schema Registry locations.
- **On 2026-10-26** it listed 103 regions:
  - AWS 31, Google Cloud 39, Azure 33;
  - Basic in 98, Standard in 98, Enterprise in 87, Freight in 9, Dedicated in 101.
- **The BY1 defaults are all listed with Basic and Standard:** `us-east-1`, `us-central1`, `eastus`. So are `ap-south-1`, `asia-south1` and `centralindia`.
- **Exceptions to "Basic and Standard everywhere"** (§80 §6.1):
  - AWS `me-south-1` and `me-central-1` are listed but not schedulable;
  - Google Cloud `europe-north2` offers Enterprise and Dedicated only;
  - Azure `jioindiacentral` and `jioindiawest` offer Dedicated only.
- **No documented public endpoint lists Kafka regions with their types.** The Flink (`fcpm/v2`) and Schema Registry (`srcm/v2`) region APIs exist; `/cmk/v2/regions` does not (404). The docs' regions page is the only other source.

## 6. OAuth

- **Cluster types.** "OAuth for Kafka is supported for Standard, Enterprise, Dedicated, and Freight clusters only" ([OAuth overview](https://docs.confluent.io/cloud/current/security/authenticate/workload-identities/identity-providers/oauth/overview.md), read 2026-10-26). **Basic has no keyless path.**
- **AWS.** The AWS IAM identity provider takes a JWT from STS `GetWebIdentityToken`. It needs AWS outbound identity federation on the account, `sts:GetWebIdentityToken` on the role, and Confluent trusting the account's issuer. The docs list EC2 among the supported compute (an instance role is expected to work: verify in CC3).
- **Azure.** Entra ID, including user-assigned managed identities.
- **GitHub Actions.** Only as a generic OIDC provider. No Confluent example exists, and nothing was tested here.
- **Not tested live.** Identity providers and pools are organisation-level objects, and CC0 had no token issuer to point them at.

## 7. What EnvironmentAdmin can do

With a Cloud API key owned by a service account whose only role is EnvironmentAdmin on `loams-ci`:

| Action | Result |
|---|---|
| List environments | Sees `loams-ci` only. `GET /cmk/v2/clusters?environment=<default>` is 403 |
| Create an environment | 403 |
| Create a Basic or Standard cluster in `loams-ci`; change `max_ecku`; delete it | Allowed (OpenTofu create and destroy) |
| **Create a service account** (an organisation-level object) | **Allowed** (201) |
| List all service accounts in the organisation | Allowed |
| **Delete a service account**, even one it created | **403** (found at teardown; the owner's key deleted them) |
| Create a **Kafka** API key for itself or another service account on a cluster in `loams-ci` | Allowed |
| Create a **Cloud** API key (no resource) for another service account | **403** |
| Topics and ACLs through the Kafka REST API with its Kafka key | Allowed |
| Role binding MetricsViewer at **organisation** scope | **403** (`SecurityMetadata … Alter not permitted`) |
| Role binding MetricsViewer at **environment** scope; CloudClusterAdmin at **cluster** scope | Allowed |
| Costs API | 403 |

**Consequences:**
- **The CI service account can make everything the nightly needs except a service account it can later delete.** It creates service accounts but cannot delete them, so per-run accounts would leak. The owner therefore creates one persistent `loams-ci-data` account, and each run makes only Kafka keys and ACLs for it, which go with the run's cluster.
- **The metrics account can be scoped to one environment.** A Cloud key for a service account with environment-scoped MetricsViewer read `lkc-v75g0r5`'s metrics and got 403 on the oracle in `default`. §80 §7.1 said MetricsViewer is organisation-wide. It need not be.
- **The provisioner can read metrics itself.** A Cloud API key for a separate metrics account cannot be made by EnvironmentAdmin, because Metrics API keys are Cloud keys. But EnvironmentAdmin's own Cloud key reads the Metrics API for its environment's clusters (200). So CI needs no separate metrics key. For a user install, the user's own key makes the metrics account's key (Task 6).
- **The guard's throttle works with the narrow role.** A key with CloudClusterAdmin on one cluster can `PATCH /cmk/v2/clusters/{id}` `max_ecku` (202).

## 8. Metrics API

- **Breakdowns.** The descriptor list (85 metrics for `kafka`) gives `received_bytes`, `sent_bytes` and `retained_bytes` the labels `topic` and `partition` only. A query grouped by `metric.principal_id` is refused: "Invalid label … Valid labels for this metric are: metric.topic, metric.partition, …".
- **The per-principal metrics are request and response bytes.** `request_bytes` and `response_bytes` (labels `type`, `principal_id`, `principal_name`), `request_count`, `active_connection_count` and `successful_authentication_count` carry `principal_id`. They answered on **Basic** (`lkc-v75g0r5`, the oracle) and **Standard** (`lkc-rrkd9x7`). Confluent's FAQ mentions the principal label only for Dedicated; on Basic and Standard it works anyway.
- **How close they are to the billed bytes.** Over 23:00–23:40 UTC on Basic `us-east-1`:

  | Metric | Bytes |
  |---|---|
  | `received_bytes` | 1,583,711 |
  | `request_bytes{type=Produce}` | 1,637,747 (+3.4 %) |
  | `sent_bytes` | 394,501 |
  | `response_bytes{type=Fetch}` | 422,927 (+7.2 %) |

  Confluent bills "all data transfers, including requests for produce and consume and every other type of incoming or outgoing request" ([billing dimensions](https://docs.confluent.io/cloud/current/billing/billing-dimensions.md)). So **the guard's per-principal estimate is the sum of `request_bytes` (in) and `response_bytes` (out) over all request types for Loams' principal.** It reads slightly high, which is the safe direction for a budget.
- **Delay.** At 23:44:00 UTC the newest `PT1M` point was 23:43 (`request_bytes`) and 23:38 to 23:43 (`received_bytes`, sparse). The API's documentation says data is "typically available … within 3 minutes".
- **Limits.** "A global rate limit of 300 requests per IP address, per minute" (the [Metrics API reference](https://api.telemetry.confluent.cloud/docs)). Retention and the largest interval are 7 days. An hourly guard makes about 5 queries per cluster per hour, far below the limit.

## 9. Storage billing and the Costs API

- **Storage.**
  - The billing docs: "Confluent Cloud charges for storage based on the total post-replication volume of data … billed storage is typically about 3x the volume you write" ([billing dimensions](https://docs.confluent.io/cloud/current/billing/billing-dimensions.md)).
  - The pricing page, under the $0.08/GB-month row: "Prices shown are pre-replication."
  - `retained_bytes` is pre-replication. For `loams-cc0-latency` in `ap-south-1` it was 863,576 bytes, exactly the topic's `received_bytes`.

  Until a Costs API line for a known topic settles it, `confluent_ceiling_monthly_usd` counts storage as `Σ retention.bytes × partitions × 3 × $0.08`. That is the conservative reading. The CC1 nightly compares the `KAFKA_STORAGE` line's quantity with `retained_bytes` and fixes the factor (R0.8).
- **Costs API.**
  - `GET /billing/v1/costs?start_date=…&end_date=…` works with the owner's key. It refuses a window longer than a month ("end date and start date are too far apart") and accepts an `end_date` in the future.
  - At 23:50 UTC on 2026-10-25 it returned only `PROMO_CREDIT` lines for 2026-10-24 and 2026-10-25 (`original_amount` 0 and −0.0001). Those lines have no `product`, `resource`, `unit` or `quantity`. The oracle's usage lines were not there yet. That fits "up to 72 hours".
  - EnvironmentAdmin and MetricsViewer keys get 403.
  - Not measured, because CC0 cannot wait three days: the lag for this run's clusters, and the shape of a `KAFKA_*` line. **The CC1 nightly records both** (R0.9).

## 10. Cloud-side data transfer

Read on 2026-10-26. These are for the guide's cost table, **to be checked against each cloud's price list API before the guide quotes them** (Task 9):

| Cloud | VM → Confluent public endpoint, same region | Through a NAT gateway | Source |
|---|---|---|---|
| AWS | $0.01/GB in each direction for in-region traffic over public IPv4 (AWS networking blog; the EC2 pricing page's table did not render) (verify) | NAT gateway $0.045/GB processed plus $0.045/hour (the page's example is US East, Ohio) | [EC2 pricing](https://aws.amazon.com/ec2/pricing/on-demand/), [VPC pricing](https://aws.amazon.com/vpc/pricing/) |
| Google Cloud | Free to a Google Cloud service's external IP in the same region (verify that Confluent's endpoints count; they are Confluent's own load balancers on Google Cloud, so likely billed as internet egress) | Cloud NAT $0.045/GiB processed plus $0.0014 per VM-hour | [VPC network pricing](https://cloud.google.com/vpc/network-pricing), [Cloud NAT pricing](https://cloud.google.com/nat/pricing) |
| Azure | Internet egress, Zone 1: $0.087/GB after 100 GB free (Microsoft network routing; $0.08 with internet routing). The page does not price intra-region traffic to a public IP | NAT Gateway: the page showed no figures (verify) | [Bandwidth](https://azure.microsoft.com/en-us/pricing/details/bandwidth/), [NAT Gateway](https://azure.microsoft.com/en-us/pricing/details/azure-nat-gateway/) |

The BY1 VM modules give the VM a public IPv4 address for egress (no NAT gateway), so the AWS row is $0.01/GB each way (verify). For the nightly's 600 MB a run, that is under $0.02.

## 11. OpenTofu and the provider

- **Versions.**
  - OpenTofu 1.13.2: the repository's pin in `.github/workflows/byoc-static.yml`, checked by the same SHA-256. The plan said "OpenTofu ≥ 1.11"; 1.13.2 is used.
  - The provider: `registry.opentofu.org/confluentinc/confluent` 2.90.0, released 2026-10-08 and so at least 14 days old today. 2.91.0 (2026-10-14) becomes eligible on 2026-10-28. The plan's 2.89.0 also qualifies.
- **What worked:**
  - `tofu init` (the lock file records the registry's hashes), `tofu validate` and `tofu fmt`;
  - `tofu apply` and `tofu destroy` of the spike root on three clusters, each with:
    - 18 resources: cluster, two service accounts' keys, two topics and 12 ACLs;
    - `encryption { key_provider "pbkdf2"; method "aes_gcm"; state { enforced = true }; plan { enforced = true } }`.

    `confluent_api_key.secret` sits in that encrypted state.
- **`tofu test` with `mock_provider "confluent"`** (two runs: a Basic plan with ACL assertions, and a refused `dedicated`) passed under `unshare -rn`, with no network and no credentials. **Gotcha:** the provider validates references at plan time. `confluent_api_key.owner.id` must match `^(u|sa)-` and `api_version`/`kind` must be `iam/v2`/`ServiceAccount`, so the mocks must give `mock_resource "confluent_service_account"` and `"confluent_api_key"` realistic `defaults`. Random mock strings fail validation.
- **`mock_data "confluent_environment"`** must not set `id` when the configuration sets it ("overriding configuration values is not allowed").
- Q1336 is "no", so no HCP workspace was created.

## 12. Findings the plan did not ask for

- **Basic at `max_ecku = 1` is a 30-partition cluster.** Creating a 12-partition topic on a cluster with 19 partitions failed: "You may not create more than 11 new partitions. Adding the requested number of partitions will exceed 30 total partitions" (policy violation). Visible topics count; internal ones do not.
  - The module's topics, the checkpoint topic and every topic a test creates share the 30.
  - The nightly must delete its topics, or use a fresh cluster each run (which it does).
  - `confluent_unbounded` and the guide should say this. The module should refuse a `topics` map over 30 partitions at `basic` / `max_ecku = 1`.
- **The oracle cluster is full.** `lkc-120zwvj` holds 10 topics with 30 partitions, left by the Kafka gateway oracle's runs (`loams-oracle-<run>-…`). Its next run cannot create a topic until they are deleted. CC0 did not touch them.
- **`bootstrap_endpoint` has two formats.** Some clusters return `SASL_SSL://pkc-oxqxx9.us-east-1.aws.confluent.cloud:9092`, a shared endpoint with a scheme; others return `lkc-do8mr8o.ap-south-1.aws.public.confluent.cloud:9092`, per cluster with no scheme. Both formats appeared on new Basic and Standard clusters. The module's output and the CLI must strip an optional `SASL_SSL://`.
- **The Basic price beyond the free eCKU is $0.14 per eCKU-hour** (pricing page). Basic at `max_ecku = 1` still costs $0 in eCKUs.

## 13. Resources and spend

Every create and delete is logged in `loams-wt/.he0-shared/confluent-ci-created.tsv` and `-deleted.tsv` (ids only).

| Resource | Ids | State at the end |
|---|---|---|
| Environment `loams-ci` | `env-3kqygo` | **Kept**: D1831's CI environment (owner checklist item 2), empty, Stream Governance Essentials (free) |
| Clusters `loams-ci-cc0-basic`, `-basic-aps1`, `-standard` | `lkc-v75g0r5`, `lkc-do8mr8o`, `lkc-rrkd9x7` | Deleted, verified by id |
| Service accounts (provisioner, probe, three data accounts) | `sa-3n37qko`, `sa-wzy2qxm`, `sa-3n37qq2`, `sa-5m15j78`, `sa-kg6j3qp` | Deleted, verified |
| API keys (two Cloud keys, six Kafka keys) | in the logs | Deleted, verified |
| Role bindings | `rb-10bBbK`, `rb-Zg7vmb`, `rb-Kr2ZqJ` | Deleted, verified |
| Oracle cluster `lkc-120zwvj` and its service account `sa-3n32p6j` (environment `default`) | — | Untouched |

**Estimated spend: about $1 (list prices; not reconciled):**
- the Standard cluster at one eCKU from 23:39 to 23:52 UTC, which is one billed hour ($0.75);
- Basic eCKUs at $0 (the first eCKU is free);
- about 30 MB of traffic at $0.05/GB (under $0.01);
- storage under 10 MB for a few hours (under $0.01).

The Costs API will have the reconciled number within 72 hours of the teardown. Check it with `GET /billing/v1/costs?start_date=2026-10-25&end_date=2026-10-28`.
