# Confluent Cloud with a Loams BYOC install

Use Confluent Cloud as the Kafka edge of a Loams install that runs in your own cloud account ([index.md](index.md)). Loams reads topics into its streams and writes its streams to topics. Your Confluent organisation, your cluster and your bill stay yours: Ostrium's Confluent credits never pay for your traffic (design 80, [D1831](../../design/80-confluent-byoc.md)).

> **Status (2026-10-27).** Built:
> - the OpenTofu module (`deploy/tofu/modules/confluent/`) and its examples;
> - the region and price tables (`deploy/byoc/confluent/`);
> - the boot-time secrets fetch;
> - `loams byoc confluent key create|rotate|check` (in releases after 0.1.2).
>
> The module was applied and destroyed in Ostrium's CI environment on Basic in `us-east-1`. **Not built yet:**
> - the connector runtime on the VM, `loams-fabric connect` (plan CC1 Task 1). Until it ships, nothing on the VM moves records;
> - the budget guard (CC2);
> - keyless OAuth and PrivateLink (CC3);
> - the Google Cloud and Azure examples.
>
> If something breaks, open an issue on `ostrium-labs/loams`.

## 1. What "with Confluent" means

There are three ways to use Confluent with Loams. You can combine them.

| | (a) Connect to your cluster | (b) Create a cluster next to Loams | (c) Confluent instead of the Iggy `kafka` profile |
|---|---|---|---|
| For | Teams that already run Confluent: Debezium CDC topics, event streams, producers | Teams that want Kafka for Loams and have none | Teams choosing a Kafka endpoint for their own apps |
| Loams does | Kafka source instances read topics into streams; sink instances write streams to topics | The module creates the environment (or uses yours), a Basic or Standard cluster in the VM's region, the topics, a least-privilege account and its ACLs; then as (a) | Nothing new: you point your apps at Confluent rather than at the VM's Iggy |
| Your cost | Your Confluent bill plus Loams' traffic (§6) | A new Confluent bill: Basic from $0 (the first eCKU is free), Standard from $0.75 an eCKU-hour | Confluent's bill instead of the VM's resources |

Confluent is always an **edge** of Loams, never its internal log. The bucket stays the source of truth, and the install keeps working when Confluent is unreachable.

**Iggy or Confluent** for your own apps:

| | Iggy `kafka` profile | Confluent Cloud |
|---|---|---|
| Runs | On the Loams VM, loopback by default | In Confluent's cloud, on a public endpoint |
| Kafka compatibility | Preview; no transactions yet | Kafka; transactions (exactly-once sinks pass on Basic and Standard) |
| Durability | One node, one disk | Replicated; 99.5% SLA on Basic, 99.9% on Standard |
| Cost | The VM's CPU, memory and disk | Confluent's bill |

## 2. Where the cluster goes

The cluster's cloud and region are the VM's. Confluent uses the clouds' own region codes, so a VM in `us-east-1` gets a cluster in AWS `us-east-1`. The module refuses another cloud or region unless you set `allow_cross_region = true`. Cross-region traffic pays your cloud's internet data transfer on every byte and adds tens of milliseconds to every round trip.

`deploy/byoc/confluent/regions.toml` lists every region and the cluster types it offers. A weekly job refreshes it from Confluent's region listing. Basic and Standard are offered almost everywhere. Exceptions on 2026-10-26:
- Google Cloud `europe-north2` offers Enterprise and Dedicated only;
- AWS `me-south-1` and `me-central-1` take no new clusters;
- the Azure Jio regions are Dedicated only.

For an existing cluster in another region, `loams byoc confluent check` warns you.

## 3. Networking

On Basic and Standard, Loams reaches the cluster's **public endpoint**, outbound from the VM, over TLS with full hostname verification and SASL/PLAIN (port 9092, and 443 for REST and Schema Registry). The VM's security group already allows outbound traffic. **Nothing in Confluent ever connects to Loams**: no inbound rule, Tunnel route or port is added.

