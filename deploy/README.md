# deploy

Single-host deployment of the one binary (systemd unit, container image, backups with an off-site copy), and the cluster form for larger installs.

## Single-host form

`rehearsal/` brings the one binary up on one host with its Postgres, the image and the backups ([rehearsal/REHEARSAL.md](rehearsal/REHEARSAL.md)).

## Cluster form

The rest of this folder is the Helmfile deployment for a Kubernetes cluster, imported from the former deployment repository: `helmfile-root.yaml.gotmpl`, `components/`, `charts/`, `policies/`, `scripts/`, `tests/` and the `justfile` recipes. [CLUSTER.md](CLUSTER.md) is its guide.

- The public example environments live in `ci/example-deployments/environments/`; `just _dev-assemble` copies them into `deployment/`.
- The real environments' values are private: set `JC_ENVIRONMENTS` to the private `environments/` checkout and `just _dev-assemble` overlays them ([docs/Deployment/01-repository-layout.md](../docs/Deployment/01-repository-layout.md)).
- Tests: `cd deploy && just _dev-assemble && pytest tests`. CI runs them in the `deploy-*` jobs of `.github/workflows/ci.yml`.
- Release notes and the changelog of the old deployment repository stay in the private repository.
