# One single-node k3s cluster via kube-hetzner; clusters/<name>.tfvars says which (T-6444, README).
# Cheapest possible: one server, klipper LB (no paid Hetzner LB), PVCs on the node disk (local-path
# default, no paid volumes). Nodes on openSUSE Leap Micro (the stable, signature-verified image:
# Tumbleweed MicroOS churned weekly and broke cloud-init in 2026-08; upstream's default since v3).
# This directory is the root module on purpose: resource addresses equal the kit's, so an existing
# kit state moves in by copying the file.

module "kube-hetzner" {
  providers = {
    hcloud = hcloud
  }
  source  = "kube-hetzner/kube-hetzner/hcloud"
  version = "3.2.1" # T-4677: secrets-encryption hardening, Leap Micro first-SSH fix (supervisor review)

  hcloud_token = var.hcloud_token

  ssh_public_key  = file("${var.ssh_key_dir}/${var.ssh_key_name}.pub")
  ssh_private_key = file("${var.ssh_key_dir}/${var.ssh_key_name}")

  cluster_name   = var.cluster_name
  network_region = var.network_region

  # The kit's kubeconfig and kustomization backup files would land in this directory, inside the
  # repository. The recipes write the kubeconfig to the state directory from the output instead.
  create_kubeconfig    = false
  create_kustomization = false

  control_plane_nodepools = [
    {
      name        = var.node_pool_name,
      server_type = var.server_type,
      location    = var.location,
      os          = "leapmicro", # v3 default for new pools — explicit on purpose
      labels      = [],
      taints      = [],
      count       = 1
    },
  ]
  agent_nodepools = []

  # Single node: klipper LB binds 80/443 on the node itself — no Hetzner LB
  # (~€6/mo saved). Scheduling on the control plane is required for that.
  enable_klipper_metal_lb           = true
  allow_scheduling_on_control_plane = true
  ingress_replica_count             = 1
  load_balancer_type                = "lb11" # unused with klipper, module requires it
  load_balancer_location            = var.location

  # Weekly patch window (T-4684, owner 2026-10-19T19:40Z): Sunday 03:00-04:00 Europe/Bratislava.
  # One node, so every reboot is a full outage: the transactional-update timer stages OS updates
  # in a new snapshot, and kured reboots into it only inside the window. Announce each window in
  # AI_shared_folder.md beforehand.
  automatically_upgrade_os = var.automatically_upgrade_os
  kured_options = {
    "reboot-days" = "sun"
    "start-time"  = "03:00"
    "end-time"    = "04:00"
    "time-zone"   = "Europe/Bratislava"
  }
  # k3s patch bumps: raise k3s_version below (v1.35.x only); system-upgrade-controller applies the
  # plan inside the same window once the node carries k3s_upgrade=true (README, patch window).
  # Kept false here: true would rewrite the node's k3s config to add that label.
  automatically_upgrade_kubernetes = false
  system_upgrade_schedule_window = {
    days      = ["sunday"]
    startTime = "03:00"
    endTime   = "04:00"
    timeZone  = "Europe/Bratislava"
  }
  # Freeze the k3s minor (v3 defaults to 'stable', which floats). Bump deliberately, then
  # re-verify the platform.
  k3s_channel = var.k3s_channel
  k3s_version = var.k3s_version

  # Merged into the node's k3s config.yaml, so a re-created node keeps it.
  control_planes_custom_config = var.k3s_server_config

  # Kubernetes Secrets encrypted at rest in the k3s datastore from the first apply (T-4677;
  # the module refuses turning it on or off in place later).
  enable_secrets_encryption = var.enable_secrets_encryption

  enable_cert_manager = true

  # PVCs live on the node disk for free. Without the Hetzner CSI nothing can create a paid hcloud
  # volume; dev runs it because the cluster it took over did (T-3040).
  enable_local_storage = true
  enable_hetzner_csi   = var.enable_hetzner_csi

  # SSH + kube API reachable only from the admin CIDRs (validated non-open).
  firewall_kube_api_source = local.admin_cidrs
  firewall_ssh_source      = local.admin_cidrs

  extra_firewall_rules = concat(local.smtp_out, var.mqtt_ports_open ? local.mqtt_in : [])

  dns_servers = [
    "1.1.1.1",
    "8.8.8.8",
    "2606:4700:4700::1111",
  ]

