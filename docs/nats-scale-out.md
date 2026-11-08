# NATS JetStream, NATS KV and scale-out

ekiben's `nats_jetstream` input and output and its `nats_kv` cache, processor and output talk to
a real NATS server (`async-nats`) with the fields and defaults of Bento 1.22. A `nats://memory/...`
URL selects the in-process broker the unit tests use.

## 1. Fields

| Component | Fields (default) |
|---|---|
| `nats_jetstream` input | `urls`, `subject`, `queue`, `durable`, `stream`, `bind` (false), `deliver` (`all`; `last`, `last_per_subject`, `new`), `ack_wait` (30s), `max_ack_pending` (1024), `tls`, `auth` |
| `nats_jetstream` output | `urls`, `subject` (interpolated), `headers` (interpolated), `metadata.include_prefixes/include_patterns`, `max_in_flight` (1024), `tls`, `auth` |
| `nats_kv` processor | `urls`, `bucket`, `operation` (`get`, `get_revision`, `create`, `put`, `update`, `delete`, `purge`, `history`, `keys`), `key`, `revision`, `timeout`, `tls`, `auth` |
| `nats_kv` output and cache | `urls`, `bucket`, `key` (output), `max_in_flight`, `tls`, `auth` |

The input sets `nats_subject`, `nats_sequence_stream`, `nats_sequence_consumer`,
`nats_num_delivered`, `nats_num_pending`, `nats_timestamp_unix_nano`, `nats_domain` (only when the
server has a domain) and every message header. An acked message is acked on the server, a
failed one is Nak'ed and redelivered. The output acks a batch upstream only once JetStream stored
every message of it.

Consumers, as `nats.go` Subscribe does for Bento:

- `queue` set: a push consumer with that deliver group; `durable` defaults to the queue name.
- `durable` set, no `queue`: a pull consumer. Bento makes a push consumer here, which a second
  Bento instance cannot bind; ekiben's pull consumer can be shared. An existing consumer is used
  with its own type, so ekiben and Bento can take turns on one durable.
- neither: an ephemeral push consumer per instance (each instance reads every message).
- `bind: true`: the existing consumer `stream`/`durable` as it is (refused when it does not
  exist), or a new consumer on `stream`.

## 2. Security

- Credentials: `auth.user_credentials_file` and `auth.nkey_file` read a mounted secret;
  `auth.user_jwt`/`auth.user_nkey_seed` are held as redacted secrets and never logged. Give them
  through `${ENV}` interpolation from a Kubernetes Secret, never as literals in a config.
- TLS: `tls.enabled` verifies the server certificate against `tls.root_cas`/`tls.root_cas_file`
  (or the system roots). `skip_cert_verify: true`, `enable_renegotiation: true` and an encrypted
  client key (`client_certs[].password`) are refused when the stream is built.
- Organizations: an organization with `nats_subject_prefix: org_a` reads and writes only subjects
  `org_a` and `org_a.>`. This holds for `subject`, for a bound consumer's filter subjects and, when
  a consumer or `bind: true` stream has no filter, for every subject of the stream. A KV bucket
  lives under `$KV.<bucket>.>`, outside the prefix, so such an organization may only name buckets
  `org_a`, `org_a_<name>` or `org_a-<name>`.

## 3. Several instances share one source

Bento scales by running several instances on one source, each one taking part of the messages
through the source's own grouping, with no coordination between instances. ekiben does the same:

- JetStream: every replica runs the same stream config with `queue: <group>` (push consumer with
  a deliver group, Bento's pattern) or `durable: <name>` (shared pull consumer). JetStream hands each
  message to one replica; a replica that dies before acking has its in-flight messages redelivered
  to the others after `ack_wait` (at-least-once, as Bento).
- MQTT: a shared subscription `$share/<group>/<topic>` (T-6647).

### 3.1 Deployment pattern

A Kubernetes Deployment (or StatefulSet) with N replicas of `ekiben streams --config-dir
/etc/ekiben/streams`, the directory a ConfigMap mounted in every pod:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: ekiben-streams
data:
  hfp.yaml: |
    org: helsinki
    input:
      nats_jetstream:
        urls: [ nats://nats:4222 ]
        subject: helsinki.hfp.>
        queue: hfp-workers
        auth:
          user_credentials_file: /run/secrets/nats/helsinki.creds
    pipeline: { processors: [ ... ] }
    output: { ... }
  quotas.yaml: |
    helsinki:
      max_messages_per_sec: 50000
      nats_subject_prefix: helsinki
```

Each `*.yaml` is a stream (id = file stem) and `quotas.yaml` maps organization ids to their
quotas. Every replica reads the directory once a second: a new file starts a stream, a changed
one restarts that stream, a removed one stops it, and a quotas change applies to running streams
without restarting them (they pace by the new rate within a second). A file that does not parse,
an unknown quota field or a limit below 1 is logged and the last good version keeps running; at
start the first read must succeed or the pod exits. Kubernetes refreshes a mounted ConfigMap within
about a minute, so all replicas converge on one change from one source. The admin API
(`PUT /orgs/{org}/quotas`, `PUT /streams/{id}`) still changes one replica at once, for a test.

## 4. Evidence

- Parity: `scripts/nats-parity/parity.py --bento .bento/bento --ekiben <ekiben>` runs each case
  through Bento 1.22.0 and ekiben on fresh `nats:2.12-alpine -js` servers and compares stdout
  (payload and metadata), the consumer each created, the stream and the KV bucket afterwards.
- Scale: `scripts/nats-parity/scale.py --bento .bento/bento --ekiben <ekiben> -n 100000`.
- Integration tests: `cargo test -p ekiben-io-nats --test nats_live_tests` (a SKIP line without
  docker).

The measured tables are in T-6648.
