output "environment_id" {
  description = "The Confluent environment."
  value       = local.env_id
}

output "cluster_id" {
  description = "The Kafka cluster (lkc-...)."
  value       = local.cluster.id
}

output "cluster_type" {
  description = "basic, standard, enterprise, freight or dedicated."
  value       = local.cluster_type
}

output "cluster_region" {
  description = "<cloud>/<region> of the cluster."
  value       = "${local.cluster_cloud}/${local.cluster_region}"
}

output "bootstrap_endpoint" {
  description = "host:port for the Kafka clients (without the SASL_SSL:// some API versions prefix, CC0 ruling R0.12)."
  value       = trimprefix(local.cluster.bootstrap, "SASL_SSL://")
}

output "rest_endpoint" {
  description = "The cluster's REST endpoint (KAFKA_REST_ENDPOINT for kafka_admin = provider)."
  value       = local.cluster.rest_endpoint
}

output "schema_registry_endpoint" {
  description = "The environment's Schema Registry, or null."
  value       = try(data.confluent_schema_registry_cluster.this[0].rest_endpoint, null)
}

output "data_service_account_id" {
  description = "The account Loams' Kafka key belongs to (pass it to `loams byoc confluent key create --service-account`)."
  value       = local.data_sa.id
}

output "metrics_service_account_id" {
  description = "The budget guard's MetricsViewer account, or null."
  value       = try(confluent_service_account.metrics[0].id, null)
}

output "ops_service_account_id" {
  description = "The opt-in max_ecku throttle's account, or null."
  value       = try(confluent_service_account.ops[0].id, null)
}

output "topics" {
  description = "The topics this module manages, with their partitions and retention."
  value       = local.managed_topics
}

output "consumer_group_prefix" {
  description = "Consumer groups Loams may use (PREFIXED ACL)."
  value       = local.group_prefix
}

output "transactional_id_prefix" {
  description = "Transactional ids Loams may use (PREFIXED ACL)."
  value       = local.txn_prefix
}

output "checkpoint_topic" {
  description = "The transactional sink's checkpoint topic, or null."
  value       = var.exactly_once_sinks ? var.checkpoint_topic : null
}

output "secret_names" {
  description = "Where the install's Confluent secrets live in the cloud secret store (names only, never values). `loams byoc confluent key create` writes them; the VM's fetcher reads them."
  value       = local.secret_names
}

output "connect_integration" {
  description = "What a `loams-fabric connect` Kafka instance needs for this integration: endpoint, secret references (files under the connect role's secret root), and the prefixes its ACLs allow."
  value = {
    integration             = var.integration
    bootstrap_endpoint      = trimprefix(local.cluster.bootstrap, "SASL_SSL://")
    security_protocol       = "sasl_ssl"
    sasl_mechanism          = "plain"
    sasl_username_ref       = "confluent-${var.integration}/username"
    sasl_password_ref       = "confluent-${var.integration}/password"
    schema_registry_url     = try(data.confluent_schema_registry_cluster.this[0].rest_endpoint, null)
    consumer_group_prefix   = local.group_prefix
    transactional_id_prefix = local.txn_prefix
    checkpoint_topic        = var.exactly_once_sinks ? var.checkpoint_topic : null
    topics                  = local.data_topic_names
  }
}

output "confluent_ceiling_monthly_usd" {
  description = "What this configuration bounds, per month, in USD: an estimate at list prices (see confluent_ceiling for the parts and the price table's date). Not a cap on your bill: see confluent_unbounded."
  value       = local.ceiling_monthly_usd
}

output "confluent_ceiling" {
  description = "The ceiling's parts and its label."
  value = {
    label       = "estimate (list prices of ${local.tables.prices_read}, ${local.tables.prices_source})"
    ecku_usd    = local.ceiling_ecku
    storage_usd = local.ceiling_storage
    storage_gb  = local.storage_gb
    in_usd      = local.ceiling_in
    out_usd     = local.ceiling_out
    total_usd   = local.ceiling_monthly_usd
    budget_usd  = var.confluent_budget_monthly_usd
  }
}

output "confluent_unbounded" {
  description = "What the ceiling does not bound (design 80 §8.1)."
  value       = local.unbounded
}

output "data_api_key" {
  description = "The data account's Kafka key id and secret, only with create_api_keys = true (then it is in your encrypted state). Write it to the secret store; never print it."
  value       = var.create_api_keys ? { id = confluent_api_key.data[0].id, secret = confluent_api_key.data[0].secret } : null
  sensitive   = true
}
