# Gitea Runner

The forge's Actions runner (docs ADR-N-028, Deployment/04 §6): `gitea-runner` in host mode on
the builder image the Portal release publishes (`joinedcontext-app-builder`, AP-82). It builds
every application repository with the workflow the Portal commits into it.

Deploy the component with:
```bash
helmfile apply -i --selector component=gitea-runner
```

## What it wires

- **Forge**: `components/gitea` turns Actions and the package registry on, and its bootstrap Job
  writes the organization's registration token into `gitea-runner-registration` here.
- **The loop**: the image's `runner` (portal `builder/runner.sh`) registers `--ephemeral`, runs
  one job, kills what the job left behind, wipes `/tmp/runner` and registers again. The token is
  read once from memory and deleted, so deleting the pod is how the runner restarts.
- **Network**: no ingress; egress to the forge (3000), the Portal API (8080) and CoreDNS.
- **The lane's token** (AP-73, ADR-N-028 §5, T-2636): this component declares the Keycloak
  client `jc-build-lane` (`client_credentials` only, audience `portal-api`, 15-minute tokens),
  whose secret is copied to the forge's namespace alone. There the `gitea-lane-token` CronJob of
  the forge's bootstrap chart mints a token every 5 minutes and writes it as the organization's
  Actions secret `JC_LANE_TOKEN`, with a forge token scoped `write:organization` alone. The
  `propose` job reads it; the `build` job never does. The account's one right is the seeded
  `build-lane` Role. Check it: `kubectl -n <ns> get pods -l app.kubernetes.io/name=lane-token`
  prints the last runs, and a run's log says which secret it wrote or which call was refused.

- **Build pods** (AP-130, AP-131, T-2794): an App's `build` job asks for `app-build-node` or
  `app-build-rust`, which the shared runner never carries. The Portal reads the forge's queue and
  starts one `Job` per queued App build here, registered with that repository's token alone, on
  the configuration `gitea-runner-build-{class}` and with the App's own cache claim
  `build-cache-{app}` at `/cache`. The part `build-pods` holds those two ConfigMaps and the Role
  `portal-build-pods` (Jobs, their token Secrets and the cache claims; no pods, no logs). The
  shared runner keeps `propose` and any job still asking for `node-22` or `rust-1.90`. Check it:
  `kubectl -n <ns> get jobs,pvc -l app.kubernetes.io/component=build-pod`.

## Pinning

`images.yaml` pins `joinedcontext-app-builder`. Only a build that contains
`/usr/local/bin/runner` can run here: re-pin to it before listing the component in an
environment that applies it.
