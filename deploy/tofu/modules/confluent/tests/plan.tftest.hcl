# Offline tests (mock provider, no Confluent account): `tofu test` in the module directory.
# Mock defaults look like real ids because the provider checks references at plan time
# (CC0 ruling R0.12).

mock_provider "confluent" {
  mock_resource "confluent_environment" {
    defaults = {
      id            = "env-test01"
      resource_name = "crn://confluent.cloud/organization=org-test01/environment=env-test01"
    }
  }
  mock_resource "confluent_kafka_cluster" {
    defaults = {
      id                 = "lkc-test01"
      api_version        = "cmk/v2"
      kind               = "Cluster"
      rest_endpoint      = "https://pkc-test01.us-east-1.aws.confluent.cloud:443"
      bootstrap_endpoint = "SASL_SSL://pkc-test01.us-east-1.aws.confluent.cloud:9092"
      rbac_crn           = "crn://confluent.cloud/organization=org-test01/environment=env-test01/cloud-cluster=lkc-test01"
    }
  }
  mock_resource "confluent_service_account" {
    defaults = {
      id          = "sa-test01"
      api_version = "iam/v2"
      kind        = "ServiceAccount"
    }
  }
  mock_resource "confluent_api_key" {
    defaults = {
      id     = "TESTKEY01"
      secret = "not-a-secret"
    }
  }
  mock_data "confluent_environment" {
    defaults = {
      resource_name = "crn://confluent.cloud/organization=org-test01/environment=env-test02"
    }
  }
  mock_data "confluent_kafka_cluster" {
    defaults = {
      api_version        = "cmk/v2"
      kind               = "Cluster"
      cloud              = "AWS"
      region             = "us-east-1"
      rest_endpoint      = "https://pkc-test02.us-east-1.aws.confluent.cloud:443"
      bootstrap_endpoint = "pkc-test02.us-east-1.aws.confluent.cloud:9092"
      rbac_crn           = "crn://confluent.cloud/organization=org-test01/environment=env-test02/cloud-cluster=lkc-test02"
    }
  }
  mock_data "confluent_schema_registry_cluster" {
    defaults = {
      rest_endpoint = "https://psrc-test01.us-east-1.aws.confluent.cloud"
    }
  }
}

variables {
  name   = "t"
  cloud  = "aws"
  region = "us-east-1"
  topics = {
    "loams-t-events" = { partitions = 2, retention_bytes = 104857600 }
  }
}

run "defaults_basic_one_ecku_in_the_vms_region" {
  command = plan

  assert {
    condition     = length(confluent_kafka_cluster.this[0].basic) == 1 && length(confluent_kafka_cluster.this[0].standard) == 0
    error_message = "the default is a Basic cluster"
  }
  assert {
    condition     = confluent_kafka_cluster.this[0].basic[0].max_ecku == 1
    error_message = "max_ecku defaults to 1"
  }
  assert {
    condition     = confluent_kafka_cluster.this[0].cloud == "AWS" && confluent_kafka_cluster.this[0].region == "us-east-1"
    error_message = "the cluster is in the VM's cloud and region"
  }
  assert {
    condition     = confluent_kafka_cluster.this[0].availability == "SINGLE_ZONE"
    error_message = "single zone by default"
  }
  assert {
    condition     = length(confluent_environment.this) == 1 && confluent_environment.this[0].display_name == "loams-t" && confluent_environment.this[0].stream_governance[0].package == "ESSENTIALS"
    error_message = "an environment loams-t with Essentials is created when none is given"
  }
  assert {
    condition     = confluent_service_account.data[0].display_name == "loams-t-data"
    error_message = "the data account is loams-<name>-data"
  }
}

run "no_api_key_resource_by_default" {
  command = plan

  assert {
    condition     = length(confluent_api_key.data) == 0 && length(confluent_api_key.admin) == 0 && length(confluent_service_account.admin) == 0
    error_message = "no Kafka key is created (so none is in state) unless create_api_keys"
  }
  assert {
    condition     = alltrue([for t in confluent_kafka_topic.this : length(t.credentials) == 0]) && alltrue([for a in confluent_kafka_acl.data : length(a.credentials) == 0])
    error_message = "with kafka_admin = provider, topics and ACLs carry no credentials (the provider's KAFKA_* settings do)"
  }
}

