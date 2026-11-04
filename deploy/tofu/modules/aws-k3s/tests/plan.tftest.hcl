# Offline tests (mock providers): `tofu test` in this directory.
# The aws-k3s module is aws-vm's network, bucket, key and role around the k3s home.

mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_region" {
    defaults = { region = "us-east-1" }
  }
  mock_data "aws_ssm_parameter" {
    defaults = { insecure_value = "ami-0123456789abcdef0" }
  }
  mock_data "aws_vpc" {
    defaults = { id = "vpc-0123456789abcdef0" }
  }
  mock_data "aws_subnets" {
    defaults = { ids = ["subnet-0bbbbbbbbbbbbbbbb", "subnet-0aaaaaaaaaaaaaaaa"] }
  }
  mock_data "aws_subnet" {
    defaults = { vpc_id = "vpc-0123456789abcdef0" }
  }
  mock_data "aws_secretsmanager_secret" {
    defaults = { arn = "arn:aws:secretsmanager:us-east-1:123456789012:secret:loams/t/tailnet-authkey-AbCdEf" }
  }
  mock_data "aws_iam_policy_document" {
    defaults = { json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}" }
  }
}
mock_provider "random" {}
mock_provider "tls" {
  mock_resource "tls_private_key" {
    defaults = {
      private_key_pem = "-----BEGIN EC PRIVATE KEY-----\nTESTKEY\n-----END EC PRIVATE KEY-----\n"
      public_key_pem  = "-----BEGIN PUBLIC KEY-----\nTESTPUB\n-----END PUBLIC KEY-----\n"
    }
  }
  mock_resource "tls_self_signed_cert" {
    defaults = {
      cert_pem = "-----BEGIN CERTIFICATE-----\nTESTCA\n-----END CERTIFICATE-----\n"
    }
  }
  mock_resource "tls_cert_request" {
    defaults = {
      cert_request_pem = "-----BEGIN CERTIFICATE REQUEST-----\nTESTCSR\n-----END CERTIFICATE REQUEST-----\n"
    }
  }
  mock_resource "tls_locally_signed_cert" {
    defaults = {
      cert_pem = "-----BEGIN CERTIFICATE-----\nTESTCLIENT\n-----END CERTIFICATE-----\n"
    }
  }
}

variables {
  name           = "t"
  release        = "0.1.4"
  secret_prefix  = "loams/t"
  tailnet_domain = "tail1234.ts.net"
  home_spec = {
    base = {
      profile = "light"
      access = {
        identity = { database = { secretRef = { name = "loams-access-identity-db", key = "dsn" } } }
        openfga  = { database = { secretRef = { name = "loams-access-openfga-db", key = "dsn" } } }
      }
    }
  }
  operator_values = { replicas = 1 }
}

run "defaults_never_public" {
  command = plan

  assert {
    condition     = length(aws_security_group.this.ingress) == 0
    error_message = "the security group must have no inbound rule"
  }
  assert {
    condition     = aws_instance.this.metadata_options[0].http_tokens == "required" && aws_instance.this.metadata_options[0].http_put_response_hop_limit == 1
    error_message = "IMDSv2 only, hop limit 1: a pod must not reach the VM's role through the metadata service"
  }
  assert {
    condition     = aws_instance.this.root_block_device[0].encrypted
    error_message = "the root volume is encrypted"
  }
  assert {
    condition     = aws_instance.this.instance_type == "t3.large"
    error_message = "small is t3.large"
  }
  assert {
    condition     = aws_instance.this.subnet_id == "subnet-0aaaaaaaaaaaaaaaa"
    error_message = "the first default subnet, sorted, is used"
  }
}

