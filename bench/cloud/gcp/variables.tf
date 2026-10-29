variable "run_id" {
  description = "The run id: every labelled resource carries loams-bench=<run_id> (plan §3)."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,15}$", var.run_id))
    error_message = "run_id is lowercase letters, digits and - (up to 16)."
  }
}

variable "row" {
  type = string
  validation {
    condition     = var.row == "gcp-free"
    error_message = "row is gcp-free."
  }
}

variable "project_id" {
  type = string
}

variable "billing_account" {
  type = string
}

variable "region" {
  type    = string
  default = "us-central1"
}

variable "zone" {
  type    = string
  default = "us-central1-a"
}

variable "commit" {
  type    = string
  default = ""
}

variable "image" {
  description = "The bench app's image by digest, in the module's Artifact Registry repository. run.sh applies the repository alone first (with the placeholder), pushes, then applies everything with the digest."
  type        = string
  default     = "us-docker.pkg.dev/cloudrun/container/hello"
}

variable "create_firestore" {
  description = "Create the project's (default) Firestore database; false when it exists already (plan §9)."
  type        = bool
  default     = true
}

variable "loadgen" {
  description = "Create the loadgen VM. false for the first, repository-only apply."
  type        = bool
  default     = true
}

variable "loadgen_machine_type" {
  type    = string
  default = "e2-standard-2"
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
