# A Loams home on one Google Cloud VM running k3s: a root module for ../../modules/gcp-k3s.
# This is what `loams deploy` runs for the gcp-k3s target; it also works by hand.
#
# To use the module from your own root, instead of this directory:
#   source = "git::https://github.com/ostrium-labs/loams.git//deploy/tofu/modules/gcp-k3s?ref=v<release>"
#
# Before `tofu apply`, create the Tailscale auth key's secret yourself (never in a
# variable or in this directory; the prefix is loams-<name>). `gcloud secrets create`
# offers to enable the Secret Manager API; the module enables the others (enable_apis).
#   printf %s "$TS_AUTHKEY" | gcloud secrets create loams-prod-tailnet-authkey --data-file=- --project "$PROJECT"
# State and plan encryption is enforced: see encryption.tf.

terraform {
  required_version = ">= 1.6"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 8.0"
    }
  }
}

variable "name" {
  description = "The deployment's name (a lowercase letter, then lowercase letters, digits and -; up to 21)."
  type        = string
  default     = "loams"
}

variable "project" {
  description = "The Google Cloud project that holds everything."
  type        = string
}

variable "region" {
  type    = string
  default = "us-central1"
}

variable "zone" {
  type    = string
  default = "us-central1-a"
}

variable "release" {
  description = "The Loams release to install, a version such as 0.1.4."
  type        = string
}

variable "size" {
  description = "small (e2-standard-2) or medium (e2-standard-4)."
  type        = string
  default     = "small"
}

variable "tailnet_domain" {
  description = "Your tailnet's DNS name, for example tail1234.ts.net."
  type        = string
}

variable "home_spec" {
  description = "The home LoamsCluster's spec."
  type        = any
}

variable "operator_values" {
  description = "Other values for the operator chart."
  type        = any
  default     = {}
}

variable "access_db" {
  description = "local (a Postgres in the home) or external (access_db_dsn)."
  type        = string
  default     = "local"
}

variable "access_db_dsn" {
  description = "The managed Postgres DSN when access_db is external."
  type        = string
  default     = null
  sensitive   = true
}

variable "chart_digest" {
  description = "Pins the operator chart by digest (sha256:<64 hex>), or empty."
  type        = string
  default     = ""
}

provider "google" {
  project = var.project
  region  = var.region
}

module "target" {
  source = "../../modules/gcp-k3s"

  name            = var.name
  project         = var.project
  region          = var.region
  zone            = var.zone
  release         = var.release
  size            = var.size
  tailnet_domain  = var.tailnet_domain
  home_spec       = var.home_spec
  operator_values = var.operator_values
  access_db       = var.access_db
  access_db_dsn   = var.access_db_dsn
  chart_digest    = var.chart_digest
  enable_apis     = true
}

output "kubeconfig" {
  value     = module.target.kubeconfig
  sensitive = true
}

output "api_url" {
  value = module.target.api_url
}

output "cost_items" {
  value = module.target.cost_items
}

output "bucket_url" {
  value = module.target.bucket_url
}

output "cloud" {
  value = module.target.cloud
}

output "region" {
  value = module.target.region
}

output "instance_name" {
  value = module.target.instance_name
}

output "service_account_email" {
  value = module.target.service_account_email
}

output "kms_key_id" {
  value = module.target.kms_key_id
}

output "secret_prefix" {
  value = module.target.secret_prefix
}
