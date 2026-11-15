# The production-shaped namespaces enforce the restricted Pod Security Standard (T-6719, G-1):
# a privileged pod, a host path or a root container is refused at admission, whoever asks
# (jc's account stamps the MQTT brokers in jc-prod-mqtt). Kyverno's rules cover dev's
# namespaces only. helmfile creates jc-prod and jc-dev without labels; applying this file over
# a namespace that exists merges the labels and changes nothing else. Every pod running in the
# four namespaces passed `kubectl label --dry-run=server ... enforce=restricted` on 2026-11-11
# (no warning; kube-system warned in the same check). No baseline exception is needed.
apiVersion: v1
kind: Namespace
metadata:
  name: jc-prod
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
    pod-security.kubernetes.io/audit: restricted
    pod-security.kubernetes.io/warn: restricted
---
apiVersion: v1
kind: Namespace
metadata:
  name: jc-prod-mqtt
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
    pod-security.kubernetes.io/audit: restricted
    pod-security.kubernetes.io/warn: restricted
---
apiVersion: v1
kind: Namespace
metadata:
  name: jc-dev
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
    pod-security.kubernetes.io/audit: restricted
    pod-security.kubernetes.io/warn: restricted
---
apiVersion: v1
kind: Namespace
metadata:
  name: prod-final-backup
  annotations:
    joinedcontext.com/note: second copy of the old prod final export (GOAL-prod-decommission); never delete without the supervisor
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
    pod-security.kubernetes.io/audit: restricted
    pod-security.kubernetes.io/warn: restricted
    joinedcontext.com/purpose: prod-final-backup
