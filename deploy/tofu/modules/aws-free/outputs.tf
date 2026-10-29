output "url" {
  description = "The app's Function URL."
  value       = aws_lambda_function_url.this.function_url
}

output "function_name" {
  description = "The Lambda function."
  value       = aws_lambda_function.this.function_name
}

output "items_table" {
  description = "The DynamoDB table behind the items port."
  value       = aws_dynamodb_table.items.name
}

output "queue_url" {
  description = "The SQS queue behind the queue port (null without one)."
  value       = var.create_queue ? aws_sqs_queue.this[0].url : null
}

output "bucket" {
  description = "The blobs bucket (null without one)."
  value       = var.create_bucket ? aws_s3_bucket.blobs[0].bucket : null
}

output "budget_name" {
  description = "The AWS Budgets budget."
  value       = aws_budgets_budget.this.name
}

output "tags" {
  description = "The tags on every resource."
  value       = local.tags
}

output "free_tier" {
  description = "What this configuration uses of the always-free tier, and what is not free (design 71 §7.2)."
  value = {
    free = concat(
      ["Lambda: 1M requests and 400,000 GB-s a month", "SQS: 1M requests a month", "CloudWatch Logs: 5 GB"],
      local.provisioned ? ["DynamoDB: ${var.read_capacity} RCU / ${var.write_capacity} WCU of the account's free 25/25"] : [],
    )
    not_free = concat(
      local.provisioned ? [] : ["DynamoDB on-demand requests"],
      var.create_bucket ? ["S3 on accounts opened since 2025-07-15 (credits only)"] : [],
    )
  }
}
