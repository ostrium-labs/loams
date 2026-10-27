# deploy/tofu

OpenTofu modules for Loams on one VM in your own cloud account (design 70 §9.1, D1362; plan BY1), and for a Confluent Cloud cluster next to it (design 80, plan CC1). OpenTofu is MPL-2.0. The modules depend on the official `hashicorp/aws`, `hashicorp/google`, `hashicorp/azurerm` and `hashicorp/random` providers and on Confluent's `confluentinc/confluent` (Apache-2.0); they never include provider source. Terraform may work; only OpenTofu is tested.

```
modules/
  shared/      renders deploy/byoc/<cloud>/cloud-init.yaml (the same value checks as deploy/byoc/render.sh)
  aws-vm/      EC2 + bucket + KMS key + Secrets Manager + instance role, no inbound rule
  gcp-vm/      Compute Engine + bucket + Cloud KMS key + Secret Manager + service account, no ingress rule
  azure-vm/    VM + storage account + Key Vault key and secrets + managed identity, no inbound allow rule
  aws-free/    app BYOC on AWS's always-free tier (design 71 §7.2): Lambda + Function URL, DynamoDB
               provisioned 25/25, SQS, optional EventBridge Scheduler and S3
  gcp-free/    app BYOC on Google Cloud's free tier (§7.3): Cloud Run (request-based, scale to zero),
               Firestore (default), Cloud Tasks, optional Cloud Scheduler, GCS, Artifact Registry
  azure-free/  app BYOC on Azure's free allowances (§7.4): Functions Flex Consumption, Cosmos DB free
               tier capped at 1,000 RU/s, Queue Storage, Application Insights with a daily cap
  confluent/   Confluent Cloud: environment, Basic/Standard cluster in the VM's region, topics, a least-privilege
               data account and its ACLs, the ceiling estimate (see modules/confluent/README.md)
examples/<module>/   a minimal root module each; examples/aws-vm-confluent/ wires aws-vm and confluent
               (state and plan encryption enforced on your KMS key)
```

Status: **validated offline, not applied to a real account** (the `*-free` modules: see plan CB1 for their first real runs). `tofu validate`, `tofu fmt`, `tflint` and `tofu test` (mock providers) run in CI (`.github/workflows/byoc-static.yml`). Nobody has run `tofu apply` on the VM modules yet; the first real apply will find edge cases (IAM propagation, quotas, naming). The `confluent` module (through `examples/confluent`) was applied and destroyed once in Ostrium's `loams-ci` Confluent environment on 2026-10-26 (Basic, `us-east-1`).

## What every module guarantees (and tests)

- No inbound rule: the AWS security group has no `ingress`, the Google network has no firewall rule (IAP SSH is opt-in), the Azure NSG has no rule. The engine is reached over your tailnet.
- No static cloud key: the VM reads its role, service account or managed identity. The Azure storage account has shared keys turned off.
- The release is a pinned version (`latest` is refused) and its signature is checked on the VM.
- The bucket is private, versioned and encrypted with a customer-managed key; noncurrent versions expire after 30 days.
- Secrets are secret IDs, never values: you create `<prefix>-tailnet-authkey` yourself (AWS: before apply; Azure: after, the VM retries). The setup token is random and created by the module (its value is in your state; turn off with `create_setup_token = false`).
- **State is yours.** The modules configure no backend.

## The free-tier app modules (`*-free`)

They are the app-BYOC targets of design 71 (D1395), which `loams app deploy` will call from its generated root module (D1406); plan CB1 benchmarks them. Each one:

- **creates the budget first:** every other resource depends on it. It alerts at 50%, 80% and 100% of actual spend and at 100% of forecast, and never stops anything (Q1102);
- **tags every resource** with `loams-app=<name>`, and `loams-bench=<run_id>` when `run_id` is set (Google labels only where a resource takes them);
- **sets the cloud's hard caps:**
  - AWS: DynamoDB provisioned capacity with no autoscaling; Lambda reserved concurrency (5); SQS mapping concurrency (2).
  - Google Cloud: Cloud Run maximum instances (2) and no minimum.
  - Azure: Flex maximum instances (2); the Cosmos DB account's total throughput limit (1,000 RU/s); a daily log cap (0.1 GB).
- **refuses non-free settings unless `allow_paid = true`** (D1400): on-demand DynamoDB, more than 25 units, a Google region outside the free US regions, Cosmos DB without the free tier or above 1,000 RU/s, more than 4 instances;
- **creates no VPC, NAT, public IP or backups**, and keeps logs 1 day or capped;
- **prints a `free_tier` output** saying what is free and what is not.

Not free even so: S3 on AWS accounts opened since 2025-07-15 (off by default) and the Azure storage account (cents).

## Use

```bash
cd deploy/tofu/examples/aws-vm        # or gcp-vm, azure-vm
tofu init && tofu plan                # prints estimated_monthly_usd
tofu apply
```

Inputs and outputs are in each module's `variables.tf` and `outputs.tf`. Not yet built (BY1 milestones): container-service modules, Kubernetes modules, `private_reach`, `budget_monthly_usd`, the mirror repository and registry listing (`ostrium-labs/terraform-loams-byoc`).

Run the checks locally with `tests/byoc/tofu/run.sh`.
