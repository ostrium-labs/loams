data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

locals {
  region        = data.aws_region.current.region
  account       = data.aws_caller_identity.current.account_id
  create_bucket = var.bucket == null
  bucket        = local.create_bucket ? "loams-${var.name}-${local.account}" : var.bucket
  create_key    = var.kms_key_arn == null && local.create_bucket
  key_arn       = local.create_key ? aws_kms_key.bucket[0].arn : var.kms_key_arn
  prefix        = coalesce(var.secret_prefix, "loams/${var.name}")
  instance_type = coalesce(var.instance_type, var.size == "small" ? "t3.large" : "t3.xlarge")
  tags          = merge({ "loams:install" = var.name, "loams:managed-by" = "opentofu" }, var.tags)

  # Estimates, us-east-1 on-demand prices of 2026-10-21 (design 70 §6.1): the
  # instance for 730 hours, the root volume, 50 GiB of S3, a key, an IPv4.
  hourly = {
    "t3.large"  = 0.0832
    "t3.xlarge" = 0.1664
  }
  estimated_monthly_usd = floor(lookup(local.hourly, local.instance_type, 0.1) * 730 + var.disk_size_gb * 0.08 + 1.15 + (local.create_key ? 1 : 0) + 3.65)
}

# --- the bucket, private and versioned, encrypted with the key ----------------
resource "aws_kms_key" "bucket" {
  count                   = local.create_key ? 1 : 0
  description             = "Loams ${var.name}: the bucket's default encryption"
  enable_key_rotation     = true
  deletion_window_in_days = 30
  tags                    = local.tags
}

resource "aws_kms_alias" "bucket" {
  count         = local.create_key ? 1 : 0
  name          = "alias/loams-${var.name}"
  target_key_id = aws_kms_key.bucket[0].key_id
}

resource "aws_s3_bucket" "this" {
  count  = local.create_bucket ? 1 : 0
  bucket = local.bucket
  tags   = local.tags
}

