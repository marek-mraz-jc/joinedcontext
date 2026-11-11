# Pipeline

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Pipeline`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/pipelines`
- Repository path: `projects/{project}/pipelines/{name}/pipeline.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (PipelineSpec)

Desired specification of a `Pipeline` resource (PL-01..PL-28).

Two shapes (PL-54, ADR-N-023): `v1alpha1` holds one `source`, one `compute`, one `output` and `targetEndpoint`; `v1alpha2` holds `sources`, `steps` and `outputs`. The accessors [`PipelineSpec::sources`], [`PipelineSpec::steps`] and [`PipelineSpec::outputs`] read either as the second, so nothing downstream has to know which one a file was written in.

| Field | Type | Required | Description |
|---|---|---|---|
| `allowFeedback` | boolean | no | The author knows this resident pipeline triggers itself and accepts it (PL-37). A derived pipeline whose output type is the type its own subscription watches feeds its own trigger, and the reconciler refuses it. Setting this says the loop is deliberate, which is why PL-37 puts the change in the yellow lane: a person reviews it. |
| `class` | [PipelineClass](#pipelineclass) | yes | Operational execution class (PL-04, PL-05). |
| `compute` | [Compute](#compute) | no | Optional compute execution specification (PL-33). |
| `enabled` | boolean | no | Whether the pipeline runs (PL-40). `false` pauses it: the reconciler leaves a resident stream out of the runner and suspends a scheduled CronJob, the manifest stays, and a Resume is the flag flipped back. Absent means `true`, and `true` is not written back. |
| `expiry` | [Expiry](#expiry) | no | Deletes the entities its source stopped sending; absent, nothing is deleted (PL-64). |
| `history` | list of [History](#history) | no | Which history rows the broker keeps of each written attribute and where the time of a row comes from (PL-106…PL-110). |
| `lifetime` | list of [Lifetime](#lifetime) | no | When entities of a written type, or chosen attributes of them, end (PL-80…PL-83): the rendered stream writes their `expiresAt` after the author's steps. |
| `output` | [Output](#output) | no | Optional output entity type and write mode. |
| `outputs` | list of [PipelineOutput](#pipelineoutput) | no | Where the result is written, each through its own Endpoint (PL-52, PL-55); `v1alpha2`. |
| `period` | string | no | How often the pipeline polls its source, as a Bento duration (`15s`, `5m`). The reconciler reads it to pick the class (PL-26) and copies it into the runner's `input.generate.interval` (PL-27). Absent means push-based: an MQTT or subscription input driven by its source rather than by a clock. |
| `quotas` | [PipelineQuotas](#pipelinequotas) | no | Resource quotas allocated to this pipeline runner (PL-11). |
| `routes` | list of [PipelineRoute](#pipelineroute) | no | Routes by condition (PL-73): a message after `steps` takes the first route whose `when` holds; one no route takes is written through `outputs`; `v1alpha2`. |
| `schedule` | string | no | Standard cron schedule expression for scheduled runs (PL-04, PL-26..PL-28). |
| `secretRefs` | list of [SecretRef](#secretref) | no | Secret references injected into runner environments (PL-14..PL-16). |
| `source` | [PipelineSource](#pipelinesource) | no | Optional context source query or subscription trigger for derived pipelines (PL-31). |
| `sourceDeletes` | [SourceDeletes](#sourcedeletes) | no | How the pipeline follows a record its source removed (PL-96…PL-100); absent, nothing is deleted, as with `mode: never`. |
| `sources` | list of [PipelineSource](#pipelinesource) | no | The inputs, merged by a Bento `broker` when there are several (PL-52, PL-53); `v1alpha2`. |
| `steps` | list of [Step](#step) | no | The processors, in the order they run (PL-52); `v1alpha2`. |
| `targetEndpoint` | [Urn](#urn) | no | Target endpoint URN through which writes occur (PF-39, PL-18); `v1alpha1` only. |

## PipelineClass

Operational execution class of a data pipeline (PL-04, PL-05).

Type: one of `"auto"`, `"resident"`, `"scheduled"`

## Compute

Compute execution specification (PL-33).

| Field | Type | Required | Description |
|---|---|---|---|
| `bloblang` | string | no | The inline mapping of a `bloblang` step (PL-41): the reconciler renders it as the last `mapping` processor of the generated `bento.yaml`. Absent means the author keeps the mapping in `bento.yaml`. |
| `datacube` | [DatacubeStep](#datacubestep) | no | The cubes of a `datacube` step (PL-112). |
| `function` | string | no | Entry point function name (for wasm). |
| `kind` | [ComputeKind](#computekind) | yes | Engine kind executing compute logic. |
| `kpi` | [KpiStep](#kpistep) | no | The indicator of a `kpi` step over several sources (PL-102…PL-104). |
| `mappingRef` | [Ref](#ref) | no | Reference to a `Mapping` resource. |
| `module` | string | no | Path or package of the compute module (for wasm). |

## Expiry

Stale-entity expiry: entities of `types` not modified for `after` are deleted (PL-64).

| Field | Type | Required | Description |
|---|---|---|---|
| `after` | string | yes | Whole hours or days, `1h` to `365d`. |
| `types` | list of string | yes | The entity types the sweep deletes, one to twenty. |

## History

The history plan of one written type (PL-106).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | list of [AttributeHistory](#attributehistory) | yes | One to 64 entries, each attribute in one of them. |
| `type` | string | yes | The entity type, one an output of the pipeline writes. |

## Lifetime

The lifetime of one written type (PL-80): the whole entity, chosen attributes, or both.

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | list of [AttributeLifetime](#attributelifetime) | no | When chosen attributes end: the `expiresAt` member of each named attribute. |
| `entity` | [LifetimeMode](#lifetimemode) | no | When the whole entity ends: its `expiresAt`. |
| `type` | string | yes | The entity type, a class of the target space's model that an output writes. |

## Output

Output specification for derived pipeline computations (PL-32).

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | [IdMint](#idmint) | no | How the renderer sets each entity's id (ADR-N-041 §3.4, PF-44); absent, the mapping sets it itself. |
| `mode` | [OutputMode](#outputmode) | yes | Write mode applied to target context space. |
| `type` | string | yes | Target entity type name produced. |

## PipelineOutput

One output of a `v1alpha2` pipeline (PL-52): the Endpoint written through and how.

| Field | Type | Required | Description |
|---|---|---|---|
| `mode` | [OutputMode](#outputmode) | no | Write mode; absent is `upsert`. |
| `targetEndpoint` | [Urn](#urn) | yes | The Endpoint the writes go through (PF-39, PL-18). |
| `type` | string | no | The entity type written, when the pipeline names one. |

## PipelineQuotas

Resource quotas for a pipeline runner (PL-11).

| Field | Type | Required | Description |
|---|---|---|---|
| `cpuMillicores` | integer (uint32) | no | CPU allocation in millicores. |
| `maxMemoryMb` | integer (uint32) | no | Maximum allowed memory allocation in megabytes. |

## PipelineRoute

One route of a `v1alpha2` pipeline (PL-73): its condition, its own steps and its outputs.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | A DNS label, unique among the pipeline's routes. |
| `outputs` | list of [PipelineOutput](#pipelineoutput) | yes | Where the route writes, each through its own Endpoint (PL-55). |
| `steps` | list of [Step](#step) | no | The route's own steps, after the pipeline's `steps`. |
| `when` | string | yes | A Bloblang query; the messages it is true for take this route. |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## PipelineSource

Source specification for derived pipelines (PL-31).

| Field | Type | Required | Description |
|---|---|---|---|
| `dataSourceRef` | [Ref](#ref) | no | Reference to the `DataSource` whose connection becomes this pipeline's Bento input (PL-39, MF-35). Excludes `endpointRef`: a pipeline reads the outside world or the platform's own spaces, not both in one input. |
| `endpointRef` | [Ref](#ref) | no | Reference to source Endpoint for reading context entities. |
| `name` | string | no | The input's name, by which a `kpi` step's formula reads it (PL-102); a DNS label unique among the sources. |
| `query` | [SourceQuery](#sourcequery) | no | Query parameters for pulling entities. |
| `trigger` | [Trigger](#trigger) | no | Event subscription trigger for resident processing. |

## SourceDeletes

`spec.sourceDeletes` (PL-96).

| Field | Type | Required | Description |
|---|---|---|---|
| `deletedWhen` | string | no | `events`: a Bloblang query on the mapped record, true when the source says the record is deleted (for example `this.status.value == "withdrawn"`). |
| `maxShare` | integer (uint8) | no | `snapshot`: the most of the ledger, in percent (1 to 100, default 10), one run deletes; a run that would delete more deletes nothing and is held. |
| `misses` | integer (uint8) | no | `snapshot`: consecutive complete runs an id is missing from before it goes, 1 to 10 (default 2). |
| `mode` | [SourceDeleteMode](#sourcedeletemode) | yes | `never`, `snapshot`, `events` or `lease`. |

## Step

One step of a `v1alpha2` pipeline (PL-52): a compute kind of PL-33, or one processor the pinned runner ships, its configuration verbatim in the processor's own field names.

Type: [ProcessorStep](#processorstep) or [Compute](#compute)

## Urn

NGSI-LD entity URN (ADR-N-041, PF-43): urn:ngsi-ld:{Type}:{id}; the prefixed shape urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId} is optional

Type: string

## DatacubeStep

A `datacube` step (PL-112).

| Field | Type | Required | Description |
|---|---|---|---|
| `cubes` | map of [Cube](#cube) | yes | The cubes by their code, `vh3001rr`. |
| `datasetPage` | string | yes | The catalogue page of a cube, `{cube}` standing for its code: `https://opendata.example.org/dataset/susr-{cube}`. |
| `wholeArea` | string | yes | The territory code of a cube without an area dimension, `SK0` for Slovakia. |

