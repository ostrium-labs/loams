# A Loams home on one AWS VM running k3s: a root module for ../../modules/aws-k3s.
# This is what `loams deploy` runs for the aws-k3s target; it also works by hand.
#
# To use the module from your own root, instead of this directory:
#   source = "git::https://github.com/ostrium-labs/loams.git//deploy/tofu/modules/aws-k3s?ref=v<release>"
#
# Before `tofu apply`, create the Tailscale auth key's secret yourself (never in a
# variable or in this directory; the prefix is loams/<name>):
#   aws secretsmanager create-secret --name loams/prod/tailnet-authkey --secret-string "$TS_AUTHKEY"
# State and plan encryption is enforced: see encryption.tf.

terraform {
  required_version = ">= 1.6"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

variable "name" {
  description = "The deployment's name (lowercase letters, digits and -)."
  type        = string
  default     = "loams"
}

variable "region" {
  type    = string
  default = "us-east-1"
}

variable "release" {
  description = "The Loams release to install, a version such as 0.1.4."
  type        = string
}

variable "size" {
  description = "small (t3.large) or medium (t3.xlarge)."
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

provider "aws" {
  region = var.region
}

module "target" {
  source = "../../modules/aws-k3s"

  name            = var.name
  region          = var.region
  release         = var.release
  size            = var.size
  tailnet_domain  = var.tailnet_domain
  home_spec       = var.home_spec
  operator_values = var.operator_values
  access_db       = var.access_db
  access_db_dsn   = var.access_db_dsn
  chart_digest    = var.chart_digest
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

output "instance_id" {
  value = module.target.instance_id
}

output "kms_key_arn" {
  value = module.target.kms_key_arn
}

output "secret_prefix" {
  value = module.target.secret_prefix
}
