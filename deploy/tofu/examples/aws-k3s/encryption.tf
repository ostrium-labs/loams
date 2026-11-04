# State and plan encryption, enforced (D1827). This root's state holds the cluster
# CAs, the Access database password (or DSN) and the setup token, so it is never
# written or read unencrypted. Keep the passphrase outside this directory:
#   export TF_VAR_state_passphrase="$(openssl rand -base64 32)"   # keep it safe
# Losing it loses the state (the VM keeps running; `tofu import` can rebuild a root).
terraform {
  encryption {
    key_provider "pbkdf2" "passphrase" {
      passphrase = var.state_passphrase
    }
    method "aes_gcm" "state" {
      keys = key_provider.pbkdf2.passphrase
    }
    state {
      method   = method.aes_gcm.state
      enforced = true
    }
    plan {
      method   = method.aes_gcm.state
      enforced = true
    }
  }
}

variable "state_passphrase" {
  description = "At least 16 characters; keep it outside this directory (TF_VAR_state_passphrase)."
  type        = string
  sensitive   = true
}
