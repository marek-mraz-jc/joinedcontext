# Surfaces

A surface is an HTTP API that a cargo feature adds beside the NGSI-LD root,
under a reserved prefix. It reaches context data only through a
[view](views.md) and the operation layer `antares-ops` (ADR-0027). The
default build links none of them, and its ETSI result is the
[Conformance](conformance.md) page. This chapter ties the surface pages
together. It covers features, configuration, module rules, what each
surface hides, and which CI job proves each surface's conformance.

## 1. Features and mounts

| Surface | Cargo feature | `ANTARES_API_SURFACES` name | Endpoint | Page |
|---|---|---|---|---|
| NGSI-LD view root | `views` | `views` | `/x/views/{view}/ngsi-ld/v1` | [Endpoint views](views.md) |
| MCP | `mcp` (adds `views`) | `mcp` | `/x/views/{view}/mcp` | [MCP endpoint of a view](mcp.md) |
| LinkML | `linkml` (adds `views`) | `linkml` | `/x/views/{view}/linkml` | [LinkML schema of a view](linkml.md) |
| OData | `odata` (adds `views`) | `odata` | `/x/views/{view}/odata/v4` | [OData endpoint of a view](odata.md) |
| OGC API - Features | `ogc` (adds `views`) | `ogc` | `/x/views/{view}/ogc/features` | [OGC API - Features endpoint of a view](ogc.md) |
| SensorThings API | `sta` (adds `views`) | `sta` | `/x/views/{view}/sta/v1.1` | [SensorThings API endpoint of a view](sta.md) |
| SensorThings MQTT | `sta` and `ws` | `sta-mqtt` | `/x/views/{view}/sta/mqtt` | [SensorThings API endpoint of a view](sta.md#5-mqtt-subscriptions) |
| iCalendar | `ical` (adds `views`) | `ical` | `/x/views/{view}/ical/v1/events.ics` | [iCalendar feed of a view](ical.md) |
| GeoJSON of a view | `geojson` (adds `views`) | `geojson` | `/x/views/{view}/geojson/v1/{type}` | [GeoJSON of a view](geojson.md) |
| Feeds of a view | `feed` (adds `views`) | `feed` | `/x/views/{view}/feed/v1/{name}/{atom.xml\|rss.xml\|feed.json}` | [Feeds of a view](feed.md) |
| GBFS of a view | `gbfs` (adds `views`) | `gbfs` | `/x/views/{view}/gbfs/v1/{name}/{file}.json` | [GBFS of a view](gbfs.md) |
| GTFS Realtime of a view | `gtfsrt` (adds `views`) | `gtfsrt` | `/x/views/{view}/gtfsrt/v1/vehicle-positions.{pb,json}` | [GTFS Realtime of a view](gtfsrt.md) |
| JSON-stat of a view | `jsonstat` (adds `views`) | `jsonstat` | `/x/views/{view}/jsonstat/v1/{table}` | [JSON-stat of a view](jsonstat.md) |
| WebSocket notifications | `ws` | `ws`, and `ws-views` for a view | `/x/ws/ngsi-ld/v1`, `/x/views/{view}/ws` | [WebSocket notifications](websocket.md) |
| Internal context sources | `internal-csr` (adds `views`) | none (it serves forwards, not requests) | none | [Internal context sources](internal-context-sources.md) |

`full` is `views`, `mcp`, `odata`, `ogc`, `sta` and `ws`. `parquet` (the OGC
`items` as a Parquet file, [OGC](ogc.md#2-items)), `tabular` (CSV
files and a ZIP of a CSV per collection) and `ical` (a view's Events as an
iCalendar feed, [iCalendar](ical.md)) are named beside it,
because they were kept out of `full` under its ADR-0027 size budget of
vanilla + 10 MiB (15 MiB since T-5246). `linkml` and
`internal-csr` stay separate. A feature links a surface; only
`ANTARES_API_SURFACES` mounts one ([Configuration](configuration.md)). A
name the binary was not built with stops the broker at start.

## 2. Configuration

- **Views**: written to [`/q/views`](admin-api.md#get-qviews), reloaded on
  every pod each `ANTARES_VIEWS_RELOAD_MS`. A surface serves only through
  a loaded view: with none, it answers `404` and `/q/health` reports it
  `closed` (rule M4).
- **Schema**: the Tenant's LinkML, written to
  [`/q/schemas/{tenant}`](admin-api.md#schemas) and bounded by
  `ANTARES_SCHEMA_MAX_BYTES`. OData, OGC and LinkML build their models from
  it; without it they answer `404`.
- **Grants and self URLs**: `internal-csr` forwards in process to the URLs
  in `ANTARES_SELF_URLS`, under an export grant written to `/q/grants`
  ([Internal context sources](internal-context-sources.md)).
- **Policy**: every surface asks `ANTARES_POLICY` through the same
  operation layer, so a caller sees what the view and the engine both
  allow (`view ∧ engine`, ADR-0023 rule 5).

## 3. Module rules

`dev/check-module-rules.py` runs in CI on every push (workspace job). It
checks the rules of ADR-0027 section 2 from `cargo metadata`:

- **M1**: a protocol crate (`odata-*`, `ogc-*`, `linkml-model`) depends on
  no `antares-*` crate. Each surface sits behind its own broker feature,
  and the broker's `default` feature reaches none.
- **M2**: a surface depends inside the workspace only on `antares-ops` and
  its protocol crates, and `antares-ops` never on the store, SQL, bus or
  notifier. No surface can reach the store by a dependency (threat F1).
- **M3**: the vanilla build links no surface. `ci/vanilla-tree.txt` is the
  committed feature tree, and `dev/check-vanilla-symbols.sh` checks the
  binary.
- **M4**: a surface with no view serves nothing.

## 4. What each surface hides

A view selects Entities and shows Attributes. Every surface answers a
hidden name as it answers an absent one, so a caller learns nothing it
could not already read. Each row names the test that holds the claim.

| Surface | What is hidden, and how | Test |
|---|---|---|
| NGSI-LD view root | a hidden term never reaches the store | `antares-views` `views_q::a_hidden_term_never_reaches_the_store` |
| NGSI-LD view root | errors name no hidden term and carry no inner detail | `views_errors::views_errors_carry_no_hidden_name_and_no_inner_detail` |
| NGSI-LD view root | a Relationship target the view does not show answers as an absent one (threat D7) | `views_writes::a_relationship_target_must_be_visible_and_absent_answers_alike` |
| NGSI-LD view root | notifications drop hidden changes and strip their previous value | `views_subscriptions::views_notifications_drop_hidden_changes_and_strip_previous_value` |
| NGSI-LD view root | a view that narrows nothing answers what the broker answers | `views_all::views_all_answers_what_the_broker_answers` |
| MCP | a hidden name and an absent one answer the same bytes on every method | `antares-mcp` `mcp_discovery::a_hidden_and_an_absent_name_answer_the_same_bytes_on_every_method` |
| LinkML | a hidden class leaves with its prefixes, ranges and inverses | `antares-linkml` `narrow::a_hidden_class_goes_with_its_prefix_ranges_and_inverses` |
| LinkML | errors name no hidden class or slot | `linkml_view::linkml_errors_carry_no_hidden_name` |
| OData | `$metadata` names no hidden type or Attribute | `antares-odata` `metadata::metadata_never_names_a_hidden_type_or_attribute` |
| OData | a hidden target or value reads as an absent one | `odata_expand::odata_a_hidden_target_or_value_reads_like_an_absent_one` |
| OGC API - Features | a hidden collection and an unknown one are the same `404` | `antares-ogc` `collections::hidden_and_unknown_collection_are_the_same_404` |
| OGC API - Features | a hidden geometry or time property is no role | `model::a_hidden_geometry_or_time_is_none` |
| SensorThings API | a hidden type, Attribute or Device is in no set and its id is the unknown `404` | `antares-sta` `sets::things_are_the_entities_of_the_shown_types_and_a_hidden_one_is_unknown`, `sets::datastreams_are_one_per_measured_instance_and_a_hidden_attribute_has_none` |
| iCalendar | a hidden type or Attribute is in no `VEVENT` | `antares-ical` `a_hidden_type_and_a_hidden_attribute_are_in_no_event` |
| WebSocket | a view refuses what it does not allow | `antares-ws` `ws_seams::ws_a_view_refuses_what_it_does_not_allow` |
| Internal context sources | a grant's view narrows what the target Tenant exports | `antares-icsr` `icsr_grant::icsr_a_grant_view_narrows_what_the_target_exports` |

## 5. Conformance statements

Each statement names the suite, its version and the CI job that runs it.
The jobs are in `.github/workflows/surfaces-conformance.yml` unless the row
says otherwise. A job not listed for pull requests runs nightly, on `v*`
tags and on dispatch.

| Surface | Claim | Suite and version | Job |
|---|---|---|---|
| NGSI-LD root of the `full` build | answers as vanilla, TP by TP (E3, on the `full,jc-policy` image with `ANTARES_POLICY=allow-all`) | ETSI `ngsi-ld-test-suite` vendored (GS CIM 009 V1.9.1), `dev/etsi-baseline.json` | `image-jc.yml` `etsi` (nightly and on dispatch, T-4721) |
| NGSI-LD view root | view `all` answers as the plain root of the same image, TP by TP, except the refusals the view owes (E2) | the same suite, `dev/etsi-view-diff.py` | `etsi-full-all` (pull requests: Consumption and Provision) |
| NGSI-LD restricted view | view `etsi-restricted` (Building and City, `airQualityLevel` hidden, reads only) answers as the plain root, TP by TP, except a write, a hidden type or a hidden Attribute the TP names (E4) | the same suite, `dev/etsi-view-diff.py` | `etsi-full-restricted` (nightly) |
| NGSI-LD over five brokers | the all-features image answers as vanilla | ETSI IOP suite (TS 104 188) | `etsi-iop` |
| Internal context sources | a forward in process answers as one over HTTP | ETSI DistributedOperations | `etsi-icsr` |
| MCP | the server scenarios pass but the listed baseline | `@modelcontextprotocol/conformance` 0.1.16, revisions 2025-11-25 and 2025-06-18, `ci/mcp-baseline.yaml` | `mcp-conformance` (pull requests) |
| MCP | protocol, isolation and read parity with NGSI-LD | joinedcontext-conformance `tests/mcp` at `CONFORMANCE_REF` | `mcp-suites` (pull requests: 5 parity cases) |
| OData | ABNF cases, both `$metadata` validations, `$filter` parity, Capabilities | joinedcontext-conformance `tests/odata` | `odata-conformance` (pull requests) |
| OData | no test that passes in the baseline fails | NLstn odata-compliance-suite v1.0.3, `ci/odata-compliance-baseline.json` | `odata-compliance-external` (pull requests) |
| WebSocket | the scripted sessions keep to the published document | `@asyncapi/cli` 2.16.0 on `docs/api/asyncapi-ws.yaml`, joinedcontext-conformance `tests/ws` | `ws-conformance` (pull requests) |
| LinkML | the served schema lints clean, generates, and validates the view's Entities | linkml 1.12.0, pyshacl 0.40.1, joinedcontext-conformance `tests/linkml` | `linkml-conformance` (pull requests) |
| OGC API - Features | no external suite runs in CI yet; the crate's tests cover Part 1, 2, 4 and 5 | `cargo test -p antares-ogc` | `workspace.yml` |
| SensorThings API | no external suite runs in CI yet; the crate's tests cover the eight Sensing sets | `cargo test -p antares-sta` | `workspace.yml` |
| iCalendar | the feed parses as RFC 5545 and round-trips | the `icalendar` crate 0.17 parser in `cargo test -p antares-ical` | `workspace.yml` |

The TS 104 187 ICS of the `full` build is
`docs/analysis/ics-ts104187-full.md`, generated by
`dev/ics-fill.py --full`. Its NGSI-LD tables equal vanilla's because E3
holds the root TP by TP. Known differences through a view are listed in
`dev/etsi-view-known.json` with the task that fixes each.

## Related

- [Endpoint views](views.md), [Conformance](conformance.md),
  [Configuration](configuration.md), [Admin API](admin-api.md)
- ADR-0023 endpoint views, ADR-0024 OData, ADR-0025 internal context
  sources, ADR-0026 WebSocket channel, ADR-0027 surfaces and module rules,
  ADR-0029 OGC API - Features
