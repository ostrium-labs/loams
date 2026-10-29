# App BYOC on AWS's always-free tier (design 71 §7.2, D1395, D1400, D1402):
# Lambda with a Function URL, DynamoDB provisioned at the free capacity, SQS,
# optionally EventBridge Scheduler and S3. The budget is created first; every
# other resource depends on it. No VPC, no NAT, no public IPv4.

data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

locals {
  region   = data.aws_region.current.region
  account  = data.aws_caller_identity.current.account_id
  fullname = "loams-app-${var.name}"
  tags = merge(
    var.tags,
    { "loams-app" = var.name, "loams-managed-by" = "opentofu" },
    var.run_id == null ? {} : { "loams-bench" = var.run_id },
  )
  provisioned = var.items_capacity_mode == "provisioned"
  bucket      = "${local.fullname}-${local.account}"
}

# --- the budget, first (D1402) -------------------------------------------------
# Account-wide: a tag-filtered budget sees nothing until the tag is activated as a
# cost allocation tag, which AWS allows only after the tag has appeared in billing
# data (up to 24 hours). It alerts; it never stops anything (Q1102).
resource "aws_budgets_budget" "this" {
  name         = "${local.fullname}-${local.region}"
  budget_type  = "COST"
  limit_amount = format("%.2f", var.budget_monthly_usd)
  limit_unit   = "USD"
  time_unit    = "MONTHLY"
  tags         = local.tags

  dynamic "notification" {
    for_each = [50, 80, 100]
    content {
      comparison_operator        = "GREATER_THAN"
      threshold                  = notification.value
      threshold_type             = "PERCENTAGE"
      notification_type          = "ACTUAL"
      subscriber_email_addresses = var.budget_emails
    }
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    notification_type          = "FORECASTED"
    subscriber_email_addresses = var.budget_emails
  }
}

# --- items: DynamoDB, provisioned, autoscaling never set -----------------------
resource "aws_dynamodb_table" "items" {
  name           = "${local.fullname}-items"
  billing_mode   = local.provisioned ? "PROVISIONED" : "PAY_PER_REQUEST"
  read_capacity  = local.provisioned ? var.read_capacity : null
  write_capacity = local.provisioned ? var.write_capacity : null
  hash_key       = "pk"
  range_key      = "sk"
  table_class    = "STANDARD" # the free 25 GB and 25/25 units are Standard only

  attribute {
    name = "pk"
    type = "S"
  }
  attribute {
    name = "sk"
    type = "S"
  }

  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  # No backups (PITR costs per GB-month), AWS-owned encryption key (free).
  point_in_time_recovery {
    enabled = false
  }
  deletion_protection_enabled = false

  tags       = local.tags
  depends_on = [aws_budgets_budget.this]
}

# --- queue: SQS and its dead-letter queue --------------------------------------
resource "aws_sqs_queue" "dead_letter" {
  count                     = var.create_queue ? 1 : 0
  name                      = "${local.fullname}-dlq"
  message_retention_seconds = 345600 # 4 days
  sqs_managed_sse_enabled   = true
  tags                      = local.tags
  depends_on                = [aws_budgets_budget.this]
}

resource "aws_sqs_queue" "this" {
  count                      = var.create_queue ? 1 : 0
  name                       = "${local.fullname}-queue"
  visibility_timeout_seconds = max(30, var.timeout_s * 6) # AWS's advice for Lambda consumers
  message_retention_seconds  = 345600
  max_message_size           = 65536 # the port's messages are at most 48 KiB (D1410)
  sqs_managed_sse_enabled    = true
  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.dead_letter[0].arn
    maxReceiveCount     = var.queue_max_retries
  })
  tags = local.tags
}

# --- blobs: S3, optional --------------------------------------------------------
resource "aws_s3_bucket" "blobs" {
  count         = var.create_bucket ? 1 : 0
  bucket        = local.bucket
  force_destroy = var.bucket_force_destroy
  tags          = local.tags
  depends_on    = [aws_budgets_budget.this]
}

resource "aws_s3_bucket_public_access_block" "blobs" {
  count                   = var.create_bucket ? 1 : 0
  bucket                  = aws_s3_bucket.blobs[0].id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "blobs" {
  count  = var.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.blobs[0].id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "blobs" {
  count  = var.create_bucket ? 1 : 0
  bucket = aws_s3_bucket.blobs[0].id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256" # SSE-S3: no KMS key to pay for
    }
  }
}

# --- the function's role ----------------------------------------------------------
data "aws_iam_policy_document" "assume_lambda" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "this" {
  name               = local.fullname
  assume_role_policy = data.aws_iam_policy_document.assume_lambda.json
  tags               = local.tags
  depends_on         = [aws_budgets_budget.this]
}

