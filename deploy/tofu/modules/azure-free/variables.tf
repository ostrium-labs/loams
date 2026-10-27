variable "name" {
  description = "The app's name. The resource group is rg-loams-<name>; globally named resources get a random suffix."
  type        = string
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,29}$", var.name))
    error_message = "name starts with a letter and is lowercase letters, digits and - (up to 30)."
  }
}

variable "location" {
  description = "The Azure region (Q1105: eastus). It must offer Flex Consumption."
  type        = string
  default     = "eastus"
}

variable "run_id" {
  description = "A benchmark run id. When set, every resource is tagged loams-bench=<run_id> (CB1 plan); null for an ordinary app."
  type        = string
  default     = null
  validation {
    condition     = var.run_id == null || can(regex("^[a-z0-9][a-z0-9-]{0,39}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 40)."
  }
}

variable "tags" {
  description = "Extra tags for every resource."
  type        = map(string)
  default     = {}
}

variable "allow_paid" {
  description = "Allow settings that leave the free allowances (Cosmos DB above 1,000 RU/s or without the free tier, more than 4 instances). Design 71 D1400: off by default."
  type        = bool
  default     = false
}

# --- budget (created first) -----------------------------------------------------
variable "budget_amount" {
  description = "The resource group's monthly Cost Management budget, in the billing currency (design 71 §9.3). It alerts and never stops anything (Q1102)."
  type        = number
  default     = 1
  validation {
    condition     = var.budget_amount > 0
    error_message = "budget_amount is above 0."
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

# --- compute: Functions Flex Consumption ----------------------------------------------
variable "package_path" {
  description = "The app's zip (the app bundled with the Azure Functions shim). null deploys nothing; deploy later with az functionapp deployment source config-zip."
  type        = string
  default     = null
}

variable "runtime_version" {
  description = "The Node.js version (Node for Azure Functions, design 79 D1791)."
  type        = string
  default     = "22"
  validation {
    condition     = contains(["22", "24"], var.runtime_version)
    error_message = "runtime_version is 22 or 24 (Flex Consumption's Node versions)."
  }
}

variable "instance_memory_mb" {
  description = "Instance memory: 512, 2048 or 4096. Flex bills GB-s per active instance with a 1 s minimum per execution, so 512 keeps the free 100,000 GB-s furthest away."
  type        = number
  default     = 512
  validation {
    condition     = contains([512, 2048, 4096], var.instance_memory_mb)
    error_message = "instance_memory_mb is 512, 2048 or 4096."
  }
}

variable "max_instances" {
  description = "The hard cap on on-demand instances (design 71 §9.3: default 2; Flex allows 1 to 1000)."
  type        = number
  default     = 2
  validation {
    condition     = var.max_instances >= 1 && (var.max_instances <= 4 || var.allow_paid)
    error_message = "max_instances is 1 to 4 unless allow_paid."
  }
}

variable "http_concurrency" {
  description = "HTTP executions per instance. null keeps Flex's default for the memory size."
  type        = number
  default     = null
}

variable "environment" {
  description = "Extra app settings (never secrets: design 71 §6.5)."
  type        = map(string)
  default     = {}
}

variable "create_app_insights" {
  description = "Create Application Insights on a Log Analytics workspace with a daily cap (5 GB a month of ingestion is free)."
  type        = bool
  default     = true
}

variable "log_daily_cap_gb" {
  description = "The workspace's and Application Insights' daily ingestion cap: a hard stop on log cost."
  type        = number
  default     = 0.1
}

# --- items: Cosmos DB free tier -----------------------------------------------------
variable "cosmos_free_tier" {
  description = "Create the Cosmos DB account with the lifetime free tier (1,000 RU/s and 25 GB). One per subscription, chosen at creation. false needs allow_paid."
  type        = bool
  default     = true
  validation {
    condition     = var.cosmos_free_tier || var.allow_paid
    error_message = "Cosmos DB without the free tier is not free; set allow_paid = true."
  }
}

variable "cosmos_throughput" {
  description = "The database's shared, fixed (not autoscale) RU/s. The account's total throughput limit is set to the same number, a hard cap."
  type        = number
  default     = 1000
  validation {
    condition     = var.cosmos_throughput >= 400 && (var.cosmos_throughput <= 1000 || var.allow_paid)
    error_message = "cosmos_throughput is 400 to 1000 (the free tier) unless allow_paid."
  }
}

# --- queue: Queue Storage ---------------------------------------------------------------
variable "create_queue" {
  description = "Create the queue port's storage queue (the Functions queue trigger moves poison messages to <queue>-poison)."
  type        = bool
  default     = true
}

# --- blobs: Blob Storage ------------------------------------------------------------------
variable "create_blobs_container" {
  description = "Create a private blobs container in the app's storage account."
  type        = bool
  default     = false
}
