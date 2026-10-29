variable "name" {
  description = "Names the instance, role, security group and (by default) the bucket."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,30}$", var.name))
    error_message = "name is lowercase letters, digits and - (up to 31)."
  }
}

variable "release" {
  description = "The Loams release to install. The bundle's signature is checked on the VM."
  type        = string
  default     = "0.1.3"
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release is a version such as 0.1.3, never latest."
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

variable "instance_type" {
  description = "Overrides size's instance type (x86-64 only until every image is arm64, D1384)."
  type        = string
  default     = null
}

variable "disk_size_gb" {
  description = "The encrypted gp3 root volume, which holds Docker's volumes (the stores)."
  type        = number
  default     = 60
  validation {
    condition     = var.disk_size_gb >= 30 && var.disk_size_gb <= 16000
    error_message = "disk_size_gb is between 30 and 16000."
  }
}

variable "profiles" {
  description = "COMPOSE_PROFILES (tikv, rustfs, ...). Choose before the first start: the engine refuses to switch stores on existing data."
  type        = string
  default     = ""
}

variable "hostname" {
  description = "The engine's tailnet name (https://<hostname>.<tailnet>.ts.net)."
  type        = string
  default     = "loams"
}

variable "bucket" {
  description = "An existing bucket to use. null creates one (versioned, encrypted, private) named loams-<name>-<account id>."
  type        = string
  default     = null
}

variable "kms_key_arn" {
  description = "An existing KMS key for the bucket. null creates a key when the module creates the bucket; with an existing bucket, null means the module grants no KMS permission (the bucket uses SSE-S3 or its own key's policy)."
  type        = string
  default     = null
}

variable "secret_prefix" {
  description = "The Secrets Manager prefix; the install reads <prefix>/tailnet-authkey (must exist: create it with your Tailscale auth key, never in this module's variables), <prefix>/setup-token and optionally <prefix>/api-keys. null is loams/<name>."
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
  description = "Extra tags on every resource."
  type        = map(string)
  default     = {}
}

variable "confluent_integrations" {
  description = "Confluent integrations whose keys the VM fetches from the secret store into the connect role's secret files (names only; design 80 §7.2). Create the keys with `loams byoc confluent key create`."
  type        = list(string)
  default     = []
}
