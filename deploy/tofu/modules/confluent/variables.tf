variable "name" {
  description = "The install's name. Resources are named loams-<name>[-data|-metrics|...] (or <name>... when it already starts with loams-). A name starting with loams-ci- marks Ostrium's CI: deletion protection is off."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,40}$", var.name))
    error_message = "name is lowercase letters, digits and - (up to 41)."
  }
}

variable "cloud" {
  description = "The Loams VM's cloud (aws, gcp or azure): wire it from the VM module (design 80 §6.1, D1824)."
  type        = string
  validation {
    condition     = contains(["aws", "gcp", "azure"], var.cloud)
    error_message = "cloud is aws, gcp or azure."
  }
}

variable "region" {
  description = "The Loams VM's region, in the cloud's own code (us-east-1, us-central1, eastus): wire it from the VM module."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9-]{3,40}$", var.region))
    error_message = "region is a cloud region code such as us-east-1."
  }
}

variable "cluster_cloud" {
  description = "The cluster's cloud when it differs from the VM's. null: the VM's. Another cloud needs allow_cross_region."
  type        = string
  default     = null
  validation {
    condition     = var.cluster_cloud == null || contains(["aws", "gcp", "azure"], coalesce(var.cluster_cloud, "aws"))
    error_message = "cluster_cloud is aws, gcp or azure."
  }
}

variable "cluster_region" {
  description = "The cluster's region when it differs from the VM's. null: the VM's. Another region needs allow_cross_region."
  type        = string
  default     = null
}

variable "allow_cross_region" {
  description = "Allow a cluster in another cloud or region than the VM. Every byte then pays the cloud's internet egress and tens of milliseconds per round trip (design 80 §6.1)."
  type        = bool
  default     = false
}

variable "environment_id" {
  description = "An existing Confluent environment (env-...). Empty: the module creates loams-<name> with Stream Governance Essentials."
  type        = string
  default     = ""
  validation {
    condition     = var.environment_id == "" || can(regex("^env-[a-z0-9]+$", var.environment_id))
    error_message = "environment_id is an env-... id or empty."
  }
}

variable "cluster_id" {
  description = "An existing Kafka cluster (lkc-...) in environment_id, for mode (a). Empty: the module creates one."
  type        = string
  default     = ""
  validation {
    condition     = var.cluster_id == "" || can(regex("^lkc-[a-z0-9]+$", var.cluster_id))
    error_message = "cluster_id is an lkc-... id or empty."
  }
}

variable "cluster_type" {
  description = "basic (default; the first eCKU is free) or standard. enterprise, freight and dedicated only with allow_premium_cluster_types (D1823)."
  type        = string
  default     = "basic"
  validation {
    condition     = contains(["basic", "standard", "enterprise", "freight", "dedicated"], var.cluster_type)
    error_message = "cluster_type is basic, standard, enterprise, freight or dedicated."
  }
}

variable "allow_premium_cluster_types" {
  description = "Allow enterprise, freight and dedicated (from about $1,280 a month at one eCKU around the clock). Off by default."
  type        = bool
  default     = false
}

variable "max_ecku" {
  description = "The cluster's eCKU ceiling: a hard cap on eCKU charges (design 80 §8.1). Basic at 1 costs nothing in eCKUs; Standard at 1 is at most $547.50 a month. For dedicated, the CKU count."
  type        = number
  default     = 1
  validation {
    condition     = var.max_ecku >= 1 && floor(var.max_ecku) == var.max_ecku
    error_message = "max_ecku is a whole number of at least 1."
  }
}

variable "availability" {
  description = "SINGLE_ZONE (default) or MULTI_ZONE (Standard and above, with max_ecku >= 2)."
  type        = string
  default     = "SINGLE_ZONE"
  validation {
    condition     = contains(["SINGLE_ZONE", "MULTI_ZONE"], var.availability)
    error_message = "availability is SINGLE_ZONE or MULTI_ZONE."
  }
}

variable "confluent_networking" {
  description = "public (default: TLS and SASL to public endpoints, outbound from the VM, D1825). privatelink comes in CC3."
  type        = string
  default     = "public"
  validation {
    condition     = var.confluent_networking == "public"
    error_message = "only public networking is built (CC1); privatelink with Enterprise comes in CC3."
  }
}

variable "deletion_protection" {
  description = "Override the cluster's deletion protection. null: on, unless the name starts with loams-ci-."
  type        = bool
  default     = null
}

