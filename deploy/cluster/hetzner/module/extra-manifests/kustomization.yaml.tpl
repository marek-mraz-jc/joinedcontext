# NOTE: kube-hetzner only uploads/applies *.yaml.tpl files from this folder —
# plain .yaml files are silently ignored.
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - 00-letsencrypt-clusterissuer.yaml
  - 05-local-path-retain.yaml
  - 06-prod-namespaces.yaml
  - 10-kyverno.yaml
%{ if enable_monitoring == "true" ~}
  - 20-monitoring.yaml
%{ endif ~}
  - 25-linkerd-cni.yaml
  - 30-linkerd.yaml
  - 40-traefik-linkerd.yaml
