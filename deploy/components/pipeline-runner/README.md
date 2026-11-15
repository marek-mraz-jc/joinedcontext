# Pipeline Runner

Bento telemetry stream processor. The component has the two execution modes of PL-04: a
resident streams process for anything push-based or faster than thirty seconds, and one
ephemeral CronJob per pipeline for anything slower.

Deploy the component with:
```bash
helmfile apply -i --selector component=pipeline-runner
```

## Resident streams (`runner`)

`bento -w streams /streams`: the streams come from a ConfigMap mounted at `/streams` and the
watcher reloads a changed file without restarting the pod (PL-09). One process per project.

```yaml
pipeline-runner:
  runner:
    project: ovzdusie
    streams:
      mqtt-traffic.yaml: |
        input: { mqtt: { urls: ["tls://mqtt.example.org:8883"], topics: ["sensors/#"] } }
        output: { http_client: { url: "…/entityOperations/upsert", verb: POST } }
```

## Scheduled pipelines (`scheduled`)

Off by default. Each entry becomes a ConfigMap with the Bento configuration and a CronJob that
runs it; the pod exists for the fetch and nothing survives it (PL-10).

```yaml
pipeline-runner:
  scheduled:
    enabled: true
    project: ovzdusie
    pipelines:
      air-quality-batch:
        periodSeconds: 45          # or `schedule: '0 3 * * *'` for a cron expression
        config:                    # native Bento, WITHOUT `input`
          pipeline:
            processors:
              - http:
                  url: 'https://example.org/air_quality.csv'
                  verb: GET
              - mapping: |
                  root = content().string().parse_csv()
          output:
            http_client:
              url: 'http://context-gateway…/ngsi-ld/v1/entityOperations/upsert'
              verb: POST
```

The cadence becomes a schedule and a trigger (PL-26, PL-27):

| `periodSeconds` | `schedule` | Rendered |
|---|---|---|
| under 30 | any | refused: that pipeline belongs in the resident runner |
| 30 … 59 | ignored | `* * * * *`, `input.generate.interval` = the period, `count` = floor(60 / period) |
| 60 or more | absent | `* * * * *`, `interval` = the period, `count` = 1 |
| any | given | the cron expression, `count` = 1 |

`input` belongs to the chart, because that is where the cadence lands: a configuration that
declares its own input is refused rather than silently overwritten. The fetch is the first
processor. The same rule decides the class of a `kind: Pipeline` in
`jcctl::pipelines::cron_job`; two implementations of PL-27 that disagree would be the bug.

Egress is default-deny in both modes: the Context Gateway and CoreDNS, nothing else. A
scheduled pipeline that fetches from outside the cluster needs that address opened in
`networkpolicies.yaml` — until the reconciler renders a policy per pipeline from the
`DataSource` it names, that rule is written by hand.

Under Linkerd the scheduled pods carry
`config.alpha.linkerd.io/proxy-enable-native-sidecar`. Without it the injected proxy keeps
running after Bento exits, the Job never completes, and the run is failed by its deadline once
a minute.
