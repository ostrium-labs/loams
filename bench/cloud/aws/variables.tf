variable "run_id" {
  description = "The run id: every resource carries loams-bench=<run_id> (plan §3). Short, so derived names fit every limit."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,15}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 16)."
  }
}

variable "row" {
  description = "The benchmark row (bench/cloud/rows.json): aws-free or aws-ondemand."
  type        = string
  validation {
    condition     = contains(["aws-free", "aws-ondemand"], var.row)
    error_message = "row is aws-free or aws-ondemand."
  }
}

variable "region" {
  type    = string
  default = "us-east-1"
}

variable "commit" {
  description = "The commit the run is built from (recorded in the results)."
  type        = string
  default     = ""
}

variable "package_path" {
  description = "The Lambda zip (bench/cloud/app: node build.mjs aws)."
  type        = string
}

variable "budget_emails" {
  type = list(string)
}

variable "items_capacity_mode" {
  type    = string
  default = "provisioned"
}

variable "allow_paid" {
  type    = bool
  default = false
}

variable "reserved_concurrency" {
  description = "The function's reserved concurrency; null while the account's concurrency limit is 10 (plan §9)."
  type        = number
  default     = 5
}

variable "loadgen_instance_type" {
  description = "The in-region loadgen (plan §5.4): 2 vCPU, not burstable."
  type        = string
  default     = "c7a.large"
}

variable "max_minutes" {
  description = "The loadgen's hard deadline: it powers off (and terminates) after this many minutes."
  type        = number
  default     = 90
  validation {
    condition     = var.max_minutes >= 10 && var.max_minutes <= 180
    error_message = "max_minutes is 10 to 180."
  }
}

variable "bench_env" {
  description = "Extra BENCH_* settings for run-scenarios.sh (sizes; never secrets)."
  type        = map(string)
  default     = {}
}

variable "oha_url" {
  type    = string
  default = "https://github.com/hatoo/oha/releases/download/v1.16.0/oha-linux-amd64"
}

variable "oha_sha256" {
  type    = string
  default = "620bb9e16fb53eabc9a3fc45f88bdb41fefa3fee5c05e75892011ce320391716"
}
