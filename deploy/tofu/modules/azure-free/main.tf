# App BYOC on Azure's free allowances (design 71 §7.4, D1395, D1400, D1402):
# Functions on Flex Consumption (on-demand only), a Cosmos DB free-tier account
# capped at its free throughput, Queue Storage, optionally a blobs container and
# Application Insights with a daily cap. One resource group; its budget is created
# first and every other resource depends on it. No VNet, no NAT, no public IP.
#
# The provider needs no storage keys: containers and queues are created through
# Azure Resource Manager, and the app reaches storage and Cosmos DB with a
# user-assigned identity whose roles exist before the app is created.


resource "random_id" "suffix" {
  byte_length = 4
}

locals {
  fullname = "loams-${var.name}"
  suffix   = random_id.suffix.hex
  tags = merge(
    var.tags,
    { "loams-app" = var.name, "loams-managed-by" = "opentofu" },
    var.run_id == null ? {} : { "loams-bench" = var.run_id },
  )
  # Storage account names: 3 to 24 lowercase letters and digits, globally unique.
  storage_name      = "loams${substr(replace(var.name, "-", ""), 0, 11)}${local.suffix}"
  queue_name        = "${local.fullname}-queue"
  package_container = "app-package"
}

resource "azurerm_resource_group" "this" {
  name     = "rg-${local.fullname}"
  location = var.location
  tags     = local.tags
}

# --- the budget, first (D1402) -------------------------------------------------
resource "azurerm_consumption_budget_resource_group" "this" {
  name              = "${local.fullname}-budget"
  resource_group_id = azurerm_resource_group.this.id
  amount            = var.budget_amount
  time_grain        = "Monthly"

  time_period {
    start_date = formatdate("YYYY-MM-01'T'00:00:00Z", plantimestamp())
  }

  dynamic "notification" {
    for_each = [50, 80, 100]
    content {
      enabled        = true
      operator       = "GreaterThan"
      threshold      = notification.value
      threshold_type = "Actual"
      contact_emails = var.budget_emails
    }
  }

  notification {
    enabled        = true
    operator       = "GreaterThan"
    threshold      = 100
    threshold_type = "Forecasted"
    contact_emails = var.budget_emails
  }

  lifecycle {
    ignore_changes = [time_period] # the start month is fixed at creation
  }
}

# --- the app's identity and storage ------------------------------------------------
resource "azurerm_user_assigned_identity" "app" {
  name                = "id-${local.fullname}"
  resource_group_name = azurerm_resource_group.this.name
  location            = var.location
  tags                = local.tags
  depends_on          = [azurerm_consumption_budget_resource_group.this]
}

resource "azurerm_storage_account" "this" {
  name                             = local.storage_name
  resource_group_name              = azurerm_resource_group.this.name
  location                         = var.location
  account_tier                     = "Standard"
  account_replication_type         = "LRS"
  account_kind                     = "StorageV2"
  min_tls_version                  = "TLS1_2"
  shared_access_key_enabled        = false # identity only
  default_to_oauth_authentication  = true
  allow_nested_items_to_be_public  = false
  cross_tenant_replication_enabled = false
  tags                             = local.tags
  depends_on                       = [azurerm_consumption_budget_resource_group.this]
}

resource "azurerm_storage_container" "package" {
  name                  = local.package_container
  storage_account_id    = azurerm_storage_account.this.id
  container_access_type = "private"
}

resource "azurerm_storage_container" "blobs" {
  count                 = var.create_blobs_container ? 1 : 0
  name                  = "blobs"
  storage_account_id    = azurerm_storage_account.this.id
  container_access_type = "private"
}

resource "azurerm_storage_queue" "this" {
  count              = var.create_queue ? 1 : 0
  name               = local.queue_name
  storage_account_id = azurerm_storage_account.this.id
}

resource "azurerm_role_assignment" "storage" {
  for_each             = toset(["Storage Blob Data Owner", "Storage Queue Data Contributor", "Storage Table Data Contributor"])
  scope                = azurerm_storage_account.this.id
  role_definition_name = each.value
  principal_id         = azurerm_user_assigned_identity.app.principal_id
  principal_type       = "ServicePrincipal"
}

# --- items: Cosmos DB free tier, capped at its free throughput ---------------------
resource "azurerm_cosmosdb_account" "this" {
  name                          = "${local.fullname}-${local.suffix}"
  resource_group_name           = azurerm_resource_group.this.name
  location                      = var.location
  offer_type                    = "Standard"
  kind                          = "GlobalDocumentDB"
  free_tier_enabled             = var.cosmos_free_tier
  local_authentication_enabled  = false # Entra ID only, no keys
  automatic_failover_enabled    = false
  public_network_access_enabled = true
  tags                          = local.tags

  capacity {
    total_throughput_limit = var.cosmos_throughput # the account can never provision more
  }

  consistency_policy {
    consistency_level = "Session"
  }

  geo_location {
    location          = var.location
    failover_priority = 0
  }

  # The periodic default's two copies are free; keep them local, nothing more.
  backup {
    type                = "Periodic"
    interval_in_minutes = 1440
    retention_in_hours  = 48
    storage_redundancy  = "Local"
  }

  depends_on = [azurerm_consumption_budget_resource_group.this]
}

resource "azurerm_cosmosdb_sql_database" "this" {
  name                = "loams"
  resource_group_name = azurerm_resource_group.this.name
  account_name        = azurerm_cosmosdb_account.this.name
  throughput          = var.cosmos_throughput # shared, fixed: the free tier's 1,000 RU/s
}

