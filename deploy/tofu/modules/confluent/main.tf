# The confluent module (design 80 §5, D1823-D1828; plan CC1 Task 7): a Confluent
# Cloud Kafka cluster next to a Loams BYOC VM, or the access Loams needs on an
# existing one. The provider reads the user's Cloud API key from the environment
# (CONFLUENT_CLOUD_API_KEY, CONFLUENT_CLOUD_API_SECRET); it is never in state.
# By default no Kafka key is in state either (kafka_admin, create_api_keys).

locals {
  base  = startswith(var.name, "loams-") ? var.name : "loams-${var.name}"
  is_ci = startswith(local.base, "loams-ci-")

  # deploy/byoc/confluent/{regions,prices}.toml, copied by scripts/byoc/confluent-tables.sh gen
  tables = jsondecode(file("${path.module}/confluent-tables.json"))

  create_env     = var.environment_id == ""
  create_cluster = var.cluster_id == ""
  premium        = contains(["enterprise", "freight", "dedicated"], var.cluster_type)

  want_cloud  = coalesce(var.cluster_cloud, var.cloud)
  want_region = coalesce(var.cluster_region, var.region)
  region_row  = lookup(local.tables.regions, "${local.want_cloud}/${local.want_region}", null)

  existing = one(data.confluent_kafka_cluster.existing[*])
  # The type of an existing cluster, from the block its description carries
  # (cluster_type when it carries none).
  existing_type = (
    length(try(local.existing.basic, [])) > 0 ? "basic" :
    length(try(local.existing.standard, [])) > 0 ? "standard" :
    length(try(local.existing.enterprise, [])) > 0 ? "enterprise" :
    length(try(local.existing.freight, [])) > 0 ? "freight" :
    length(try(local.existing.dedicated, [])) > 0 ? "dedicated" : var.cluster_type
  )
  cluster_type  = local.create_cluster ? var.cluster_type : local.existing_type
  cluster_cloud = local.create_cluster ? local.want_cloud : lower(try(local.existing.cloud, ""))
  # Confluent reports regions in the clouds' own codes (CC0 ruling R0.4).
  cluster_region = local.create_cluster ? local.want_region : try(local.existing.region, "")
  cross_region   = local.cluster_cloud != var.cloud || local.cluster_region != var.region

  price = lookup(local.tables.types, local.cluster_type, null)

  env_id  = local.create_env ? confluent_environment.this[0].id : var.environment_id
  env_crn = one(concat(confluent_environment.this[*].resource_name, data.confluent_environment.existing[*].resource_name))

  cluster_src = one(concat(confluent_kafka_cluster.this, data.confluent_kafka_cluster.existing))
  cluster = {
    id            = local.cluster_src.id
    api_version   = local.cluster_src.api_version
    kind          = local.cluster_src.kind
    rest_endpoint = local.cluster_src.rest_endpoint
    bootstrap     = local.cluster_src.bootstrap_endpoint
    rbac_crn      = local.cluster_src.rbac_crn
  }

  group_prefix = coalesce(var.consumer_group_prefix, "${local.base}-")
  txn_prefix   = coalesce(var.transactional_id_prefix, "${local.base}-")

  # Every topic the module creates, the checkpoint topic included.
  managed_topics = merge(
    { for n, t in var.topics : n => t },
    var.exactly_once_sinks ? {
      (var.checkpoint_topic) = {
        partitions      = 1
        retention_bytes = var.checkpoint_retention_bytes
        retention_ms    = -1
        cleanup_policy  = "compact"
      }
    } : {}
  )
  partitions = sum(concat([0], [for t in local.managed_topics : t.partitions]))

  kafka_resources = var.kafka_admin != "none"
  module_key      = var.kafka_admin == "module_key"

  data_sa = var.data_service_account_id != "" ? {
    id          = var.data_service_account_id
    api_version = "iam/v2"
    kind        = "ServiceAccount"
    } : {
    id          = confluent_service_account.data[0].id
    api_version = confluent_service_account.data[0].api_version
    kind        = confluent_service_account.data[0].kind
  }

  # D1826: the data account's whole grant set. Never topic CREATE, DELETE, ALTER or
  # ALTER_CONFIGS, and no ACL operation: Loams cannot create topics, add partitions,
  # raise retention or widen its own access.
  data_topic_names = distinct(concat(keys(local.managed_topics), var.data_topics))
  data_acls = merge(
    { for pair in setproduct(local.data_topic_names, ["READ", "WRITE", "DESCRIBE", "DESCRIBE_CONFIGS"]) :
      "topic/${pair[0]}/${pair[1]}" => { type = "TOPIC", name = pair[0], pattern = "LITERAL", op = pair[1] }
    },
    { for op in ["READ", "DESCRIBE"] :
      "group/${op}" => { type = "GROUP", name = local.group_prefix, pattern = "PREFIXED", op = op }
    },
    { for op in ["WRITE", "DESCRIBE"] :
      "txn/${op}" => { type = "TRANSACTIONAL_ID", name = local.txn_prefix, pattern = "PREFIXED", op = op }
    },
    { "cluster/IDEMPOTENT_WRITE" = { type = "CLUSTER", name = "kafka-cluster", pattern = "LITERAL", op = "IDEMPOTENT_WRITE" } },
  )

  prefix = coalesce(var.secret_prefix, "loams/${var.name}")
  secret_names = {
    for k in ["api-key", "api-secret", "sr-key", "sr-secret", "metrics-key", "metrics-secret"] :
    k => var.cloud == "aws" ? "${local.prefix}/confluent-${var.integration}-${k}" : "${replace(local.prefix, "/", "-")}-confluent-${var.integration}-${k}"
  }
}

