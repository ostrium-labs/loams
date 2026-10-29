# CB1 root module for the AWS app rows (plan §5.1): the aws-free module (budget
# first) plus a results bucket and the in-region loadgen VM. Applied and
# destroyed by bench/cloud/run.sh; never left running.

provider "aws" {
  region = var.region
  # Every resource, the module's and the loadgen's, carries the run's tag, which
  # sweep.sh lists after destroy.
  default_tags {
    tags = { "loams-bench" = var.run_id }
  }
}

data "aws_caller_identity" "current" {}

locals {
  name      = "b-${var.run_id}"
  memory_mb = 256 # plan §5.1
  account   = data.aws_caller_identity.current.account_id
  prefix    = "${var.run_id}/${var.row}/"
}

module "app" {
  source               = "../../../deploy/tofu/modules/aws-free"
  name                 = local.name
  run_id               = var.run_id
  package_path         = var.package_path
  budget_emails        = var.budget_emails
  items_capacity_mode  = var.items_capacity_mode
  allow_paid           = var.allow_paid
  reserved_concurrency = var.reserved_concurrency
  memory_mb            = local.memory_mb
}

# --- the results bucket ---------------------------------------------------------
# Credit-billed (S3 is not free on new accounts), a few MB for an hour. run.sh
# downloads before destroy; force_destroy lets destroy empty it.
resource "aws_s3_bucket" "results" {
  bucket        = "loams-bench-${var.run_id}-${local.account}"
  force_destroy = true
  depends_on    = [module.app] # the budget first (D1402)
}

resource "aws_s3_bucket_public_access_block" "results" {
  bucket                  = aws_s3_bucket.results.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "results" {
  bucket = aws_s3_bucket.results.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

# A backstop if a destroy is ever missed: objects expire after 7 days.
resource "aws_s3_bucket_lifecycle_configuration" "results" {
  bucket = aws_s3_bucket.results.id
  rule {
    id     = "expire"
    status = "Enabled"
    filter {}
    expiration {
      days = 7
    }
  }
}
