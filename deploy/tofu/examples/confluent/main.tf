# A root module for ../../modules/confluent alone: a Confluent Cloud cluster (or access
# to an existing one) for a Loams install whose VM is managed elsewhere, and the root
# Ostrium's CI applies (scripts/byoc/confluent-ci.sh). See docs/guides/byoc/confluent.md.
#
# Credentials, all from the environment, none in a file here:
#   CONFLUENT_CLOUD_API_KEY / CONFLUENT_CLOUD_API_SECRET   your Cloud API key (the provider never stores it)
#   TF_VAR_state_passphrase                                  the state and plan encryption passphrase
#   KAFKA_ID, KAFKA_REST_ENDPOINT, KAFKA_API_KEY, KAFKA_API_SECRET
#       a Kafka key with admin rights on the cluster, only for the second phase
#       (kafka_admin = "provider"), so topics and ACLs never put a key in state.
#
# A new cluster takes two applies:
#   tofu apply -var kafka_admin=none     # environment, cluster, service accounts
#   (create an admin Kafka key for yourself on the cluster; export KAFKA_*)
#   tofu apply                           # topics and ACLs
# Destroy needs the KAFKA_* settings too (topics and ACLs are deleted through them).

terraform {
  required_version = ">= 1.11"
  required_providers {
    confluent = {
      source  = "confluentinc/confluent"
      version = "= 2.90.0"
    }
  }

  # State and plans are encrypted, and OpenTofu refuses to write either unencrypted.
  # This root holds no key unless create_api_keys = true, but it still holds ids and
  # endpoints; with a key in state, encryption is required (D1827).
  encryption {
    key_provider "pbkdf2" "passphrase" {
      passphrase = var.state_passphrase
    }
    method "aes_gcm" "state" {
      keys = key_provider.pbkdf2.passphrase
    }
    state {
      method   = method.aes_gcm.state
      enforced = true
    }
    plan {
      method   = method.aes_gcm.state
      enforced = true
    }
  }
}

variable "state_passphrase" {
  description = "At least 16 characters; keep it outside this directory (TF_VAR_state_passphrase)."
  type        = string
  sensitive   = true
}

variable "name" {
  type = string
}

variable "cloud" {
  type    = string
  default = "aws"
}

variable "region" {
  type    = string
  default = "us-east-1"
}

variable "environment_id" {
  type    = string
  default = ""
}

variable "cluster_id" {
  type    = string
  default = ""
}

variable "cluster_type" {
  type    = string
  default = "basic"
}

variable "max_ecku" {
  type    = number
  default = 1
}

variable "topics" {
  type = map(object({
    partitions      = number
    retention_bytes = number
    retention_ms    = optional(number, 604800000)
    cleanup_policy  = optional(string, "delete")
  }))
  default = {}
}

variable "data_service_account_id" {
  type    = string
  default = ""
}

variable "create_metrics_account" {
  type    = bool
  default = true
}

variable "kafka_admin" {
  type    = string
  default = "provider"
}

variable "connector_bytes_per_sec" {
  type = object({
    in  = number
    out = number
  })
  default = null
}

variable "confluent_budget_monthly_usd" {
  type    = number
  default = null
}

provider "confluent" {}

module "confluent" {
  source                       = "../../modules/confluent"
  name                         = var.name
  cloud                        = var.cloud
  region                       = var.region
  environment_id               = var.environment_id
  cluster_id                   = var.cluster_id
  cluster_type                 = var.cluster_type
  max_ecku                     = var.max_ecku
  topics                       = var.topics
  data_service_account_id      = var.data_service_account_id
  create_metrics_account       = var.create_metrics_account
  kafka_admin                  = var.kafka_admin
  connector_bytes_per_sec      = var.connector_bytes_per_sec
  confluent_budget_monthly_usd = var.confluent_budget_monthly_usd
}

output "environment_id" {
  value = module.confluent.environment_id
}

output "cluster_id" {
  value = module.confluent.cluster_id
}

output "bootstrap_endpoint" {
  value = module.confluent.bootstrap_endpoint
}

output "rest_endpoint" {
  value = module.confluent.rest_endpoint
}

output "data_service_account_id" {
  value = module.confluent.data_service_account_id
}

output "connect_integration" {
  value = module.confluent.connect_integration
}

output "confluent_ceiling_monthly_usd" {
  value = module.confluent.confluent_ceiling_monthly_usd
}

output "confluent_ceiling" {
  value = module.confluent.confluent_ceiling
}

output "confluent_unbounded" {
  value = module.confluent.confluent_unbounded
}