# --- the environment and the cluster ---------------------------------------------
resource "confluent_environment" "this" {
  count        = local.create_env ? 1 : 0
  display_name = local.base
  # Essentials is free (100 schemas) and gives the environment a Schema Registry.
  stream_governance {
    package = "ESSENTIALS"
  }
}

data "confluent_environment" "existing" {
  count = local.create_env ? 0 : 1
  id    = var.environment_id
}

data "confluent_kafka_cluster" "existing" {
  count = local.create_cluster ? 0 : 1
  id    = var.cluster_id
  environment {
    id = var.environment_id
  }
}

# The refusals (D1823, D1824, D1827): a plan fails here before anything is created.
resource "terraform_data" "checks" {
  input = {
    cluster_type = local.cluster_type
    region       = "${local.cluster_cloud}/${local.cluster_region}"
    networking   = var.confluent_networking
  }
  lifecycle {
    precondition {
      condition     = !local.create_cluster || !local.premium || var.allow_premium_cluster_types
      error_message = "cluster_type ${var.cluster_type} is a premium type (from about $1,280 a month at one eCKU around the clock). Set allow_premium_cluster_types = true to create it."
    }
    precondition {
      condition     = !local.cross_region || var.allow_cross_region
      error_message = "the cluster (${local.cluster_cloud} ${local.cluster_region}) is not in the VM's cloud and region (${var.cloud} ${var.region}). Every byte would pay the cloud's internet egress and tens of milliseconds per round trip; set allow_cross_region = true to accept that."
    }
    precondition {
      condition     = !local.create_cluster || (local.region_row != null ? (local.region_row.schedulable && contains(local.region_row.types, var.cluster_type)) : false)
      error_message = "Confluent does not offer ${var.cluster_type} clusters in ${local.want_cloud} ${local.want_region} (deploy/byoc/confluent/regions.toml, read ${local.tables.regions_read})."
    }
    precondition {
      condition     = var.availability == "SINGLE_ZONE" || (var.cluster_type != "basic" && var.max_ecku >= 2)
      error_message = "MULTI_ZONE needs Standard or above and max_ecku >= 2; Basic is single-zone only."
    }
    precondition {
      condition     = !local.create_cluster || local.price == null || var.max_ecku <= try(local.price.max_ecku, 1000)
      error_message = "max_ecku ${var.max_ecku} is above the ${var.cluster_type} type's maximum (${try(local.price.max_ecku, "?")})."
    }
    precondition {
      condition     = !local.create_cluster || var.cluster_type != "freight" || var.max_ecku >= 2
      error_message = "Freight needs max_ecku >= 2."
    }
    precondition {
      # CC0 ruling R0.13: a Basic cluster at max_ecku 1 holds at most 30 partitions.
      condition     = local.price == null || !local.create_cluster || local.partitions <= min(local.price.partitions_per_ecku * var.max_ecku, local.price.partitions_max)
      error_message = "the module's topics need ${local.partitions} partitions; a ${var.cluster_type} cluster at max_ecku ${var.max_ecku} holds at most ${try(min(local.price.partitions_per_ecku * var.max_ecku, local.price.partitions_max), "?")}."
    }
    precondition {
      condition     = !local.module_key || var.create_api_keys
      error_message = "kafka_admin = \"module_key\" puts a Kafka key in state: it needs create_api_keys = true, in a root module with state and plan encryption enforced (D1827)."
    }
  }
}