run "bucket_is_private_versioned_and_encrypted" {
  command = plan

  assert {
    condition     = aws_s3_bucket_public_access_block.this[0].block_public_acls && aws_s3_bucket_public_access_block.this[0].block_public_policy && aws_s3_bucket_public_access_block.this[0].ignore_public_acls && aws_s3_bucket_public_access_block.this[0].restrict_public_buckets
    error_message = "all four public access blocks are on"
  }
  assert {
    condition     = aws_s3_bucket_versioning.this[0].versioning_configuration[0].status == "Enabled"
    error_message = "versioning is on"
  }
  assert {
    condition     = contains([for r in aws_s3_bucket_server_side_encryption_configuration.this[0].rule : one(r.apply_server_side_encryption_by_default).sse_algorithm], "aws:kms")
    error_message = "the bucket's default encryption is KMS"
  }
  assert {
    condition     = aws_kms_key.bucket[0].enable_key_rotation
    error_message = "the key rotates"
  }
  assert {
    condition     = output.bucket_url == "s3://loams-t-123456789012/loams"
    error_message = "the bucket URL names the account-scoped bucket and the loams prefix"
  }
}

run "user_data_is_k3s_not_compose" {
  command = plan

  assert {
    condition     = can(regex("/usr/local/bin/loams-k3s-bootstrap", module.home.user_data))
    error_message = "the user data installs the k3s bootstrap"
  }
  assert {
    condition     = !can(regex("docker-ce", module.home.user_data))
    error_message = "the k3s home does not install Docker"
  }
  assert {
    condition     = can(regex("LOAMS_BUCKET_URL=s3://loams-t-123456789012/loams", module.home.user_data)) && can(regex("REGION=us-east-1", module.home.user_data)) && can(regex("SECRET_PREFIX=loams/t", module.home.user_data))
    error_message = "the rendered config names the bucket, region and secret prefix"
  }
  assert {
    condition     = !can(regex("AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY", module.home.user_data))
    error_message = "no AWS key in the user data"
  }
}

run "k3s_ca_and_access_db_secrets_created" {
  command = plan

  assert {
    condition     = aws_secretsmanager_secret.k3s_ca.name == "loams/t/k3s-ca" && aws_secretsmanager_secret.access_db.name == "loams/t/access-db"
    error_message = "the bootstrap reads <prefix>/k3s-ca and <prefix>/access-db"
  }
  assert {
    condition     = aws_secretsmanager_secret_version.k3s_ca.secret_string == module.home.ca_bundle_json && aws_secretsmanager_secret_version.access_db.secret_string == module.home.access_db_json
    error_message = "the secrets hold k3s-home's CA bundle and Access database JSON"
  }
  assert {
    condition     = aws_secretsmanager_secret.k3s_ca.recovery_window_in_days == 7 && aws_secretsmanager_secret.access_db.recovery_window_in_days == 7
    error_message = "a deleted secret can be recovered for 7 days"
  }
}

run "target_facts_reach_the_user_data" {
  command = plan

  assert {
    condition     = can(regex("TARGET_FACTS_B64=[A-Za-z0-9+/=]+", module.home.user_data))
    error_message = "the loams-target facts are in the config"
  }
  assert {
    condition     = can(regex("TAILNET_DOMAIN=tail1234.ts.net\n", module.home.user_data))
    error_message = "the tailnet domain is in the config"
  }
}

run "chart_digest_is_passed_through" {
  command = plan

  variables {
    chart_digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }

  assert {
    condition     = can(regex("CHART_DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n", module.home.user_data))
    error_message = "the chart digest pin reaches the user data"
  }
}

run "rejects_a_malformed_chart_digest" {
  command = plan

  variables {
    chart_digest = "latest"
  }

  expect_failures = [var.chart_digest]
}

run "outputs_follow_the_contract" {
  command = plan

  assert {
    condition     = output.cloud == "aws" && output.region == "us-east-1" && output.secret_prefix == "loams/t"
    error_message = "cloud, region and secret_prefix"
  }
  assert {
    condition     = output.api_url == "https://loams-t.tail1234.ts.net:6443"
    error_message = "the API is on the tailnet under loams-<name>"
  }
}