run "deletion_protection_on_unless_ci" {
  command = plan

  assert {
    condition     = confluent_kafka_cluster.this[0].deletion_protection == true
    error_message = "a user's cluster has deletion protection"
  }
}

run "deletion_protection_off_for_ci" {
  command = plan
  variables {
    name = "loams-ci-12345"
  }
  assert {
    condition     = confluent_kafka_cluster.this[0].deletion_protection == false && confluent_kafka_cluster.this[0].display_name == "loams-ci-12345"
    error_message = "loams-ci-* clusters have no deletion protection, and the name is not doubled"
  }
}

run "refuses_premium_types_without_flag" {
  command = plan
  variables {
    cluster_type = "enterprise"
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_freight_without_flag" {
  command = plan
  variables {
    cluster_type = "freight"
    max_ecku     = 2
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_dedicated_without_flag" {
  command = plan
  variables {
    cluster_type = "dedicated"
  }
  expect_failures = [terraform_data.checks]
}

run "premium_with_flag" {
  command = plan
  variables {
    cluster_type                = "enterprise"
    allow_premium_cluster_types = true
  }
  assert {
    condition     = length(confluent_kafka_cluster.this[0].enterprise) == 1
    error_message = "enterprise is created when asked twice"
  }
}

run "standard_on_request" {
  command = plan
  variables {
    cluster_type = "standard"
  }
  assert {
    condition     = length(confluent_kafka_cluster.this[0].standard) == 1 && confluent_kafka_cluster.this[0].standard[0].max_ecku == 1
    error_message = "standard at max_ecku 1"
  }
}

run "refuses_cross_region_without_flag" {
  command = plan
  variables {
    cluster_region = "us-west-2"
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_cross_cloud_without_flag" {
  command = plan
  variables {
    cluster_cloud  = "gcp"
    cluster_region = "us-central1"
  }
  expect_failures = [terraform_data.checks]
}

run "cross_region_with_flag" {
  command = plan
  variables {
    cluster_region     = "us-west-2"
    allow_cross_region = true
  }
  assert {
    condition     = confluent_kafka_cluster.this[0].region == "us-west-2" && contains(output.confluent_unbounded, "the cloud's internet egress for cross-region traffic")
    error_message = "cross-region is allowed with the flag and listed as unbounded"
  }
}

run "refuses_region_without_the_type" {
  command = plan
  variables {
    cloud  = "gcp"
    region = "europe-north2" # Enterprise and Dedicated only (regions.toml)
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_unschedulable_region" {
  command = plan
  variables {
    region = "me-south-1"
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_unknown_region" {
  command = plan
  variables {
    region = "xx-nowhere-1"
  }
  expect_failures = [terraform_data.checks]
}

run "existing_cluster_in_another_region_is_refused" {
  command = plan
  variables {
    environment_id = "env-test02"
    cluster_id     = "lkc-test02"
    region         = "ap-south-1"
  }
  expect_failures = [terraform_data.checks]
}

run "existing_environment_and_cluster_mode_a" {
  command = plan
  variables {
    environment_id = "env-test02"
    cluster_id     = "lkc-test02"
    topics         = {}
    data_topics    = ["orders", "dbserver1.public.customers"]
  }
  assert {
    condition     = length(confluent_environment.this) == 0 && length(confluent_kafka_cluster.this) == 0
    error_message = "mode (a) creates no environment and no cluster"
  }
  assert {
    condition     = output.cluster_id == "lkc-test02" && output.bootstrap_endpoint == "pkc-test02.us-east-1.aws.confluent.cloud:9092"
    error_message = "mode (a) reads the existing cluster"
  }
  assert {
    condition     = confluent_kafka_acl.data["topic/orders/READ"].pattern_type == "LITERAL" && confluent_kafka_acl.data["topic/dbserver1.public.customers/WRITE"].resource_name == "dbserver1.public.customers"
    error_message = "the listed topics get LITERAL grants"
  }
  assert {
    condition     = output.confluent_ceiling.ecku_usd == 0 && contains(output.confluent_unbounded, "the existing cluster's eCKUs (already on your bill)")
    error_message = "an existing cluster's eCKUs are not in the ceiling and are listed as unbounded"
  }
  assert {
    condition     = confluent_role_binding.metrics[0].crn_pattern == "crn://confluent.cloud/organization=org-test01/environment=env-test02"
    error_message = "MetricsViewer is bound on the existing environment (R0.7)"
  }
}

run "every_topic_has_retention_bytes" {
  command = plan
  variables {
    topics = {
      "loams-t-a" = { partitions = 1, retention_bytes = 1048576 }
      "loams-t-b" = { partitions = 3, retention_bytes = 2097152, retention_ms = 3600000, cleanup_policy = "compact,delete" }
    }
  }
  assert {
    condition     = alltrue([for t in confluent_kafka_topic.this : tonumber(t.config["retention.bytes"]) > 0])
    error_message = "every topic carries retention.bytes"
  }
  assert {
    condition     = confluent_kafka_topic.this["__loams_checkpoints"].config["cleanup.policy"] == "compact" && confluent_kafka_topic.this["__loams_checkpoints"].partitions_count == 1
    error_message = "the checkpoint topic is compacted with one partition"
  }
}

run "refuses_unbounded_retention" {
  command = plan
  variables {
    topics = {
      "loams-t-a" = { partitions = 1, retention_bytes = -1 }
    }
  }
  expect_failures = [var.topics]
}

run "data_account_acls_are_minimal" {
  command = plan

  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : contains(["READ", "WRITE", "DESCRIBE", "DESCRIBE_CONFIGS", "IDEMPOTENT_WRITE"], a.operation)])
    error_message = "the data account gets only READ, WRITE, DESCRIBE, DESCRIBE_CONFIGS and IDEMPOTENT_WRITE: never CREATE, DELETE, ALTER, ALTER_CONFIGS, ALL or an ACL operation"
  }
  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : a.permission == "ALLOW" && a.principal == "User:sa-test01" && a.host == "*"])
    error_message = "ALLOW for the data account only"
  }
  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : a.resource_type != "TOPIC" || a.pattern_type == "LITERAL"])
    error_message = "topic grants are LITERAL (named topics only)"
  }
  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : a.resource_type != "GROUP" || (a.pattern_type == "PREFIXED" && a.resource_name == "loams-t-" && contains(["READ", "DESCRIBE"], a.operation))])
    error_message = "groups: READ and DESCRIBE, PREFIXED to the configured prefix"
  }
  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : a.resource_type != "TRANSACTIONAL_ID" || (a.pattern_type == "PREFIXED" && a.resource_name == "loams-t-" && contains(["WRITE", "DESCRIBE"], a.operation))])
    error_message = "transactional ids: WRITE and DESCRIBE, PREFIXED to the configured prefix"
  }
  assert {
    condition     = alltrue([for a in confluent_kafka_acl.data : a.resource_type != "CLUSTER" || a.operation == "IDEMPOTENT_WRITE"])
    error_message = "the only cluster grant is IDEMPOTENT_WRITE"
  }
  assert {
    condition     = length([for a in confluent_kafka_acl.data : a if a.resource_type == "TOPIC"]) == 8
    error_message = "four grants for each of the two topics (events, checkpoints)"
  }
}

