# Offline tests (mock providers): `tofu test` in this directory.

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

variables {
  name          = "t"
  secret_prefix = "loams/t"
}

run "defaults_never_public" {
  command = plan

  assert {
    condition     = length(aws_security_group.this.ingress) == 0
    error_message = "the security group must have no inbound rule"
  }
  assert {
    condition     = aws_instance.this.metadata_options[0].http_tokens == "required" && aws_instance.this.metadata_options[0].http_put_response_hop_limit == 2
    error_message = "IMDSv2 only, with hop limit 2 for the engine's container"
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

run "no_static_key_anywhere" {
  command = plan

  # The user data is what the VM runs; it must not carry a cloud key.
  assert {
    condition     = !can(regex("AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY", module.cloud_init.user_data))
    error_message = "no AWS key in the user data"
  }
  assert {
    condition     = can(regex("LOAMS_BUCKET_URL=s3://loams-t-123456789012/loams", module.cloud_init.user_data)) && can(regex("REGION=us-east-1", module.cloud_init.user_data)) && can(regex("SECRET_PREFIX=loams/t", module.cloud_init.user_data))
    error_message = "the rendered config names the bucket, region and secret prefix"
  }
  assert {
    condition     = can(regex("LOAMS_VERSION=[0-9]+\\.[0-9]+\\.[0-9]+\\n", module.cloud_init.user_data))
    error_message = "the release is pinned"
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

run "confluent_integrations_are_names_only" {
  command = plan
  variables {
    confluent_integrations = ["confluent", "events"]
  }
  assert {
    condition     = can(regex("CONFLUENT_INTEGRATIONS=confluent,events\n", module.cloud_init.user_data))
    error_message = "config.env lists the integrations"
  }
  assert {
    condition     = !can(regex("(?m)^\\s*(CONFLUENT|KAFKA)_[A-Z_]*(KEY|SECRET)[A-Z_]*=", module.cloud_init.user_data))
    error_message = "no Confluent key or secret is in user data"
  }
}