## ComputeKind

Execution technology category for pipeline compute steps (PL-33).

Type: one of `"bloblang"`, `"mapping"`, `"wasm"`, `"container"`, `"kpi"`, `"datacube"`

## KpiStep

A `kpi` step (PL-102).

| Field | Type | Required | Description |
|---|---|---|---|
| `formula` | string | yes | Arithmetic over `sum\|avg\|min\|max(input.attribute)` and `count(input)` (PL-103), a term read one period back as `prev(sum(input.attribute), P7D)` (PL-117). |
| `join` | [Join](#join) | no | How the sources' rows meet; `by: time` when absent. |
| `name` | string | yes | The indicator's `{localId}`; a key's slug is appended for an entity or area join. |
| `title` | string | no | A title for people. |
| `unitCode` | string | yes | The result's UN/CEFACT common code. |
| `window` | string | no | The window the value covers, `PT15M`, `PT1H`, `P1D`; `PT1H` when absent. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## AttributeHistory

The plan of chosen attributes of a type (PL-106).

| Field | Type | Required | Description |
|---|---|---|---|
| `heartbeat` | string | no | With `changes` only: a row at least this often, `PT1M` to `P3650D`. |
| `keep` | [Keep](#keep) | yes | Which rows the broker keeps. |
| `names` | list of string | yes | One to 64 slots of the type. |
| `observedAt` | string | no | A date-time slot of the type whose value is the row's time, or [`RECEIVED`]; absent, the mapping decides. |

## AttributeLifetime

The lifetime of chosen attributes of a type (PL-80): `names` and one mode.

| Field | Type | Required | Description |
|---|---|---|---|
| `at` | string | no | See [`LifetimeMode::at`]. |
| `from` | string | no | See [`LifetimeMode::from`]. |
| `names` | list of string | yes | One to 64 slots of the type. |
| `notAfter` | string | no | See [`LifetimeMode::not_after`]. |
| `plus` | string | no | See [`LifetimeMode::plus`]. |
| `rolling` | string | no | See [`LifetimeMode::rolling`]. |

## LifetimeMode

One of three modes (PL-80): `from` a field (with `plus` and `notAfter`), `rolling`, or `at`.

| Field | Type | Required | Description |
|---|---|---|---|
| `at` | string | no | One fixed RFC 3339 instant with an offset. |
| `from` | string | no | A date-time slot of the type, or `observedAt`. |
| `notAfter` | string | no | Caps a `from` end at the write time plus this duration. |
| `plus` | string | no | Added to the `from` time, an ISO 8601 duration. |
| `rolling` | string | no | The write time plus this duration, renewed by each write. |

## IdMint

The mint option of a pipeline's output (ADR-N-041 §3.4, Architecture/08).

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | string | no | The record field the id is made from (`prefixed`, `keep`): a path such as `a.b_c`. |
| `mint` | [Mint](#mint) | yes | `prefixed`, `keep` or `template`. |
| `template` | string | no | The id with `{field}` placeholders (`template`), starting `urn:ngsi-ld:{Type}:`. |

## OutputMode

Mutation mode for pipeline output writers (PL-32).

Type: one of `"upsert"`, `"replace"`, `"update-attrs"`

## SourceQuery

NGSI-LD query parameters for pipeline source entity retrieval.

| Field | Type | Required | Description |
|---|---|---|---|
| `attrs` | list of string | no | Attributes projection list. |
| `geoQ` | string | no | Geographic query filter string. |
| `ids` | list of [Urn](#urn) | no | The entities to read instead of the whole type (PL-42): PF-42 URNs whose `{Type}` is `type` when both are set. Rendered as the `id=` parameter of the fetch. |
| `q` | string | no | Entity query filter string. |
| `scopeQ` | string | no | Scope query filter string. |
| `temporalQ` | [TemporalWindow](#temporalwindow) | no | Temporal window constraint. |
| `type` | string | no | Target entity type short name. |

## Trigger

Subscription trigger for resident pipelines (PL-31).

| Field | Type | Required | Description |
|---|---|---|---|
| `subscription` | [SubscriptionTrigger](#subscriptiontrigger) | yes | Underlying context subscription details. |

## SourceDeleteMode

How a pipeline follows deletions at its source (PL-96).

Type: one of `"never"`, `"snapshot"`, `"events"`, `"lease"`

## ProcessorStep

A step that names a runner processor (PL-52).

| Field | Type | Required | Description |
|---|---|---|---|
| `processor` | object | yes | Exactly one entry: the processor's name and its configuration. |

## Cube

One cube of a `datacube` step: which dimension plays which role.

| Field | Type | Required | Description |
|---|---|---|---|
| `area` | string | no | The territory dimension; absent, every cell is `wholeArea`. |
| `indicator` | string | no | The indicator dimension; absent, the cube's code is the indicator. |
| `keys` | list of string | no | Further dimensions, in order, that tell cells apart (`dimensionKey`). |
| `period` | string | no | The period dimension, `vh3001rr_rok`. |
| `source` | string | yes | The address the cube is read from, as its DataSource reads it. |
| `unit` | string | no | The unit dimension, whose label is the cell's `unitText`. |
| `update` | string | no | The release date the publisher gives, `2025-10-31`; the answer's own `update` wins. |

## Join

How the sources' rows meet (PL-102, PL-104).

| Field | Type | Required | Description |
|---|---|---|---|
| `by` | [JoinBy](#joinby) | no | `time`, `entity` or `area`. |
| `keys` | map of string | no | Per source name, the attribute (or `id`) naming the entity or area a row is about. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## Keep

What the broker keeps of an attribute's writes (PL-106).

Type: one of `"every"`, `"changes"`, `"latest"`

## Mint

How an output's ids are made (ADR-N-041 §3.4).

Type: one of `"prefixed"`, `"keep"`, `"template"`

## TemporalWindow

ISO 8601 temporal window for source queries.

| Field | Type | Required | Description |
|---|---|---|---|
| `window` | string | yes | ISO 8601 duration string (e.g. `P1D`). |

## SubscriptionTrigger

Subscription configuration triggering pipeline execution.

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | Target entity type name. |
| `watchedAttributes` | list of string | no | Whitelist of watched attribute names. |

## JoinBy

The join kinds.

Type: one of `"time"`, `"entity"`, `"area"`
