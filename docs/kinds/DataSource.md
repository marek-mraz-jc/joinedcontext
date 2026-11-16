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
| `webhook` | [WebhookConnection](#webhookconnection) | no | The webhook a sender pushes to, present when `type` is `webhook`. |

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
| `format` | [HttpBodyFormat](#httpbodyformat) | no | How the body becomes messages: `csv` or `xlsx` hands each row to the steps as one message keyed by the header's column names, `shapefile` each record of a zipped shapefile as a WGS 84 GeoJSON Feature; absent, the whole body is one message (PL-114). |
| `headers` | map of string | no | Static request headers. A header carrying a credential belongs in `authorization`. |
| `maxBytes` | integer (uint64) | no | The largest response body accepted, in bytes; 32 MiB when absent, at most 256 MiB. The runner refuses a larger body before any step of the pipeline sees it (PL-113). |
| `paging` | [HttpPaging](#httppaging) | no | How the runner walks a paged source page by page (PL-31, T-7056): each page is one message, split by `records` as a single body is. Absent, the URL is read once. |
| `preset` | [HttpPreset](#httppreset) | no | A known kind of open data API (T-7067): the runner sends its query parameters and walks its pages, so `paging` and `records` stay unset. CKAN, ArcGIS, OData, WFS, OGC API Features or SensorThings, with the URL of its search, query or items resource. |
| `records` | string | no | Where the records sit in a JSON body: a dotted path to an array, `data.stations` for a GBFS feed. The runner hands each element to the steps as one message; absent, the whole body is one message. Names only, never an expression (PL-114). |
| `sheet` | string | no | The sheet of an `xlsx` body to read, by its name as the workbook's tab shows it (`Stations 2026`); absent, the first sheet (T-7086). |
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

## WebhookConnection

A pushed feed: the runner's shared ingest listener answers its POSTs (PL-122, ADR-N-071).

Only the reconciler renders the `http_server` input this becomes, so the managed door keeps authentication and both caps whatever a manifest says; an author-written `http_server` stays refused (PL-50).

| Field | Type | Required | Description |
|---|---|---|---|
| `auth` | [WebhookAuth](#webhookauth) | yes | How a sender proves it may post: exactly one of `hmac` and `bearer`. |
| `maxBodyBytes` | integer (uint64) | no | The largest body accepted, in bytes (1 to 16777216; 1048576 when absent). A larger one answers 413 before any step runs. |
| `maxInFlight` | integer (uint32) | no | How many requests may wait for their answer at once (1 to 1024; 64 when absent); more answer 503. |

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

Type: one of `"csv"`, `"xlsx"`, `"shapefile"`

## HttpPaging

How a paged `http` source is walked (PL-31, T-7056): rendered to the runner's `paginate` option, so no mapping writes its own loop. `records` (`spec.http.records`) names each page's array; `page`, `offset`, `total` and `maxRecords` count it. A run is complete only when a stop rule held: an empty or short page, the total reached, `more` false, or no next cursor or URL.

| Field | Type | Required | Description |
|---|---|---|---|
| `cursor` | string | no | `cursor`: the body path to the next cursor, `meta.next`. |
| `cursorHeader` | string | no | `cursor`: the header the cursor is sent in, instead of a parameter. |
| `cursorParam` | string | no | `cursor`: the query parameter the cursor is sent in. |
| `delay` | string | no | The pause between pages as a duration, `200ms`, at most a minute. |
| `limit` | integer (uint64) | no | `offset`: the page size asked for. The offset moves by the records received. |
| `limitParam` | string | no | `offset`: the page size parameter, `limit` when absent (ArcGIS `resultRecordCount`). |
| `maxPages` | integer (uint64) | no | The most pages one run reads, 1 to 10000, 1000 when absent; reaching it fails the run. |
| `maxRecords` | integer (uint64) | no | The most records one run reads; reaching it before a stop rule fails the run. |
| `mode` | [HttpPagingMode](#httppagingmode) | yes | `page`, `offset`, `cursor`, `link` (RFC 8288 `rel=next`) or `nextUrl`. |
| `more` | string | no | The body path to a boolean saying another page exists, `exceededTransferLimit`. |
| `next` | string | no | `nextUrl`: the body path to the next URL, `@odata.nextLink` or `links[rel=next].href`. |
| `offsetParam` | string | no | `offset`: the offset parameter, `offset` when absent (ArcGIS `resultOffset`, OData `$skip`). |
| `orderBy` | [HttpPagingOrder](#httppagingorder) | no | A parameter that keeps the order stable across pages, `orderByFields=OBJECTID`. |
| `param` | string | no | `page`: the page number parameter, `page` when absent. |
| `size` | integer (uint64) | no | `page`: the page size asked for; a shorter page ends the walk. |
| `sizeParam` | string | no | `page`: the page size parameter, sent with `size`. |
| `start` | integer (uint64) | no | `page` and `offset`: the first page number (1) or offset (0). |
| `total` | string | no | The body path to the count the first page announces, `numberMatched`; a run that read another number of records fails. |

## HttpPreset

A known kind of open data API an `http` connection reads (T-7067): the preset sends the API's own query parameters and walks its pages, so a person picks the kind and the URL and types no parameter. Each field belongs to the kinds its comment names; another kind refuses it.

| Field | Type | Required | Description |
|---|---|---|---|
| `expand` | string | no | `sensorThings`: the related entities read with each one, `Datastreams/Observations`. |
| `kind` | [HttpPresetKind](#httppresetkind) | yes | `ckan`, `arcgisFeature`, `odata`, `wfs`, `ogcFeatures` or `sensorThings`. |
| `objectIdField` | string | no | `arcgisFeature`: the field that keeps the order stable across pages, `OBJECTID` when absent. |
| `organization` | string | no | `ckan` (`package_search`): the one organization whose datasets are read, `bcn`. |
| `outFields` | list of string | no | `arcgisFeature`: the fields read, named one by one (never `*`), so a new column of the layer reaches no pipeline unasked. |
| `pageSize` | integer (uint64) | no | Every kind: the records asked for per page, 1 to 10000, 1000 when absent. `odata` with a page size pages by `$top`/`$skip`; without one it follows `@odata.nextLink`. |
| `resourceId` | string | no | `ckan` (`datastore_search`): the resource whose rows are read. |
| `srsName` | string | no | `wfs`: the coordinate system asked for, `EPSG:4326` when absent. |
| `typeNames` | string | no | `wfs`: the feature type read, `avoindata:Helsinki_osoiteluettelo`. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## WebhookAuth

A webhook's authentication; there is no `public` option.

| Field | Type | Required | Description |
|---|---|---|---|
| `bearer` | [WebhookBearer](#webhookbearer) | no | `Authorization: Bearer <token>`. |
| `hmac` | [WebhookHmac](#webhookhmac) | no | The body's hex SHA-256 HMAC in the provider's header, with or without `sha256=`. |

## ArchiveFormat

The archive formats an `http` connection reads (PL-72).

Type: `"zip"`

## HttpPagingMode

How a paged source names its next page.

Type: one of `"page"`, `"offset"`, `"cursor"`, `"link"`, `"nextUrl"`

## HttpPagingOrder

The order parameter of a paged source.

| Field | Type | Required | Description |
|---|---|---|---|
| `param` | string | yes | The parameter name, `orderByFields` or `$orderby`. |
| `value` | string | yes | Its value, `OBJECTID`. |

## HttpPresetKind

The open data APIs a preset reads.

Type: one of `"ckan"`, `"arcgisFeature"`, `"odata"`, `"wfs"`, `"ogcFeatures"`, `"sensorThings"`

## WebhookBearer

A webhook that sends a fixed bearer token.

| Field | Type | Required | Description |
|---|---|---|---|
| `tokenRef` | [SecretRef](#secretref) | yes | The token the sender presents. |

## WebhookHmac

An HMAC-signed webhook (GitHub's `X-Hub-Signature-256`, a form tool's own header).

| Field | Type | Required | Description |
|---|---|---|---|
| `header` | string | yes | The header the provider writes its signature into, such as `X-Hub-Signature-256`. |
| `secretRef` | [SecretRef](#secretref) | yes | The shared signing secret. |
