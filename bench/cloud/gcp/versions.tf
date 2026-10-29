terraform {
  required_version = ">= 1.9"
  # Local state in the run's work directory (run.sh passes -backend-config=path=...).
  backend "local" {}
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 8.0"
    }
  }
}