As a consequence, Confluent's fully managed connectors that would *pull* from a Loams database (for example Debezium PostgreSQL on Confluent) are **not supported**. To get CDC out of Loams, use Loams' own Postgres CDC source and a Kafka sink, which is outbound.

PrivateLink needs Enterprise or Dedicated. It comes in CC3, opt-in, with its price stated (Enterprise starts at about $1,280 a month at one eCKU).

## 4. Create a cluster next to the VM (mode b)

`deploy/tofu/examples/aws-vm-confluent/` wires `modules/aws-vm` and `modules/confluent` together. `deploy/tofu/examples/confluent/` is the Confluent half alone, for a VM managed elsewhere.

What you need:
- **Your Confluent Cloud API key** in `CONFLUENT_CLOUD_API_KEY` and `CONFLUENT_CLOUD_API_SECRET`. The provider reads it from the environment and never writes it to state.
- **A KMS key for the state** (the example enforces state and plan encryption):
  ```bash
  aws kms create-key --description "OpenTofu state for Loams prod" --query KeyMetadata.KeyId --output text
  ```
- What [aws.md](aws.md) needs for the VM, for example the Tailscale auth key's secret.

Topics and ACLs need a Kafka key with admin rights on the cluster. By default (`kafka_admin = "provider"`), that key reaches OpenTofu only through the provider's `KAFKA_*` environment variables, so it is never in state. A new cluster therefore takes two applies:

```bash
cd deploy/tofu/examples/aws-vm-confluent
tofu init
tofu apply -var state_kms_key_id=<key id> -var kafka_admin=none     # VM, environment, cluster, accounts

# An admin key for yourself on the new cluster, for topics and ACLs only:
confluent api-key create --resource "$(tofu output -raw confluent_cluster_id)"
export KAFKA_ID=$(tofu output -raw confluent_cluster_id)
export KAFKA_REST_ENDPOINT=$(tofu output -raw confluent_rest_endpoint)
export KAFKA_API_KEY=<key> KAFKA_API_SECRET=<secret>
tofu apply -var state_kms_key_id=<key id>                            # topics and ACLs
```

Keep the admin key out of shell history and delete it when you are done. `tofu destroy` needs it again to delete the topics and ACLs.

With `kafka_admin = "module_key"` and `create_api_keys = true`, the module creates the admin and data keys itself in one apply. The keys are then in your state, which is why the example's `encryption.tf` sets `enforced = true` for both state and plans (D1827).

**Defaults that bound spend:**
- `cluster_type = "basic"`, `max_ecku = 1`;
- one topic with 3 partitions and `retention_bytes` of 1 GiB per partition;
- Loams' connector rate limit at 64 KiB/s each way;
- a `confluent_budget_monthly_usd` of 25.

`tofu plan` prints `confluent_ceiling_monthly_usd` (about $16.78 for these defaults) and `confluent_unbounded` before anything is created (§6).

**Enterprise, Freight and Dedicated** are refused unless you set `allow_premium_cluster_types = true`. Mode (a) with an existing cluster: set `environment_id`, `cluster_id` and `data_topics` (the existing topics Loams may use); the module then creates only the account and its ACLs.

## 5. Identities and keys

| Account | Where its key lives | Grants |
|---|---|---|
| `loams-<name>-data` | Your cloud's secret store, then the connect role's secret files on the VM | Topic READ, WRITE, DESCRIBE and DESCRIBE_CONFIGS on the named topics; group READ and DESCRIBE on `loams-<name>-*`; transactional-id WRITE and DESCRIBE on `loams-<name>-*`; IDEMPOTENT_WRITE. **Never** topic CREATE, DELETE, ALTER or ALTER_CONFIGS, or any ACL operation, so Loams cannot create topics, add partitions, raise retention or widen its own access |
| `loams-<name>-metrics` | For the budget guard (CC2) | MetricsViewer on the install's environment |
| `loams-<name>-ops` (off by default) | Only if you opt into the guard's `max_ecku` throttle | CloudClusterAdmin on that one cluster |
| Your own key | Your shell | Whatever you have; used by `tofu` and `loams byoc confluent key create` |

