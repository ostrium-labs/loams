variable "run_id" {
  description = "The run id: every resource carries loams-bench=<run_id> (plan §3)."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,15}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 16)."
  }
}

variable "row" {
  type = string
  validation {
    condition     = var.row == "azure-free"
    error_message = "row is azure-free."
  }
}

variable "location" {
  type    = string
  default = "eastus"
}

variable "commit" {
  type    = string
  default = ""
}

variable "package_path" {
  description = "The Flex Consumption zip (bench/cloud/app: node build.mjs azure)."
  type        = string
}

variable "budget_emails" {
  type = list(string)
}

variable "loadgen_vm_size" {
  type    = string
  default = "Standard_D2as_v5"
}

variable "max_minutes" {
  type    = number
  default = 90
  validation {
    condition     = var.max_minutes >= 10 && var.max_minutes <= 180
    error_message = "max_minutes is 10 to 180."
  }
}

variable "bench_env" {
  type    = map(string)
  default = {}
}

variable "oha_url" {
  type    = string
  default = "https://github.com/hatoo/oha/releases/download/v1.16.0/oha-linux-amd64"
}

variable "oha_sha256" {
  type    = string
  default = "620bb9e16fb53eabc9a3fc45f88bdb41fefa3fee5c05e75892011ce320391716"
}