variable "topics" {
  description = "Topics the module creates and the data account may use: name => { partitions, retention_bytes (per partition, required: it bounds storage, D1828), retention_ms, cleanup_policy }."
  type = map(object({
    partitions      = number
    retention_bytes = number
    retention_ms    = optional(number, 604800000)
    cleanup_policy  = optional(string, "delete")
  }))
  default = {}
  validation {
    condition     = alltrue([for n, t in var.topics : can(regex("^[a-zA-Z0-9._-]{1,249}$", n))])
    error_message = "topic names are 1-249 of a-z, A-Z, 0-9, ., _ and -."
  }
  validation {
    condition     = alltrue([for t in var.topics : t.retention_bytes > 0])
    error_message = "every topic needs retention_bytes > 0 (no unbounded storage)."
  }
  validation {
    condition     = alltrue([for t in var.topics : t.partitions >= 1 && floor(t.partitions) == t.partitions])
    error_message = "partitions is a whole number of at least 1."
  }
  validation {
    condition     = alltrue([for t in var.topics : t.retention_ms == -1 || t.retention_ms > 0])
    error_message = "retention_ms is positive, or -1 (bounded by retention_bytes only)."
  }
  validation {
    condition     = alltrue([for t in var.topics : contains(["delete", "compact", "compact,delete"], t.cleanup_policy)])
    error_message = "cleanup_policy is delete, compact or compact,delete."
  }
}

variable "data_topics" {
  description = "Existing topics (not created here) the data account may read and write, by exact name (mode a)."
  type        = list(string)
  default     = []
}

variable "exactly_once_sinks" {
  description = "Create the compacted checkpoint topic (one partition) that transactional Kafka sinks need (the connector profile does not create it: the data account has no CREATE)."
  type        = bool
  default     = true
}

variable "checkpoint_topic" {
  description = "The checkpoint topic's name; the Kafka sink's checkpoint_topic must match."
  type        = string
  default     = "__loams_checkpoints"
}

variable "checkpoint_retention_bytes" {
  description = "retention.bytes of the checkpoint topic."
  type        = number
  default     = 104857600
}

variable "consumer_group_prefix" {
  description = "Consumer groups the data account may use (PREFIXED). null: loams-<name>-."
  type        = string
  default     = null
}

variable "transactional_id_prefix" {
  description = "Transactional ids the data account may use (PREFIXED). null: loams-<name>-."
  type        = string
  default     = null
}

variable "data_service_account_id" {
  description = "An existing service account (sa-...) for Loams' data access. Empty: the module creates loams-<name>-data. Ostrium's CI passes its persistent loams-ci-data (CC0 ruling R0.6)."
  type        = string
  default     = ""
  validation {
    condition     = var.data_service_account_id == "" || can(regex("^sa-[a-z0-9]+$", var.data_service_account_id))
    error_message = "data_service_account_id is an sa-... id or empty."
  }
}

variable "create_metrics_account" {
  description = "Create loams-<name>-metrics with MetricsViewer on the environment, for the budget guard (CC2)."
  type        = bool
  default     = true
}

variable "create_ops_account" {
  description = "Create loams-<name>-ops with CloudClusterAdmin on this cluster, used only by the guard's opt-in max_ecku throttle (Q1338). Off by default."
  type        = bool
  default     = false
}

variable "kafka_admin" {
  description = <<-EOT
    How topics and ACLs are written (they need a Kafka key with admin rights on the cluster):
    - provider (default): through the provider's own Kafka settings (KAFKA_ID, KAFKA_REST_ENDPOINT, KAFKA_API_KEY, KAFKA_API_SECRET in the environment), which never enter state. A new cluster needs a first apply with kafka_admin = "none".
    - none: no topics and no ACLs (the first phase for a new cluster).
    - module_key: the module creates loams-<name>-admin with CloudClusterAdmin and its key; one apply, but the key is in state, so it needs create_api_keys and a root with enforced state encryption (D1827).
  EOT
  type        = string
  default     = "provider"
  validation {
    condition     = contains(["provider", "none", "module_key"], var.kafka_admin)
    error_message = "kafka_admin is provider, none or module_key."
  }
}

variable "create_api_keys" {
  description = "Create Kafka API keys (the data account's, and the admin's with kafka_admin = module_key) in this configuration. The secrets are then in state: allowed only in a root with state and plan encryption enforced = true (D1827). Default: keys are created out of band with `loams byoc confluent key create`."
  type        = bool
  default     = false
}

variable "secret_prefix" {
  description = "The cloud secret store prefix of the install (BY1 naming, loams/<name>). The module outputs the Confluent secret names under it; it never holds values."
  type        = string
  default     = null
}

variable "integration" {
  description = "The integration's name in secret names and secret files (confluent-<integration>-api-key)."
  type        = string
  default     = "confluent"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,30}$", var.integration))
    error_message = "integration is lowercase letters, digits and - (up to 31)."
  }
}

variable "lookup_schema_registry" {
  description = "Read the environment's Schema Registry endpoint (Stream Governance Essentials is attached to environments the module creates)."
  type        = bool
  default     = true
}

variable "connector_bytes_per_sec" {
  description = "Loams' connector rate limits on this cluster (in: produce to Confluent, out: consume from it), in bytes per second. Bounds Loams' share of throughput charges in the ceiling; null leaves it unbounded."
  type = object({
    in  = number
    out = number
  })
  default = null
}

variable "confluent_budget_monthly_usd" {
  description = "The monthly budget for this integration (the guard's, CC2). A check warns when the ceiling is above it. null: no budget."
  type        = number
  default     = null
}
