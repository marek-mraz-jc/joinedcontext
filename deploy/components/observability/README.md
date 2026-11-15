# observability

The OpenTelemetry Collector that fills the Portal's Activity stream
(`docs/Deployment/05-monitoring-logging.md` section 5, `docs/Architecture/09-portal.md`
section 6).

It does three things and no fourth: it accepts OTLP from the broker, the gateway and the
Bento runners, it deletes every attribute that is not on the allow-list of OPS-48, and it
posts what is left to the Portal's ingest route with a token from its own Keycloak
ServiceAccount client. It holds no database credential and writes no row.

The mapping from a component's own vocabulary to an activity event does not live here. Each
emitter names its own events, because the component that knows a forward was cut off by the
loop guard is the broker, and because the released contrib distribution has no connector that
turns a span into a log and no exporter that writes to SQL.

Ports: `4317` OTLP/gRPC, `4318` OTLP/HTTP, `8888` its own Prometheus metrics, `13133` the
health check the probes read. No APISIX route publishes any of them.

Deploy the component with:
```sh
# navigate to `deployment/` and run:
helmfile apply -i --selector component=observability
```
