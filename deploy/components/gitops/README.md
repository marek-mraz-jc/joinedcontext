# gitops

Argo CD, keeping an environment on `main` of this repository (ADR-N-056, T-4674; docs
Deployment/15-gitops.md).

- `argocd`: the upstream `argo-cd` chart, pinned by version and every image by digest, in its
  own namespace. No Dex, no ApplicationSet controller, no HA; 576 Mi of requests. The helmfile
  plugin is a sidecar of the repo server running `gitops/cmp/generate.sh` in our own helmfile
  image (`images/gitops-helmfile`, T-4706); Argo CD itself runs `images/gitops-argocd`, the upstream
  image with fixed git-lfs and kustomize builds. Login through the Keycloak client `argocd` only (`keycloak-clients.yaml`); the realm
  role `platform-admin` administers, everyone else reads.
- `root`: the root Application, `gitops/chart` with `gitops/<environment>.yaml` and
  `gitops/hold.yaml`. Off by default: installing Argo CD changes nothing until an operator turns
  `gitops.root.enabled` on.

An add-on: an installation lists `gitops` in its components. Argo CD never manages this
component, so `just dev-apply` can always rebuild it.
