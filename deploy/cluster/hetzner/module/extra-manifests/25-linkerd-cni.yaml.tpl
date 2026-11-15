# Linkerd CNI plugin (T-2832, T-4384, T-5327): meshed pods get their iptables
# rules from this DaemonSet instead of a linkerd-init container with NET_ADMIN,
# so PSS-restricted namespaces (the Portal's App namespaces) admit them.
# Live on dev since 2026-09-26 and on prod (smartcity) since 2026-10-19. It needs
# `cniEnabled: true` on linkerd-control-plane in 30-linkerd.yaml.tpl. Keep the chart
# version equal to linkerd-crds and linkerd-control-plane.
apiVersion: v1
kind: Namespace
metadata:
  name: linkerd-cni
  labels:
    # The DaemonSet mounts the node's CNI dirs (hostPath) and runs privileged.
    pod-security.kubernetes.io/enforce: privileged
    config.linkerd.io/admission-webhooks: disabled
    linkerd.io/cni-resource: "true"
  annotations:
    linkerd.io/inject: disabled
---
apiVersion: helm.cattle.io/v1
kind: HelmChart
metadata:
  name: linkerd2-cni
  namespace: kube-system
spec:
  repo: https://helm.linkerd.io/edge
  chart: linkerd2-cni
  version: "2026.10.1" # keep in lockstep with linkerd-crds and linkerd-control-plane
  targetNamespace: linkerd-cni
  valuesContent: |-
    # k3s paths, not the chart's /etc/cni/net.d and /opt/cni/bin. The bin dir is
    # `bin_dir` in /var/lib/rancher/k3s/agent/etc/containerd/config.toml; older
    # k3s used data/current/bin. A wrong dir breaks every pod sandbox on the node.
    destCNINetDir: /var/lib/rancher/k3s/agent/etc/cni/net.d
    destCNIBinDir: /var/lib/rancher/k3s/data/cni
    # Leap Micro enforces SELinux: install-cni cannot write the host dirs unprivileged.
    privileged: true
    # A node reboot can start pods before the plugin re-chains the conflist; the
    # repair controller restarts pods whose network validator failed (T-4384).
    repairController:
      enabled: true
      resources:
        cpu:
          request: 10m
        memory:
          request: 32Mi
          limit: 128Mi