resource "confluent_kafka_cluster" "this" {
  count               = local.create_cluster ? 1 : 0
  display_name        = local.base
  availability        = var.availability
  cloud               = upper(local.want_cloud)
  region              = local.want_region
  deletion_protection = coalesce(var.deletion_protection, !local.is_ci)

  dynamic "basic" {
    for_each = var.cluster_type == "basic" ? [1] : []
    content {
      max_ecku = var.max_ecku
    }
  }
  dynamic "standard" {
    for_each = var.cluster_type == "standard" ? [1] : []
    content {
      max_ecku = var.max_ecku
    }
  }
  dynamic "enterprise" {
    for_each = var.cluster_type == "enterprise" ? [1] : []
    content {
      max_ecku = var.max_ecku
    }
  }
  dynamic "freight" {
    for_each = var.cluster_type == "freight" ? [1] : []
    content {
      max_ecku = var.max_ecku
    }
  }
  dynamic "dedicated" {
    for_each = var.cluster_type == "dedicated" ? [1] : []
    content {
      cku = var.max_ecku
    }
  }

  environment {
    id = local.env_id
  }

  depends_on = [terraform_data.checks]
}

data "confluent_schema_registry_cluster" "this" {
  count = var.lookup_schema_registry ? 1 : 0
  environment {
    id = local.env_id
  }
  depends_on = [confluent_kafka_cluster.this]
}

# --- identities (D1826) -------------------------------------------------------------
resource "confluent_service_account" "data" {
  count        = var.data_service_account_id == "" ? 1 : 0
  display_name = "${local.base}-data"
  description  = "Loams ${var.name}: data access (Kafka ACLs only; no topic create, delete or config change)"
}

resource "confluent_service_account" "metrics" {
  count        = var.create_metrics_account ? 1 : 0
  display_name = "${local.base}-metrics"
  description  = "Loams ${var.name}: MetricsViewer on the environment, for the budget guard"
}

# Environment scope reads this environment's clusters only (CC0 ruling R0.7).
resource "confluent_role_binding" "metrics" {
  count       = var.create_metrics_account ? 1 : 0
  principal   = "User:${confluent_service_account.metrics[0].id}"
  role_name   = "MetricsViewer"
  crn_pattern = local.env_crn
}

resource "confluent_service_account" "ops" {
  count        = var.create_ops_account ? 1 : 0
  display_name = "${local.base}-ops"
  description  = "Loams ${var.name}: CloudClusterAdmin on one cluster, used only to lower max_ecku (opt-in guard throttle)"
}

resource "confluent_role_binding" "ops" {
  count       = var.create_ops_account ? 1 : 0
  principal   = "User:${confluent_service_account.ops[0].id}"
  role_name   = "CloudClusterAdmin"
  crn_pattern = local.cluster.rbac_crn
}

# --- the admin key, only with kafka_admin = module_key (state must be encrypted) -----
resource "confluent_service_account" "admin" {
  count        = local.module_key ? 1 : 0
  display_name = "${local.base}-admin"
  description  = "Loams ${var.name}: writes this cluster's topics and ACLs from OpenTofu"
}

resource "confluent_role_binding" "admin" {
  count       = local.module_key ? 1 : 0
  principal   = "User:${confluent_service_account.admin[0].id}"
  role_name   = "CloudClusterAdmin"
  crn_pattern = local.cluster.rbac_crn
}

resource "confluent_api_key" "admin" {
  count        = local.module_key && var.create_api_keys ? 1 : 0
  display_name = "${local.base}-admin"
  description  = "Loams ${var.name}: OpenTofu's key for topics and ACLs"
  owner {
    id          = confluent_service_account.admin[0].id
    api_version = confluent_service_account.admin[0].api_version
    kind        = confluent_service_account.admin[0].kind
  }
  managed_resource {
    id          = local.cluster.id
    api_version = local.cluster.api_version
    kind        = local.cluster.kind
    environment {
      id = local.env_id
    }
  }
  depends_on = [confluent_role_binding.admin]
}

resource "confluent_api_key" "data" {
  count        = var.create_api_keys ? 1 : 0
  display_name = "${local.base}-data"
  description  = "Loams ${var.name}: the data account's Kafka key"
  owner {
    id          = local.data_sa.id
    api_version = local.data_sa.api_version
    kind        = local.data_sa.kind
  }
  managed_resource {
    id          = local.cluster.id
    api_version = local.cluster.api_version
    kind        = local.cluster.kind
    environment {
      id = local.env_id
    }
  }
}