resource "aws_s3_bucket_ownership_controls" "this" {
  count  = local.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_public_access_block" "this" {
  count                   = local.create_bucket ? 1 : 0
  bucket                  = aws_s3_bucket.this[0].id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "this" {
  count  = local.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "this" {
  count  = local.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id
  rule {
    bucket_key_enabled = true
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = local.key_arn
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "this" {
  count  = local.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id
  rule {
    id     = "noncurrent-and-multipart"
    status = "Enabled"
    filter {}
    noncurrent_version_expiration {
      noncurrent_days = 30
    }
    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
  depends_on = [aws_s3_bucket_versioning.this]
}

data "aws_iam_policy_document" "bucket_tls" {
  count = local.create_bucket ? 1 : 0
  statement {
    sid     = "DenyInsecureTransport"
    effect  = "Deny"
    actions = ["s3:*"]
    resources = [
      aws_s3_bucket.this[0].arn,
      "${aws_s3_bucket.this[0].arn}/*",
    ]
    principals {
      type        = "*"
      identifiers = ["*"]
    }
    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "this" {
  count      = local.create_bucket ? 1 : 0
  bucket     = aws_s3_bucket.this[0].id
  policy     = data.aws_iam_policy_document.bucket_tls[0].json
  depends_on = [aws_s3_bucket_public_access_block.this]
}

# --- the secrets --------------------------------------------------------------
# The Tailscale auth key is put here by you, outside this configuration.
data "aws_secretsmanager_secret" "tailnet_authkey" {
  name = "${local.prefix}/tailnet-authkey"
}

resource "random_id" "setup_token" {
  count       = var.create_setup_token ? 1 : 0
  byte_length = 32
}

resource "aws_secretsmanager_secret" "setup_token" {
  count                   = var.create_setup_token ? 1 : 0
  name                    = "${local.prefix}/setup-token"
  description             = "Loams ${var.name}: the first-run setup token"
  recovery_window_in_days = 7
  tags                    = local.tags
}

resource "aws_secretsmanager_secret_version" "setup_token" {
  count         = var.create_setup_token ? 1 : 0
  secret_id     = aws_secretsmanager_secret.setup_token[0].id
  secret_string = random_id.setup_token[0].hex
}

# --- the instance's identity (deploy/byoc/aws/instance-policy.json) -------------
data "aws_iam_policy_document" "instance" {
  statement {
    sid = "BucketObjects"
    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
      "s3:AbortMultipartUpload",
      "s3:ListMultipartUploadParts",
    ]
    resources = ["arn:aws:s3:::${local.bucket}/loams/*"]
  }
  statement {
    sid       = "BucketList"
    actions   = ["s3:ListBucket"]
    resources = ["arn:aws:s3:::${local.bucket}"]
    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["loams", "loams/*"]
    }
  }
  dynamic "statement" {
    for_each = local.key_arn == null ? [] : [local.key_arn]
    content {
      sid       = "BucketKey"
      actions   = ["kms:GenerateDataKey", "kms:Decrypt"]
      resources = [statement.value]
    }
  }
  statement {
    sid       = "InstallSecrets"
    actions   = ["secretsmanager:GetSecretValue"]
    resources = ["arn:aws:secretsmanager:${local.region}:${local.account}:secret:${local.prefix}/*"]
  }
}

data "aws_iam_policy_document" "trust" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ec2.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "this" {
  name               = "loams-${var.name}"
  assume_role_policy = data.aws_iam_policy_document.trust.json
  tags               = local.tags
}

resource "aws_iam_role_policy" "this" {
  name   = "loams"
  role   = aws_iam_role.this.id
  policy = data.aws_iam_policy_document.instance.json
}

resource "aws_iam_role_policy_attachment" "ssm" {
  count      = var.ssm ? 1 : 0
  role       = aws_iam_role.this.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_instance_profile" "this" {
  name = "loams-${var.name}"
  role = aws_iam_role.this.name
  tags = local.tags
}

# --- the network: a security group with no inbound rule -------------------------
data "aws_vpc" "default" {
  count   = var.subnet_id == null ? 1 : 0
  default = true
}

data "aws_subnets" "default" {
  count = var.subnet_id == null ? 1 : 0
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default[0].id]
  }
  filter {
    name   = "default-for-az"
    values = ["true"]
  }
}

locals {
  subnet_id = var.subnet_id != null ? var.subnet_id : sort(data.aws_subnets.default[0].ids)[0]
}

data "aws_subnet" "this" {
  id = local.subnet_id
}

resource "aws_security_group" "this" {
  name        = "loams-${var.name}"
  description = "Loams ${var.name}: no inbound rule; the engine is reached over the tailnet"
  vpc_id      = data.aws_subnet.this.vpc_id
  tags        = local.tags

  # No ingress block: nothing can connect in. Outbound is open for the tailnet,
  # GitHub, Docker's registry and the AWS APIs.
  egress {
    description = "all outbound"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

# --- the instance ------------------------------------------------------------------
data "aws_ssm_parameter" "ubuntu" {
  name = "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
}

module "cloud_init" {
  source                 = "../shared"
  cloud                  = "aws"
  release                = var.release
  bucket                 = local.bucket
  region                 = local.region
  secret_prefix          = local.prefix
  profiles               = var.profiles
  hostname               = var.hostname
  confluent_integrations = var.confluent_integrations
}

resource "aws_instance" "this" {
  ami                         = data.aws_ssm_parameter.ubuntu.insecure_value
  instance_type               = local.instance_type
  subnet_id                   = local.subnet_id
  vpc_security_group_ids      = [aws_security_group.this.id]
  iam_instance_profile        = aws_iam_instance_profile.this.name
  associate_public_ip_address = true # outbound only: the group has no inbound rule
  user_data_base64            = module.cloud_init.user_data_base64gz

  # IMDSv2 only. Hop limit 2: the engine runs in a container, one hop further.
  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 2
  }

  root_block_device {
    volume_type = "gp3"
    volume_size = var.disk_size_gb
    encrypted   = true
    tags        = local.tags
  }

  tags = merge(local.tags, { Name = "loams-${var.name}" })

  # A new AMI release must not replace a running install: upgrade with `loams upgrade`.
  lifecycle {
    ignore_changes = [ami, user_data_base64]
  }

  depends_on = [
    aws_iam_role_policy.this,
    aws_secretsmanager_secret_version.setup_token,
    aws_s3_bucket_server_side_encryption_configuration.this,
  ]
}
