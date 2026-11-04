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
  hostname      = "loams-${var.name}"
  tags          = merge({ "loams:install" = var.name, "loams:managed-by" = "opentofu" }, var.tags, { "loams-deploy" = var.name })

  # EC2 user data is at most 16,384 bytes; it is sent as gzip, base64-encoded.
  user_data_max = 21848

  # The SKUs of deploy/tofu/prices.toml. The instance type is looked up so that
  # every priced SKU appears here in full; a type that is not priced is
  # reported as unpriced by the estimator, never as free.
  ec2_skus = {
    "t3.large"  = "aws/ec2/t3.large"
    "t3.xlarge" = "aws/ec2/t3.xlarge"
  }
  cost_items = concat(
    [
      { sku = lookup(local.ec2_skus, local.instance_type, "aws/ec2/${local.instance_type}"), quantity = 730, unit = "hour", note = "the instance, ${local.instance_type}, 730 hours" },
      { sku = "aws/ebs/gp3", quantity = var.disk_size_gb, unit = "gib-month", note = "the encrypted root volume" },
      { sku = "aws/vpc/public-ipv4", quantity = 730, unit = "hour", note = "the instance's public IPv4 address" },
    ],
    local.create_key ? [{ sku = "aws/kms/key", quantity = 1, unit = "month", note = "the bucket's and secrets' key" }] : [],
    [
      { sku = "aws/secretsmanager/secret", quantity = 4, unit = "month", note = "tailnet key, setup token, k3s CA, Access database" },
    ],
  )
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
  kms_key_id              = local.key_arn
  recovery_window_in_days = 7
  tags                    = local.tags
}

resource "aws_secretsmanager_secret_version" "setup_token" {
  count         = var.create_setup_token ? 1 : 0
  secret_id     = aws_secretsmanager_secret.setup_token[0].id
  secret_string = random_id.setup_token[0].hex
}

# The cluster CAs and the Access database's password (or DSN), made by k3s-home.
# The VM reads them with its role (the InstallSecrets statement below, which
# covers everything under the prefix); they are never in the user data.
resource "aws_secretsmanager_secret" "k3s_ca" {
  name                    = "${local.prefix}/k3s-ca"
  description             = "Loams ${var.name}: the k3s server and client CAs"
  kms_key_id              = local.key_arn
  recovery_window_in_days = 7
  tags                    = local.tags
}

resource "aws_secretsmanager_secret_version" "k3s_ca" {
  secret_id     = aws_secretsmanager_secret.k3s_ca.id
  secret_string = module.home.ca_bundle_json
}

resource "aws_secretsmanager_secret" "access_db" {
  name                    = "${local.prefix}/access-db"
  description             = "Loams ${var.name}: the Access database's password or DSN"
  kms_key_id              = local.key_arn
  recovery_window_in_days = 7
  tags                    = local.tags
}

resource "aws_secretsmanager_secret_version" "access_db" {
  secret_id     = aws_secretsmanager_secret.access_db.id
  secret_string = module.home.access_db_json
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

module "home" {
  source = "../k3s-home"

  cloud           = "aws"
  release         = var.release
  hostname        = local.hostname
  tailnet_domain  = var.tailnet_domain
  deployment      = var.name
  home_spec       = var.home_spec
  operator_values = var.operator_values
  access_db       = var.access_db
  access_db_dsn   = var.access_db_dsn
  chart_digest    = var.chart_digest
  bucket          = local.bucket
  region          = local.region
  secret_prefix   = local.prefix

  target_facts = {
    deployment        = var.name
    target            = "aws-k3s"
    cloud             = "aws"
    region            = local.region
    bucket_url        = "s3://${local.bucket}"
    workload_identity = aws_iam_role.this.arn
  }
}

resource "aws_instance" "this" {
  ami                         = data.aws_ssm_parameter.ubuntu.insecure_value
  instance_type               = local.instance_type
  subnet_id                   = local.subnet_id
  vpc_security_group_ids      = [aws_security_group.this.id]
  iam_instance_profile        = aws_iam_instance_profile.this.name
  associate_public_ip_address = true # outbound only: the group has no inbound rule
  user_data_base64            = module.home.user_data_base64gz

  # IMDSv2 only, hop limit 1: the response to a packet from a pod (one hop
  # further, through the node's network namespace) is dropped, so a pod cannot
  # take the VM's role. k3s itself and the bootstrap run on the host. The
  # bootstrap also drops pod-CIDR traffic to 169.254.169.254.
  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
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

    precondition {
      condition     = length(module.home.user_data_base64gz) <= local.user_data_max
      error_message = "The VM's user data is ${length(module.home.user_data_base64gz)} characters of gzip and base64; EC2 allows ${local.user_data_max} (16,384 bytes before compression). Shrink home_spec or operator_values (move large values into the chart's own defaults)."
    }
    precondition {
      condition     = var.region == null || var.region == local.region
      error_message = "region is ${coalesce(var.region, "null")} but the AWS provider is set to ${local.region}; set the provider's region instead."
    }
  }

  depends_on = [
    aws_iam_role_policy.this,
    aws_secretsmanager_secret_version.setup_token,
    aws_secretsmanager_secret_version.k3s_ca,
    aws_secretsmanager_secret_version.access_db,
    aws_s3_bucket_server_side_encryption_configuration.this,
  ]
}
