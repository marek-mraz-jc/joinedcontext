variable "hcloud_token" {
  description = "Hetzner Cloud API token (Read & Write)"
  type        = string
  sensitive   = true
}

variable "acme_email" {
  description = "Email for Let's Encrypt certificate registration"
  type        = string
}

variable "ssh_key_dir" {
  description = "Directory of the cluster's ssh key pair; cluster.sh sets it inside the state directory"
  type        = string
}

variable "ssh_key_name" {
  description = "File name of the passphrase-less ed25519 key pair in ssh_key_dir"
  type        = string
}

variable "cluster_name" {
  description = "Prefix of every Hetzner object; a change replaces the node"
  type        = string
}

variable "node_pool_name" {
  description = "Name of the control-plane pool, part of the server name; a change replaces the node"
  type        = string
  default     = "control-plane"
}

variable "enable_hetzner_csi" {
  description = "Install the Hetzner CSI driver, which can create paid volumes"
  type        = bool
  default     = false
}

variable "server_type" {
  description = "Hetzner server type for the single node (x86 required by CIVITAS)"
  type        = string
}

variable "location" {
  description = "Hetzner location (nbg1/fsn1/hel1/ash/hil/sin)"
  type        = string
}

variable "network_region" {
  description = "Hetzner network region matching the location"
  type        = string
  default     = "eu-central"
}

variable "enable_monitoring" {
  description = "Install kube-prometheus-stack (Prometheus, no Grafana) on the cluster"
  type        = string
  default     = "false"
}

variable "firewall_allowed_ips" {
  description = "CIDRs allowed to reach SSH and the Kube API (Hetzner firewall)"
  type        = list(string)
  # No default on purpose: fail closed. Open to the world is never accepted.
  validation {
    condition     = length(var.firewall_allowed_ips) > 0 && alltrue([for c in var.firewall_allowed_ips : !endswith(c, "/0")])
    error_message = "firewall_allowed_ips must be a non-empty list of admin CIDRs, none of them /0."
  }
}

variable "firewall_extra_ips" {
  description = "More admin CIDRs, from <state dir>/private.tfvars only: a person's address stays out of Git"
  type        = list(string)
  default     = []
  validation {
    condition     = alltrue([for c in var.firewall_extra_ips : !endswith(c, "/0")])
    error_message = "firewall_extra_ips must not hold a /0."
  }
}

variable "mqtt_ports_open" {
  description = "Open 8883 and 18830-18839 in the firewall and give Traefik their entrypoints (T-3996, T-5856)"
  type        = bool
  default     = false
}

variable "k3s_channel" {
  description = "k3s minor line"
  type        = string
}

variable "k3s_version" {
  description = "Exact k3s release, empty to take the module's release of the channel"
  type        = string
  default     = ""
}

variable "k3s_server_config" {
  description = "Extra keys of the node's k3s config.yaml (server flags)"
  type        = any
  default     = {}
}

variable "enable_secrets_encryption" {
  description = "Secrets encrypted at rest; fixed at creation, the module refuses a change in place"
  type        = bool
}

variable "automatically_upgrade_os" {
  description = "Weekly OS patch window (kured, Sunday 03:00-04:00 Europe/Bratislava)"
  type        = bool
}
