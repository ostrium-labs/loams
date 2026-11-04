variable "name" {
  description = "Names the instance, role, security group and (by default) the bucket; the deployment's name."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,30}$", var.name))
    error_message = "name is lowercase letters, digits and - (up to 31)."
  }
}

variable "release" {
  description = "The Loams release to install (also the operator chart's tag). The bundle's signature is checked on the VM."
  type        = string
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release is a version such as 0.1.4, never latest."
  }
}

variable "size" {
  description = "small (t3.large, 2 vCPU and 8 GiB) or medium (t3.xlarge, 4 vCPU and 16 GiB)."
  type        = string
  default     = "small"
  validation {
    condition     = contains(["small", "medium"], var.size)
    error_message = "size is small or medium."
  }
}

variable "home_spec" {
  description = "The home LoamsCluster's spec (the operator chart's home.spec). Keep it small: it travels in the VM's user data (16 KB)."
  type        = any
}

variable "operator_values" {
  description = "Other values for the operator chart. Keep them small: they travel in the VM's user data (16 KB)."
  type        = any
  default     = {}
}

variable "access_db" {
  description = "local (a Postgres StatefulSet in the home) or external (access_db_dsn)."
  type        = string
  default     = "local"
  validation {
    condition     = contains(["local", "external"], var.access_db)
    error_message = "access_db is local or external."
  }
}

variable "access_db_dsn" {
  description = "The managed Postgres DSN when access_db is external. It is stored in the <prefix>/access-db secret and in this configuration's (encrypted) state."
  type        = string
  default     = null
  sensitive   = true
}

variable "chart_digest" {
  description = "Pins the operator chart by its manifest digest (sha256:<64 hex>). Empty means not pinned; the bootstrap records the digest it saw."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^(sha256:[0-9a-f]{64})?$", var.chart_digest))
    error_message = "chart_digest is sha256:<64 hex digits>, or empty."
  }
}

variable "tailnet_domain" {
  description = "Your tailnet's DNS name, for example tail1234.ts.net. The API is https://loams-<name>.<tailnet_domain>:6443."
  type        = string
}

variable "region" {
  description = "The region the AWS provider is set to, or null. It cannot change the provider's region; a value that differs fails the plan."
  type        = string
  default     = null
}

variable "instance_type" {
  description = "Overrides size's instance type (x86-64 only until every image is arm64, D1384)."
  type        = string
  default     = null
}

variable "disk_size_gb" {
  description = "The encrypted gp3 root volume, which holds k3s's data (/var/lib/rancher)."
  type        = number
  default     = 60
  validation {
    condition     = var.disk_size_gb >= 30 && var.disk_size_gb <= 16000
    error_message = "disk_size_gb is between 30 and 16000."
  }
}

variable "bucket" {
  description = "An existing bucket to use. null creates one (versioned, encrypted, private) named loams-<name>-<account id>."
  type        = string
  default     = null
}

variable "kms_key_arn" {
  description = "An existing KMS key for the bucket and the secrets. null creates a key when the module creates the bucket; with an existing bucket, null means the module grants no KMS permission (the bucket uses SSE-S3 or its own key's policy) and the secrets use the AWS-managed key."
  type        = string
  default     = null
}

variable "secret_prefix" {
  description = "The Secrets Manager prefix; the VM reads <prefix>/tailnet-authkey (must exist: create it with your Tailscale auth key, never in this module's variables), <prefix>/k3s-ca, <prefix>/access-db and <prefix>/setup-token. null is loams/<name>."
  type        = string
  default     = null
}

variable "create_setup_token" {
  description = "Create <prefix>/setup-token with a random value. The value is in this configuration's state; set false to create the secret yourself."
  type        = bool
  default     = true
}

variable "ssm" {
  description = "Attach AmazonSSMManagedInstanceCore, for a shell through Session Manager without opening a port."
  type        = bool
  default     = true
}

variable "subnet_id" {
  description = "The subnet. null uses a subnet of the default VPC. The instance needs outbound internet (a public IP is attached) and no inbound rule."
  type        = string
  default     = null
}

variable "tags" {
  description = "Extra tags on every resource. loams-deploy is always the deployment's name."
  type        = map(string)
  default     = {}
}