data "aws_iam_policy_document" "app" {
  statement {
    sid       = "Logs"
    actions   = ["logs:CreateLogStream", "logs:PutLogEvents"]
    resources = ["${aws_cloudwatch_log_group.this.arn}:*"]
  }
  statement {
    sid = "Items"
    actions = [
      "dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:UpdateItem", "dynamodb:DeleteItem",
      "dynamodb:Query", "dynamodb:ConditionCheckItem", "dynamodb:DescribeTable",
    ]
    resources = [aws_dynamodb_table.items.arn]
  }
  dynamic "statement" {
    for_each = var.create_queue ? [1] : []
    content {
      sid = "Queue"
      actions = [
        "sqs:SendMessage", "sqs:ReceiveMessage", "sqs:DeleteMessage",
        "sqs:ChangeMessageVisibility", "sqs:GetQueueAttributes",
      ]
      resources = [aws_sqs_queue.this[0].arn]
    }
  }
  dynamic "statement" {
    for_each = var.create_bucket ? [1] : []
    content {
      sid       = "Blobs"
      actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
      resources = ["${aws_s3_bucket.blobs[0].arn}/*"]
    }
  }
  dynamic "statement" {
    for_each = var.create_bucket ? [1] : []
    content {
      sid       = "BlobsList"
      actions   = ["s3:ListBucket"]
      resources = [aws_s3_bucket.blobs[0].arn]
    }
  }
}

resource "aws_iam_role_policy" "this" {
  name   = "app"
  role   = aws_iam_role.this.id
  policy = data.aws_iam_policy_document.app.json
}

# --- compute: Lambda with a Function URL ------------------------------------------
resource "aws_cloudwatch_log_group" "this" {
  name              = "/aws/lambda/${local.fullname}"
  retention_in_days = var.log_retention_days
  tags              = local.tags
  depends_on        = [aws_budgets_budget.this]
}

resource "aws_lambda_function" "this" {
  function_name                  = local.fullname
  role                           = aws_iam_role.this.arn
  filename                       = var.package_path
  source_code_hash               = filebase64sha256(var.package_path)
  handler                        = var.handler
  runtime                        = var.runtime
  architectures                  = ["arm64"]
  memory_size                    = var.memory_mb
  timeout                        = var.timeout_s
  reserved_concurrent_executions = var.reserved_concurrency == null ? -1 : var.reserved_concurrency

  environment {
    variables = merge(var.environment, {
      LOAMS_APP_TARGET = "aws"
      LOAMS_ITEMS      = aws_dynamodb_table.items.name
      LOAMS_QUEUE_URL  = var.create_queue ? aws_sqs_queue.this[0].url : ""
      LOAMS_BUCKET     = var.create_bucket ? aws_s3_bucket.blobs[0].bucket : ""
    })
  }

  logging_config {
    log_format = "Text"
    log_group  = aws_cloudwatch_log_group.this.name
  }

  tags       = local.tags
  depends_on = [aws_iam_role_policy.this]
}

resource "aws_lambda_function_url" "this" {
  function_name      = aws_lambda_function.this.function_name
  authorization_type = var.public_url ? "NONE" : "AWS_IAM"
  invoke_mode        = "BUFFERED"
}

# A public Function URL needs both permissions since October 2025: invoking the
# URL and invoking the function through the URL.
resource "aws_lambda_permission" "url" {
  count                  = var.public_url ? 1 : 0
  statement_id           = "FunctionURLPublic"
  action                 = "lambda:InvokeFunctionUrl"
  function_name          = aws_lambda_function.this.function_name
  principal              = "*"
  function_url_auth_type = "NONE"
}

resource "aws_lambda_permission" "url_invoke" {
  count                    = var.public_url ? 1 : 0
  statement_id             = "FunctionURLInvoke"
  action                   = "lambda:InvokeFunction"
  function_name            = aws_lambda_function.this.function_name
  principal                = "*"
  invoked_via_function_url = true
}

resource "aws_lambda_event_source_mapping" "queue" {
  count                   = var.create_queue ? 1 : 0
  event_source_arn        = aws_sqs_queue.this[0].arn
  function_name           = aws_lambda_function.this.arn
  batch_size              = 10
  function_response_types = ["ReportBatchItemFailures"]
  scaling_config {
    maximum_concurrency = var.queue_max_concurrency
  }
  tags = local.tags
}

# --- schedule: EventBridge Scheduler, optional ---------------------------------------
resource "aws_scheduler_schedule_group" "this" {
  count      = var.schedule_expression == null ? 0 : 1
  name       = local.fullname
  tags       = local.tags
  depends_on = [aws_budgets_budget.this]
}

data "aws_iam_policy_document" "assume_scheduler" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["scheduler.amazonaws.com"]
    }
    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [local.account]
    }
  }
}

resource "aws_iam_role" "scheduler" {
  count              = var.schedule_expression == null ? 0 : 1
  name               = "${local.fullname}-scheduler"
  assume_role_policy = data.aws_iam_policy_document.assume_scheduler.json
  tags               = local.tags
}

resource "aws_iam_role_policy" "scheduler" {
  count = var.schedule_expression == null ? 0 : 1
  name  = "invoke"
  role  = aws_iam_role.scheduler[0].id
  policy = jsonencode({
    Version   = "2012-10-17"
    Statement = [{ Effect = "Allow", Action = "lambda:InvokeFunction", Resource = aws_lambda_function.this.arn }]
  })
}

resource "aws_scheduler_schedule" "this" {
  count                        = var.schedule_expression == null ? 0 : 1
  name                         = local.fullname
  group_name                   = aws_scheduler_schedule_group.this[0].name
  schedule_expression          = var.schedule_expression
  schedule_expression_timezone = "UTC"
  flexible_time_window {
    mode = "OFF"
  }
  target {
    arn      = aws_lambda_function.this.arn
    role_arn = aws_iam_role.scheduler[0].arn
    input    = jsonencode({ "loams-scheduled" = { cron = var.schedule_expression } })
  }
}
