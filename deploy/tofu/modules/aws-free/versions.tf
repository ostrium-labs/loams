terraform {
  # 1.9 for validations that read other variables (the free-tier refusals).
  required_version = ">= 1.9"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}
