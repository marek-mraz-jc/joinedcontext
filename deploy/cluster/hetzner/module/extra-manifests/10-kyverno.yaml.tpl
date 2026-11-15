# Kyverno — required by CIVITAS' runtime-policies component (ClusterPolicy CRD).
# Installed via the k3s helm-controller (HelmChart CR). Single replicas — one node.
apiVersion: v1
kind: Namespace
metadata:
  name: kyverno
---
apiVersion: helm.cattle.io/v1
kind: HelmChart
metadata:
  name: kyverno
  namespace: kube-system
spec:
  repo: https://kyverno.github.io/kyverno/
  chart: kyverno
  # Pinned: unpinned charts float to whatever upstream published this week on
  # every NEW cluster. Bump deliberately (helm search repo -l), then re-verify.
  version: "3.9.1"
  targetNamespace: kyverno
  valuesContent: |-
    admissionController:
      replicas: 1
    backgroundController:
      replicas: 1
    cleanupController:
      replicas: 1
    reportsController:
      replicas: 1