  # extra-manifests/*.yaml.tpl (Let's Encrypt issuer, Linkerd mTLS PKI,
  # optional monitoring toggle) — v3 replaced extra_kustomize_* with this map.
  user_kustomizations = {
    "1" = {
      source_folder = "extra-manifests"
      kustomize_parameters = {
        acme_email                  = var.acme_email
        enable_monitoring           = var.enable_monitoring
        mqtt_ports_open             = var.mqtt_ports_open ? "true" : "false"
        linkerd_trust_anchor_cert_4 = indent(4, tls_self_signed_cert.linkerd_trust_anchor.cert_pem)
        linkerd_trust_anchor_cert_6 = indent(6, tls_self_signed_cert.linkerd_trust_anchor.cert_pem)
        linkerd_issuer_cert_4       = indent(4, tls_locally_signed_cert.linkerd_issuer.cert_pem)
        linkerd_issuer_key_4        = indent(4, tls_private_key.linkerd_issuer.private_key_pem)
      }
      pre_commands  = ""
      post_commands = ""
    }
  }
}

locals {
  admin_cidrs = concat(var.firewall_allowed_ips, var.firewall_extra_ips)

  # Keycloak sends mails directly: allow SMTP out.
  smtp_out = [
    {
      description     = "Allow SMTP outbound"
      direction       = "out"
      protocol        = "tcp"
      port            = "25"
      source_ips      = []
      destination_ips = ["0.0.0.0/0", "::/0"]
    },
    {
      description     = "Allow SMTP Submission outbound"
      direction       = "out"
      protocol        = "tcp"
      port            = "587"
      source_ips      = []
      destination_ips = ["0.0.0.0/0", "::/0"]
    },
    {
      description     = "Allow SMTPS outbound"
      direction       = "out"
      protocol        = "tcp"
      port            = "465"
      source_ips      = []
      destination_ips = ["0.0.0.0/0", "::/0"]
    },
  ]

  # T-3996: MQTT over TLS to the brokers' Traefik entrypoint; 1883/1884 stay closed.
  # T-5856: the dedicated MQTT ports, one Traefik entrypoint each, TLS only.
  mqtt_in = [
    {
      description     = "Allow MQTTS inbound"
      direction       = "in"
      protocol        = "tcp"
      port            = "8883"
      source_ips      = ["0.0.0.0/0", "::/0"]
      destination_ips = []
    },
    {
      description     = "Allow MQTT dedicated ports inbound"
      direction       = "in"
      protocol        = "tcp"
      port            = "18830-18839"
      source_ips      = ["0.0.0.0/0", "::/0"]
      destination_ips = []
    },
  ]
}

provider "hcloud" {
  token = var.hcloud_token
}

terraform {
  required_version = ">= 1.10.1" # kube-hetzner v3 floor
  # The state path is a -backend-config of cluster.sh: one state per cluster, outside the repository.
  backend "local" {}
  # Exact: .terraform.lock.hcl holds the same versions with their hashes.
  required_providers {
    hcloud = {
      source  = "hetznercloud/hcloud"
      version = "1.70.2"
    }
    tls = {
      source  = "hashicorp/tls"
      version = "4.3.0"
    }
  }
}

# --- Linkerd mTLS trust anchor + identity issuer (10y), consumed by
# --- extra-manifests/30-linkerd.yaml.tpl via extra_kustomize_parameters.
resource "tls_private_key" "linkerd_trust_anchor" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_self_signed_cert" "linkerd_trust_anchor" {
  private_key_pem       = tls_private_key.linkerd_trust_anchor.private_key_pem
  validity_period_hours = 87600
  is_ca_certificate     = true
  set_subject_key_id    = true
  allowed_uses          = ["cert_signing", "crl_signing"]
  subject {
    common_name = "root.linkerd.cluster.local"
  }
}

resource "tls_private_key" "linkerd_issuer" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_cert_request" "linkerd_issuer" {
  private_key_pem = tls_private_key.linkerd_issuer.private_key_pem
  subject {
    common_name = "identity.linkerd.cluster.local"
  }
}

resource "tls_locally_signed_cert" "linkerd_issuer" {
  cert_request_pem      = tls_cert_request.linkerd_issuer.cert_request_pem
  ca_private_key_pem    = tls_private_key.linkerd_trust_anchor.private_key_pem
  ca_cert_pem           = tls_self_signed_cert.linkerd_trust_anchor.cert_pem
  validity_period_hours = 87600
  is_ca_certificate     = true
  set_subject_key_id    = true
  allowed_uses          = ["cert_signing", "crl_signing"]
}

output "kubeconfig" {
  value     = module.kube-hetzner.kubeconfig
  sensitive = true
}

output "node_ipv4" {
  value = module.kube-hetzner.control_planes_public_ipv4
}
