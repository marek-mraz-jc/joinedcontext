# monitoring

Prometheus Operator scrape configuration and edge alerting for the platform (OPS-16, TS-22).

The component deploys no workload. It renders one `ServiceMonitor` per platform component that
exports Prometheus metrics on its Service and has no monitor of its own, and the `PrometheusRule` of the edge runbook.
The CloudNativePG instances (`postgres-cluster`, `apps-db`) get a `PodMonitor` on their exporter port `metrics`
(:9187); the cnpg/cluster chart keeps its own rules, among them the connections alert at 80 % of
`max_connections` (T-5125), and its PodMonitor stays off because it sets no interval.

APISIX is not in the target list: its own chart renders a `ServiceMonitor` for port 9091
(`/apisix/prometheus/metrics`) as soon as metrics are enabled, so a second monitor here would
scrape the edge twice and double every counter. The alert rules are still this component's,
because the chart ships none.

- Enabled by `global.metrics.enabled`. Off, the component renders nothing: scrape configuration
  without a Prometheus watching is dead configuration.
- Every monitor is rendered into the namespace of the component it scrapes and selects that one
  namespace, so a second instance on the same cluster is never scraped by the first one's monitor.
- Ports are named, never numbered. A renamed port breaks the monitor loudly instead of scraping
  whatever now sits on that number.
- Metrics endpoints stay inside the cluster: they carry no APISIX route and no ingress.

Targets and rules live in `values/monitoring/base-values.yaml.gotmpl`; each is guarded by the
component it belongs to, so an instance that omits a component carries no monitor for it.

## Objectives and alerts (T-5104)

Each public surface has two objectives over 30 days, measured by the synthetic probe (component
`probes`) from outside the edge, per target and surface:

| Objective | Events | Budget |
|---|---|---|
| Availability 99.5 % | `jc_probe_failures_total` of `jc_probe_request_duration_seconds_count` | 0.5 % failed probes |
| p95 latency under 500 ms | probes above the `le="0.5"` bucket | 5 % slower probes |

The group `slo-public-surfaces` records the ratios over 5m, 30m, 1h, 6h, 3d and 30d
(`jc:probe_failure:ratio_rate*`, `jc:probe_slow:ratio_rate*`), the 30-day availability and p95,
and alerts by multiwindow burn rate: `...BudgetBurnFast` (critical) at 14.4x over 1h with 5m, or
6x over 6h with 30m; `...BudgetBurnSlow` (warning) at 1x over 3 days with 6h. Grafana's
"SLOs — public surfaces" dashboard draws the same records. Pipeline freshness is `PipelineDataStale`.

Beside them: `TLSCertificateExpiresSoon` (under 14 days, or a certificate that fails verification)
and `DomainExpiresSoon` (under 30 days) from the probe; `NodeDiskAlmostFull`, `VolumeAlmostFull`
(over 85 %) and `NodeMemoryHigh` (over 90 %) from node-exporter and the kubelet; and
`PostgresBackupStale` when the newest local dump (`<cluster>-local-backup` CronJob, through
kube-state-metrics) or CNPG backup is older than 26 hours. These read the series the Prometheus
Operator's own stack (kube-prometheus-stack) scrapes, which the `prometheus` component deploys
(T-5154).

`tests/promtool/alerts.test.yaml` injects a fault for every alert and sees it fire and resolve;
`tests/test_alert_rules.py` runs it with `promtool test rules` from the pinned Prometheus image.

Delivery: `scripts/alert-notes.py` turns Alertmanager's `/api/v2/alerts` into one line per alert
that started or resolved, with only allow-listed labels (hosts, surfaces, Endpoints, projects,
volumes), for the cluster-watch host job to post as `tasks/note "@deployer ..."` through
`scripts/alert-watch.sh` (T-5154). E-mail follows
when the platform has SMTP (T-2767).
