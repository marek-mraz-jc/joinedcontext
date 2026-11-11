# DataSource

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DataSource`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/datasources`
- Repository path: `projects/{project}/datasources/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DataSourceSpec)

Desired state of a `DataSource`: one connection and its credentials (MF-35).

| Field | Type | Required | Description |
|---|---|---|---|
| `gtfsRt` | [GtfsRtConnection](#gtfsrtconnection) | no | The GTFS-realtime connection, present when `type` is `gtfs-rt`. |
| `http` | [HttpConnection](#httpconnection) | no | The HTTP connection, present when `type` is `http`. |
| `input` | any | no | Verbatim input configuration document for runner-provided inputs (PL-50). |
| `mqtt` | [MqttConnection](#mqttconnection) | no | The MQTT connection, present when `type` is `mqtt`. |
| `personalFields` | list of string | no | The fields of this source's records that hold personal data (AG-158): a sample of the source answered to an MCP client or an agent run leaves them out and names them. |
| `secrets` | list of [SecretRef](#secretref) | no | Secret references for runner-provided inputs (PL-50, MF-35). |
| `tls` | [TlsSettings](#tlssettings) | no | Transport security for the connection above. |
| `type` | [DataSourceType](#datasourcetype) | yes | Which connection block below is the live one. |
| `webSocket` | [WebSocketConnection](#websocketconnection) | no | The WebSocket connection, present when `type` is `websocket`. |

## GtfsRtConnection

A GTFS-realtime feed (Architecture/08 §6).

| Field | Type | Required | Description |
|---|---|---|---|
| `feed` | [GtfsFeed](#gtfsfeed) | yes | Which of the three GTFS-realtime feeds this URL serves. |
| `url` | string | yes | The protobuf feed URL, `http://` or `https://`. |

## HttpConnection

An HTTP resource the runner polls (Architecture/08 §6).

| Field | Type | Required | Description |
|---|---|---|---|
| `archive` | [HttpArchive](#httparchive) | no | The members of a zip archive to read, by HTTP `Range`, instead of the whole body (PL-72). |
| `authorization` | [Authorization](#authorization) | no | The `Authorization` header, built from a secret (CC-06). |
| `format` | [HttpBodyFormat](#httpbodyformat) | no | How the body becomes messages: `csv` hands each row to the steps as one message keyed by the header's column names; absent, the whole body is one message (PL-114). |
| `headers` | map of string | no | Static request headers. A header carrying a credential belongs in `authorization`. |
| `maxBytes` | integer (uint64) | no | The largest response body accepted, in bytes; 32 MiB when absent, at most 256 MiB. The runner refuses a larger body before any step of the pipeline sees it (PL-113). |
| `records` | string | no | Where the records sit in a JSON body: a dotted path to an array, `data.stations` for a GBFS feed. The runner hands each element to the steps as one message; absent, the whole body is one message. Names only, never an expression (PL-114). |
| `timeout` | string | no | Request timeout as a Bento duration, `10s`. |
| `url` | string | yes | The URL to fetch, `http://` or `https://`. |
| `verb` | string | no | HTTP method, `GET` when absent. |

## MqttConnection

An MQTT connection (Architecture/08 §6).

| Field | Type | Required | Description |
|---|---|---|---|
| `brokerRef` | [TypedRef](#typedref) | no | An `MqttBroker` of the DataSource's own project, read on its internal listener with no credential (PL-76); takes the place of `urls`, `username` and `passwordRef`. |
| `cleanSession` | boolean | no | Whether the broker forgets the subscription between connections. |
| `passwordRef` | [SecretRef](#secretref) | no | Where the password comes from (CC-06). |
| `qos` | integer (uint8) | no | Quality of service, 0, 1 or 2. Absent leaves Bento's default. |
| `topics` | list of string | yes | Topic filters to subscribe to; at least one. |
| `urls` | list of string | no | Broker URLs, `tcp://host:1883` or `tls://host:8883`; at least one, unless `brokerRef` names a broker of the platform instead. |
| `username` | string | no | The user the runner authenticates as; the password is a reference. |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## TlsSettings

How the runner trusts the feed's certificate.

| Field | Type | Required | Description |
|---|---|---|---|
| `caCertRef` | [SecretRef](#secretref) | no | A private certificate authority the feed's certificate chains to. |
| `insecureSkipVerify` | boolean | no | Kept so a manifest can state the choice; only `false` is accepted (see [`DataSourceSpec::validate`]). |

## DataSourceType

Which kind of feed a DataSource connects to: a typed connection or any runner input (MF-35, PL-50).

Type: string

## WebSocketConnection

A WebSocket the runner keeps open (Architecture/08 §6).

| Field | Type | Required | Description |
|---|---|---|---|
| `openMessage` | string | no | A message sent once the socket opens, usually a subscription request. A socket that needs a credential authenticates inside this message: Bento's websocket input sends no `Authorization` header, and a field the renderer cannot honour would be a promise the runner breaks. |
| `url` | string | yes | The socket URL, `ws://` or `wss://`. |

## GtfsFeed

The three feeds GTFS-realtime defines.

Type: one of `"vehiclePositions"`, `"tripUpdates"`, `"alerts"`

## HttpArchive

The named members of the archive an `http` connection serves (PL-72, Architecture/08 §6).

The runner reads the directory from the archive's last 64 KiB and then each member by its own range, so a member nobody named is never fetched or inflated.

| Field | Type | Required | Description |
|---|---|---|---|
| `format` | [ArchiveFormat](#archiveformat) | yes | The archive format; `zip` is the only one. |
| `maxBytes` | integer (uint64) | no | The largest member accepted, packed and unpacked, in bytes; 32 MiB when absent, at most 256 MiB. A deflated member's inflate stops at the size its directory declares. |
| `members` | list of string | yes | The member names to read, at most 20, each letters, digits, `.`, `_`, `-` and `/`. |

## Authorization

An `Authorization` header whose value is assembled from a secret (CC-06).

| Field | Type | Required | Description |
|---|---|---|---|
| `header` | string | no | The header the credential is written into, `Authorization` when absent. An open-data feed that authenticates with an API key usually wants it in a header of its own; without this the only way to reach such a feed would be to write the key into `headers` as a value, which is what MF-35 exists to prevent. |
| `headerRef` | [SecretRef](#secretref) | yes | Where the credential comes from. |
| `scheme` | string | no | The scheme written before the secret. `Bearer` when absent on `Authorization`, and nothing at all on any other header: a vendor key header takes the bare key and refuses a scheme in front of it. An explicit scheme always wins. |

## HttpBodyFormat

How an `http` connection splits the body it fetched into messages (PL-114).

Type: `"csv"`

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## ArchiveFormat

The archive formats an `http` connection reads (PL-72).

Type: `"zip"`
