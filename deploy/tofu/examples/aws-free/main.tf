# A minimal root module for ../../modules/aws-free: an app on AWS's always-free
# tier (design 71 §7.2). Build the app's Lambda zip first (the app bundled with the
# Lambda entry shim), then:
#   tofu init && tofu apply -var package_path=dist/aws.zip -var 'budget_emails=["you@example.com"]'

terraform {
  required_version = ">= 1.9"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

variable "region" {
  type    = string
  default = "us-east-1" # Q1105
}

variable "package_path" {
  type = string
}

variable "budget_emails" {
  type = list(string)
}

provider "aws" {
  region = var.region
}

module "app" {
  source        = "../../modules/aws-free"
  name          = "notes"
  package_path  = var.package_path
  budget_emails = var.budget_emails
}

output "url" {
  value = module.app.url
}

output "free_tier" {
  value = module.app.free_tier
}
