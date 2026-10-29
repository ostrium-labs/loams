# Loams on one EC2 instance (your AWS account)

Status: **new, tested without a cloud account** (cloud-init schema, the secrets fetcher against a local mock of IMDS and Secrets Manager, policy checks). A real-account run is still to be done; see "Status".

Creates, in your account: one EC2 instance (Ubuntu 24.04) with an instance profile, and a security group with **no inbound rules**. The bucket, its KMS key and the three secrets are yours and are created by you first. Loams 0.1.3's light default stack runs on the instance: the engine on its embedded stores with the bucket, House, Loams Postgres on local folders, agentd and the Tailscale sidecar. It is single node: no replication, so back up the volume (snapshots) and keep bucket versioning on.

The engine is reached over your tailnet only. The instance has no key: the engine reads the instance role through IMDSv2, and `loams-byoc-secrets` reads the secrets with the same role.

## Permissions the instance profile has

`instance-policy.json` (replace `BUCKET`, `REGION`, `ACCOUNT`, `KEY_ID`, `SECRET_PREFIX`) and nothing else; the managed policy `AmazonSSMManagedInstanceCore` is optional and gives you a shell without opening a port.

- S3: `GetObject`, `PutObject`, `DeleteObject`, `AbortMultipartUpload`, `ListMultipartUploadParts` on `BUCKET/loams/*`; `ListBucket` on `BUCKET` for the prefix `loams`.
- KMS: `GenerateDataKey`, `Decrypt` on the one key.
- Secrets Manager: `GetSecretValue` on `SECRET_PREFIX/*`.

## Use it

```bash
# 1. Secrets (values from your own tailnet admin console; never commit them).
aws secretsmanager create-secret --name loams/prod/tailnet-authkey --secret-string "$TS_AUTHKEY"
aws secretsmanager create-secret --name loams/prod/setup-token --secret-string "$(openssl rand -hex 32)"

# 2. The role and instance profile.
sed -e "s/BUCKET/$BUCKET/g" -e "s/REGION/$REGION/g" -e "s/ACCOUNT/$ACCOUNT/g" \
    -e "s/KEY_ID/$KEY_ID/g" -e "s#SECRET_PREFIX#loams/prod#g" deploy/byoc/aws/instance-policy.json > policy.json
aws iam create-role --role-name loams --assume-role-policy-document file://deploy/byoc/aws/instance-trust-policy.json
aws iam put-role-policy --role-name loams --policy-name loams --policy-document file://policy.json
aws iam create-instance-profile --instance-profile-name loams
aws iam add-role-to-instance-profile --instance-profile-name loams --role-name loams

# 3. A security group with no inbound rules (a new group has none).
SG=$(aws ec2 create-security-group --group-name loams --description "Loams: no inbound" --query GroupId --output text)

# 4. The user-data (gzip: the plain file is over EC2's 16 KB limit).
deploy/byoc/render.sh aws --bucket "$BUCKET" --region "$REGION" --install prod --gzip > user-data.yaml.gz

# 5. The instance: IMDSv2 only, hop limit 2 so the containers can reach it.
AMI=$(aws ssm get-parameter --name /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id --query Parameter.Value --output text)
aws ec2 run-instances --image-id "$AMI" --instance-type t3.large \
  --iam-instance-profile Name=loams --security-group-ids "$SG" --associate-public-ip-address \
  --metadata-options HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled \
  --block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=60,VolumeType=gp3,Encrypted=true}' \
  --user-data fileb://user-data.yaml.gz --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=loams}]'
```

The public IP is only for outbound traffic (the tailnet, GitHub, Docker's registry); with no inbound rule nothing can connect to it. Watch the first boot with `aws ssm start-session` and `journalctl -u cloud-final -u loams-byoc`.

## Limits and status

- One node, one disk. Not highly available.
- `HttpPutResponseHopLimit=2` is required: with 1, the engine in its container cannot reach IMDSv2 and the bucket fails with a timeout.
- Not yet run against a real AWS account in CI (the nightly row of BY0 Task 9 needs the owner's role). Until then treat first-boot timing and IAM edge cases as unverified.

See [variables.md](variables.md), [CHANGELOG.md](CHANGELOG.md), and design 70 (`docs/design/70-byoc-clouds.md`).
