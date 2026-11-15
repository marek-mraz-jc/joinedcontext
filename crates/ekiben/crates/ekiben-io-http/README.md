# ekiben-io-http

HTTP components of ekiben: the `http` processor, the `http_client` input and output, the
`http_server` input with the shared ingest listener, and the `websocket` input (feature
`websocket`, on by default).

Bento 1.22 field names, defaults and answers are kept. What ekiben adds or refuses is listed
here.

## `http` and `http_client` input: `paginate`

ekiben extension (T-7056). `paginate` walks a paged source page by page, so a mapping no longer
writes its own loop around `http`. Without the key both components behave exactly as in Bento.

```yaml
paginate:
  mode: offset                  # page | offset | cursor | link | next_url
  offset_param: resultOffset
  limit_param: resultRecordCount
  limit: 1000
  records: features             # the array of one page
  more: exceededTransferLimit   # a boolean: another page exists
  order_by: {param: orderByFields, value: OBJECTID}
  max_pages: 1000               # 1..10000, default 1000
  max_records: 50000
  delay: 200ms
```

- Modes:
  - `page`: `param` (default `page`), `start` (1), and `size_param` with `size`.
  - `offset`: `offset_param` (`offset`), `limit_param` (`limit`), `limit` (required), `start` (0).
    The offset moves by the records received, so a server that caps a page below `limit` loses
    nothing.
  - `cursor`: the next cursor from `cursor` (a body path) or `cursor_query` (an interpolation,
    `${! json("meta.next") }`), sent as `cursor_param` or `cursor_header`.
  - `link`: the RFC 8288 `Link` header entry with `rel="next"`.
  - `next_url`: `next`, a body path to the next URL: `@odata.nextLink`, `@iot.nextLink`,
    `links[rel=next].href`, `_links.next` (an object's `href` is taken).
- Body paths join keys with `.`; a key holding a dot (`@odata.nextLink`) matches whole, `[n]`
  picks an index and `[key=value]` the first element whose `key` is `value`.
- Stop rules: an empty page, the `total` (read from the first page) reached, `more` false, a
  short page (page and offset modes without `more`), or no next cursor or URL.
- A run is complete only when a stop rule held. Each of these fails it: `max_pages` or
  `max_records` reached first, a total that does not match the records read, a page whose
  `records` path holds no array, a page naming itself (the same cursor or URL) as the next one.
  The input then ends with that error instead of the end of input, and a page the output nacked
  fails the next read. The processor flags the message with the error and hands on no page.
- Each page is one message with `http_page` (1-based) metadata; the processor also sets
  `http_page_count` on every page of a complete walk.
- A next URL must keep the configured scheme, host and port and carry no credentials, or the
  paging step is refused. Every page passes the outbound rule as any request.
- Refused together: `paginate` with `stream.enabled` on the input, with `batch_as_multipart` or
  `parallel` on the processor.

## `http_server`

ekiben extensions:

- **Ingest listener.** In `streams` mode, one shared listener (`--ingest-addr`) serves every
  `http_server` input at `/{org}/{stream}{path}`. In that mode `address`, `cert_file`, `key_file`
  and `ws_path` are lint errors, and `auth` is required. An unknown organization, stream or path
  answers `404 page not found`; another organization's stream looks exactly like an unknown one.
- **`auth`** takes exactly one of three forms:
  - `bearer: <token>`
  - `hmac: {header: X-Signature, secret: <secret>, algorithm: sha256}`: the header carries the hex
    HMAC of the body, optionally prefixed `sha256=`.
  - `public: true`

  Tokens and signatures are compared in constant time. A missing credential and a wrong one get
  the same `401`. The credential header never becomes message metadata, and `Debug` output
  redacts the secret.
- **`max_body_bytes`** defaults to 1 MiB and may be at most 16 MiB. A larger body gets `413`,
  including a chunked body sent without a length.
- **`max_in_flight`** defaults to 64. It caps the requests waiting for their answer; more get
  `503` with `Retry-After: 1`.
- **Answer after ack.** A request is answered only once its messages are acked by the output, or
  by `sync_response`. A nack answers `502` with the error.

Refused:

- `cert_file` / `key_file`: terminate TLS in front of ekiben.
- `ws_path`: websocket ingestion. Use the `websocket` input instead.
- Path variable patterns (`{name:regex}`).

## `websocket`

Every connection follows the outbound rule (`outbound.rs`):

- The name is resolved once and every resolved address is checked. The TCP connection then goes
  to a checked address, so DNS rebinding cannot swap the target.
- Loopback, link-local, cloud metadata, private and reserved ranges are refused unless the
  organization's `allow_private_ips` quota is on.
- `private_hosts` (an ekiben quota extension) lists exact names that may resolve to private
  ranges. Those names still never reach loopback, link-local or metadata addresses.
- TLS (`wss`) uses the system roots plus `tls.root_cas` (PEM text).

ekiben extensions:

- **`max_message_bytes`** defaults to 16 MiB and may be at most 64 MiB. A larger message closes
  the connection, which is then reconnected.
- **Reconnects** happen inside the read, with backoff from 1 s to 30 s.
- **Replay queue.** With `auto_replay_nacks` (Bento's default `true`), nacked messages are
  replayed from a queue capped at 1024 batches.

Refused: `proxy_url`, `jwt`, `tls.skip_cert_verify`, `tls.root_cas_file`, `tls.client_certs`.