run "prefixes_and_existing_data_account" {
  command = plan
  variables {
    name                    = "loams-ci-777"
    data_service_account_id = "sa-ci0data"
    create_metrics_account  = false
    consumer_group_prefix   = "loams-ci-777-g-"
    transactional_id_prefix = "loams-ci-777-tx-"
  }
  assert {
    condition     = length(confluent_service_account.data) == 0 && length(confluent_service_account.metrics) == 0
    error_message = "with an existing data account and no metrics account, the module creates no service account (CI: R0.6)"
  }
  assert {
    condition     = confluent_kafka_acl.data["group/READ"].resource_name == "loams-ci-777-g-" && confluent_kafka_acl.data["txn/WRITE"].resource_name == "loams-ci-777-tx-" && confluent_kafka_acl.data["group/READ"].principal == "User:sa-ci0data"
    error_message = "the prefixes and the existing account are used"
  }
}

run "kafka_admin_none_creates_no_topic_or_acl" {
  command = plan
  variables {
    kafka_admin = "none"
  }
  assert {
    condition     = length(confluent_kafka_topic.this) == 0 && length(confluent_kafka_acl.data) == 0 && length(confluent_kafka_cluster.this) == 1
    error_message = "phase one creates the cluster and accounts only"
  }
}

run "module_key_needs_create_api_keys" {
  command = plan
  variables {
    kafka_admin = "module_key"
  }
  expect_failures = [terraform_data.checks]
}

