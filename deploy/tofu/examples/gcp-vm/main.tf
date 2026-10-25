# A minimal root module for ../../modules/gcp-vm. Before `tofu apply`, enable the
# APIs (or set enable_apis) and create the Tailscale auth key's secret yourself
# (never in a variable or in this directory):
#   printf %s "$TS_AUTHKEY" | gcloud secrets create loams-prod-tailnet-authkey --data-file=- --project "$PROJECT"

terraform {
  required_version = ">= 1.6"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 8.0"
    }
  }
}

variable "project" {
  type = string
}

provider "google" {
  project = var.project
  region  = "us-central1"
}

module "loams" {
  source      = "../../modules/gcp-vm"
  project     = var.project
  name        = "prod"
  enable_apis = true
}

output "bucket_url" {
  value = module.loams.bucket_url
}

output "instance_name" {
  value = module.loams.instance_name
}

output "estimated_monthly_usd" {
  value = module.loams.estimated_monthly_usd
}
