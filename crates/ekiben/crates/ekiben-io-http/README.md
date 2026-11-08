# ekiben-io-http

HTTP components of ekiben: the `http` processor, the `http_client` input and output, the
`http_server` input with the shared ingest listener, and the `websocket` input (feature
`websocket`, on by default).

Bento 1.22 field names, defaults and answers are kept. What ekiben adds or refuses is listed
here.

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
