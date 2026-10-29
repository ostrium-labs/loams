# Offline tests (mock providers): `tofu test` in this directory.

mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_region" {
    defaults = { region = "us-east-1" }
  }
  mock_data "aws_iam_policy_document" {
    defaults = { json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}" }
  }
  mock_resource "aws_cloudwatch_log_group" {
    defaults = { arn = "arn:aws:logs:us-east-1:123456789012:log-group:/aws/lambda/loams-app-t" }
  }
  mock_resource "aws_dynamodb_table" {
    defaults = { arn = "arn:aws:dynamodb:us-east-1:123456789012:table/loams-app-t-items" }
  }
  mock_resource "aws_sqs_queue" {
    defaults = { arn = "arn:aws:sqs:us-east-1:123456789012:loams-app-t-queue" }
  }
  mock_resource "aws_iam_role" {
    defaults = { arn = "arn:aws:iam::123456789012:role/loams-app-t" }
  }
  mock_resource "aws_lambda_function" {
    defaults = { arn = "arn:aws:lambda:us-east-1:123456789012:function:loams-app-t" }
  }
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::loams-app-t-123456789012" }
  }
}

variables {
  name          = "t"
  run_id        = "cb1-r1"
  package_path  = "tests/package.zip"
  budget_emails = ["ops@example.com"]
}

run "free_by_default_with_caps" {
  command = plan

  assert {
    condition     = aws_dynamodb_table.items.billing_mode == "PROVISIONED" && aws_dynamodb_table.items.read_capacity == 25 && aws_dynamodb_table.items.write_capacity == 25
    error_message = "DynamoDB is provisioned at the free 25 RCU / 25 WCU"
  }
  assert {
    condition     = aws_dynamodb_table.items.table_class == "STANDARD" && !aws_dynamodb_table.items.point_in_time_recovery[0].enabled
    error_message = "Standard table class (the free tier's), no PITR backups"
  }
  assert {
    condition     = aws_lambda_function.this.reserved_concurrent_executions == 5
    error_message = "reserved concurrency caps the function at 5 by default"
  }
  assert {
    condition     = aws_lambda_function.this.architectures[0] == "arm64" && aws_lambda_function.this.memory_size == 256
    error_message = "arm64 (Q1106), 256 MB"
  }
  assert {
    condition     = aws_lambda_event_source_mapping.queue[0].scaling_config[0].maximum_concurrency == 2
    error_message = "the queue consumer is capped at 2 concurrent invocations"
  }
  assert {
    condition     = aws_cloudwatch_log_group.this.retention_in_days == 1
    error_message = "logs are kept 1 day"
  }
  assert {
    condition     = length(aws_s3_bucket.blobs) == 0
    error_message = "S3 is not free on new accounts, so no bucket by default"
  }
  assert {
    condition     = length(aws_scheduler_schedule.this) == 0
    error_message = "no schedule unless one is asked for"
  }
}

run "budget_alerts_at_50_80_100_and_forecast" {
  command = plan

  assert {
    condition     = aws_budgets_budget.this.limit_amount == "1.00" && aws_budgets_budget.this.time_unit == "MONTHLY"
    error_message = "a $1 monthly budget by default (design 71 §9.3)"
  }
  assert {
    condition     = length(aws_budgets_budget.this.notification) == 4
    error_message = "four alerts: 50, 80, 100 actual and 100 forecast"
  }
  assert {
    condition     = length([for n in aws_budgets_budget.this.notification : n if n.notification_type == "FORECASTED"]) == 1
    error_message = "one forecast alert"
  }
}

run "every_resource_is_tagged" {
  command = plan
  variables {
    create_bucket       = true
    schedule_expression = "cron(0 7 * * ? *)"
  }

  assert {
    condition = alltrue([for t in [
      aws_budgets_budget.this.tags, aws_dynamodb_table.items.tags, aws_sqs_queue.this[0].tags,
      aws_sqs_queue.dead_letter[0].tags, aws_s3_bucket.blobs[0].tags, aws_iam_role.this.tags,
      aws_cloudwatch_log_group.this.tags, aws_lambda_function.this.tags,
      aws_lambda_event_source_mapping.queue[0].tags, aws_scheduler_schedule_group.this[0].tags,
      aws_iam_role.scheduler[0].tags,
    ] : t["loams-bench"] == "cb1-r1" && t["loams-app"] == "t"])
    error_message = "every taggable resource carries loams-bench and loams-app"
  }
  assert {
    condition     = aws_s3_bucket_public_access_block.blobs[0].block_public_acls && aws_s3_bucket_public_access_block.blobs[0].block_public_policy && aws_s3_bucket_public_access_block.blobs[0].restrict_public_buckets
    error_message = "the blobs bucket is private"
  }
}

run "public_url_has_both_permissions" {
  command = plan

  assert {
    condition     = aws_lambda_function_url.this.authorization_type == "NONE"
    error_message = "the app's URL is public by default (D1409)"
  }
  assert {
    condition     = aws_lambda_permission.url[0].action == "lambda:InvokeFunctionUrl" && aws_lambda_permission.url_invoke[0].invoked_via_function_url
    error_message = "both URL permissions exist"
  }
}

run "unreserved_when_asked" {
  command = plan
  variables {
    reserved_concurrency = null
  }

  assert {
    condition     = aws_lambda_function.this.reserved_concurrent_executions == -1
    error_message = "null leaves the function unreserved (accounts limited to 10)"
  }
}

run "on_demand_refused_without_allow_paid" {
  command = plan
  variables {
    items_capacity_mode = "on_demand"
  }
  expect_failures = [var.items_capacity_mode]
}

run "over_free_capacity_refused" {
  command = plan
  variables {
    read_capacity = 26
  }
  expect_failures = [var.read_capacity]
}

run "on_demand_with_allow_paid" {
  command = plan
  variables {
    items_capacity_mode = "on_demand"
    allow_paid          = true
  }

  assert {
    condition     = aws_dynamodb_table.items.billing_mode == "PAY_PER_REQUEST"
    error_message = "on-demand when allowed"
  }
}

run "budget_needs_an_email" {
  command = plan
  variables {
    budget_emails = []
  }
  expect_failures = [var.budget_emails]
}
