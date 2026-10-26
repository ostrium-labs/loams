# modules/confluent

A Confluent Cloud Kafka cluster next to a Loams BYOC VM, or Loams' access to a cluster you already run (design 80 §5, D1823–D1828; guide: [`docs/guides/byoc/confluent.md`](../../../../docs/guides/byoc/confluent.md)). It uses Confluent's own provider, `confluentinc/confluent` (Apache-2.0, from the OpenTofu registry). Only OpenTofu is tested.

## What it creates

| Resource | When |
|---|---|
| Environment `loams-<name>`, with Stream Governance Essentials (free) | `environment_id` is empty |
| Kafka cluster `loams-<name>` in the VM's cloud and region: Basic by default, Standard on request, at `max_ecku` (default 1); deletion protection on unless the name starts with `loams-ci-` | `cluster_id` is empty |
| Topics from `topics`, each with `retention.bytes`; the compacted checkpoint topic `__loams_checkpoints` (one partition) | `kafka_admin` is not `none` |
| Service account `loams-<name>-data` and its Kafka ACLs: READ, WRITE, DESCRIBE and DESCRIBE_CONFIGS on the named topics (LITERAL); READ and DESCRIBE on `consumer_group_prefix` groups; WRITE and DESCRIBE on `transactional_id_prefix` ids (PREFIXED); IDEMPOTENT_WRITE. **Never** CREATE, DELETE, ALTER, ALTER_CONFIGS or an ACL operation | the account unless `data_service_account_id` is given; the ACLs when `kafka_admin` is not `none` |
| `loams-<name>-metrics` with MetricsViewer on the environment, for the budget guard | `create_metrics_account` (default on) |
| `loams-<name>-ops` with CloudClusterAdmin on the cluster, only for the guard's opt-in `max_ecku` throttle | `create_ops_account` (default off) |
| Kafka API keys | only with `create_api_keys = true` (see below) |

## What it refuses (a plan fails)

- `enterprise`, `freight` or `dedicated` without `allow_premium_cluster_types = true`.
- A cluster in another cloud or region than the VM's (`cloud` and `region`, wired from the VM module) without `allow_cross_region = true`. This applies to an existing cluster too.
- A region that `deploy/byoc/confluent/regions.toml` does not list as offering the type, or lists as not schedulable.
- More partitions than the type allows at `max_ecku` (Basic: 30 per eCKU).
- A topic without a positive `retention_bytes`; `MULTI_ZONE` on Basic; PrivateLink (CC3).
- `kafka_admin = "module_key"` without `create_api_keys`.

## Keys never in plain state

The provider reads your Cloud API key from `CONFLUENT_CLOUD_API_KEY` and `CONFLUENT_CLOUD_API_SECRET` and does not store it. Topics and ACLs need a Kafka key with admin rights on the cluster. `kafka_admin` chooses where that key comes from:

| `kafka_admin` | Admin key | Applies for a new cluster |
|---|---|---|
| `provider` (default) | The provider's own settings: `KAFKA_ID`, `KAFKA_REST_ENDPOINT`, `KAFKA_API_KEY`, `KAFKA_API_SECRET` in the environment. Never in state | Two: first `-var kafka_admin=none`, then create an admin key for yourself on the cluster, export it, and apply again |
| `none` | None: no topics, no ACLs | (the first of the two) |
| `module_key` | The module creates `loams-<name>-admin` and its key. **The key is in state** | One. Needs `create_api_keys = true` in a root whose state and plan encryption is `enforced = true` (D1827) |

Loams' own data key is created out of band by default, with `loams byoc confluent key create`, straight into the cloud secret store; the module outputs only the secret names. `create_api_keys = true` creates it here instead, so it sits in the encrypted state (`data_api_key`, sensitive).

## The ceiling

`confluent_ceiling_monthly_usd` sums what this configuration bounds, at list prices from `deploy/byoc/confluent/prices.toml`:
- eCKUs: (`max_ecku` minus the free eCKUs) × the price × 730 hours. Basic at 1 eCKU is $0.
- Storage: every managed partition full to `retention.bytes`, × 3 for replication (CC0 ruling R0.8).
- Loams' own traffic at `connector_bytes_per_sec`.

`confluent_unbounded` lists what it does not bound. On Basic and Standard, that includes other clients' throughput: Confluent has no client quotas on those types and no budget API. It is an **estimate**, labelled with the price table's date (`confluent_ceiling.label`), never a cap on your bill. A `check` warns when the ceiling exceeds `confluent_budget_monthly_usd`.

## Tables

`confluent-tables.json` is a generated copy of `deploy/byoc/confluent/{regions,prices}.toml`, because OpenTofu reads JSON, not TOML. Regenerate it with `scripts/byoc/confluent-tables.sh gen`; `check` fails when it is stale.

## Tests

`tofu test` in this directory runs `tests/plan.tftest.hcl` with a mock provider, offline. Among its cases: `refuses_premium_types_without_flag`, `refuses_cross_region_without_flag`, `every_topic_has_retention_bytes`, `data_account_acls_are_minimal`, `no_api_key_resource_by_default`, `deletion_protection_on_unless_ci`, `refuses_partitions_over_type_limit` and `ceiling_is_computed`. `tests/byoc/tofu/run.sh` runs these with fmt, validate, tflint and the static check `example_with_keys_enforces_encryption`.