run "cost_items_small" {
  command = plan

  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/ec2/t3.large|730|hour")
    error_message = "t3.large for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/ebs/gp3|60|gib-month")
    error_message = "60 GiB of gp3"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/vpc/public-ipv4|730|hour")
    error_message = "one public IPv4 for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/kms/key|1|month")
    error_message = "the module's own key"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/secretsmanager/secret|4|month")
    error_message = "four secrets: tailnet key, setup token, k3s CA, Access database"
  }
  assert {
    condition     = length(output.cost_items) == 5
    error_message = "nothing else is listed"
  }
}

run "cost_items_medium_existing_bucket_and_key" {
  command = plan

  variables {
    size         = "medium"
    disk_size_gb = 100
    bucket       = "my-existing-bucket"
    kms_key_arn  = "arn:aws:kms:us-east-1:123456789012:key/11111111-2222-3333-4444-555555555555"
  }

  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/ec2/t3.xlarge|730|hour")
    error_message = "t3.xlarge for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "aws/ebs/gp3|100|gib-month")
    error_message = "100 GiB of gp3"
  }
  assert {
    condition     = length([for i in output.cost_items : i if i.sku == "aws/kms/key"]) == 0
    error_message = "a key the module did not create is not billed to it"
  }
}

run "every_resource_tagged" {
  command = plan

  variables {
    tags = { "loams-deploy" = "someone-else", team = "x" }
  }

  assert {
    condition     = aws_instance.this.tags["loams-deploy"] == "t" && aws_s3_bucket.this[0].tags["loams-deploy"] == "t" && aws_kms_key.bucket[0].tags["loams-deploy"] == "t"
    error_message = "the instance, bucket and key carry loams-deploy=<name>, whatever the user's tags say"
  }
  assert {
    condition     = aws_iam_role.this.tags["loams-deploy"] == "t" && aws_iam_instance_profile.this.tags["loams-deploy"] == "t" && aws_security_group.this.tags["loams-deploy"] == "t"
    error_message = "the role, instance profile and security group are tagged"
  }
  assert {
    condition     = aws_secretsmanager_secret.k3s_ca.tags["loams-deploy"] == "t" && aws_secretsmanager_secret.access_db.tags["loams-deploy"] == "t" && aws_secretsmanager_secret.setup_token[0].tags["loams-deploy"] == "t"
    error_message = "every secret is tagged"
  }
  assert {
    condition     = aws_instance.this.root_block_device[0].tags["loams-deploy"] == "t" && aws_instance.this.tags["team"] == "x"
    error_message = "the root volume is tagged and the user's tags are kept"
  }
}

run "existing_bucket_creates_none" {
  command = plan

  variables {
    bucket = "my-existing-bucket"
  }

  assert {
    condition     = length(aws_s3_bucket.this) == 0 && length(aws_kms_key.bucket) == 0
    error_message = "an existing bucket means no bucket and no key are created"
  }
  assert {
    condition     = output.bucket_url == "s3://my-existing-bucket/loams"
    error_message = "the URL names the existing bucket"
  }
}

run "medium_size" {
  command = plan

  variables {
    size = "medium"
  }

  assert {
    condition     = aws_instance.this.instance_type == "t3.xlarge"
    error_message = "medium is t3.xlarge"
  }
}

run "rejects_a_latest_release" {
  command = plan

  variables {
    release = "latest"
  }

  expect_failures = [var.release]
}

run "rejects_a_region_that_is_not_the_providers" {
  command = plan

  variables {
    region = "eu-west-1"
  }

  expect_failures = [aws_instance.this]
}

run "huge_operator_values_do_not_fit_user_data" {
  command = plan

  variables {
    # Hex digests do not compress: about 64 KB of them is far over EC2's 16 KB.
    operator_values = { blob = [for i in range(1000) : sha256("x${i}")] }
  }

  expect_failures = [aws_instance.this]
}
