# deploy/byoc/confluent

Tables for Confluent Cloud next to a Loams BYOC install (design 80, plan CC1).

| File | What | Maintained by |
|---|---|---|
| `regions.toml` | Every Confluent Cloud region: `cloud`, `region`, `schedulable`, and the cluster `types` offered there | `scripts/byoc/confluent-tables.sh refresh`, from `https://confluent.cloud/api/env_metadata` (the endpoint `confluent kafka region list` calls; undocumented, CC0 ruling R0.4). Do not edit by hand |
| `prices.toml` | List prices and limits per cluster type: eCKU-hour, $/GB in and out, $/GB-month stored, the storage replication factor, partitions per eCKU and per cluster, maximum eCKU | By hand, from the pricing and cluster-types pages; every value carries its source URL and read date |

The `confluent` OpenTofu module uses `regions.toml` for its co-location check (D1824) and `prices.toml` for `confluent_ceiling_monthly_usd` (D1828); the budget guard (CC2) uses `prices.toml` for its hourly estimate. Every number derived from `prices.toml` is an **estimate** at list price, labelled with the table's read date.

Region codes are the clouds' own (`us-east-1`, `us-central1`, `eastus`), so a cluster next to a Loams VM uses the VM's region. A region with `schedulable = false` (AWS `me-south-1` and `me-central-1`, the Azure Jio regions on 2026-10-26) is listed but takes no new cluster.

## Checks

```bash
scripts/byoc/confluent-tables.sh check            # offline: parses, BY1 defaults covered, sources and dates, prices younger than 90 days
CONFLUENT_CLOUD_API_KEY=... CONFLUENT_CLOUD_API_SECRET=... \
  scripts/byoc/confluent-tables.sh diff           # 0 unchanged, 3 the region list changed, 4 the listing changed shape
```

`.github/workflows/byoc-confluent-tables.yml` runs both every Monday. It opens a pull request when the region list changes and an issue when the listing's shape changes; it fails when the prices are older than 90 days.
