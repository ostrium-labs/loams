# Loams in your own cloud account (BYOC)

Bring your own cloud: Loams runs in your AWS account, Google Cloud project or Azure subscription, on one VM, with the data in a bucket you own, encrypted with a key you own, and the install's secrets in your cloud's secret store. Ostrium runs nothing for you and sees nothing: there is no hosted control plane in this path (design 70, [D1378](../../design/70-byoc-clouds.md)).

> **Status (2026-10-23).** The files this guide uses exist and are tested **without a cloud account**: the cloud-init schema, a local mock of each cloud's secret store, the IAM and role policies, the "no inbound rule" and "no static key" checks. **They have not been run on a real AWS, Google Cloud or Azure account yet**, so first-boot timing and IAM edge cases are unverified; if something breaks, open an issue on `ostrium-labs/loams`. Not built yet: the container tier (ECS, Cloud Run, Container Apps), the Kubernetes tier, `loams byoc deploy`, the bucket capability probe (`loams store probe`), the network-file-system guard and the support bundle. Cloudflare and Vercel are in design 70 and not in this guide set yet.

## What you get

One VM (Ubuntu 24.04, Linux 6.8) running Loams 0.1.4's **light default stack**: the engine on its embedded stores with your bucket, the House, Loams Postgres on local folders, agentd and the Tailscale sidecar. It is **single node and single disk**: no replication, so back up the disk (guides, "Backups") and keep the bucket's versioning on. The `tikv` and `rustfs` profiles add several GB of images and RAM; pick them before the first start.

## The rules

- **Never public.** Nothing opens an inbound port: the security group, firewall network or NSG has no inbound rule. The VM has a public address only for outbound traffic (the tailnet, GitHub, Docker's registry). You reach the engine over your own Tailscale tailnet, and a shell through Session Manager, IAP or `az vm run-command`.
- **No static cloud keys.** The engine and the secrets fetch use the VM's identity: the instance role (IMDSv2, hop limit 2 for the containers), the service account's metadata token, or the managed identity.
- **Pinned and verified.** The cloud-init file downloads `loams-compose-<version>.tar.gz`, checks `SHA256SUMS.asc` against the Loams release key (fingerprint `29C5 CC3F 0FD6 6A2A D32E CB30 9E8E C3E0 AAB5 BA68`; the previous key `333A C945 99D5 A1EB 2A41 6D1E 83DC 134D 8CAF 895D` is still accepted for 0.1.0 to 0.1.4) and the bundle against `SHA256SUMS`. `latest` is refused.
- **The persistence rule.** The embedded stores (the metastore on redb, Live on redb, Durable on SQLite) need a **local block disk**: an EBS volume, a Persistent Disk or an Azure managed disk. Never EFS, Azure Files, Filestore or a FUSE mount of a bucket (design 70 §5.2). The engine does not yet refuse such a mount (BY0 Task 1 is not built), so this is on you.

## Which guide

| Cloud | Guide | Files |
|---|---|---|
| AWS | [aws.md](aws.md) | `deploy/byoc/aws/`, `deploy/tofu/modules/aws-vm/` |
| Google Cloud | [gcp.md](gcp.md) | `deploy/byoc/gcp/`, `deploy/tofu/modules/gcp-vm/` |
| Azure | [azure.md](azure.md) | `deploy/byoc/azure/`, `deploy/tofu/modules/azure-vm/` |

**With Confluent Cloud:** [confluent.md](confluent.md) puts a Confluent Cloud cluster next to the VM (or connects your existing one): the `confluent` OpenTofu module, the keys in your secret store, and what a budget can and cannot cap.

Each guide gives two ways: **the CLI commands** (create the pieces, then one cloud-init file starts the VM) and **OpenTofu** (the same pieces as a module; see `deploy/tofu/README.md`). If you already have a VM with an identity, `deploy/release/byoc/install.sh --cloud ...` installs onto it over SSH (`deploy/release/README.md`, "On a cloud VM").

## Costs (estimates, small install, one month, 2026-10-21 list prices)

| Cloud | Total | Where it goes |
|---|---|---|
| AWS (`t3.large`, us-east-1) | about $73 | instance $60.74, gp3 disk, 50 GB of S3 $1.15, one KMS key $1.00, secrets $1.60, public IPv4 $3.65 |
| Google Cloud (`e2-standard-2`) | about $60 | instance $48.92, `pd-balanced` disk, 50 GiB of GCS $1.00, one key $0.06, external IPv4 about $3.65 |
| Azure (`Standard_D2s_v5`, East US) | about $82 | VM $70.08, disks about $7, 50 GB of Blob $1.04, Key Vault under $0.10, public IPv4 about $3.65 (about $72 on `B2ms`) |

Data transfer and your Tailscale plan are extra. The OpenTofu modules print `estimated_monthly_usd` from the same tables. Prices change: check the cloud's pricing page.

## Related

[Design 70](../../design/70-byoc-clouds.md), [deploy-buttons rules](../../ecosystem/deploy-buttons.md), [upgrades](../../release/upgrades.md), [signing](../../release/signing.md).
