# prometheus

The Prometheus Operator stack (kube-prometheus-stack, pinned in `charts.yaml`, every image by
digest in `images.yaml`) that evaluates what the `monitoring` component renders (T-5154, OPS-16).
One release, `prometheus-stack`, on when `global.metrics.enabled` is:

| Workload | Namespace | Requests / memory limit |
|---|---|---|
| Prometheus, 31 days (`retention`), at most 8 GB of blocks on a 10 Gi volume | the component's | 512 Mi / 1.5 Gi |
| Alertmanager, `null` receiver only | the component's | 32 Mi / 128 Mi |
| Prometheus Operator, no admission webhook | the component's | 48 Mi / 160 Mi |
| kube-state-metrics | the component's | 48 Mi / 192 Mi |
| node-exporter, one per node | `kube-system` | 24 Mi / 96 Mi |

About 0.7 Gi of requests on the one node (T-5113 budget: dev had 78 % of allocatable requested).

- Prometheus reads every ServiceMonitor, PodMonitor, Probe and PrometheusRule on the cluster: the
  monitoring component renders its monitors into each scraped component's namespace, and the
  APISIX, Keycloak and CNPG charts render their own.
- `monitoring.monitoring.prometheusNamespace` and Grafana's `prometheusUrl` default to this
  component's namespace whenever it is in the component list, so every target's NetworkPolicy
  admits this Prometheus by label from that namespace alone (T-3771).
- k3s exposes no API server, scheduler, controller manager, proxy or etcd metrics to a pod: their
  monitors and default rules are off. The upstream node rules are off too; `NodeDiskAlmostFull`
  and `NodeMemoryHigh` of the monitoring component cover the node.
- node-exporter mounts the host's root, /proc and /sys read-only, without host network or PID.
  It runs in `kube-system` beside the node's own agents (as node-storage does, T-3431), closed to
  everything but Prometheus by the `node-exporter` NetworkPolicy; `policy-exceptions.yaml` says
  why the CI policies accept its hostPath mounts.
- No route at the edge: no Ingress, no APISIX route. Grafana (the `grafana` add-on) is the only
  caller Prometheus admits. Alertmanager sends nothing anywhere: `scripts/alert-watch.sh`, run by
  the cluster-watch host job, reads `/api/v2/alerts` through a `kubectl port-forward` (the API
  server's service proxy is an unmeshed caller Linkerd refuses) and posts each started or resolved alert as `tasks/note "@deployer ..."` with allow-listed labels
  only (`scripts/alert-notes.py`).
- Helm installs the operator's CRDs on the first install and never upgrades them: a chart bump
  that changes them applies `charts/crds/crds/` of the new chart with `kubectl apply
  --server-side` first.
