variable "name" {
  description = "The app's name. Resources are named loams-app-<name>."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,39}$", var.name))
    error_message = "name is lowercase letters, digits and - (up to 40)."
  }
}

variable "run_id" {
  description = "A benchmark run id. When set, every resource is tagged loams-bench=<run_id> (CB1 plan); null for an ordinary app."
  type        = string
  default     = null
  validation {
    condition     = var.run_id == null || can(regex("^[a-z0-9][a-z0-9-]{0,39}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 40), so the same id is valid as a Google label."
  }
}

variable "tags" {
  description = "Extra tags for every resource."
  type        = map(string)
  default     = {}
}

variable "allow_paid" {
  description = "Allow settings that leave the always-free allowances (on-demand DynamoDB, more than 25 capacity units). Design 71 D1400: off by default."
  type        = bool
  default     = false
}

# --- budget (created first) -----------------------------------------------------
variable "budget_monthly_usd" {
  description = "The AWS Budgets cost budget for the account (design 71 §9.3: default $1). It alerts and never stops anything."
  type        = number
  default     = 1
  validation {
    condition     = var.budget_monthly_usd > 0
    error_message = "budget_monthly_usd is above 0."
  }
}

variable "budget_emails" {
  description = "Who receives the budget's alerts (50%, 80%, 100% actual; 100% forecast)."
  type        = list(string)
  validation {
    condition     = length(var.budget_emails) > 0 && alltrue([for e in var.budget_emails : can(regex("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$", e))])
    error_message = "budget_emails has at least one email address."
  }
}

# --- compute -------------------------------------------------------------------
variable "package_path" {
  description = "The Lambda zip: the app bundled with the Lambda entry shim (design 71 §7.8)."
  type        = string
}

variable "handler" {
  description = "The shim's exported handler."
  type        = string
  default     = "index.handler"
}

variable "runtime" {
  description = "The Lambda runtime (Node for Lambda, design 79 D1791)."
  type        = string
  default     = "nodejs22.x"
  validation {
    condition     = can(regex("^nodejs[0-9]+\\.x$", var.runtime))
    error_message = "runtime is a Node.js runtime such as nodejs22.x."
  }
}

variable "memory_mb" {
  description = "Lambda memory. arm64 (Q1106); the free tier is 400,000 GB-s a month."
  type        = number
  default     = 256
  validation {
    condition     = var.memory_mb >= 128 && var.memory_mb <= 10240
    error_message = "memory_mb is between 128 and 10240."
  }
}

variable "timeout_s" {
  description = "Lambda timeout."
  type        = number
  default     = 30
  validation {
    condition     = var.timeout_s >= 1 && var.timeout_s <= 900
    error_message = "timeout_s is between 1 and 900."
  }
}

variable "reserved_concurrency" {
  description = "Reserved concurrency, the hard cap on parallel invocations (design 71 §9.3: default 5). null leaves the function unreserved: needed while the account's concurrency limit is 10, because AWS keeps 10 unreserved."
  type        = number
  default     = 5
  validation {
    condition     = var.reserved_concurrency == null || try(var.reserved_concurrency >= 1, false)
    error_message = "reserved_concurrency is null or at least 1 (0 would stop the app)."
  }
}

variable "public_url" {
  description = "Create a Function URL with AuthType NONE: the app's own surface, authenticated by the app (D1409). false creates an IAM-authenticated URL."
  type        = bool
  default     = true
}

variable "environment" {
  description = "Extra environment variables for the app (never secrets: design 71 §6.5)."
  type        = map(string)
  default     = {}
}

variable "log_retention_days" {
  description = "CloudWatch log retention. Short by default (free tier: 5 GB)."
  type        = number
  default     = 1
}

# --- items: DynamoDB -------------------------------------------------------------
variable "items_capacity_mode" {
  description = "provisioned (free: 25 RCU and 25 WCU per account and Region, no autoscaling) or on_demand (not free; needs allow_paid)."
  type        = string
  default     = "provisioned"
  validation {
    condition     = contains(["provisioned", "on_demand"], var.items_capacity_mode)
    error_message = "items_capacity_mode is provisioned or on_demand."
  }
  validation {
    condition     = var.items_capacity_mode == "provisioned" || var.allow_paid
    error_message = "on-demand DynamoDB requests are not free (design 71 §7.2); set allow_paid = true to use them."
  }
}

variable "read_capacity" {
  description = "Provisioned read capacity: this app's share of the account's free 25 RCU (the cap; autoscaling is never set)."
  type        = number
  default     = 25
  validation {
    condition     = var.read_capacity >= 1 && (var.read_capacity <= 25 || var.allow_paid)
    error_message = "read_capacity is 1 to 25 (the account's free 25 RCU) unless allow_paid."
  }
}

variable "write_capacity" {
  description = "Provisioned write capacity: this app's share of the account's free 25 WCU."
  type        = number
  default     = 25
  validation {
    condition     = var.write_capacity >= 1 && (var.write_capacity <= 25 || var.allow_paid)
    error_message = "write_capacity is 1 to 25 (the account's free 25 WCU) unless allow_paid."
  }
}

# --- queue: SQS ----------------------------------------------------------------------
variable "create_queue" {
  description = "Create the queue port: an SQS queue, its dead-letter queue and the Lambda event-source mapping."
  type        = bool
  default     = true
}

variable "queue_max_retries" {
  description = "Deliveries before a message goes to the dead-letter queue (design 71 §6.3 max_retries)."
  type        = number
  default     = 3
}

variable "queue_max_concurrency" {
  description = "The event-source mapping's maximum concurrency (2 to 1000): caps how many invocations the queue can take from reserved concurrency."
  type        = number
  default     = 2
  validation {
    condition     = var.queue_max_concurrency >= 2 && var.queue_max_concurrency <= 1000
    error_message = "queue_max_concurrency is 2 to 1000 (an AWS limit)."
  }
}

# --- schedule: EventBridge Scheduler --------------------------------------------------
variable "schedule_expression" {
  description = "A cron for the scheduled handler, in UTC, e.g. cron(0 7 * * ? *). null creates no schedule."
  type        = string
  default     = null
}

# --- blobs: S3 (not free on new accounts) ---------------------------------------------
variable "create_bucket" {
  description = "Create the blobs bucket. S3 is credits-only on accounts opened since 2025-07-15 (design 71 §7.2), so it is off by default."
  type        = bool
  default     = false
}

variable "bucket_force_destroy" {
  description = "Let destroy delete a non-empty bucket (benchmarks)."
  type        = bool
  default     = false
}
