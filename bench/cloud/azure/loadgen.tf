# The in-region loadgen (plan §5.4): Standard_D2as_v5 on Ubuntu 24.04 in its own
# VNet, an NSG with no inbound rule (the defaults deny Internet ingress), a
# Standard public IP for egress (no NAT gateway). Its system identity may change
# only the function app's settings (the cold scenario), write the results
# container, and deallocate its own VM: an OS shutdown alone leaves an Azure VM
# billed, so the boot script deallocates at the end and at the deadline.

resource "azurerm_virtual_network" "loadgen" {
  name                = "${local.name}-loadgen"
  resource_group_name = local.rg
  location            = var.location
  address_space       = ["10.91.0.0/24"]
  tags                = local.tags
  depends_on          = [module.app] # the budget first (D1402)
}

resource "azurerm_subnet" "loadgen" {
  name                 = "loadgen"
  resource_group_name  = local.rg
  virtual_network_name = azurerm_virtual_network.loadgen.name
  address_prefixes     = ["10.91.0.0/26"]
}

resource "azurerm_network_security_group" "loadgen" {
  name                = "${local.name}-loadgen"
  resource_group_name = local.rg
  location            = var.location
  tags                = local.tags
  # no security_rule: the default rules deny inbound from the Internet
  depends_on = [module.app] # the budget first (D1402)
}

resource "azurerm_subnet_network_security_group_association" "loadgen" {
  subnet_id                 = azurerm_subnet.loadgen.id
  network_security_group_id = azurerm_network_security_group.loadgen.id
}

resource "azurerm_public_ip" "loadgen" {
  name                = "${local.name}-loadgen"
  resource_group_name = local.rg
  location            = var.location
  allocation_method   = "Static"
  sku                 = "Standard"
  tags                = local.tags
  depends_on          = [module.app] # the budget first (D1402)
}

resource "azurerm_network_interface" "loadgen" {
  name                = "${local.name}-loadgen"
  resource_group_name = local.rg
  location            = var.location
  tags                = local.tags
  ip_configuration {
    name                          = "primary"
    subnet_id                     = azurerm_subnet.loadgen.id
    private_ip_address_allocation = "Dynamic"
    public_ip_address_id          = azurerm_public_ip.loadgen.id
  }
}

# Linux VMs need an admin key; nothing can reach port 22, and the key lives
# only in the run's local state, which is deleted with the run.
resource "tls_private_key" "loadgen" {
  algorithm = "ED25519"
}

locals {
  function_app_id = "/subscriptions/${data.azurerm_client_config.current.subscription_id}/resourceGroups/${local.rg}/providers/Microsoft.Web/sites/${module.app.function_app_name}"
  loadgen_files = {
    for f in ["run-scenarios.sh", "cold-azure.sh", "upload-azure.sh"] :
    f => base64gzip(file("${path.module}/../loadgen/${f}"))
  }
  loadgen_env = merge(var.bench_env, {
    BENCH_CLOUD              = "azure"
    BENCH_RUN_ID             = var.run_id
    BENCH_ROW                = var.row
    BENCH_URL                = module.app.url
    BENCH_REGION             = var.location
    BENCH_INSTANCE_TYPE      = var.loadgen_vm_size
    BENCH_COMMIT             = var.commit
    BENCH_FUNCTION_APP_ID    = local.function_app_id
    BENCH_BLOB_CONTAINER_URL = "${data.azurerm_storage_account.app.primary_blob_endpoint}${azurerm_storage_container.results.name}"
  })
}

resource "azurerm_linux_virtual_machine" "loadgen" {
  name                            = "${local.name}-loadgen"
  resource_group_name             = local.rg
  location                        = var.location
  size                            = var.loadgen_vm_size
  admin_username                  = "bench"
  disable_password_authentication = true
  network_interface_ids           = [azurerm_network_interface.loadgen.id]
  tags                            = local.tags

  admin_ssh_key {
    username   = "bench"
    public_key = tls_private_key.loadgen.public_key_openssh
  }

  os_disk {
    caching              = "ReadWrite"
    storage_account_type = "StandardSSD_LRS"
    disk_size_gb         = 30
  }

  source_image_reference {
    publisher = "Canonical"
    offer     = "ubuntu-24_04-lts"
    sku       = "server"
    version   = "latest"
  }

  identity {
    type = "SystemAssigned"
  }

  custom_data = base64encode(templatefile("${path.module}/../loadgen/boot.sh.tftpl", {
    cloud       = "azure"
    max_minutes = var.max_minutes
    oha_url     = var.oha_url
    oha_sha256  = var.oha_sha256
    files       = local.loadgen_files
    env         = local.loadgen_env
  }))

  depends_on = [module.app, azurerm_subnet_network_security_group_association.loadgen]
}

# Role assignments take minutes to propagate; the loadgen's first use of each is
# after the readiness wait and the preload, and every use is logged if refused.
resource "azurerm_role_assignment" "loadgen_cold" {
  scope                = local.function_app_id
  role_definition_name = "Website Contributor"
  principal_id         = azurerm_linux_virtual_machine.loadgen.identity[0].principal_id
}

resource "azurerm_role_assignment" "loadgen_results" {
  scope                = azurerm_storage_container.results.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_linux_virtual_machine.loadgen.identity[0].principal_id
}

resource "azurerm_role_assignment" "loadgen_deallocate" {
  scope                = azurerm_linux_virtual_machine.loadgen.id
  role_definition_name = "Virtual Machine Contributor"
  principal_id         = azurerm_linux_virtual_machine.loadgen.identity[0].principal_id
}
