# State and plan encryption on your own KMS key, enforced: OpenTofu refuses to write
# either in plain text. Required whenever create_api_keys = true (the Kafka key is
# then in state, D1827), and on by default here because the state also holds the
# first-run setup token.
#
# The key must exist before the first `tofu init` (it cannot be one this root
# creates): for example
#   aws kms create-key --description "OpenTofu state for Loams prod" --query KeyMetadata.KeyId
# and pass its id or ARN as state_kms_key_id. Keep the state itself where you keep
# other state (this example configures no backend).
terraform {
  encryption {
    key_provider "aws_kms" "install" {
      kms_key_id = var.state_kms_key_id
      region     = var.region
      key_spec   = "AES_256"
    }
    method "aes_gcm" "install" {
      keys = key_provider.aws_kms.install
    }
    state {
      method   = method.aes_gcm.install
      enforced = true
    }
    plan {
      method   = method.aes_gcm.install
      enforced = true
    }
  }
}

variable "state_kms_key_id" {
  description = "The KMS key (id, alias or ARN) that encrypts this root's state and plans."
  type        = string
}
