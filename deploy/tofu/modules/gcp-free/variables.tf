variable "project_id" {
  description = "The Google Cloud project. Firestore's free quota is for the project's (default) database only, so one project per app is the default (Q1100)."
  type        = string
}

variable "region" {
  description = "A free-tier US region (GCS's 5 GB-month and the e2-micro are free only there): us-central1 (Q1105), us-east1 or us-west1."
  type        = string
  default     = "us-central1"
  validation {
    condition     = contains(["us-central1", "us-east1", "us-west1"], var.region) || var.allow_paid
    error_message = "region is us-central1, us-east1 or us-west1 (the free-tier regions) unless allow_paid."
  }
}

variable "name" {
  description = "The app's name. Resources are named loams-<name>. Cloud Tasks keeps a deleted queue's name for up to 7 days, so a benchmark puts its run id in the name."
  type        = string
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,22}$", var.name))
    error_message = "name starts with a letter and is lowercase letters, digits and - (up to 23, for service-account ids)."
  }
}

variable "run_id" {
  description = "A benchmark run id. When set, every labelled resource gets loams-bench=<run_id> (CB1 plan); null for an ordinary app."
  type        = string
  default     = null
  validation {
    condition     = var.run_id == null || can(regex("^[a-z0-9][a-z0-9-]{0,39}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 40), as Google labels need."
  }
}

variable "labels" {
  description = "Extra labels for every resource that takes labels."
  type        = map(string)
  default     = {}
}

variable "allow_paid" {
  description = "Allow settings that leave the free tier (a region outside the free US regions, more than 4 instances). Design 71 D1400: off by default."
  type        = bool
  default     = false
}

variable "enable_apis" {
  description = "Enable the APIs the module uses. They stay enabled on destroy (enabling costs nothing)."
  type        = bool
  default     = true
}

# --- budget (created first) -----------------------------------------------------
variable "billing_account" {
  description = "The billing account id (XXXXXX-XXXXXX-XXXXXX) for the Cloud Billing budget. The caller needs Billing Account Costs Manager on it."
  type        = string
  validation {
    condition     = can(regex("^[0-9A-F]{6}-[0-9A-F]{6}-[0-9A-F]{6}$", var.billing_account))
    error_message = "billing_account looks like 012345-6789AB-CDEF01."
  }
}

variable "budget_amount" {
  description = "The monthly budget for this project, in budget_currency (design 71 §9.3). It alerts and never stops anything (Q1102)."
  type        = number
  default     = 1
  validation {
    condition     = var.budget_amount > 0 && floor(var.budget_amount) == var.budget_amount
    error_message = "budget_amount is a whole number above 0."
  }
}

variable "budget_currency" {
  description = "The billing account's currency (USD, INR, ...). null uses the account's own currency; a different one is refused by the API."
  type        = string
  default     = null
}

variable "budget_notification_channels" {
  description = "Cloud Monitoring notification channel ids for the alerts. Billing account administrators and users always get them."
  type        = list(string)
  default     = []
}

# --- compute: Cloud Run --------------------------------------------------------------
variable "image" {
  description = "The app's container image (the app compiled with the Cloud Run shim, design 79 D1791), ideally by digest."
  type        = string
}

variable "cpu" {
  description = "vCPU per instance (free: 180,000 vCPU-s a month)."
  type        = string
  default     = "1"
}

variable "memory" {
  description = "Memory per instance (free: 360,000 GiB-s a month)."
  type        = string
  default     = "512Mi"
}

variable "max_instances" {
  description = "The hard cap on instances (design 71 §9.3: default 2)."
  type        = number
  default     = 2
  validation {
    condition     = var.max_instances >= 1 && (var.max_instances <= 4 || var.allow_paid)
    error_message = "max_instances is 1 to 4 unless allow_paid."
  }
}

variable "concurrency" {
  description = "Requests per instance."
  type        = number
  default     = 80
}

variable "timeout_s" {
  description = "Request timeout."
  type        = number
  default     = 30
}

variable "public" {
  description = "Serve without Cloud Run IAM: the app's own surface, authenticated by the app (D1409)."
  type        = bool
  default     = true
}

variable "environment" {
  description = "Extra environment variables for the app (never secrets: design 71 §6.5)."
  type        = map(string)
  default     = {}
}

# --- items: Firestore -------------------------------------------------------------
variable "create_firestore" {
  description = "Create the project's (default) Firestore database (the only one with a free quota). false when it exists already; the module then uses it and never deletes it."
  type        = bool
  default     = true
}

# --- queue: Cloud Tasks -------------------------------------------------------------
variable "create_queue" {
  description = "Create the queue port: a Cloud Tasks queue that pushes to the service's /_loams/queue."
  type        = bool
  default     = true
}

variable "queue_max_dispatches_per_second" {
  description = "The queue's dispatch rate (a cap)."
  type        = number
  default     = 10
}

variable "queue_max_concurrent" {
  description = "Tasks in flight at once (a cap)."
  type        = number
  default     = 2
}

variable "queue_max_retries" {
  description = "Attempts per task (design 71 §6.3 max_retries + 1)."
  type        = number
  default     = 4
}

# --- schedule: Cloud Scheduler --------------------------------------------------------
variable "schedule_cron" {
  description = "A cron for the scheduled handler, in UTC (3 free jobs per billing account). null creates no job."
  type        = string
  default     = null
}

# --- blobs: GCS -----------------------------------------------------------------------
variable "create_bucket" {
  description = "Create the blobs bucket in the region (free: 5 GB-month in the free US regions)."
  type        = bool
  default     = false
}

variable "bucket_force_destroy" {
  description = "Let destroy delete a non-empty bucket (benchmarks)."
  type        = bool
  default     = false
}

# --- Artifact Registry -----------------------------------------------------------------
variable "create_repository" {
  description = "Create a Docker repository for the app's image (free: 0.5 GB). Create it first (tofu apply -target), push, then apply the rest."
  type        = bool
  default     = false
}
