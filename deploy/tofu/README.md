# deploy/tofu

OpenTofu modules for Loams on one VM in your own cloud account (design 70 §9.1, D1362; plan BY1). OpenTofu is MPL-2.0; the modules use the official `hashicorp/aws`, `hashicorp/google`, `hashicorp/azurerm` and `hashicorp/random` providers as dependencies (never their source). Terraform may work; only OpenTofu is tested.

```
modules/
  shared/      renders deploy/byoc/<cloud>/cloud-init.yaml (the same value checks as deploy/byoc/render.sh)
  aws-vm/      EC2 + bucket + KMS key + Secrets Manager + instance role, no inbound rule
  gcp-vm/      Compute Engine + bucket + Cloud KMS key + Secret Manager + service account, no ingress rule
  azure-vm/    VM + storage account + Key Vault key and secrets + managed identity, no inbound allow rule
examples/<module>/   a minimal root module each
```

Status: **validated offline, not applied to a real account.** `tofu validate`, `tofu fmt`, `tflint` and `tofu test` (mock providers) run in CI (`.github/workflows/byoc-static.yml`). Nobody has run `tofu apply` on these yet; the first real apply will find edge cases (IAM propagation, quotas, naming).

## What every module guarantees (and tests)

- No inbound rule: the AWS security group has no `ingress`, the Google network has no firewall rule (IAP SSH is opt-in), the Azure NSG has no rule. The engine is reached over your tailnet.
- No static cloud key: the VM reads its role, service account or managed identity. The Azure storage account has shared keys turned off.
- The release is a pinned version (`latest` is refused) and its signature is checked on the VM.
- The bucket is private, versioned and encrypted with a customer-managed key; noncurrent versions expire after 30 days.
- Secrets are secret IDs, never values: you create `<prefix>-tailnet-authkey` yourself (AWS: before apply; Azure: after, the VM retries). The setup token is random and created by the module (its value is in your state; turn off with `create_setup_token = false`).
- **State is yours.** The modules configure no backend.

## Use

```bash
cd deploy/tofu/examples/aws-vm        # or gcp-vm, azure-vm
tofu init && tofu plan                # prints estimated_monthly_usd
tofu apply
```

Inputs and outputs are in each module's `variables.tf` and `outputs.tf`. Not yet built (BY1 milestones): container-service modules, Kubernetes modules, `private_reach`, `budget_monthly_usd`, the mirror repository and registry listing (`ostrium-labs/terraform-loams-byoc`).

Run the checks locally with `tests/byoc/tofu/run.sh`.
