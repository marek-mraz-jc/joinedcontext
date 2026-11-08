# ekiben-io-ngsild

The NGSI-LD connector pair of ekiben (items E5 and E6). These are **ekiben extensions**: Bento refuses the name
`ngsi_ld`, so a config that uses them runs on ekiben only. `ekiben list` shows them under `ekiben-extensions`.
The Bloblang helpers of E7 (`ngsild_*`) live in `ekiben-bloblang/src/ngsild.rs`.

## Components

| Kind | Name | What |
|---|---|---|
| processor | `ngsi_ld` | Writes each batch's entities and keeps the messages with the Portal's outcome metadata (drop-in for the `branch` + `http` upsert). |
| output | `ngsi_ld` | Writes and nacks exactly the messages with refused entities, as indexed errors (`fallback` / `reject_errored` dead-letter them). |
| input | `ngsi_ld` | Reads by `query` (default), `temporal` or `notification` mode. |

### Writer fields (processor and output)

| Field | Default | Notes |
|---|---|---|
| `url` | required | The Endpoint base ending `/ngsi-ld/v1`, e.g. `${JC_GATEWAY_URL}/api/endpoint/<slug>/ngsi-ld/v1`. It may not carry credentials, a query, or be the broker root. |
| `operation` | `upsert` | `upsert`, `update` or `create` (`entityOperations/<op>`). |
| `options` | `update` | `update` sends `?options=update`; `replace` sends no query (upsert only). |
| `max_batch_count` | `1000` | Entities per request, 1 to 1000 (Antares refuses more with 400). |
| `max_batch_bytes` | `1MiB` | Bytes per request body, at most `4MiB` (Antares refuses more with 413). |
| `max_in_flight` | `1` | Concurrent requests of one batch, at most 64. |
| `timeout` | `30s` | Per request. |
| `retries`, `retry_period`, `max_retry_backoff`, `backoff_on`, `successful_on` | `3`, `1s`, `300s`, `[429]`, `[]` | Bento's semantics. `Retry-After` in seconds is honoured (capped at `max_retry_backoff`). |
| `drop_on` | `[400, 413, 422]` | Never retried: a whole refusal. |
| `rate_limit` | none | A rate-limit resource (`<project>__pipeline_egress`). A missing resource is an error. |
| `oauth2` | none | `enabled`, `token_url`, `client_key` (`pl-<pipeline>` or the sidecar's `<project>/<pipeline>`), `client_secret`, `scopes`, `endpoint_params`. Client credentials; the token is cached per organization and client, refreshed 30 s (at most half its life) before it expires, and refreshed at once on a 401. |
| `headers` | none | Extra headers. `Authorization` (no static API keys), `Link`, `NGSILD-Tenant`, `Content-Type`, `Accept`, `Cookie`, `Host` and `X-JC-*` are refused. |
| `context`, `tenant` | none | A Link `@context` and an `NGSILD-Tenant`, **only** for a broker outside the platform. Both are refused on an `/api/endpoint/` URL, because the gateway sets the tenant and refuses a non-core context. |
| `tls` | system roots | Verification is always on. `skip_cert_verify`, custom roots and client certificates are refused. |
| `max_response_bytes` | `64MiB` | Cap on one answer. |

Outcome metadata (processor):

- Written, in whole or in part: `jc_refused_<label>` = `[{"id": .., "message": ..}, ..]` (`[]` when none). It is
  formatted byte for byte as the branch's `result_map` formats it (compact, keys sorted, HTML escaped).
- Refused whole (`drop_on`): `jc_failed` = the problem's `detail`, else `title`, else the error. The message carries
  no error flag and no `jc_refused_*` key.
- Still failing after the retries (429, 5xx, transport): the message carries the error flag, so the stream's
  `written` output nacks it and the input replays it.

### Input fields

`query` mode: `url`, `type`, `id`, `id_pattern`, `attrs`, `q`, `scope_q`, `geo_q` (`georel`, `geometry`,
`coordinates`, `geoproperty`), `format` (`normalized`, `concise`, `simplified`/`keyValues`), `limit` (≤ 1000),
`max_entities` (default 50000), `interval`, `changes_only`, `changes_cap` (default 100000), `changes_cache`, and the
client fields.

- Pages follow the Portal's `endpoint_input`: the first page adds `count=true`, later pages add `offset=N`.
- If the source answers a full page without `NGSILD-Results-Count`, or holds more than `max_entities`, the run fails.
- Without `interval`, the input makes one pass and then ends.

`temporal` mode: `url`, the selection fields, `timerel` (`after`, `before`, `between`), `time_at` (required),
`end_time_at`, `last_n`, `limit` (default 100), `max_pages`, `interval` (needs `after`).

- The input follows the broker's `rel="intervalafter"` / `rel="next"` Link only while it stays on the same Endpoint.
- The watermark (`timeAt`) moves to the newest `observedAt` of the pages the output acked, in read order.

`notification` mode: `path` (default `/`), `auth` (`bearer: <token>`, or explicitly `public: true`),
`max_body_bytes` (default 1MiB, at most 16MiB), `max_in_flight` (64), `timeout` (5s), `dedupe_cap` (10000).

- The input never creates a subscription; the Portal reconciler does.
- The ingress (E4) mounts `ekiben_io_ngsild::notification::router()` (`POST /{org}/{stream}/{*path}`) or calls
  `dispatch` directly.
- Responses:
  - 204: written, or a Notification id already written;
  - 400: not a Notification; 401: wrong or missing token; 404: no route; 413: too large; 415: not JSON;
  - 502: the output nacked; 408: no ack within `timeout`; 503 with `Retry-After`: the input is full, or the same
    Notification is already in flight.

## Outbound rule

Every call (broker and token endpoint):

- uses `http`/`https` only;
- connects only to public addresses, or to the organization's `private_hosts` when they resolve into a private
  range;
- never reaches loopback, link-local, metadata, multicast or reserved addresses;
- follows no redirects, uses no proxy from the environment, and caps the body.

An organization with `allow_private_ips` skips the address check (tests, a person's own `ekiben run`).

## Metrics

Kept in the stream's counters under `<label>.<name>`:

- `ngsi_ld_requests_total{status="<code>|transport"}`
- `ngsi_ld_entities_written_total`
- `ngsi_ld_entities_refused_total`
- `ngsi_ld_entities_read_total`
- the timing `ngsi_ld_request_latency_ns`

## Decisions

- **Packing.** Entities are packed across messages to save requests. One request never carries the same entity id
  twice, so a 207 `errors[].entityId` maps back to exactly one message. An error naming no entity of the request goes
  to the messages with an id-less entity, else to every message of the request: a refusal is never lost.
- **Isolating a whole refusal.** When a packed request of several messages is refused whole (400/413/422), it is
  sent again one message at a time, so a bad message does not cost the others their write. This differs from the
  Portal branch, which writes one message per request.
- **Over the byte cap.** An entity larger than `max_batch_bytes` is refused locally as a member refusal (it can never
  be written). A message that is no entity (or no array of entities) is a local whole refusal.
- **Values unchanged.** Values pass through as the mapping wrote them (number text kept). No `@context` is added.
- **The 401 retry.** A 401 with a token gets exactly one fresh token at once (Keycloak revoked or expired it early).
  A second 401 follows the retry rules.
- **Configuration.** Unknown fields are refused, so a typo never keeps a default silently. Lint-time refusals happen
  when the component is built (`ekiben lint` builds the stream).
- **Token cache.** The cache is process wide and keyed by organization, token URL, client id and a SHA-256 of the
  secret. It holds at most 1024 clients and evicts the ones no component holds. Concurrent callers share one fetch.
- **Notification answers.** A redelivered Notification is answered 204 without a second write. The dedupe memory is
  the input's (lost on restart; the broker's redelivery is then written again, and upsert makes that harmless).
- **Outbound guard location.** The guard is implemented here; the task wants it shared in ekiben-io-http, which
  belongs to another worker (follow-up).