resource "azurerm_cosmosdb_sql_container" "items" {
  name                = "${local.fullname}-items"
  resource_group_name = azurerm_resource_group.this.name
  account_name        = azurerm_cosmosdb_account.this.name
  database_name       = azurerm_cosmosdb_sql_database.this.name
  partition_key_paths = ["/pk"]
  default_ttl         = -1 # TTL on, per item (design 71 §6.1)
}

# Cosmos DB Built-in Data Contributor.
resource "azurerm_cosmosdb_sql_role_assignment" "app" {
  resource_group_name = azurerm_resource_group.this.name
  account_name        = azurerm_cosmosdb_account.this.name
  role_definition_id  = "${azurerm_cosmosdb_account.this.id}/sqlRoleDefinitions/00000000-0000-0000-0000-000000000002"
  principal_id        = azurerm_user_assigned_identity.app.principal_id
  scope               = azurerm_cosmosdb_account.this.id
}

# Role assignments take a minute or two to reach the data plane; the app's first
# deploy writes its package with this identity.
resource "time_sleep" "roles" {
  create_duration = "90s"
  depends_on      = [azurerm_role_assignment.storage, azurerm_cosmosdb_sql_role_assignment.app]
}

# --- logs: Application Insights with a daily cap, optional --------------------------
resource "azurerm_log_analytics_workspace" "this" {
  count               = var.create_app_insights ? 1 : 0
  name                = "log-${local.fullname}"
  resource_group_name = azurerm_resource_group.this.name
  location            = var.location
  sku                 = "PerGB2018"
  retention_in_days   = 30 # included in the price; the minimum
  daily_quota_gb      = var.log_daily_cap_gb
  tags                = local.tags
  depends_on          = [azurerm_consumption_budget_resource_group.this]
}

resource "azurerm_application_insights" "this" {
  count                = var.create_app_insights ? 1 : 0
  name                 = "appi-${local.fullname}"
  resource_group_name  = azurerm_resource_group.this.name
  location             = var.location
  workspace_id         = azurerm_log_analytics_workspace.this[0].id
  application_type     = "Node.JS"
  daily_data_cap_in_gb = var.log_daily_cap_gb
  retention_in_days    = 30
  tags                 = local.tags
}

# --- compute: Functions on Flex Consumption ---------------------------------------
resource "azurerm_service_plan" "this" {
  name                = "asp-${local.fullname}"
  resource_group_name = azurerm_resource_group.this.name
  location            = var.location
  os_type             = "Linux"
  sku_name            = "FC1"
  tags                = local.tags
  depends_on          = [azurerm_consumption_budget_resource_group.this]
}

resource "azurerm_function_app_flex_consumption" "this" {
  name                = "${local.fullname}-${local.suffix}"
  resource_group_name = azurerm_resource_group.this.name
  location            = var.location
  service_plan_id     = azurerm_service_plan.this.id

  storage_container_type            = "blobContainer"
  storage_container_endpoint        = "${azurerm_storage_account.this.primary_blob_endpoint}${azurerm_storage_container.package.name}"
  storage_authentication_type       = "UserAssignedIdentity"
  storage_user_assigned_identity_id = azurerm_user_assigned_identity.app.id

  runtime_name           = "node"
  runtime_version        = var.runtime_version
  instance_memory_in_mb  = var.instance_memory_mb
  maximum_instance_count = var.max_instances # the cap; no always-ready instances
  http_concurrency       = var.http_concurrency
  zip_deploy_file        = var.package_path

  https_only                                     = true
  public_network_access_enabled                  = true
  webdeploy_publish_basic_authentication_enabled = false

  identity {
    type         = "UserAssigned"
    identity_ids = [azurerm_user_assigned_identity.app.id]
  }

  app_settings = merge(var.environment, {
    AzureWebJobsStorage__accountName = azurerm_storage_account.this.name
    AzureWebJobsStorage__credential  = "managedidentity"
    AzureWebJobsStorage__clientId    = azurerm_user_assigned_identity.app.client_id
    LoamsQueue__queueServiceUri      = azurerm_storage_account.this.primary_queue_endpoint
    LoamsQueue__credential           = "managedidentity"
    LoamsQueue__clientId             = azurerm_user_assigned_identity.app.client_id
    LOAMS_APP_TARGET                 = "azure"
    LOAMS_IDENTITY_CLIENT_ID         = azurerm_user_assigned_identity.app.client_id
    LOAMS_COSMOS_ENDPOINT            = azurerm_cosmosdb_account.this.endpoint
    LOAMS_COSMOS_DATABASE            = azurerm_cosmosdb_sql_database.this.name
    LOAMS_ITEMS                      = azurerm_cosmosdb_sql_container.items.name
    LOAMS_QUEUE                      = var.create_queue ? local.queue_name : ""
    LOAMS_QUEUE_ENDPOINT             = azurerm_storage_account.this.primary_queue_endpoint
    LOAMS_BLOBS_CONTAINER            = var.create_blobs_container ? "blobs" : ""
    LOAMS_BLOB_ENDPOINT              = azurerm_storage_account.this.primary_blob_endpoint
  })

  site_config {
    minimum_tls_version                    = "1.2"
    application_insights_connection_string = var.create_app_insights ? azurerm_application_insights.this[0].connection_string : null
  }

  tags       = local.tags
  depends_on = [time_sleep.roles]
}
