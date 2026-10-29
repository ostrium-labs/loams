# Loams on Google Cloud (one Compute Engine VM)

Read [the overview](index.md) first (status, rules, costs). Files: `deploy/byoc/gcp/` (with [roles.md](../../../deploy/byoc/gcp/roles.md)) and `deploy/tofu/modules/gcp-vm/`. Commands are copy-paste for a shell with `gcloud` signed in; no console click paths.

```bash
export PROJECT=my-project REGION=us-central1 ZONE=us-central1-a BUCKET=my-loams-bucket INSTALL=prod
gcloud config set project "$PROJECT"
gcloud services enable compute.googleapis.com storage.googleapis.com secretmanager.googleapis.com cloudkms.googleapis.com iam.googleapis.com
```

## 1. Before you start

- `gcloud`, `jq` and `openssl`; a clone of `ostrium-labs/loams` at the tag you install.
- A tailnet with MagicDNS and HTTPS certificates on: [Self-hosting over Tailscale](https://loams.dev/docs/self-hosting/tailscale), steps 1 to 4.
- A Tailscale auth key (tagged `tag:loams`) in your shell only: `export TS_AUTHKEY=tskey-auth-...`.

## 2. The bucket

```bash
gcloud storage buckets create "gs://$BUCKET" --location "$REGION" --uniform-bucket-level-access --public-access-prevention
gcloud storage buckets update "gs://$BUCKET" --versioning
cat > lifecycle.json <<'JSON'
{"rule":[{"action":{"type":"Delete"},"condition":{"daysSinceNoncurrentTime":30,"isLive":false}}]}
JSON
gcloud storage buckets update "gs://$BUCKET" --lifecycle-file=lifecycle.json
```

## 3. The key

```bash
gcloud kms keyrings create "loams-$INSTALL" --location "$REGION"
gcloud kms keys create bucket --keyring "loams-$INSTALL" --location "$REGION" --purpose encryption --rotation-period 90d --next-rotation-time "$(date -u -d '+90 days' +%Y-%m-%dT%H:%M:%SZ)"
KEY="projects/$PROJECT/locations/$REGION/keyRings/loams-$INSTALL/cryptoKeys/bucket"
AGENT=$(gcloud storage service-agent --project "$PROJECT")
gcloud kms keys add-iam-policy-binding bucket --keyring "loams-$INSTALL" --location "$REGION" --member "serviceAccount:$AGENT" --role roles/cloudkms.cryptoKeyEncrypterDecrypter
gcloud storage buckets update "gs://$BUCKET" --default-encryption-key "$KEY"
```

## 4. The secrets

Secret Manager names cannot hold `/`, so the install's secrets are `loams-$INSTALL-<name>`.

```bash
printf %s "$TS_AUTHKEY" | gcloud secrets create "loams-$INSTALL-tailnet-authkey" --data-file=-
openssl rand -hex 32 | tr -d '\n' | gcloud secrets create "loams-$INSTALL-setup-token" --data-file=-
```

`loams-$INSTALL-api-keys` is optional. Read the setup token back with `gcloud secrets versions access latest --secret "loams-$INSTALL-setup-token"`.

## 5. The identity

A service account with only these roles, and **no key**:

```bash
gcloud iam service-accounts create "loams-$INSTALL"
SA="loams-$INSTALL@$PROJECT.iam.gserviceaccount.com"
gcloud storage buckets add-iam-policy-binding "gs://$BUCKET" --member "serviceAccount:$SA" --role roles/storage.objectUser
PROJECT_NUMBER=$(gcloud projects describe "$PROJECT" --format 'value(projectNumber)')
gcloud projects add-iam-policy-binding "$PROJECT" --member "serviceAccount:$SA" --role roles/secretmanager.secretAccessor \
  --condition "title=loams-$INSTALL-secrets,expression=resource.name.startsWith(\"projects/$PROJECT_NUMBER/secrets/loams-$INSTALL-\")"
```

The condition limits the grant to this install's secrets (the optional `api-keys` secret does not have to exist yet).

## 6. The VM

```bash
gcloud compute networks create loams --subnet-mode custom        # no firewall rule means no ingress; the default network allows SSH
gcloud compute networks subnets create loams --network loams --range 10.10.0.0/24 --region "$REGION"
deploy/byoc/render.sh gcp --bucket "$BUCKET" --project "$PROJECT" --install "$INSTALL" > user-data.yaml
gcloud compute instances create "loams-$INSTALL" --zone "$ZONE" \
  --machine-type e2-standard-2 --image-family ubuntu-2404-lts-amd64 --image-project ubuntu-os-cloud \
  --boot-disk-size 60GB --boot-disk-type pd-balanced --subnet loams \
  --service-account "$SA" --scopes cloud-platform \
  --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
  --metadata-from-file user-data=user-data.yaml
```

The scope is `cloud-platform` because Secret Manager's API rejects a narrower token; the roles above are what limit the account. The VM's ephemeral external IP is for outbound traffic; with no ingress rule nothing can connect to it. Add `--profiles tikv` to `render.sh` for TiKV, TiDB and PD (and use `e2-standard-4`).

Already have a VM with that service account? `deploy/release/byoc/install.sh --cloud gcp --bucket gs://$BUCKET/loams ubuntu@HOST` installs onto it over SSH.

**OpenTofu instead of steps 2 to 6:** create the tailnet secret (step 4's first command), then `cd deploy/tofu/examples/gcp-vm && tofu init && tofu apply -var project=$PROJECT`.

## 7. Check it

For a shell without an internet-facing port, allow Identity-Aware Proxy to reach SSH (IAP's range, not the internet):

```bash
gcloud compute firewall-rules create "loams-$INSTALL-iap" --network loams --direction INGRESS --allow tcp:22 --source-ranges 35.235.240.0/20
gcloud compute ssh "loams-$INSTALL" --zone "$ZONE" --tunnel-through-iap
sudo journalctl -u cloud-final -u loams-byoc --no-pager | tail -40
sudo docker compose --project-directory /opt/loams/release ps
```

(Or read the serial console: `gcloud compute instances get-serial-port-output loams-$INSTALL --zone $ZONE`.) The first boot takes a few minutes; the unit retries every 30 seconds until the secrets exist. When the sidecar has joined, the engine is at `https://loams.<your-tailnet>.ts.net`; the console asks for the setup token. The bucket capability probe (`loams store probe`) is not built yet; check the bucket by hand if you need to.

## 8. Backups

Versioning protects the bucket; protect the boot disk (it holds the metastore, Live and Durable) with a daily snapshot:

```bash
gcloud compute resource-policies create snapshot-schedule "loams-$INSTALL-daily" --region "$REGION" \
  --daily-schedule --start-time 03:00 --max-retention-days 7 --on-source-disk-delete keep-auto-snapshots
gcloud compute disks add-resource-policies "loams-$INSTALL" --zone "$ZONE" --resource-policies "loams-$INSTALL-daily"
```

To restore, create a disk from a snapshot and boot a new VM from it with the same cloud-init file, or attach it and mount it at `/var/lib/docker`. A snapshot of a running disk is crash-consistent; stop the stack first (`sudo systemctl stop loams-byoc`) for a cleaner one.

## 9. Upgrades

Open a shell (step 7) and follow [upgrades](../../release/upgrades.md): `cd /opt/loams/release`, install the target release's `loams` CLI (checked against that release's `SHA256SUMS`), then `loams upgrade check --to <version>` and `loams upgrade --to <version>`. Embedded stacks are upgraded one release at a time and by hand for now.

## 10. Private reach

An internal load balancer for the same VPC and Private Service Connect for other projects are design 69 and a later BY1 milestone; they are not set up here. Use your tailnet.

## 11. Costs

About $60 a month for `e2-standard-2` (see [the overview](index.md#costs-estimates-small-install-one-month-2026-10-21-list-prices)).

## 12. Remove it

```bash
gcloud compute instances delete "loams-$INSTALL" --zone "$ZONE"
gcloud compute networks subnets delete loams --region "$REGION"; gcloud compute networks delete loams
gcloud projects remove-iam-policy-binding "$PROJECT" --member "serviceAccount:$SA" --role roles/secretmanager.secretAccessor \
  --condition "title=loams-$INSTALL-secrets,expression=resource.name.startsWith(\"projects/$PROJECT_NUMBER/secrets/loams-$INSTALL-\")"
gcloud iam service-accounts delete "$SA"
gcloud secrets delete "loams-$INSTALL-tailnet-authkey"; gcloud secrets delete "loams-$INSTALL-setup-token"
```

Your data is in the bucket and the snapshots: keep them, or delete the bucket (`gcloud storage rm -r gs://$BUCKET`). A key ring cannot be deleted; schedule the key version's destruction if you want it gone. Remove the machine from your Tailscale admin console.
