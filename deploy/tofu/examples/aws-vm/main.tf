# A minimal root module for ../../modules/aws-vm. Before `tofu apply`, create the
# Tailscale auth key's secret yourself (never in a variable or in this directory):
#   aws secretsmanager create-secret --name loams/prod/tailnet-authkey --secret-string "$TS_AUTHKEY"

terraform {
  required_version = ">= 1.6"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

variable "region" {
  type    = string
  default = "us-east-1"
}

provider "aws" {
  region = var.region
}

module "loams" {
  source        = "../../modules/aws-vm"
  name          = "prod"
  secret_prefix = "loams/prod"
}

output "bucket_url" {
  value = module.loams.bucket_url
}

output "instance_id" {
  value = module.loams.instance_id
}

output "estimated_monthly_usd" {
  value = module.loams.estimated_monthly_usd
}
