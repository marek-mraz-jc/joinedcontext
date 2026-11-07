---
title: Behaviour profiles where the spec versions or the suites differ
task: T-5497
---

# Behaviour profiles

Antares answers one way where the texts decide. This page lists every place where ETSI GS CIM 009 V1.9.1, ETSI TS 104 175 V1.1.1 (with TS 104 176 V1.1.1, its HTTP binding) or the conformance suites accept two answers, and the choice Antares makes for each. A choice that a deployment may need the other way is a startup key; every key defaults to the behaviour the pinned ETSI suite asserts, so dev and prod change only with a deployment value.

## 1. Sources

- Rows: `final-validation.md` 4.1 and 4.3 (T-5495, forge develop `cc8711ce` against integration/all-fixes `6475ccb9`) and `final-validation-coraine.md` 4.1 (T-5494, Coraine against both spec versions).
- Clause text: CIM 009 V1.9.1 from `docs/spec/<chapter>/<clause>.md`, TS 104 175 and TS 104 176 from the per-clause split of the published PDFs (`/workspace/etsi-specs`, pages in each file's front matter). etsi.org/deliver answers 403 from the build sandbox, so the PDFs were not downloaded again.
- Keys: `antares_api::Profile` (`crates/antares-api/src/state.rs`), read once at startup by the broker (`Profile::from_env`); an unknown value stops the broker. `docs/src/configuration.md` documents each key.

## 2. Configurable differences

| key (default first) | difference | CIM 009 V1.9.1 | TS 104 175 / 104 176 | suites | code |
|---|---|---|---|---|---|
| `ANTARES_NON_OBJECT_BODY` = `invalid-request` \| `bad-request-data` | a JSON body that is no object (`[]`, `[1,2,3]`) where an Entity, fragment, Subscription or registration is expected | 5.5.4 p.149-150: "If the request payload body is not a valid JSON document then an error of type InvalidRequest shall be raised." and "If the data included by the JSON-LD document is not syntactically correct, according to the @context or the API data type definitions, then an error of type BadRequestData shall be raised." | TS 104 175 8.2.3 p.147-148: the same two sentences | ETSI 001_02_02 (pinned and develop) asserts InvalidRequest for `[]`; forge all-fixes `bd252bc2` and Coraine (empty_object_array_input, csource_reg_errors, post_query_errors step 01) assert BadRequestData | `negotiate.rs` `parse_body_in`, the `!value.is_object()` branch |
| `ANTARES_FED_WRITE_SOURCE_404` = `report` \| `ignore` | on a distributed write, one part answers 404 beside a successful part | 6.3.17 p.278-279, in the HTTP GET paragraph: "a registration endpoint responding with no data and the HTTP status code 404 - Not Found should not be considered as abnormal behaviour for distributed operations"; then "For all other operations, which correspond to HTTP Unsafe methods, the error response should be as informative as possible." | TS 104 176 6.3.5 p.26-27: the same two sentences in the same places | ETSI D002_02_01_inc, D002_02_02_inc, D004_01_inc (pinned, develop) assert 207; forge all-fixes `3f772fdc` and Coraine distop_delete_inclusive_source_404 step 03 assert 204 | `federation.rs` `combine` |
| `ANTARES_IMPLICIT_TENANTS` = `with-upsert` \| `listed` | Batch Upsert naming a Tenant that does not exist | 5.5.10 p.154-155: "One implementation option is to support the implicit creation of Tenants" for Create Entity, Batch Entity Creation, Create or Update Temporal, Create Subscription, Register Context Source, Create CSR Subscription; "All other NGSI-LD operations, e.g. for retrieving, updating, appending or deleting information that target a non-existing Tenant should raise an error of type NonexistentTenant." | TS 104 175 7.8.2 p.146: the same option and list (clauses 10.2.2, 10.3.2, 11.2.2, 10.5.2, 12.2.2) and the same "should" | ETSI: no TP; Coraine tenant_implicit_create expects NonexistentTenant | `router.rs` `tenant_exists_layer`, `implicit_create` |

Decided (worker-t5497, owner may override), per key:

- `ANTARES_NON_OBJECT_BODY`: `[]` is a valid JSON document, so the first bullet does not apply and the second one does; the text supports BadRequestData under both versions. The default stays `invalid-request` because the pinned ETSI suite gates CI and prod clients have seen that answer; `bad-request-data` is the setting for a deployment tested by forge all-fixes or Coraine. A body that is not JSON stays InvalidRequest under both values.
- `ANTARES_FED_WRITE_SOURCE_404`: both versions place the 404 sentence in the GET paragraph and ask writes to be "as informative as possible", which the 207 is; reading the 404 sentence as covering every distributed operation is also a reading of its words ("for distributed operations"). Default `report` (pinned ETSI); `ignore` drops a 404 part only beside a success, so a 404 on every part stays one 404 and any other failure stays in the 207.
- `ANTARES_IMPLICIT_TENANTS`: Batch Upsert creates Entities, which the option describes ("an NGSI-LD operation for creating information targets a new Tenant"), but the list does not name it. Default `with-upsert` (today's behaviour, platform pipelines upsert into new tenants); `listed` keeps implicit creation to the list. The key never widens the list, so it cannot relax tenant isolation.

## 3. One behaviour, no key

| difference | CIM 009 V1.9.1 | TS 104 175 / 104 176 | suites | Antares, decided (worker-t5497, owner may override) |
|---|---|---|---|---|
| an empty attribute-name segment (`DELETE /entities/{id}/attrs/`, forge 016_02_06) | 6.3.2 p.269: 405 "shall be raised when a client invokes a wrong HTTP verb over a resource" | TS 104 176 6.3.2 p.25: 405 "shall be raised when a client invokes a recognized but unsupported HTTP verb over a resource" | forge develop 405; all-fixes `36cf748b` 400 | 405 with `Allow` (`router.rs` `not_found`): the trailing slash names `/entities/{id}/attrs`, which takes POST and PATCH only, so 405 meets both wordings; no key |
| PATCH or PUT `/entities` without an id | 6.3.2 p.269 (as above) | TS 104 176 6.3.2 p.25 (as above) | ETSI 054_02, 056_02 assert 400; Coraine http_405_method_not_allowed expects 405 + `Allow` | 400 (`entities.rs` the empty id sentinel): the trailing-slash form `/entities/` reads as an Entity with an empty id, which the pinned suite asserts. Deviation from 6.3.2 for the bare `/entities` form, kept while the pinned suite asserts 400 |
| an unquoted `q` value that is no Number, DateTime or URI (`q=category==commercial`) | 4.9 p.84-92: `CompEqualityValue = OtherValue / ValueList / Range / URI` | TS 104 175 7.2.3.2 p.125-131: the same grammar | ETSI 5743_01 sends it and expects matches; Coraine q_expandvalues_langproperties and query_q_value_grammar expect 400 | accepted as a string. Deviation from the grammar of both versions, kept because the pinned suite relies on it; a request the grammar accepts gets the same answer either way |
| `offsetN` in the Links of a `lastN`/`firstN` temporal page | none | TS 104 176 6.4.7.3 p.30-31: the "intervalafter" and "intervalbefore" links "should include the offsetN parameter" under transparent pagination | forge all-fixes 020_13, 021_15 fail on their own `>;rel=` regex | Antares includes `offsetN`, which the "should" asks for; no key |
| JsonProperty `json` holding an array of non-objects | 4.5.24.2 p.78-79: "a raw JSON object (or array of objects)" | TS 104 175 5.2.6.4.10 p.65-67: `json` "See NGSI-LD Value definition in clause 3.1" | Coraine array_reduction_entity_data step 1 | spec upgrade: TS 104 175 is followed, no key, since a V1.9.1 body stays valid. The change itself is T-5463 (`expand.rs` refuses the array today) |
| a source's registrationId in a BatchEntityError | 5.2.17: registrationId 0..1 "(optional)" | the same member, optional | Coraine entity_batch_update_distops_nooverwrite expects it | omitted: optional in both; T-5464 holds the 207 body work |
| a CSR Subscription read back with a `jsonldContext` it was not given | 5.8.1.4 initialises it for entity Subscriptions only | TS 104 175, same | ETSI 044/045 expect it absent; Coraine does not test it | absent |

## 4. Test per value

`crates/antares-api/tests/behaviour_profiles_t5497.rs` runs each key at both values through the router (`cargo test -p antares-api --test it behaviour_profiles_t5497::`):

- `non_object_body_error_type_per_profile_value`: `[]` on Create Entity and `[1,2,3]` on Register Context Source; a body that is not JSON stays InvalidRequest under `bad-request-data`.
- `distributed_delete_beside_a_source_404_per_profile_value`: local Delete Entity beside an inclusive source that answers 404, 207 then 204.
- `batch_upsert_on_an_unknown_tenant_per_profile_value`: 201 and the Tenant exists, then NonexistentTenant with nothing created, while Batch Create still creates the Tenant.
- `profile_keys_parse_two_values_and_refuse_others`; `federation.rs` `ignore_profile_reads_a_404_beside_a_success_as_no_data` covers `combine` at the part level.

The vendored ETSI suite ran locally (memory store, `dev/etsi-run.sh`, MQTT TPs excluded) over the 14 suite directories that send a non-object body or a distributed write: CreateEntity, the five BatchEntities operations, AppendEntityAttributes, UpdateEntityAttributes, CreateTemporalRepresentationOfEntity, TemporalEntityAttributes/AddAttributes, CreateSubscription, UpdateContextSourceRegistration, UpdateContextSourceRegistrationSubscription and DistributedOperations/Provision.

| profile | failed TPs |
|---|---|
| default (no key set) | none |
| `bad-request-data`, `ignore`, `listed` | 001_02_02 (asserts InvalidRequest), D002_02_01_inc and D002_02_02_inc (assert 207); D004_01_inc still passes |

The alternative values fail exactly the pinned TPs that section 2 names and nothing else. Forge all-fixes and the Coraine cell with the alternative values run in the external-suites job (CI only), named in the T-5497 push note.

## Related

- `final-validation.md` 4.3: where the two suites disagree
- `final-validation-coraine.md` 4.1: Coraine cases against both spec versions
- `docs/analysis/cim009-to-ts104175.md`: clause map between the two versions
- `docs/src/configuration.md`: the keys
