terraform {
  required_version = ">= 1.9"
  # Local state in the run's work directory (run.sh passes -backend-config=path=...).
  backend "local" {}
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}