run "module_key_with_keys" {
  command = plan
  variables {
    kafka_admin     = "module_key"
    create_api_keys = true
  }
  assert {
    condition     = length(confluent_api_key.admin) == 1 && length(confluent_api_key.data) == 1 && confluent_role_binding.admin[0].role_name == "CloudClusterAdmin"
    error_message = "module_key creates the admin account, its binding and both keys"
  }
}

run "refuses_partitions_over_type_limit" {
  command = plan
  variables {
    topics = {
      "loams-t-a" = { partitions = 20, retention_bytes = 1048576 }
      "loams-t-b" = { partitions = 10, retention_bytes = 1048576 }
    }
  }
  # 30 + the checkpoint topic's 1 > 30 at Basic max_ecku 1 (R0.13)
  expect_failures = [terraform_data.checks]
}

run "partitions_at_the_limit" {
  command = plan
  variables {
    topics = {
      "loams-t-a" = { partitions = 29, retention_bytes = 1048576 }
    }
  }
  assert {
    condition     = length(confluent_kafka_topic.this) == 2
    error_message = "29 + 1 partitions fit Basic at max_ecku 1"
  }
}

run "refuses_multi_zone_basic" {
  command = plan
  variables {
    availability = "MULTI_ZONE"
  }
  expect_failures = [terraform_data.checks]
}

run "refuses_privatelink_in_cc1" {
  command = plan
  variables {
    confluent_networking = "privatelink"
  }
  expect_failures = [var.confluent_networking]
}

run "ceiling_is_computed" {
  command = plan
  variables {
    topics = {
      # 2 partitions x 1 GiB, x3 replication = 6 GB; checkpoints 100 MiB x3
      "loams-t-events" = { partitions = 2, retention_bytes = 1073741824 }
    }
    connector_bytes_per_sec = { in = 1024, out = 2048 }
  }
  assert {
    condition     = output.confluent_ceiling.ecku_usd == 0
    error_message = "Basic's first eCKU is free"
  }
  assert {
    condition     = abs(output.confluent_ceiling.storage_gb - (6 + 0.09765625 * 3)) < 0.0001
    error_message = "storage counts every partition at retention.bytes times the replication factor"
  }
  assert {
    # 1024 B/s x 2,628,000 s / 2^30 x $0.05 = $0.1253; 2048 B/s -> $0.2506; storage 6.29296875 GB x $0.08 = $0.5034
    condition     = output.confluent_ceiling_monthly_usd == 0.88
    error_message = "the ceiling is eCKU + storage + Loams' rates at list prices (got ${output.confluent_ceiling_monthly_usd})"
  }
  assert {
    condition     = startswith(output.confluent_ceiling.label, "estimate (list prices of 2026-")
    error_message = "the ceiling is labelled an estimate with the price table's date"
  }
  assert {
    condition     = !contains(output.confluent_unbounded, "Loams' own connector traffic (no connector_bytes_per_sec given)")
    error_message = "with rates given, Loams' traffic is bounded"
  }
}

run "ceiling_standard_ecku" {
  command = plan
  variables {
    cluster_type = "standard"
    max_ecku     = 2
    topics       = {}
  }
  assert {
    condition     = output.confluent_ceiling.ecku_usd == 1095 && contains(output.confluent_unbounded, "Loams' own connector traffic (no connector_bytes_per_sec given)")
    error_message = "Standard at 2 eCKU is 2 x $0.75 x 730 = $1,095; no rates means unbounded traffic"
  }
}

run "bootstrap_and_secret_names" {
  command = plan
  variables {
    environment_id = "env-test02"
    cluster_id     = "lkc-test02"
    secret_prefix  = "loams/prod"
  }
  assert {
    condition     = output.secret_names["api-secret"] == "loams/prod/confluent-confluent-api-secret"
    error_message = "AWS secret names keep the / of the prefix"
  }
  assert {
    condition     = output.connect_integration.sasl_password_ref == "confluent-confluent/password" && output.connect_integration.security_protocol == "sasl_ssl"
    error_message = "the connect integration names the secret file, never a value"
  }
}

run "gcp_secret_names_have_no_slash" {
  command = plan
  variables {
    cloud         = "gcp"
    region        = "us-central1"
    secret_prefix = "loams/prod"
    integration   = "events"
  }
  assert {
    condition     = output.secret_names["api-key"] == "loams-prod-confluent-events-api-key"
    error_message = "Google Cloud and Azure names use - for /"
  }
}