Basic clusters have no in-cluster RBAC, so ACLs scope everything. In Ostrium's CI, the data key was refused all of these: creating a topic, raising `retention.bytes`, adding partitions, deleting a topic, creating an ACL, and writing a topic it was not granted.

**Create Loams' key** with your own Confluent key and your cloud CLI's login. The command writes it straight into the secret store and prints only the key's id:

```bash
loams byoc confluent key create --integration confluent \
  --service-account "$(tofu output -raw confluent_data_service_account_id)" \
  --cluster "$(tofu output -raw confluent_cluster_id)" --environment <env-id> \
  --cloud aws --region us-east-1 --secret-prefix loams/prod
```

- The value goes to `aws secretsmanager` (or `gcloud secrets` / `az keyvault secret set`) on stdin, never in argv.
- The secrets are `loams/prod/confluent-confluent-api-key` and `-api-secret`; on Google Cloud and Azure the `/` becomes `-`.
- Add `--schema-registry <lsrc-id>` for a Schema Registry key (`-sr-key`, `-sr-secret`).

**On the VM**, `CONFLUENT_INTEGRATIONS` in `/etc/loams-byoc/config.env` names the integrations. The example sets it with `confluent_integrations = ["confluent"]`; `deploy/byoc/render.sh --confluent-integrations confluent` does the same for the CLI path.
- At every start, `loams-byoc-secrets` reads the keys with the VM's own role and writes them to `/opt/loams/release/secrets/connect/0/confluent-confluent/{username,password}` (mode 600). Nothing goes into `.env`.
- After that, `loams-byoc-confluent.timer` copies them again every 5 minutes, so a rotated key reaches the files without a restart. `sudo systemctl reload loams-byoc` copies them at once.
- A key that is not in the store yet is reported, not fatal.
- User data and state hold secret names only.

**Connector instances.** The connect role runs one instance file per topic or stream in `/opt/loams/release/connect/instances/` ([connect-role.md](../connectors/connect-role.md)). For an install over SSH, `deploy/release/byoc/install.sh --kafka confluent --confluent-bootstrap <endpoint> --confluent-topics <topics> --confluent-prefix "$(tofu output -json confluent_connect_integration | jq -r .consumer_group_prefix)"` writes them from `deploy/release/connect/templates/`, with the group and transactional ids under the module's prefix and the example's 64 KiB/s rate ([kafka.md](../kafka.md)). On a VM the example created, write them there yourself from the same templates (the `confluent_connect_integration` output has every value they need).

**Rotate** without downtime:

```bash
loams byoc confluent key rotate <same flags as create> \
  --connect-admin http://loams-connect.<tailnet>.ts.net:7790 --admin-token-file ./connect-admin-token \
  --restart-instance orders-in --restart-instance orders-out --grace 1h
```

It creates a new key and writes a new secret version. It then waits `--refresh-wait` (default 6m) for the VM's timer to copy the key, restarts the named instances, waits `--grace` and deletes the old keys. If you ran `sudo systemctl reload loams-byoc` on the VM already, pass `--refresh-wait 0s`. If a restart fails, or no instance is named, it keeps the old keys and says so. Without `--connect-admin`, it tells you to restart the instances yourself once the VM has the key.

**Check** the placement and reachability from the VM (or anywhere):

```bash
loams byoc confluent check --cloud aws --region us-east-1 --cluster <lkc-id> --environment <env-id> \
  --schema-registry-url https://psrc-....confluent.cloud
```

## 6. Budget: what is hard and what is not

Confluent Cloud has **no budget or spend-cap API**. What can be capped:

