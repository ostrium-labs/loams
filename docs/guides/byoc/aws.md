# Loams on AWS (one EC2 instance)

Read [the overview](index.md) first (status, rules, costs). Files: `deploy/byoc/aws/` and `deploy/tofu/modules/aws-vm/`. Commands are copy-paste for a shell with the AWS CLI v2 signed in to the account and region you want; no console click paths.

```bash
export REGION=us-east-1 BUCKET=my-loams-bucket INSTALL=prod
export ACCOUNT=$(aws sts get-caller-identity --query Account --output text)
```

## 1. Before you start

- The AWS CLI v2, `jq` and `openssl`; a clone of `ostrium-labs/loams` at the tag you install (the commands read `deploy/byoc/`).
- A tailnet with MagicDNS and HTTPS certificates on: [Self-hosting over Tailscale](https://loams.dev/docs/self-hosting/tailscale), steps 1 to 4.
- A Tailscale auth key (reusable or one-off, tagged `tag:loams`). Keep it in your shell only: `export TS_AUTHKEY=tskey-auth-...`.

## 2. The bucket

```bash
aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" $( [ "$REGION" = us-east-1 ] || echo --create-bucket-configuration LocationConstraint="$REGION" )
aws s3api put-public-access-block --bucket "$BUCKET" --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
aws s3api put-bucket-versioning --bucket "$BUCKET" --versioning-configuration Status=Enabled
aws s3api put-bucket-lifecycle-configuration --bucket "$BUCKET" --lifecycle-configuration '{"Rules":[{"ID":"noncurrent","Status":"Enabled","Filter":{},"NoncurrentVersionExpiration":{"NoncurrentDays":30},"AbortIncompleteMultipartUpload":{"DaysAfterInitiation":7}}]}'
```

## 3. The key

```bash
KEY_ARN=$(aws kms create-key --description "Loams $INSTALL bucket" --query KeyMetadata.Arn --output text --region "$REGION")
KEY_ID=${KEY_ARN##*/}
aws kms enable-key-rotation --key-id "$KEY_ID" --region "$REGION"
aws s3api put-bucket-encryption --bucket "$BUCKET" --server-side-encryption-configuration \
  "{\"Rules\":[{\"ApplyServerSideEncryptionByDefault\":{\"SSEAlgorithm\":\"aws:kms\",\"KMSMasterKeyID\":\"$KEY_ARN\"},\"BucketKeyEnabled\":true}]}"
```

Only the install's role gets use of the key (step 5).

## 4. The secrets

```bash
aws secretsmanager create-secret --region "$REGION" --name "loams/$INSTALL/tailnet-authkey" --secret-string "$TS_AUTHKEY"
aws secretsmanager create-secret --region "$REGION" --name "loams/$INSTALL/setup-token" --secret-string "$(openssl rand -hex 32)"
```

`loams/$INSTALL/api-keys` is optional (hashed API keys for the CLI and SDKs; it becomes `secrets/api-keys`). The setup token is what the console asks for on first run; read it back with `aws secretsmanager get-secret-value --secret-id loams/$INSTALL/setup-token --query SecretString --output text`.

## 5. The identity

```bash
sed -e "s/BUCKET/$BUCKET/g" -e "s/REGION/$REGION/g" -e "s/ACCOUNT/$ACCOUNT/g" \
    -e "s/KEY_ID/$KEY_ID/g" -e "s#SECRET_PREFIX#loams/$INSTALL#g" deploy/byoc/aws/instance-policy.json > policy.json
aws iam create-role --role-name "loams-$INSTALL" --assume-role-policy-document file://deploy/byoc/aws/instance-trust-policy.json
aws iam put-role-policy --role-name "loams-$INSTALL" --policy-name loams --policy-document file://policy.json
aws iam attach-role-policy --role-name "loams-$INSTALL" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore   # optional: a shell without a port
aws iam create-instance-profile --instance-profile-name "loams-$INSTALL"
aws iam add-role-to-instance-profile --instance-profile-name "loams-$INSTALL" --role-name "loams-$INSTALL"
```

The policy is exactly `deploy/byoc/aws/instance-policy.json`: S3 object and list access on `BUCKET/loams/*`, `kms:GenerateDataKey` and `kms:Decrypt` on the one key, `secretsmanager:GetSecretValue` on `loams/$INSTALL/*`. Nothing else.

## 6. The VM

```bash
SG=$(aws ec2 create-security-group --group-name "loams-$INSTALL" --description "Loams: no inbound" --query GroupId --output text --region "$REGION")   # a new group has no inbound rule
deploy/byoc/render.sh aws --bucket "$BUCKET" --region "$REGION" --install "$INSTALL" --gzip > user-data.yaml.gz   # the plain file is over EC2's 16 KB limit
AMI=$(aws ssm get-parameter --region "$REGION" --name /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id --query Parameter.Value --output text)
aws ec2 run-instances --region "$REGION" --image-id "$AMI" --instance-type t3.large \
  --iam-instance-profile "Name=loams-$INSTALL" --security-group-ids "$SG" --associate-public-ip-address \
  --metadata-options HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled \
  --block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=60,VolumeType=gp3,Encrypted=true}' \
  --user-data fileb://user-data.yaml.gz --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=loams-$INSTALL}]"
```

`HttpPutResponseHopLimit=2` is required: with 1 the engine's container cannot reach IMDSv2. Add `--profiles tikv` to `render.sh` to start with TiKV, TiDB and PD (and use `t3.xlarge`).

Already have a VM with the instance profile? `deploy/release/byoc/install.sh --cloud aws --bucket s3://$BUCKET/loams --region $REGION ubuntu@HOST` installs onto it over SSH instead.

**OpenTofu instead of steps 2 to 6:** create the tailnet secret (step 4's first command), then `cd deploy/tofu/examples/aws-vm && tofu init && tofu apply`. The module creates the bucket, key, setup token, role and instance with the same settings.

## 7. Check it

```bash
INSTANCE=$(aws ec2 describe-instances --region "$REGION" --filters Name=tag:Name,Values="loams-$INSTALL" Name=instance-state-name,Values=running --query 'Reservations[0].Instances[0].InstanceId' --output text)
aws ssm start-session --region "$REGION" --target "$INSTANCE"     # a shell, no port
sudo journalctl -u cloud-final -u loams-byoc --no-pager | tail -40
sudo docker compose -f /opt/loams/release/compose.yaml --project-directory /opt/loams/release ps
```

The first boot takes a few minutes (Docker, the bundle, the images). The unit retries every 30 seconds until the secrets exist. When the sidecar has joined, the engine is at `https://loams.<your-tailnet>.ts.net` from any device on your tailnet; the console asks for the setup token. Not available yet: the bucket capability probe in the log (`loams store probe`, BY0 Task 2); to check the bucket by hand, write and read a test object with the instance role.

## 8. Backups

The disk holds the metastore, Live and Durable. Versioning protects the bucket; protect the disk with a daily snapshot:

```bash
aws dlm create-lifecycle-policy --region "$REGION" --description "Loams $INSTALL daily" --state ENABLED \
  --execution-role-arn "arn:aws:iam::$ACCOUNT:role/AWSDataLifecycleManagerDefaultRole" \
  --policy-details '{"PolicyType":"EBS_SNAPSHOT_MANAGEMENT","ResourceTypes":["VOLUME"],"TargetTags":[{"Key":"loams:backup","Value":"daily"}],"Schedules":[{"Name":"daily","CreateRule":{"Interval":24,"IntervalUnit":"HOURS","Times":["03:00"]},"RetainRule":{"Count":7},"CopyTags":true}]}'
VOL=$(aws ec2 describe-instances --region "$REGION" --instance-ids "$INSTANCE" --query 'Reservations[0].Instances[0].BlockDeviceMappings[0].Ebs.VolumeId' --output text)
aws ec2 create-tags --region "$REGION" --resources "$VOL" --tags Key=loams:backup,Value=daily
```

(`aws iam create-service-linked-role --aws-service-name dlm.amazonaws.com` first if the role does not exist.) To restore: create a volume from a snapshot, launch a new VM from the same cloud-init file with that volume as the root volume, or attach it and mount it at `/var/lib/docker`. A snapshot of a running disk is crash-consistent; a stack stopped for the snapshot (`sudo systemctl stop loams-byoc`) is cleaner.

## 9. Upgrades

Open a shell (step 7) and follow [upgrades](../../release/upgrades.md): `cd /opt/loams/release`, install the target release's `loams` CLI (its Linux package, checked against that release's `SHA256SUMS`), then `loams upgrade check --to <version>` and `loams upgrade --to <version>`. `deploy/release/byoc/upgrade.sh` does this over SSH for an install made by `install.sh`; cloud-init VMs have no SSH, so use the shell. Embedded stacks are upgraded one release at a time and by hand for now.

## 10. Private reach

Reaching the engine from workloads in the same VPC (an internal load balancer) or from other accounts (PrivateLink) is design 69 and a later BY1 milestone; it is not set up here. Use your tailnet.

## 11. Costs

About $73 a month for `t3.large` (see [the overview](index.md#costs-estimates-small-install-one-month-2026-10-21-list-prices)). Stop the instance to stop the compute charge; the disk, bucket and key keep costing.

## 11a. With Confluent Cloud

To add a Confluent Cloud cluster in the same region, or to connect a cluster you already run, see [confluent.md](confluent.md) (`deploy/tofu/examples/aws-vm-confluent/`).

## 12. Remove it

```bash
aws ec2 terminate-instances --region "$REGION" --instance-ids "$INSTANCE"
aws iam remove-role-from-instance-profile --instance-profile-name "loams-$INSTALL" --role-name "loams-$INSTALL"
aws iam delete-instance-profile --instance-profile-name "loams-$INSTALL"
aws iam delete-role-policy --role-name "loams-$INSTALL" --policy-name loams
aws iam detach-role-policy --role-name "loams-$INSTALL" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
aws iam delete-role --role-name "loams-$INSTALL"
aws secretsmanager delete-secret --region "$REGION" --secret-id "loams/$INSTALL/tailnet-authkey" --recovery-window-in-days 7
aws secretsmanager delete-secret --region "$REGION" --secret-id "loams/$INSTALL/setup-token" --recovery-window-in-days 7
```

Your data is in the bucket and the disk snapshots: keep them, or delete the bucket's objects and versions and the bucket, and schedule the key's deletion (`aws kms schedule-key-deletion --key-id "$KEY_ID" --pending-window-in-days 30`). Also remove the machine from your Tailscale admin console.
