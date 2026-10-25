# Loams on one Compute Engine VM (your Google Cloud project)

Status: **new, tested without a cloud account** (cloud-init schema, the secrets fetcher against a local mock of the metadata server and Secret Manager, role checks). A real-project run is still to be done.

Creates, in your project: one VM (Ubuntu 24.04) running as a dedicated service account, on a custom network with **no ingress firewall rule**. The bucket, its Cloud KMS key and the secrets are yours and are created by you first. Loams 0.1.2's light default stack runs on the VM (single node: back up the disk, keep bucket versioning on). The engine is reached over your tailnet only.

No key is created: the engine and the secrets fetch use the VM service account's token from the metadata server. See [roles.md](roles.md) for the roles.

## Use it

```bash
PROJECT=my-project; REGION=us-central1; ZONE=us-central1-a; BUCKET=my-loams-bucket

# 1. Secrets (values from your own tailnet admin console; never commit them).
printf %s "$TS_AUTHKEY" | gcloud secrets create loams-prod-tailnet-authkey --data-file=- --project "$PROJECT"
openssl rand -hex 32 | tr -d '\n' | gcloud secrets create loams-prod-setup-token --data-file=- --project "$PROJECT"

# 2. The service account and its roles (roles.md).
gcloud iam service-accounts create loams-vm --project "$PROJECT"
SA="loams-vm@$PROJECT.iam.gserviceaccount.com"
gcloud storage buckets add-iam-policy-binding "gs://$BUCKET" --member "serviceAccount:$SA" --role roles/storage.objectUser
for s in tailnet-authkey setup-token; do
  gcloud secrets add-iam-policy-binding "loams-prod-$s" --project "$PROJECT" --member "serviceAccount:$SA" --role roles/secretmanager.secretAccessor
done

# 3. A custom network: no firewall rules means no ingress (the default network allows SSH).
gcloud compute networks create loams --subnet-mode custom --project "$PROJECT"
gcloud compute networks subnets create loams --network loams --range 10.10.0.0/24 --region "$REGION" --project "$PROJECT"

# 4. The VM, with the cloud-init file as user-data.
deploy/byoc/render.sh gcp --bucket "$BUCKET" --project "$PROJECT" --install prod > user-data.yaml
gcloud compute instances create loams --project "$PROJECT" --zone "$ZONE" \
  --machine-type e2-standard-2 --image-family ubuntu-2404-lts-amd64 --image-project ubuntu-os-cloud \
  --boot-disk-size 60GB --boot-disk-type pd-balanced --subnet loams \
  --service-account "$SA" --scopes cloud-platform \
  --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
  --metadata-from-file user-data=user-data.yaml
```

The VM has an ephemeral external IP for outbound traffic only (the tailnet, GitHub, Docker's registry); with no ingress rule nothing can connect to it. For a shell, add the IAP rule (`gcloud compute firewall-rules create loams-iap --network loams --allow tcp:22 --source-ranges 35.235.240.0/20`) and `gcloud compute ssh --tunnel-through-iap`; that is not an internet-facing rule. Watch the first boot with `gcloud compute instances get-serial-port-output loams`.

## Limits and status

- One node, one disk. Not highly available.
- Not yet run against a real project in CI (the Google Cloud row needs the owner's workload identity pool).

See [variables.md](variables.md), [CHANGELOG.md](CHANGELOG.md), and design 70 (`docs/design/70-byoc-clouds.md`).
