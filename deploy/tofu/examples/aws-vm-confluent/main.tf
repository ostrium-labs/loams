# Loams on an AWS VM with a Confluent Cloud cluster in the same region (design 80,
# plan CC1 Task 7): ../../modules/aws-vm and ../../modules/confluent, wired so the
# cluster's cloud and region are the VM's (D1824).
#
# Before `tofu apply`:
#   - the Tailscale auth key's secret, as for ../aws-vm;
#   - a KMS key for the state (encryption.tf);
#   - your Confluent Cloud API key in CONFLUENT_CLOUD_API_KEY / CONFLUENT_CLOUD_API_SECRET.
# A new cluster takes two applies (topics and ACLs need a Kafka admin key, which
# never enters state): see docs/guides/byoc/confluent.md. Then put the data
# account's key in the secret store with
#   loams byoc confluent key create --integration confluent --service-account <data_service_account_id> ...
# (create_api_keys = true does it in this apply instead, with the key in the
# encrypted state).

terraform {
  required_version = ">= 1.11"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    confluent = {
      source  = "confluentinc/confluent"
      version = "= 2.90.0"
    }
  }
}

variable "region" {
  type    = string
  default = "us-east-1"
}

variable "name" {
  type    = string
  default = "prod"
}

variable "environment_id" {
  description = "An existing Confluent environment, or empty to create loams-<name>."
  type        = string
  default     = ""
}

variable "cluster_id" {
  description = "An existing cluster in environment_id (mode a), or empty to create one."
  type        = string
  default     = ""
}

variable "cluster_type" {
  description = "basic (CC1's example) or standard."
  type        = string
  default     = "basic"
}

variable "max_ecku" {
  type    = number
  default = 1
}

variable "kafka_admin" {
  description = "none for the first apply of a new cluster, then provider (with KAFKA_* set)."
  type        = string
  default     = "provider"
}

variable "create_api_keys" {
  description = "Create the data account's key in this apply and store it in Secrets Manager (it is then in the encrypted state)."
  type        = bool
  default     = false
}

variable "data_service_account_id" {
  type    = string
  default = ""
}

variable "create_metrics_account" {
  type    = bool
  default = true
}

variable "topics" {
  description = "Topics for Loams' connectors; retention_bytes per partition bounds storage."
  type = map(object({
    partitions      = number
    retention_bytes = number
    retention_ms    = optional(number, 604800000)
    cleanup_policy  = optional(string, "delete")
  }))
  default = {
    "loams-prod-events" = { partitions = 3, retention_bytes = 1073741824 }
  }
}

variable "connector_bytes_per_sec" {
  description = "Loams' connector rate limits on this cluster (set the same rate_limit in the connector instances). 64 KiB/s each way bounds Loams' throughput charges at about $16 a month."
  type = object({
    in  = number
    out = number
  })
  default = { in = 65536, out = 65536 }
}

variable "confluent_budget_monthly_usd" {
  type    = number
  default = 25
}

provider "aws" {
  region = var.region
}

provider "confluent" {}

module "vm" {
  source        = "../../modules/aws-vm"
  name          = var.name
  secret_prefix = "loams/${var.name}"
  # The VM fetches confluent-confluent-api-key and -api-secret into the connect
  # role's secret files at boot (deploy/byoc/lib/loams-byoc-secrets).
  confluent_integrations = ["confluent"]
  # The connect role runs in the `connectors` profile; with it the engine's
  # LOAMS_AUTH_MODE becomes tailscale,apikey (loams-byoc-bootstrap).
  profiles = "connectors"
}

module "confluent" {
  source                       = "../../modules/confluent"
  name                         = var.name
  cloud                        = module.vm.cloud
  region                       = module.vm.region
  secret_prefix                = module.vm.secret_prefix
  environment_id               = var.environment_id
  cluster_id                   = var.cluster_id
  cluster_type                 = var.cluster_type
  max_ecku                     = var.max_ecku
  topics                       = var.topics
  kafka_admin                  = var.kafka_admin
  create_api_keys              = var.create_api_keys
  data_service_account_id      = var.data_service_account_id
  create_metrics_account       = var.create_metrics_account
  connector_bytes_per_sec      = var.connector_bytes_per_sec
  confluent_budget_monthly_usd = var.confluent_budget_monthly_usd
}

# The secrets' slots; `loams byoc confluent key create` (or create_api_keys) fills them.
# The VM's role may already read <prefix>/*.
resource "aws_secretsmanager_secret" "confluent" {
  for_each                = toset(["api-key", "api-secret"])
  name                    = module.confluent.secret_names[each.key]
  description             = "Loams ${var.name}: Confluent integration ${each.key} (written out of band; never in this configuration)"
  recovery_window_in_days = 7
}

resource "aws_secretsmanager_secret_version" "api_key" {
  count         = var.create_api_keys ? 1 : 0
  secret_id     = aws_secretsmanager_secret.confluent["api-key"].id
  secret_string = module.confluent.data_api_key.id
}

resource "aws_secretsmanager_secret_version" "api_secret" {
  count         = var.create_api_keys ? 1 : 0
  secret_id     = aws_secretsmanager_secret.confluent["api-secret"].id
  secret_string = module.confluent.data_api_key.secret
}

output "instance_id" {
  value = module.vm.instance_id
}

output "estimated_monthly_usd" {
  description = "The VM's estimate (us-east-1 prices; excludes data transfer)."
  value       = module.vm.estimated_monthly_usd
}

output "confluent_cluster_id" {
  value = module.confluent.cluster_id
}

output "confluent_bootstrap_endpoint" {
  value = module.confluent.bootstrap_endpoint
}

output "confluent_rest_endpoint" {
  value = module.confluent.rest_endpoint
}

output "confluent_data_service_account_id" {
  value = module.confluent.data_service_account_id
}

output "confluent_secret_names" {
  value = module.confluent.secret_names
}

output "confluent_connect_integration" {
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
