# OData compliance

The [OData endpoint](odata.md) runs against an independent black-box test
suite, NLstn [odata-compliance-suite](https://github.com/NLstn/odata-compliance-suite)
v1.0.3 (MIT; 162 test scripts, 1 210 tests over OData 4.0, 4.01 and the
vocabularies). Our own suite (`tests/odata` of joinedcontext-conformance,
T-3510) checks URLs against the OASIS ABNF and `$metadata` against the OASIS
schemas. This suite drives server behaviour end to end: query options, CRUD,
headers, errors, `$batch`. Its Minimal, Intermediate and Advanced grouping
is the suite's own coverage grouping, not OASIS certification. This page
records what the build passes today (T-3714).

## 1. The run

Job `odata-compliance-external` in `.github/workflows/surfaces-conformance.yml`
runs on every pull request to `dev`, nightly and on `v*` tags:

1. It takes the `full` broker of the `build` job and starts it with
   `ANTARES_API_SURFACES=admin,views,odata`.
2. It loads view `compliance` (`ci/odata-compliance/view.json`) and the
   suite's reference model as the Tenant's LinkML schema
   (`ci/odata-compliance/schema.json`, through `PUT /q/schemas/odata-compliance`).
3. It starts `dev/odata-compliance/reseed-proxy.py` in front of the broker
   (section 3).
4. It fetches the suite binary by release URL and checks its SHA-256
   (`c89dd48b…a7cec6`).
5. It runs the suite with `-verbose -format junit`.
6. It gates the run with `dev/odata-compliance/baseline.py check`
   (section 4).

The artifact `odata-compliance-external` holds the verbose run, the JUnit
report, the gate's Markdown and both logs; the job summary shows the
per-feature table. The broker runs in the job only. Its `/q` routes take an
admin token the job draws at random (`ANTARES_ADMIN_TOKEN_FILE` under
`RUNNER_TEMP`, masked in the log, never in the artifact); the suite itself
reads and writes only through the view, and the seed is public test data.

To run it locally:

```sh
cargo build -p antares-broker --features views,odata
ANTARES_HTTP_PORT=18714 ANTARES_API_SURFACES=admin,views,odata \
  ANTARES_SCHEMA_MAX_BYTES=1048576 target/debug/antares &
curl -X PUT -H 'Content-Type: application/json' \
  --data-binary @ci/odata-compliance/view.json localhost:18714/q/views/compliance
curl -X PUT -H 'Content-Type: application/json' \
  --data-binary @ci/odata-compliance/schema.json localhost:18714/q/schemas/odata-compliance
dev/odata-compliance/reseed-proxy.py 18715 http://localhost:18714 /x/views/compliance/odata/v4 &
odata-compliance-suite -server http://localhost:18715/x/views/compliance/odata/v4 -verbose > run.txt
dev/odata-compliance/baseline.py check run.txt ci/odata-compliance-baseline.json
```

## 2. The reference model

The suite's contract (its `CONTRACT.md`) names a fixed model. View
`compliance` serves it from one Tenant, `odata-compliance`. Every entity
set is the NGSI-LD type of the same name, and every Entity id is
`urn:ngsi-ld:{EntitySet}:{ID}`, with `ID` the suite's key value (a
deterministic uuid5 of the contract's row):

| Entity set | NGSI-LD type | Seed Entities | Notes |
|---|---|---:|---|
| `Products` | `Products` | 7 | 30 slots; `ShippingAddress` and `Dimensions` are inlined classes (Edm.Untyped); `Category`, `Descriptions`, `RelatedProducts` are Relationships |
| `Categories` | `Categories` | 3 | |
| `ProductDescriptions` | `ProductDescriptions` | 7 | |
| `MediaItems` | `MediaItems` | 2 | media entities are out of scope (section 6) |
| `ReadOnlyItems` | `ReadOnlyItems` | 2 | |
| `DecimalSamples` | `DecimalSamples` | 1 | |

The complex types `Address` and `DimensionsType` are inlined classes. The
contract's `Dimensions` type became `DimensionsType` because a class and a
slot cannot share a name in one schema. The contract's `Company` singleton
has no NGSI-LD counterpart (section 6).

## 3. `Reseed`

The suite starts every test script with `POST {root}/Reseed` and fails the
script unless the service answers 200. The OData surface serves no action
(ADR-0024), so `dev/odata-compliance/reseed-proxy.py` answers that one
request. It deletes every Entity of the view's types in the Tenant and
writes `ci/odata-compliance/seed.json` again through the NGSI-LD API. Every
other request goes to the broker as it came, headers included, and the
answer comes back as it went.

Decided (worker-4, owner may override): a test-only proxy answers `Reseed`
and the model lives in its own folder `ci/odata-compliance`. A proxy keeps
the broker free of an action no production view should have. A folder of
its own keeps the model out of the views `dev/views-rig.sh` loads for the
other conformance jobs. Options considered: an admin-only `Reseed` action
in the surface, or the model under `ci/views` beside the other views.

## 4. The baseline and the regression gate

`ci/odata-compliance-baseline.json` records every test the suite reports
on its own line (1 187 of 1 210 today). The rest belong to whole scripts the
suite skips on a declared capability. Each test is keyed
`[V4] 2.1_conformance :: Service returns service document (MUST)` with
status `PASS`, `FAIL` or `SKIP`. `baseline.py check` fails the job when a
test the baseline passes no longer passes: it failed, it was skipped, or it
is gone. A known failure never fails the job. A new pass is listed so the
baseline can be raised: run `baseline.py write` on the job's `ocs-run.txt`
and commit it in the same change as the fix.

The suite's results on dev `f0da234` were the same on two runs in a row.
A deliberate break proved the gate: the error response stopped sending the
`OData-Version` header, and the gate went red on
`[V4] 8.3_error_responses :: Error response should include OData-Version header`.

## 5. Pass rate per level

The pass rate counts passed and failed tests; skipped tests stay out of it.
The baseline was first recorded on dev `f0da234` (T-3714) and raised by
T-3719, which reads a form-encoded query, T-3723, which declares `$batch`
and serves `HEAD`, T-3731, which answers an empty path segment with 400,
and then by train 8, written from its own run (surfaces-conformance
38136321034 on `2f26761`, never merged by hand): T-3720, which keys a set by
its LinkML identifier and serves `$entity`, T-3722, which narrows Edm types
and writes the Core and Capabilities terms the schema states (section 6),
T-3721 (minted keys, C5 required properties), T-3725 (typed `$filter`
literals), T-3726 (batch framing and `continue-on-error`), T-3730 (entity
references), T-3737 (one `Content-Length` on `HEAD`), T-3738 (`$entity`
404) and T-3760 (a bind to a missing Entity is 400); and by T-3761 (a
`ProductID` naming an absent Product is 400), written from its branch run
(surfaces-conformance 38152573824 on `30ca1c7`):

| Level | Passed | Failed | Skipped | Pass rate |
|---|---:|---:|---:|---:|
| Minimal | 232 | 41 | 20 | 85.0 % |
| Intermediate | 268 | 68 | 14 | 79.8 % |
| Advanced | 323 | 206 | 38 | 61.1 % |
| **All** (suite summary) | 823 | 315 | 72 | 72.3 % |

Measured by surfaces-conformance 41546302927 on `c364c02` (train 20261102-2330), written by tasks/train.

`$metadata` declares `Capabilities.ConformanceLevel` `Minimal` (row C19). The table above is
the measured level: the claim is not proven while Minimal stays under 100 %, and each failing
Minimal test is a known gap listed in `ci/odata-compliance-baseline.json` (T-5206 finding 17).
T-5206 fixed the 4.01 MUST that system query option names are case-insensitive with an
optional `$` for `$apply` (`apply=`, `$APPLY=`); the next train run rewrites the table.

One earlier pass was a skip after train 8, by the suite's own rule: `POST with
nonexistent foreign key` posts a `ProductDescription` whose `ProductID` names
no `Product` and skips on 201, since referential constraints are a SHOULD
(Protocol 11.4.2). With T-3761 the schema states `ProductID` as the
referential constraint of the `Product` navigation, and the create is 400.

Before train 8 the baseline stood at 550 passed, 563 failed and 97 skipped
(Minimal 62.3 %, Intermediate 65.8 %, Advanced 31.5 %).

Before T-3720 the baseline stood at 455 passed, 661 failed and 94 skipped
(Minimal 57.8 %, Intermediate 49.1 %, Advanced 26.0 %).

The first baseline (T-3714) stood at Minimal 37.5 %, Intermediate 28.2 % and
Advanced 11.7 % (254 passed, 840 failed, 116 skipped).

The suite skips whole test scripts on what `$metadata` declares:
`KeyAsSegmentSupported` false on the container. Until T-3750 and T-3936
it also skipped the search and compute scripts, which `Products` declared
unsupported. Until T-3723 the container also declared
`BatchSupported` false, and the suite skipped every `$batch` script.

## 6. Failing areas

Each failing area is a task, or a decision with its reason:

| Area | Suite scripts | Disposition |
|---|---|---|
| A form-encoded query (Go `url.Values`, Java `URLEncoder`): `%24` before an option name, `+` for a space, `%2F` and `%3D` for the query's `/` and `=` | 11.2.5.x, 11.3.x, 5.1.x, 11.2.9, 11.2.14 | T-3719: read as plain. A `+` reads as a space only where the grammar requires whitespace (`RWS`); inside a literal and in optional whitespace (`BWS`) it stays a `+`, by design: OData writes a space as `%20` (Part 2, the note under Example 32) and gives `+` its own meanings (a sign, a time zone offset), so `'a+b'` keeps its plus |
| The key was always `id` (the URN); the model's own key (`ID`, Edm.Guid) did not address an Entity, and `$entity?$id=` answered 501 | 11.1, 11.2.1 to 11.2.3, 11.2.11, 11.4.11, 8.2.7 | T-3720: the schema marks `ID` `identifier: true` with `edm_type: Edm.Guid` (`ci/odata-compliance/schema.json`); the set is keyed by it and `$entity` is served |
| `@odata.id URL is dereferenceable` (11.2.2) appends the relative `@odata.id` to the service URL without a `/`, and keeps the whole path of an absolute one; under a service root with a path neither form reaches the Entity | 11.2.2 | a limit of the suite: JSON Format 4.01 section 4.5.8 lets `@odata.id` be relative to the context URL, and `$entity?$id=` with the same value passes |
| Entity references of a set or an Entity (`{Set}/$ref`, `{Set}(key)/$ref`) answer 501 | 11.2.15 | T-3730 |
| `{Set}/` and `{Set}//` read as `{Set}` | 11.1 | T-3731 |
| A create without a key was refused (`MissingKey`); the service did not mint one | 11.4.2 to 11.4.5, 11.4.12 to 11.4.15, 8.1.5, 8.2.3, 8.2.5, 8.2.8 | T-3721: a `Guid` or `String` key is minted (`Core.ComputedDefaultValue`, mapping W1), and a LinkML `required` member left out or null is 400 `MissingProperty` (mapping C5; the schema marks `Products.Name`, `Products.Price` and `ProductDescriptions.Description` required, as the suite's contract does) |
| Edm types beyond String, Double and Int64, Core and Capabilities annotations from the LinkML schema | 4.4, 5.1.1.x, 5.1.4 to 5.1.8, 14.1, vocab_* | T-3722: the schema's `edm_type`, `description`, `odata_computed`, `odata_immutable` and the class's `odata_insertable`, `odata_updatable`, `odata_deletable` (`ci/odata-compliance/schema.json`, as the reference model declares them); the type scripts' metadata tests pass, their filters stay in the rows below (functions, `cast`, arithmetic, `binary'…'` and `duration'…'` literals, the `f` suffix of a Single) |
| The vocabulary scripts look for the terms on an EntityType named `Product` in the first schema of `$metadata` (`Antares.NgsiLd`, the surface's own vocabulary) and for set restrictions only in external `<Annotations Target="….Container/Set">` blocks | vocab_core_*, vocab_capabilities_*, 14.1 | a limit of the suite: CSDL 14.3 lets an annotation sit inline on its target, which is where the surface writes it, and an EntitySet here is named after its class and so is its EntityType (`Products`). The terms are there: `ReadOnlyItems` refuses `POST`, `PATCH` and `DELETE` with 405 |
| A create naming a `ProductID` no Product has was accepted, so the suite skipped its foreign-key test (it passed on dev only because a ProductDescriptions create without a key was 400 `MissingKey`, before T-3720 and T-3721) | 11.4.15 | T-3761: the schema's `Product` navigation carries `odata_referential_constraint: ProductID` (`ci/odata-compliance/schema.json`), `$metadata` states the `ReferentialConstraint`, and a create, replace or merge whose `ProductID` names an absent or hidden Product is 400 `BadRequest` (mapping row R6); a bind (`Product@odata.bind`) to a missing Product is 400 (T-3760) |
| `duration'…'` and `binary'…'` literals, the `f` suffix of a Single, `INF`, `-INF` and `NaN`, a TimeOfDay NGSI-LD stores with or without its `Z`, an `Edm.Binary` served padded | 5.1.1.4, 5.1.1.5, 5.1.6, 5.1.7, 5.1.8 | T-5905: read and range-checked (a literal past its type, or of another type, is 400 before any read); NGSI-LD stores these values as text whose string order is not their value order, so the residual compares the value each spells; `Edm.Binary` is served as unpadded base64url (JSON Format 7.1) |
| `Data ne binary'AQID'` and `ShippingTime ne duration'P0D'` expect an Entity whose value is null to be left out | 5.1.6, 5.1.8 | by design: OData URL 5.1.1.1.2 makes `ne` true when one operand is null and the other is not (the reference server answers by SQL `<>`, which drops NULL rows) |
| A Product created without `Rating`, `Temperature` or `Quantity` reads them as null; the reference model gives them a CSDL `DefaultValue` of 0 that a create applies | 5.1.1.2, 5.1.1.3 | open: a LinkML `ifabsent` as `DefaultValue`, written on create. The seed gives the three Products the reference server seeds with 0 that value (`ci/odata-compliance/seed.json`) |
| `cast` and `isof` | 5.1.1.2 to 5.1.1.4, 5.1.5 to 5.1.8 | open (T-5905 handover): the parser answers them 501 |
| An enum as an `EnumType` with integer members (`Status eq 1`, flags) | 5.3, 5.1.1 | not supported, by design: mapping section 3.4 keeps an enum an `Edm.String` with `Validation.AllowedValues`, because NGSI-LD values such as `pm2.5` are no CSDL member names |
| `BatchSupported` was declared false while `$batch` is served; HEAD on a collection or an Entity answered 501 | 11.4.9.x, 11.4.11 | T-3723: declared and served; the batch scripts run, their failures are in the baseline |
| Multipart `$batch` framed with bare LF line ends; a JSON batch under `OData-MaxVersion: 4.0`; independent requests and `dependsOn` after a failure | 11.4.9, 11.4.9.1, 11.4.9.3, 19_json_batch | T-3726 |
| Change sets and atomicity groups, Content-ID references | 11.4.9.1, 11.4.9.3, 19_json_batch | not supported, by design: ADR-0024 section 5 (W26), mapping row C18 |
| A comparison or `in` between a property and a literal of another type answers 200 with no rows; an Int64 literal past its range is accepted | 11.3.6, 11.2.5.1, 5.1.1.5 | T-3725 |
| Options inside `$expand(...)` other than `$select` and `$expand` (Q5) | 11.2.5.9, 11.3.8 | not supported, by design: ADR-0024 section 5 answers them 501 |
| `$index`, `$deltatoken` and delta links | 11.2.4.1, 11.2.8 | not supported, by design: ADR-0024 section 5 answers them 501 |
| `$apply` answered 501 | 11.2.5.4.x | T-3937: `identity`, `filter`, `orderby`, `top`, `skip`, `aggregate` and `groupby` served over the whole answer the view shows; T-3985 adds `topcount` to `bottompercent`, `concat`, `compute` and `search`; `expand`, `nest`, joins, hierarchies and rollup answer 501 |
| `$compute` answered 501 | 11.2.5.13 | T-3936: served, each item evaluated per Entity on what the view shows |
| `$search` answered 501 | 11.2.5.6 | T-3750: served over the String properties the view shows |
| Filter functions outside the F-row subset (date and time, arithmetic, `length`, `indexof`, `substring`, `divby`, `matchesPattern`, `isof`, `cast`) | 11.3.1 to 11.3.4, 11.3.9, 11.5.x | not supported, by design: ADR-0024 section 5; the mapped rows still at 501 are T-3686 |
| Actions and functions, bound or unbound | 11.2.10, 11.4.13, 12.1, 12.2 | not supported, by design: an NGSI-LD view has no operations (ADR-0024 section 5) |
| Media entities and stream properties | 11.2.12 | not supported, by design: ADR-0024 section 5 |
| Deep insert, deep update, `$ref` writes on collection-valued navigation, `If-Match`/`If-None-Match` and ETags | 11.4.6.x, 11.4.7, 11.4.8, 11.5.1, 8.2.2 | not supported, by design: ADR-0024 section 5 (W3, W8, W18, W19, W32) |
| Asynchronous requests (`Prefer: respond-async`) | 13.1, 11.4.10 | not supported, by design: `AsynchronousRequestsSupported` is false; every request answers synchronously |
| The `Company` singleton and derived types (`ProductType` discriminator) | 9.1, 11.2.16, 11.2.13 | not supported, by design: an NGSI-LD type is an entity set, with no singleton and no inheritance beyond the abstract Relationship base (ADR-0024 section 5) |

| `any` over a collection-valued navigation, `$orderby` by a member of a single-valued navigation's target, the key missing under `$select` (the NGSI-LD `id` served instead), a bind that left the constrained `CategoryID` unset, `Prefer: return=` on `PATCH` | 11.2.9, 5.1.3, 11.3.11, 11.2.5.2, 11.2.5.11, 11.4.2.1, 11.4.12 | T-5906: `any` is the linked-entity `q` term `nav{Type:member}` (TS 104 175 4.9) resolved through the caller's narrowing, `or` distributes, `any()` is the Relationship present; the surface sorts by the joined target's value; `$select` keeps the key; the schema's `Category` carries `odata_referential_constraint: CategoryID`, as the reference model's foreign key does |
| The suite form-encodes every query (Go `url.QueryEscape`): a space inside a string literal and after a lambda's `:` arrives as `+` | 11.2.9, 5.1.3, 11.2.5.1, 11.3.5 | a limit of the suite, by design here (first row): OData URL 4.01 section 5 makes the query follow its ABNF, where `BWS` and a string literal hold `%20` and never `+` ("in real URLs they must be percent-encoded as %20", the note under Example 32). With `%20` the same requests pass (`crates/antares-odata/tests/odata_navigation.rs`). A fork that encodes with `%20` is the candidate under the owner's 2026-10-28 rule |
| `and` inside `any` over a navigation, `all` over a navigation | 11.2.9, 5.1.3 | open: TS 104 175 4.9 matches each linked term on its own target, so `any(d: a and b)` has no `q` form; a two-step read (the targets matching the body, then the Relationship's objects) is the next round |
| `POST` with `Prefer: return=minimal` expects `201` | 11.4.12 | a limit of the suite: Protocol 4.01 11.4.2 answers a create under `return=minimal` with `204 No Content` and `OData-EntityId`, which the surface does |
| Custom query options other than `ngsild.*` | 5.2 | not supported, by design: mapping row Q27 and ADR-0028 rule 6 refuse an unknown option with 400, as NGSI-LD refuses an unknown parameter, so a misspelt option never widens an answer silently |
| An enum compared by its integer (`Status eq 1`, `Status gt 0`) | 11.3.5, 5.1.1.2 | not supported, by design (the `EnumType` row above): the suite's oracle reads `Status` as an enum member name |
| Complex-type paths (`ShippingAddress/City` in `$filter`, `$orderby`, a path segment), `eq null` on a complex value | 5.2, 5.2.1 | open: an inlined class is `Edm.Untyped` (P2); a CSDL `ComplexType` from the inlined class, mapped to `q` `p[sub]`, is the next round |
| `join`, `outerjoin`, hierarchy transformations, a sequence inside `groupby`, an `aggregate` after `concat`, `topcount(0)` | 11.2.5.4.1, 11.2.5.4.2 | open (T-3985 rows): answered 501, a non-positive count is not yet 400 |

A by-design line names its reason; the gate keeps such tests as known
failures and never skips them.

## Related

- [OData endpoint of a view](odata.md)
- [Conformance](conformance.md)
- [Endpoint views](views.md)
- `docs/adr/ADR-0024-odata-library.md`
- `docs/analysis/surfaces-conformance.md`
