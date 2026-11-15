# probes

Synthetic probes of every public surface, through the public hosts, every minute (T-5103).

One pod per cluster (`probe`, Python standard library on the official slim image) probes its own
environment and each `peers` entry. dev peers with smartcity, so a production cluster that is down
entirely still goes red on dev; smartcity has no peer, since the production render names no dev host
(tests/test_smartcity.py).

## What it checks

Per target and minute, at most `budget` requests (default 120, under the edge's 300 a minute per
address), paced evenly:

| Surface | Request | Holds when |
|---|---|---|
| catalogue | `package_search?rows=1` on the CKAN host | `success` and a count above 0 |
| portal | `/api/v1/health` on the Portal host | `status` is `ok` |
| assistant | `/api/v1/d/{assistantId}/config` (no model call) | a config with `publicId` |
| ngsi-ld | `entities?type={T}&limit=1&count=true` | one entity and `NGSILD-Results-Count` above 0 |
| odata | `odata/v4/{T}?$top=1` | a row in `value` |
| ogc | `ogc/features/collections/{T}/items?limit=1` | a GeoJSON FeatureCollection with a Feature |
| sta | `sta/v1.1/Things?$top=1` | a Thing with `@iot.id` |
| csv | `file.csv`, first 64 KiB | `text/csv`, an `id` column and a row |
| ical | `ical/v1/events.ics` | `BEGIN:VCALENDAR` with a `VEVENT` |
| mcp | `POST mcp` `tools/list` | named tools |
| websocket | `ws` upgrade, subprotocol `ngsi-ld-ws` | `101` and an `x-welcome` first message |

The public Endpoints come from the catalogue's `endpoint` extra; each Endpoint's record lists the
surfaces it serves (`dcat:distribution`), and `ngsi-ld/v1/types` gives the type `{T}`. A record is
read again every `discoverySeconds` (900). A `types` read that fails (an error status, a timeout, a
body that is no JSON object) keeps the type learned before and is asked again the next round; an
Endpoint with no type learned yet is left out of `{T}` surfaces until one answers, and only a `200`
listing no usable type fails them (T-5660). Each surface kind probes its next Endpoints round robin:
every kind gets at least one probe each round even when the budget is smaller than the kinds, and at
least one record is refreshed each round (T-5184); a kind broken everywhere is red in the first
minute; one Endpoint's surface within
`ceil(endpoints x kinds / budget)` minutes (about 6 for 80 Endpoints at 120).

Decided (worker-t5103, owner may override): round robin under a per-target budget instead of every
Endpoint surface every minute, because 80 public Endpoints with 8 surfaces each are 640 requests a
minute, twice the edge's per-address limit; options: every surface every minute (trips the limit
visitors share), one Endpoint per kind (misses a single broken Endpoint for good).

## Results

- `GET :9100/metrics`: `jc_probe_up`, `jc_probe_duration_seconds`, `jc_probe_timestamp_seconds`
  per target, Endpoint and surface; `jc_probe_surface_failing` per target and surface;
  `jc_probe_round_timestamp_seconds` per target. The monitoring component scrapes it and alerts
  `ProbeSurfaceDown` (`jc_probe_up == 0` for 1 minute) and `ProbeStalled`.
- Since T-5104 also `jc_probe_failures_total` and the histogram `jc_probe_request_duration_seconds`
  per target and surface (the SLO's events), and, checked once an hour from the same outside view,
  `jc_probe_tls_cert_expiry_timestamp_seconds` per host (0 when the certificate fails verification)
  and `jc_probe_domain_expiry_timestamp_seconds` per registered domain (RDAP, its server from
  IANA's bootstrap list at data.iana.org).
- `GET :9100/results.json`: the newest result per check with its reason, for the status page
  (T-5102) and the health log (T-5067). A reason is the probe's own sentence, never a response body.
- `GET :9100/healthz`: 503 when a target finished no round for three minutes (the liveness probe).

## Status page

The `status` part serves the public status page on `status.{domain}` (T-5102, `files/status.py`, the
same image). Once a minute it reads this environment's rows of `/results.json` from the probe beside
it and shows each public surface as up (every probed Endpoint holds), degraded (some fail), down
(all fail) or no data (nothing newer than 10 minutes). It counts the minute per surface and UTC day
in `history.json` on its 64 Mi volume and draws 90 days per surface; uptime counts degraded minutes
as available. The uptime label names the days actually measured ("99.9 % uptime over 3 days", 90 at
most): days before a surface's first sample stay grey and count nowhere (T-5184). One rule decides
"no data" (`current()`), and the page and `status.json` render the same result of it. `/` answers in Slovak or English by `Accept-Language`, `/sk/` and `/en/` in one
language, `/status.json` the same states for machines.

The page names failing Endpoints by their public catalogue slug and never shows a probe reason, a
response body, a host or an address. Answers carry `Cache-Control: public, max-age=30,
stale-while-revalidate=30, stale-if-error=86400`: a cache keeps the page through an outage of the
pod, and a new outage still shows within two minutes (one probe round, one tick, 30 s of cache).

Decided (worker-t5102, owner may override): the page reads the probe only, not the Portal's
health_check rows (T-5067), because those are per project behind a login and have no public flag;
options: a public flag on health_check rows read through the Portal (a second public reader of
the Portal), the probe's results (the same surfaces, already anonymous).

### Incidents

The page opens an incident itself when a surface is down or degraded for 3 minutes in a row, adds
each further surface that reaches 3 bad minutes while it is open, and closes it after every one of
its surfaces was up for 10 minutes in a row; its end is the last bad minute (T-5184). A minute
without probe data changes neither count. The title ("Interfaces down" or "Interfaces degraded")
and the note come from fixed sk/en templates in `status.py`; the surfaces are the page's own names.
The state lives in `watch.json` beside the history, so a restart keeps an open incident; a damaged
file starts a new watch. `status.json` marks these entries `"automatic": true`.

The deployer writes `files/incidents.yaml` (its header shows the fields); the next apply puts it on
the page. An open incident (no `end`) stands at the top, a closed one stays 90 days. A note that covers an automatic incident's surfaces
at an overlapping time replaces the automatic text. A malformed
entry stops the pod at start with the field to fix, and `tests/test_status_page.py` holds the
committed file to the same rules.

## Security

Anonymous only: no token, no Secret, no `envFrom`. Every URL is built from the https origins in
`PROBE_TARGETS`, never taken from an answer, except the RDAP server, which the hourly expiry check
takes from IANA's bootstrap answer (https only, no redirects, body capped); redirects are not followed.
Egress is 443 to public addresses and the ingress controller, and CoreDNS; ingress only from
Prometheus and the status page on 9100. The status page takes ingress from the edge on 8080 only and
dials the probe and CoreDNS.
