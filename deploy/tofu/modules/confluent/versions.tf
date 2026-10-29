terraform {
  # 1.11: the examples' state and plan encryption with `enforced = true` (D1827).
  required_version = ">= 1.11"
  required_providers {
    # Apache-2.0, from the OpenTofu registry (design 80 §5.1). The examples pin the
    # exact release (at least 14 days old, CC0 ruling R0.12); Dependabot moves it.
    confluent = {
      source  = "confluentinc/confluent"
      version = ">= 2.90.0, < 3.0.0"
    }
  }
}