| Lever | Enforced by | Hard? | What it bounds |
|---|---|---|---|
| Cluster type | The module | Hard | Enterprise, Freight and Dedicated need an explicit flag |
| `max_ecku` | Confluent | Hard | eCKU charges at most `max_ecku` × price × 730 hours. Basic at 1 eCKU: $0; Standard at 1: at most $547.50 a month |
| Per-eCKU capacity | Confluent | Hard, but large | Basic: 5 MBps in and 15 MBps out per eCKU. At $0.05/GB, that alone bounds a month at hundreds of dollars: a bound, not a budget |
| Topic count and partitions | The module (Loams' account has no CREATE) | Hard for Loams | Basic: 30 partitions per eCKU, 1,500 per cluster; the module refuses more |
| `retention.bytes` per partition | The module (no ALTER_CONFIGS) | Hard for module topics | Storage at most Σ partitions × `retention.bytes` × 3 (replication) × $0.08/GB-month |
| Loams' connector byte rate | The connect role's `rate_limit` | Hard for Loams' own traffic | Loams' share of throughput charges |
| Client quotas | Confluent, Enterprise/Freight/Dedicated only | Hard on those types | Not available on Basic or Standard |
| A monthly spend cap | Nobody | **Not available** | The guard (CC2) will be best-effort |

`confluent_ceiling_monthly_usd` adds up what the configuration bounds: eCKUs above the free one, managed storage, and Loams' connector rates at list prices. Its label (`confluent_ceiling.label`) gives the price table's date. `confluent_unbounded` lists what it does not bound:
- other clients on a shared cluster;
- Schema Registry beyond 100 schemas;
- anything created outside the module;
- cloud-side data transfer.

It is an **estimate**, never a cap on your bill.

**The budget guard is CC2, not built yet.** It will estimate spend hourly from the Metrics API, reconcile daily with the Costs API, alert at 50% and 80%, throttle Loams' connectors at 90%, and (opt-in) pause them at 100%. It never deletes anything. Until then, keep Confluent's own "Free usage is 80% consumed" and service-quota notifications on.

**Cloud-side transfer** (between the VM and Confluent's public endpoint; figures from 2026-10-26, verify them on your cloud's pricing page):

| Cloud | Same-region traffic to a public endpoint |
|---|---|
| AWS | About $0.01/GB each way over public IPv4; through a NAT gateway, $0.045/GB plus $0.045 an hour (the example VM uses its own public IP, no NAT) |
| Google Cloud | Cloud NAT $0.045/GiB; whether traffic to Confluent's in-region endpoints counts as internet egress is not settled |
| Azure | Internet egress about $0.087/GB after the first 100 GB |

## 7. Troubleshooting

| Symptom | Likely cause |
|---|---|
| `Error: ... cluster_type enterprise is a premium type` | Set `allow_premium_cluster_types = true`, or keep Basic or Standard |
| `... is not in the VM's cloud and region` | The cluster's region differs from the VM's; move it or set `allow_cross_region = true` |
| `Confluent does not offer basic clusters in ...` | `regions.toml` says the region lacks the type or takes no new clusters |
| `the module's topics need N partitions` | Basic holds 30 partitions per eCKU; use fewer partitions or a higher `max_ecku` |
| Phase two fails with `kafka_api_key` errors | `KAFKA_ID`, `KAFKA_REST_ENDPOINT`, `KAFKA_API_KEY` and `KAFKA_API_SECRET` must all be set |
| Topic or ACL creation fails right after the admin key was created | A new key can take a minute to work everywhere; apply again |
| `loams-byoc-secrets: confluent-confluent: ... is not in the secret store yet` | Run `loams byoc confluent key create`, then `sudo systemctl restart loams-byoc` on the VM |
| `check` warns about the region | The cluster and the VM are in different regions; expect egress charges and latency |

## 8. Remove it

Delete the connector instances first. Then run `tofu destroy` with the `KAFKA_*` settings exported, as in §4. A cluster the module created has deletion protection on: set `deletion_protection = false` and apply before you destroy. Delete the admin key you made by hand, and the secrets if the example did not create them.

## Related

[Design 80](../../design/80-confluent-byoc.md), [plan CC1](../../plans/2026-10-25-cc1-confluent-byoc.md), [aws.md](aws.md), [the Kafka connectors](../connectors/kafka.md).