# --- topics and ACLs --------------------------------------------------------------------
# kafka_admin = provider: no kafka_cluster, rest_endpoint or credentials here; the
# provider's KAFKA_* settings supply them and nothing about the key reaches state.
resource "confluent_kafka_topic" "this" {
  for_each         = local.kafka_resources ? local.managed_topics : {}
  topic_name       = each.key
  partitions_count = each.value.partitions
  rest_endpoint    = local.module_key ? local.cluster.rest_endpoint : null
  config = {
    "cleanup.policy"  = each.value.cleanup_policy
    "retention.bytes" = tostring(each.value.retention_bytes)
    "retention.ms"    = tostring(each.value.retention_ms)
  }

  dynamic "kafka_cluster" {
    for_each = local.module_key ? [1] : []
    content {
      id = local.cluster.id
    }
  }
  dynamic "credentials" {
    for_each = local.module_key ? [1] : []
    content {
      key    = confluent_api_key.admin[0].id
      secret = confluent_api_key.admin[0].secret
    }
  }

  depends_on = [terraform_data.checks]
}

resource "confluent_kafka_acl" "data" {
  for_each      = local.kafka_resources ? local.data_acls : {}
  resource_type = each.value.type
  resource_name = each.value.name
  pattern_type  = each.value.pattern
  operation     = each.value.op
  principal     = "User:${local.data_sa.id}"
  host          = "*"
  permission    = "ALLOW"
  rest_endpoint = local.module_key ? local.cluster.rest_endpoint : null

  dynamic "kafka_cluster" {
    for_each = local.module_key ? [1] : []
    content {
      id = local.cluster.id
    }
  }
  dynamic "credentials" {
    for_each = local.module_key ? [1] : []
    content {
      key    = confluent_api_key.admin[0].id
      secret = confluent_api_key.admin[0].secret
    }
  }
}

# --- the ceiling (D1828) ------------------------------------------------------------------
locals {
  hours_per_month   = 730
  seconds_per_month = 730 * 3600
  gib               = 1073741824

  # eCKUs: only for a cluster this module creates (an existing cluster is the user's bill already).
  ceiling_ecku = local.create_cluster && local.price != null ? max(0, var.max_ecku - local.price.free_ecku) * local.price.ecku_hour_usd * local.hours_per_month : 0
  # Storage: every managed partition full to retention.bytes, times the replication factor (R0.8).
  storage_gb      = sum(concat([0], [for t in local.managed_topics : t.partitions * t.retention_bytes])) / local.gib * local.tables.storage_replication_factor
  ceiling_storage = local.price != null ? local.storage_gb * local.price.storage_gb_month_usd : 0
  # Loams' own traffic, bounded by its connector rate limits.
  ceiling_in  = local.price != null && var.connector_bytes_per_sec != null ? var.connector_bytes_per_sec.in * local.seconds_per_month / local.gib * local.price.in_gb_usd : 0
  ceiling_out = local.price != null && var.connector_bytes_per_sec != null ? var.connector_bytes_per_sec.out * local.seconds_per_month / local.gib * local.price.out_gb_usd : 0

  ceiling_monthly_usd = floor((local.ceiling_ecku + local.ceiling_storage + local.ceiling_in + local.ceiling_out) * 100 + 0.5) / 100

  unbounded = compact([
    local.price == null ? "${local.cluster_type}: no list price; nothing about this cluster is bounded here" : "",
    "throughput of clients other than Loams' connectors (Basic and Standard have no client quotas; at most ${try(local.price.ingress_mbps_per_ecku * var.max_ecku, "?")} MBps in and ${try(local.price.egress_mbps_per_ecku * var.max_ecku, "?")} MBps out per cluster)",
    var.connector_bytes_per_sec == null ? "Loams' own connector traffic (no connector_bytes_per_sec given)" : "",
    "storage of topics not created by this module",
    "Schema Registry beyond the 100 schemas Essentials includes",
    "anything created outside this module",
    local.create_cluster ? "" : "the existing cluster's eCKUs (already on your bill)",
    local.cross_region ? "the cloud's internet egress for cross-region traffic" : "",
    "the cloud's own data transfer charges between the VM and Confluent's public endpoint",
  ])
}

check "budget_covers_the_ceiling" {
  assert {
    condition     = var.confluent_budget_monthly_usd == null || local.ceiling_monthly_usd <= coalesce(var.confluent_budget_monthly_usd, 0)
    error_message = "confluent_ceiling_monthly_usd (${local.ceiling_monthly_usd}, an estimate at list prices of ${local.tables.prices_read}) is above confluent_budget_monthly_usd (${coalesce(var.confluent_budget_monthly_usd, 0)}): lower max_ecku, partitions, retention or the connector rates."
  }
}
