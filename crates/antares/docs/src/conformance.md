# Conformance

Antares implements ETSI GS CIM 009 V1.9.1. Conformance is tracked in two
places that check each other: the clause ledger in `docs/spec/`, and the
ETSI Robot Framework suite vendored at `ngsi-ld-test-suite/`, run in a
matrix of store modes on every push.

Every claim on this page is a claim about the broker as it ships: the
built-in `allow-all` policy engine, no addon crate, no feature this
repository leaves off by default. A deployment that attaches its own
policy engine ([Policy engines](extending.md#policy-engines)) narrows what
its callers are answered with, and the conformance statement no longer
describes what those callers see.

## The ledger

`docs/spec/` holds one file per clause of CIM 009, 947 files, each
carrying the clause text, the PDF pages it came from, and three
hand-maintained fields: `status`, `evidence` (code and test anchors) and
`notes`. The `robot:` list is generated: `python3 dev/spec.py robot`
scans the suite for `[Tags]` in the clause form (`5_6_6`) and writes the
matching TP names into the clause file.

| status | meaning |
|---|---|
| `implemented` | every SHALL of the clause holds, with evidence anchors |
| `partial` | a named gap in `notes` |
| `not-implemented` | audited, not yet built |
| `staged-v1x` | deferred to a later spec version by decision |
| `informative` | a heading, an umbrella clause or an informative annex; the requirements are audited in the leaf clauses it delegates to |

Current counts (`python3 dev/spec.py status`):

```text
947 sections
  implemented       476
  informative       468
  partial             3
  robot-tagged       193
```

A `partial` names its gap in `notes:` and is the honest status for a clause
whose normative surface is not fully closed; `not-implemented` is empty.
`python3 dev/spec.py check` fails on a malformed file, a stale `robot:` list
or a count in this chapter that no longer matches the ledger; `dev/spec.py
gaps` lists leaf clauses without a TP.

## The suite

The suite directory holds 672 `.robot` files under
`TP/NGSI-LD/{CommonBehaviours, ContextInformation, ContextSource,
DistributedOperations, jsonldContext}`. Most carry ETSI's own numbering
(`002_01`, `D018_01`); the rest are additions written here for normative
surface the official set leaves untested, either clause-numbered (`566_01`
for 5.6.6, `5510_01` for 5.5.10, `4233_01` for 4.23.3) or slotted into the
ETSI family they extend, and all following the same conventions and tagged
with their clause so the ledger picks them up. Every file expands to test
cases; one full run of a native cell is 1833 test cases:

| suite | test cases |
|---|---|
| CommonBehaviours | 65 |
| Consumption | 545 |
| EntityMap | 22 |
| Provision | 406 |
| Snapshot | 5 |
| Subscription | 156 |
| ContextSource | 145 |
| DistributedOperations | 135 |
| IOP | 286 |
| jsonldContext | 68 |

Behaviour Antares defines for itself, where CIM 009 is silent, does not go
in `TP/` — that directory is run against other brokers in interoperability
campaigns, so every file in it has to assert a SHALL the spec text carries.
Those tests live in `ngsi-ld-test-suite/AntaresSpecificTests/` instead, and
each says in its own documentation that it is an Antares decision rather
than a CIM 009 requirement, with the reason the behaviour exists.

## The matrix

The same 1833 cases run once per native cell; `wasm-file` runs 1821 of
them, the twelve MQTT cases having no broker socket to run against in the
browser build. Every push gates on the quick preset; the full preset runs
twice a week, on `v*` tags and on dispatch, and is what the report page and
the badges render.

| cell | store | preset | what it adds |
|---|---|---|---|
| file | redb file store | quick, full | durability across restart |
| postgres | PostGIS | quick, full | the production current-state path |
| timescale | PostGIS + TimescaleDB | quick, full | hypertable history, columnstore |
| memory | in-RAM | full | the zero-dependency binary |
| postgres-nats | PostGIS + NATS JetStream | full | ten containers in split roles, rolled during the run |
| timescale-nats | as above on TimescaleDB | full | same, on the temporal-heavy backend |
| wasm-file | browser artifact over the file store | full | five Node shims driving the WebAssembly build; MQTT excluded, the browser has no broker socket |

A native cell passes at 1833/1833, `wasm-file` at 1821/1821.
`dev/etsi-matrix-summary.py` folds the per-cell results into one table and
lists every failure across the matrix; a release requires that list to be
empty.

A smaller green run is still green, so the matrix also gates per TP.
`dev/etsi-baseline.json` names every TP a cell runs, keyed by file and
name, with the vendored suite's tree hash and the spec version it covers.
`dev/etsi-baseline.py check` fails a cell where a baseline TP did not pass
or did not run, and names it. `wasm-file` may lack only the 12 MQTT TPs. A
TP that passes and is not yet listed prints `ratchet: update baseline`;
regenerate the file with `dev/etsi-baseline.py write <results-dir>` from a
green run, or from a `robot --dryrun` folded by `dev/etsi-report.py`.

## Running a suite locally

One store mode per run, the one the change touches:

```bash
dev/etsi-local.sh                                  # workspace tests + memory cell
STORE=timescale dev/etsi-local.sh                  # one cell, all suites
STORE=all dev/etsi-local.sh                        # the quick trio, serially
STORE=postgres STOP_ON_ERROR=1 dev/etsi-pipeline.sh    # halt at the first red TP
STORE=file SUITES=Consumption,Subscription SKIP_BUILD=1 dev/etsi-pipeline.sh
```

`dev/etsi-pipeline.sh` knobs: `STORE`, `STOP_ON_ERROR` (default 1, CI
sets 0), `SKIP_BUILD` (reuse the local image), `SUITES` (comma list),
`MQTT=1` (include the `058_*` MQTT cases), `KEEP_UP=1` (leave the stack
running), `MEM_LIMIT_MB` (per-broker peak-RSS gate), `ROLES_SPLIT=1
ROLL_DURING_RUN=1` (reproduce a `-nats` cell), `WASM=1 WASM_DOCKER=1
STORE=file` (reproduce the wasm cell). Results land in `results/$STORE`
with Robot's own `log.html` per suite.

The suite names its own `@context` by a raw.githubusercontent.com URL on
`dev`, in `variables.py` and in 331 data and expectation files. A pipeline
run instead serves `resources/jsonld-contexts/` from the checkout on port
8079 (`dev/etsi-local-context.sh`; it refuses a port a suite file names, the
IOP mocks own 8089). It rewrites that URL for the length of
the run and puts the files back in teardown, so a commit is tested with its
own context file and a GitHub outage fails nothing. The cell fails when a
file still names the GitHub URL, or when no broker fetched the context from
the local server (`local-context-check.txt` and `local-context/access.log`
in the cell artifact).

For a single clause during development, one broker without Docker is
enough. `resources/variables.py` carries the suite's own compose addresses
(`scorpio1` for the broker, `172.28.0.18` for the notification and
context-source mocks), which the runners rewrite and a bare `robot` does
not, so the recipe overrides them itself:

```bash
cargo build -q -p antares-broker -j 2
ANTARES_HTTP_PORT=9377 ./target/debug/antares &
cd ngsi-ld-test-suite && robot --variable url:http://localhost:9377/ngsi-ld/v1 \
  --variable temporal_api_url:http://localhost:9377/ngsi-ld/v1 \
  --variable notification_server_host:127.0.0.1 \
  --variable context_source_host:127.0.0.1 \
  --variable context_server_host:127.0.0.1 \
  --variable mqtt_broker_host:ngsi-ld-test-suite-mosquitto-container \
  --outputdir /tmp/robot-566 TP/NGSI-LD/ContextInformation/Provision/Entities/DeleteEntity/566_01.robot
```

## Suite and spec defects

A red TP is proven against the clause text before any broker change. When
the text says the TP or the spec is wrong, the finding goes to
`docs/upstream/etsi-raises.md` as a ready-to-file issue, and the fork
carries the fix. The current list:

| # | target | finding |
|---|---|---|
| 1 | spec | CIM 009 6.3.17 scopes 508 to a single exclusive or redirect source; TS 104 175 9.7 makes every detected loop an error (`D018_01`) |
| 2 | suite | nine official `_exc` TPs create exclusive registrations that 4.3.6.3 forbids |
| 3 | suite | `LdContextNotAvailable` fixtures assert 503; V1.9.1 mandates 504 |
| 4 | spec | 5.3.4 SnapshotNotification: member naming conflict and a phantom `snapshotReady` |
| 5 | spec | Table 6.6.3.2-2 (Update Attributes 207): Data Type and Remarks conflict |
| 6 | spec | 5.7.4.4 / Table 5.2.21-1: `lastN` versus values-filter ordering is unspecified |
| 7 | spec | CIM 029 A.5.2.26 cites clause 5.15.4, which does not exist in CIM 009 V1.9.1 |
| 8 | openapi | v1.8.1 temporal GET operations declare the `options` parameter twice |
| 9 | suite | three temporal TPs assert `Content-Range` unit `date-time`; 6.3.10 mandates `DateTime` |
| 10 | spec | 4.5.19.0's unbounded period count contradicts the aggregation fixtures |
| 11 | spec | Annex B maps `attributeCount`/`attributeDetails` to bare terms |
| 13 | spec | TS 104 175 Table 5.2.6.5.2-1 calls `timeInterval` not applicable to CSR subscriptions; 12.4.2.4/12.4.7 define periodic CSR notification |
| 14 | spec | TS 104 176 clause 5 answers a successful HEAD with 204, its 7.x GET/HEAD tables with 200 (Antares: 200) |
| 15 | suite | 020_17 to 020_20 delete an Attribute of an Entity held only as a temporal evolution; 5.6.5.4 answers 404 |
| 16 | suite | 020_19 and 020_20 expect a deleted scope as `[]`; 4.18 makes it `urn:ngsi-ld:null` |
| 17 | spec | TS 104 176 answers Create and Clone Snapshot with 202, CIM 009 with 201, and names no version a client can prefer (Antares: 201) |
| 18 | suite | 021_08 expects temporal instances that fail `q`; 5.7.4.4 keeps only those that meet it |
| 19 | suite | 037_11 and 041_03 query without Accept and expect ld+json pagination Links; 6.3.10 types them after the response |
| 20 | suite | 051_04_01 expects 400 for an unknown @context id with `reload=true`; 5.13.5.4 raises ResourceNotFound first |
| 21 | suite | 008_01 expects a second temporal upsert to replace instances at the same observedAt; 5.6.11.4 adds them |
| 22 | suite | 046_22_08 reads a deleted three-instance Attribute as one object; 5.8.6 tombstones each instance with its datasetId |
| 23 | suite | 046_05 percent-encodes the q of a Subscription body; Table 5.2.12-1 makes it a JSON string read as written |
| 24 | spec | TS 104 176 9.4.3.1 and 9.4.3.2 carry each other's headings: Create Context Source Registration Subscription is headed GET/HEAD, Query is headed POST; Antares serves POST for create, GET for query |
| 25 | suite | 054_02_02 and 056_02_02 expect 400 for PUT and PATCH on /entities; TS 104 176 6.3.2 raises 405 for a recognized but unsupported verb |
| 26 | suite | 003_06_01 and 003_08_01 expect 207 for a batch whose Content-Type and @context disagree; TS 104 176 6.2.4 raises one BadRequestData error response |
| 27 | suite | the 047 TPs take the initial csource notification TS 104 175 12.4.7 mandates for the one a registration triggers |
| 28 | suite | integration/all-fixes TPs that over-specify: Link whitespace, `langProperties`, jsonKeys on a JsonProperty, `pick` only in a forward stub, a lastN page above the configured limit |

Filing is manual; each entry carries the clause quotation and the
proposed fix so it can be pasted into the ETSI tracker as is.

## Spec-statement coverage

The ledger says implemented or partial per clause; this table says which
of those clauses no Robot test exercises at all. `python3 dev/spec.py
statements` counts the SHALL statements in each leaf clause's text against
the TPs tagged with its number (or its operation's number) and the
code/test anchors its evidence cites. It adds no tests; it names where the
next ones belong.

310 leaf clauses carry 1661 SHALL statements; 117 of them have no Robot
TP (338 SHALLs), 62 cite no code/test anchor.

The fifteen untested clauses with the most SHALL statements:

| clause | title | SHALL | robot TPs | code/test anchors |
|---|---|---:|---:|---:|
| 6.18.3.2 | Resource methods › GET | 23 | 0 | 1 |
| 6.8.3.2 | Resource methods › GET | 17 | 0 | 1 |
| 6.5.3.1 | Resource methods › GET | 11 | 0 | 0 |
| 5.2.39 | EntityMap | 10 | 0 | 4 |
| 4.2.3 | Cross Domain Ontology | 8 | 0 | 1 |
| 5.2.35 | VocabProperty | 8 | 0 | 0 |
| 5.2.38 | JsonProperty | 8 | 0 | 0 |
| 7.2 | Notification behaviour | 8 | 0 | 2 |
| 5.2.36 | ListProperty | 7 | 0 | 0 |
| 5.2.5 | Property | 7 | 0 | 0 |
| 5.2.7 | GeoProperty | 7 | 0 | 0 |
| 6.3.8 | Notification behaviour | 7 | 0 | 6 |
| 5.2.37 | ListRelationship | 6 | 0 | 0 |
| 5.2.6 | Relationship | 6 | 0 | 0 |
| 5.3.4 | SnapshotNotification | 6 | 0 | 2 |

A SHALL count is a proxy: one sentence can carry several rules, and a
clause's unit tests (the anchors column) may assert what no TP does. The
counts are a snapshot to regenerate after ledger or suite changes, not a
gate.
