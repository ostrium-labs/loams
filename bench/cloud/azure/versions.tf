terraform {
  required_version = ">= 1.9"
  # Local state in the run's work directory (run.sh passes -backend-config=path=...).
  backend "local" {}
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.0"
    }
    tls = {
      source  = "hashicorp/tls"
      version = "~> 4.0"
    }
  }
}
