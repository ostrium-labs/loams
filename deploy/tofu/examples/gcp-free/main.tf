# A minimal root module for ../../modules/gcp-free: an app on Google Cloud's free
# tier (design 71 §7.3). The budget API needs a quota project, hence
# user_project_override. Push the app's image first (the module's
# create_repository can make the repository: apply with -target, push, apply).

terraform {
  required_version = ">= 1.9"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 8.0"
    }
  }
}

variable "project_id" {
  type = string
}

variable "billing_account" {
  type = string
}

variable "image" {
  type = string
}

provider "google" {
  project               = var.project_id
  region                = "us-central1" # Q1105, a free GCS region
  billing_project       = var.project_id
  user_project_override = true
}

module "app" {
  source          = "../../modules/gcp-free"
  project_id      = var.project_id
  name            = "notes"
  billing_account = var.billing_account
  image           = var.image
}

output "url" {
  value = module.app.url
}

output "free_tier" {
  value = module.app.free_tier
}
