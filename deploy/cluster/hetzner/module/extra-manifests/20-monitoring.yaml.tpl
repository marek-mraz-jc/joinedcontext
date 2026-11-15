# Prometheus — CIVITAS deploys ServiceMonitors when global.metrics.enabled: true.
# Optional (enable_monitoring terraform var). grafana/alertmanager off,
# single small Prometheus; enable them when actually needed.
apiVersion: v1
kind: Namespace
metadata:
  name: monitoring
---
apiVersion: helm.cattle.io/v1
kind: HelmChart
metadata:
  name: kube-prometheus-stack
  namespace: kube-system
spec:
  repo: https://prometheus-community.github.io/helm-charts
  chart: kube-prometheus-stack
  targetNamespace: monitoring
  valuesContent: |-
    grafana:
      enabled: false
    alertmanager:
      enabled: false
    prometheus:
      prometheusSpec:
        retention: 7d
        # Scrape every ServiceMonitor/PodMonitor on the cluster, not only
        # helm-labelled ones — CIVITAS' monitors live in instance namespaces.
        serviceMonitorSelectorNilUsesHelmValues: false
        podMonitorSelectorNilUsesHelmValues: false
        resources:
          requests:
            cpu: 100m
            memory: 512Mi
