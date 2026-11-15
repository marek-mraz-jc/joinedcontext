# Prepare

Prepares the cluster before deployment:

- Creates namespaces, if `global.createNamespaces` is true.
- Patches namespaces to use linkerd if
  - `global.serviceMesh.patchNamespaces` is true
  - `global.serviceMesh.enabled` is true
  - `global.serviceMesh.type` is `linkerd`
- Gives the injected `linkerd-proxy` (and `linkerd-network-validator`, which Linkerd gives the
  same `proxy.resources`) requests and a memory limit through the meshed namespace's
  `config.linkerd.io/proxy-cpu-request`, `proxy-memory-request` and `proxy-memory-limit`
  annotations, from `global.serviceMesh.proxyResources` (`cpuRequest`, `memoryRequest`,
  `memoryLimit`, for example `10m`, `24Mi`, `250Mi`; an empty value removes the annotation, so
  Linkerd's own default applies). The
  proxy image stays tag-referenced: Linkerd injects it by version, so `require-image-checksum`
  keeps reporting it.

Next steps:
1. Configure helm charts in `components/prepare/charts.yaml`
2. Configure values for every release in `components/prepare/values`
3. Update your README.md
4. Update component to `defaults/environment/global.yaml` - `components` list

Deploy the component with:
$ # navigate to `deployment/` and run:
$ helmfile apply -i --selector component=prepare
