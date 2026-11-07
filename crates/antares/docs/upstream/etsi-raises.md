# Upstream raises — ready to file

Issue texts prepared for filing. Two targets:

- **Suite issues** → https://forge.etsi.org/rep/cim/ngsi-ld-test-suite
  (issues 1–3). Our fork already carries the fixes; each issue links the
  fork commit so the patch can be cherry-picked.
- **Spec feedback** → ETSI ISG CIM issue tracker for GS CIM 009 V1.9.1
  (issues 4–6).

---

## 1. [spec] CIM 009 6.3.17 and TS 104 175 9.7 disagree on a loop through an inclusive registration

**Title:** 508 Loop Detected is scoped to a single exclusive or redirect
source in 6.3.17, while TS 104 175 9.7 makes every detected loop an error

**Body:**

Test `D018_01 Loop Detection With Via Header` registers its Context Source
with `mode=inclusive` (fixture
`context-source-registration-vehicle-redirection-ops.jsonld`), replays the
broker's own `Via` pseudonym on `DELETE /entities/{id}`, and asserts
**508**.

GS CIM 009 V1.9.1 clause 6.3.17 (p. 278), repeated in TS 104 176 6.3.5,
scopes 508 to the case where "all of the data is held outside of the
Context Broker and held in a single registered source", and Table 6.3.18-2
(p. 279) makes the Via listing an input to registration matching, which
reads as: drop the looping source, execute locally, answer 204.

TS 104 175 clause 9.7 adds: "If the requested endpoint is also listed as
previously encountered when a Context Source receives a call to that
endpoint, this is an indication that a new loop has been detected. A
suitable error shall be returned, indicating that the loop has been
detected." Under 9.7 the 508 that D018_01 asserts is right whatever the
registration mode, and the 6.3.17 scoping is the outdated text.

Antares follows 9.7 for writes (508 whenever the `Via` already names the
broker, T-3459) and serves reads locally with a `199` warning, so a
query that crosses its own registration still answers with local data.

**Proposed fix (spec):** align 6.3.17 and TS 104 176 6.3.5 with TS 104 175
9.7, and say whether a read that detects a loop may be served locally.
Our fork's D018_01 setup uses `mode=redirect`, which passes under either
reading.

---

## 2. [suite] Nine official `_exc` TPs create exclusive registrations clause 4.3.6.3 forbids

**Title:** `_exc` variants register mode=exclusive without Attributes /
with idPattern, both ruled out by 4.3.6.3

**Body:**

Clause 4.3.6.3 (p. 41): "An exclusive registration shall always relate to
specific Attributes found on a single Entity. Thus, the registration shall
define both: an entity id (i.e. an id pattern or Entity type defining a
group of entities is not supported for exclusive registrations) [and]
Attributes." Yet:

- `D001_02_exc`, `D002_02_exc` register `mode=exclusive` from the
  attribute-less `context-source-registration-vehicle-redirection-ops`
  fixture (entity id, no propertyNames/relationshipNames) and assert 201.
- `D012_01_exc`, `D013_01_exc`, `D013_02_exc`, `D014_01_exc`,
  `D014_02_exc`, `D015_01_exc`, `D016_01_exc` register `mode=exclusive`
  with `idPattern=urn:ngsi-ld:Vehicle:*` and assert 201 — an id pattern is
  exactly what the clause rules out.

A broker enforcing 4.3.6.3 (BadRequestData 400) fails all nine setups.

**Proposed fix (in our fork):** D001_02_exc/D002_02_exc use the
attribute-carrying `vehicle-speed-with-redirection-ops` fixture; the seven
batch `_exc` setups replace the idPattern with the concrete generated
entity ids. The semantics of every test are preserved. An extension TP
(`436_03`) pins the negative cases the official suite skips.

---

## 3. [suite] LdContextNotAvailable fixtures assert 503; V1.9.1 mandates 504

Status: resolved. Forge develop `b3bb243f` asserts 504; 043_01_01/02/03/05 and
028_07_02 pass on forge `cc8711ce` (T-5495).

**Title:** 043_01 / 028_07 / 051_05_01 / 053_05_01 assert 503 for
LdContextNotAvailable — V1.8-era expectation

**Body:**

GS CIM 009 V1.9.1 Table 6.3.2-1 (p. 269) maps
`https://uri.etsi.org/ngsi-ld/errors/LdContextNotAvailable` to **504
Gateway Timeout**. The listed fixtures still assert 503 Service
Unavailable, which was the V1.8 mapping. Suggest updating status code and
reason-phrase assertions to 504 / "Gateway Timeout" (done in our fork).

---

## 4. [spec] 5.3.4 SnapshotNotification: member naming conflict + phantom `snapshotReady`

**Title:** GS CIM 009 V1.9.1 clause 5.3.4 — two internal inconsistencies
in Table 5.3.4-1

**Body:**

1. **Member naming.** The Snapshot data type (Table 5.2.41-2) names the
   temporal details list `snapshotTemporalQueriesDetails`; the
   SnapshotNotification table (5.3.4-1) names the same list
   `temporalSnapshotQueriesDetails`. One of the two should be aligned —
   implementations currently have to pick (we emit each datatype's own
   spelling in its own payload).
2. **Phantom member.** The `expiresAt` row of Table 5.3.4-1 says "In this
   case, snapshotReady shall be set to false", but no `snapshotReady`
   member is defined in Table 5.3.4-1 (or anywhere in 5.2.41) — it reads
   like a leftover from an earlier draft. Either define the member or
   drop the sentence (the expiresAt-before-notifiedAt encoding already
   carries the deletion signal).

Additionally, the prose of 5.16.1.4 sets the initial status to
"preparation" while Table 5.2.41-2's vocabulary is "preparing" — worth
aligning in the same pass.

---

## 5. [spec] Table 6.6.3.2-2 (Update Attributes 207): Data Type vs Remarks conflict

**Title:** GS CIM 009 V1.9.1 Table 6.6.3.2-2 — 207 body declared
UpdateResult but Remarks say BatchOperationResult

**Body:**

Table 6.6.3.2-2 (p. 297) declares the 207 Multi-Status response body Data
Type = `UpdateResult`, but its own Remarks cell says distributed errors
are returned "in a BatchOperationResult structure". Every sibling table —
6.6.3.1-2 (Append), 6.7.3.1-2 (Partial Update), 6.7.3.2-2 (Delete),
6.7.3.3 (Replace) — says UpdateResult in both places, and 5.2.18 (p. 122)
defines UpdateResult as the result of attribute update operations
"regardless of whether local or distributed". The Remarks cell looks like
a copy-paste from the batch chapter; suggest correcting it to
UpdateResult.

---

## 6. [spec] 5.7.4.4 / Table 5.2.21-1: lastN vs values-filter ordering is unspecified

**Title:** GS CIM 009 V1.9.1 — interaction of `lastN` with the values
filter (and geoquery) on temporal queries is undefined

**Body:**

Table 5.2.21-1 (p. 123) defines `lastN` as "Only the last n instances,
per Attribute, per Entity (under the specified time interval) shall be
retrieved" — window-scoped, but silent on its ordering relative to the
other filters. Clause 5.7.4.4 S2 (p. 209) says the values filter "shall
be checked against all the Attribute instances resulting from the
initial filtering performed by the temporal query"; whether the lastN
cap is part of that "initial filtering" is unspecified. The two readings
give different answers whenever a matching instance is in the window but
not among the last n:

- lastN-first: the entity may FAIL the values filter (the matching
  instance was capped away);
- filter-last (lastN as a presentation cap): the entity matches, and
  lastN trims the returned instances.

The same question arises for the geoquery (S3) and the Scope query (S4,
with the 4.18 validity semantics additionally trimming instances after
S-selection). Suggest an explicit sentence in 5.7.4.4, e.g. "the lastN
restriction is applied to the instances of the final result set, after
S4" (which matches the S-chain's structure — none of S1-S4 mentions
lastN). Until then, implementations differ silently; ours applies lastN
after all S-filters (API-side presentation cap) and withholds any
store-level lastN optimization when q/geoQ/scopeQ is present.

## 7. [spec] CIM 029 A.5.2.26 cites clause 5.15.4, which does not exist in CIM 009 V1.9.1

**Title:** GS CIM 029 V2.1.1 annex A table A.5.2.26 (`/temporal/entityMaps/`)
cites a non-existent CIM 009 clause

**Body:**

Table A.5.2.26 of GS CIM 029 V2.1.1 (the ICS pro forma table for the
`/temporal/entityMaps/` resource) cites clause **5.15.4** as its CIM 009
reference. In GS CIM 009 V1.9.1 clause 5.15 is "Context Source Identity
Information" and contains only 5.15.1 — there is no 5.15.4. The clause
that carries the table's own subject (entity-map retrieval/deletion for
temporal queries) is **5.14.5**, with the HTTP binding in 6.35.3.1 /
6.35.3.2.

Presumably the citation predates a renumbering between the CIM 009
version the pro forma was authored against (V1.6.1 per clause 2) and
V1.9.1. Suggest correcting the citation to 5.14.5 in the next CIM 029
revision, and re-checking the neighbouring temporal tables for the same
drift.

## 8. [openapi] v1.8.1 temporal GET operations declare the `options` parameter twice

**Title:** ngsi-ld-openapi v1.8.1: `GET /temporal/entities` and
`GET /temporal/entities/{entityId}` carry two parameter components both
named `options`

**Body:**

In `openapi-3.1.0/ngsi-ld-api.yaml` at tag v1.8.1, the operations
`GET /temporal/entities` and `GET /temporal/entities/{entityId}` each
list two `$ref`s to parameter components that resolve to the same
parameter name `options` (in the same `query` location). The OpenAPI 3.1
specification requires parameter uniqueness by name+location, so strict
validators (e.g. openapi-spec-validator) reject the document with
"Duplicate parameter 'options'". Suggest merging the two component
definitions (their enum sets appear to have been split between temporal
and non-temporal option values) or renaming one.

## 9. [suite] three temporal TPs assert a Content-Range unit the clause does not define

**Title:** ngsi-ld-test-suite: `Content-Range` unit asserted as `date-time`,
CIM 009 6.3.10 mandates `DateTime`

**Body:**

GS CIM 009 V1.9.1 clause 6.3.10 (p. 275) specifies the temporal Partial
Content response verbatim as: `"unit"` shall be equal to `"DateTime"`.

Three test purposes assert the lowercase spelling instead:

- `TP/NGSI-LD/ContextInformation/Consumption/TemporalEntity/RetrieveTemporalEvolutionOfEntity/020_13.robot:91`
- `TP/NGSI-LD/ContextInformation/Consumption/TemporalEntity/QueryTemporalEvolutionOfEntities/021_15.robot:88`
- `TP/NGSI-LD/ContextInformation/Consumption/TemporalEntity/QueryTemporalEvolutionOfEntities/021_16.robot:46`

IETF RFC 7233 clause 4.2 makes the range unit a token matched literally, so
the two spellings are not interchangeable: an implementation that follows
6.3.10 fails all three TPs, and one that passes them emits a unit the clause
does not define. Suggest correcting the three assertions to `DateTime`, or,
if the lowercase form is the intent, correcting 6.3.10.

## 10. [spec] 4.5.19.0's period count contradicts the aggregation test fixtures

**Title:** CIM 009 V1.9.1 4.5.19.0: "as many array elements as there are
periods in the time range of the query" is unbounded and contradicts the
CTI aggregation expectations

**Body:**

Clause 4.5.19.0 (p. 71) states that each aggregation-method member's value
"shall be a JSON-LD Array that shall contain as many array elements as there
are periods in the time range of the query".

Two problems follow.

1) The ETSI test suite requires the opposite. Test case `021_17_01`
(`QueryTemporalEvolutionOfEntities/021_17.robot`) queries
`timerel=after&timeAt=2020-01-01T12:03:00Z` with `aggrPeriodDuration=PT1H`
over an Entity whose only `speed` instances are at 12:03, 12:05 and 12:07
on 2020-08-01. Its expectation file
`vehicle-temporal-representation-aggregated-avg-PT1H.json` contains exactly
one array element. The query's time range spans roughly 5 200 hourly
periods, so an implementation following 4.5.19.0 literally fails the test
purpose. `020_11.robot` has the same shape.

2) The literal reading has no ceiling. `timerel=between` with
`aggrPeriodDuration=PT1S` over one year asks for 31 536 000 array elements
per method per Attribute, and no clause defines a limit or an error type for
refusing it (unlike the pagination limits of 6.3.10 or the `TooManyResults`
of Table 6.3.2-1, neither of which is referenced here).

Suggest either restating 4.5.19.0 as "as many array elements as there are
periods holding at least one Attribute instance", or defining both the
padding value for an empty period and the error an implementation raises
when the requested period count is beyond what it will serve, and updating
the CTI fixtures to match.

## 11. [spec] Annex B maps `attributeCount` and `attributeDetails` to bare terms

**Title:** CIM 009 V1.9.1 Annex B: `attributeCount` and `attributeDetails`
expand into the default @context, not into the NGSI-LD namespace

**Body:**

The Core NGSI-LD @context of Annex B (p. 365) defines every member it
carries against the `ngsi-ld` prefix, with two exceptions:

```
"attributeCount": "attributeCount",
"attributeDetails": "attributeDetails",
```

A bare string that is not an absolute IRI and carries no prefix is not a
term definition JSON-LD can resolve against the NGSI-LD namespace, so both
fall through to the annex's own
`"@vocab": "https://uri.etsi.org/ngsi-ld/default-context/"`. Their expanded
IRIs become `https://uri.etsi.org/ngsi-ld/default-context/attributeCount`
and `.../attributeDetails`, while every neighbouring member of the same
data types expands to `https://uri.etsi.org/ngsi-ld/...`.

Both are normative members: `attributeCount` in Table 5.2.28-1 (Attribute,
p. 127) and `attributeDetails` in Table 5.2.26-1 (EntityTypeInfo, p. 125).
The default @context is the namespace reserved for terms the Core @context
does not define, so two members the Core @context does define land there,
and an expansion round-trip of an EntityTypeInfo or Attribute payload does
not recover the NGSI-LD names.

Suggest defining both as `"ngsi-ld:attributeCount"` and
`"ngsi-ld:attributeDetails"`, the form the rest of the annex uses.

Status: TS 104 175 V1.1.1 Annex B defines both that way, inside the
`Attribute` and `EntityTypeInfo` scoped contexts; Antares pins that
document for the v1.9 URL since T-3458.

## 12. [spec+suite] 4.19's ABNF derives no unparenthesized conjunction, but a test point sends one

Clause 4.19 (V1.9.1 p. 98) defines the Scope Query Language as

```
ScopesQ  = OrScopeQ *(orOp OrScopeQ)
OrScopeQ = %x28 ScopeQ *(andOp ScopeQ) %x29     ; (ScopeQ;ScopeQ)
OrScopeQ =/ ScopeQ *1(%x2F %23)
andOp    = %x3B                                 ; ;
orOp     = %x7C / %x2C                          ; |  ,
```

`andOp` appears in the parenthesized alternative only, the prose says "For
logical *and* grouping parenthesis are needed", and EXAMPLES 4 and 5 both
parenthesize. A bare `a;b` is not derivable from `ScopesQ`.

The conformance suite nevertheless requires it. Test point
`019_01_06 QueryWithAndScope` queries with

```
scopeQ=/Madrid/Gardens/ParqueNorte;/CompanyA/OrganizationB/UnitC
```

and expects 200 with the one entity carrying both Scopes. An implementation
that follows the grammar literally fails that test point; one that accepts
the bare form has to invent the precedence of `;` against `,`/`|`, which
the grammar never states because it never needs to.

The precedence is not academic. A deployment that narrows a consumer's
request by rewriting `scopeQ` at its gateway emits exactly this shape
(`(a;b),(c;d)` or `a;b,c;d`), and two brokers that guess differently
disclose different sets of Entities for the same narrowed request.

Suggest either extending `OrScopeQ` with an unparenthesized
`ScopeQ *(andOp ScopeQ)` alternative and stating that `andOp` binds tighter
than `orOp`, or parenthesizing the conjunction in `019_01_06`.

Status: resolved on the suite side. Forge develop `cc8711ce` (2026-10-21)
parenthesizes `019_01_06_05` and adds `019_03_07_05`, which expects
BadRequestData for the bare `/Madrid;/CompanyA`; TS 104 175 V1.1.1 clause 7.2.5
keeps the V1.9.1 grammar. Antares refuses the bare form since T-5495.

## 13. [spec] TS 104 175 Table 5.2.6.5.2-1 calls `timeInterval` not applicable to CSR subscriptions; 12.4.2.4 and 12.4.7 define periodic CSR notification on it

**Title:** TS 104 175: Table 5.2.6.5.2-1 and clauses 12.4.2.4/12.4.7 disagree on
`timeInterval` for Context Source Registration subscriptions

**Body:**

Table 5.2.6.5.2-1 lists the Subscription members that "are not applicable and
shall be ignored" when the Subscription is about Context Source Registrations
(`notificationTrigger`, `q`, `expandValues`, `jsonKeys`, `lang`, `localOnly`,
`splitEntities`, `throttling`) and counts `timeInterval` among the members
that do not apply. Clause 12.4.2.4 and clause 12.4.7 nevertheless define a
periodic Context Source Notification sent every `timeInterval` seconds for
such a Subscription, the same way 5.8.x does for an Entity Subscription.

An implementation that follows the table ignores `timeInterval` and sends
change-driven notifications; one that follows 12.4.x sends periodic ones. A
client cannot tell from the text which it gets.

Antares follows 12.4.7 (periodic notification on `timeInterval`), since the
behaviour clause is the more specific one, and pins the table's other
members as ignored (`csource_subscription_ignored_members.rs`, T-3452).

Suggest removing `timeInterval` from the table's not-applicable list, or, if
the table is meant, removing the periodic paragraphs of 12.4.2.4 and 12.4.7.

## 14. [spec] TS 104 176 clause 5 answers a successful HEAD with 204, its 7.x operation tables with 200

**Title:** TS 104 176: clause 5 and the GET/HEAD tables of clause 7 give a
successful HEAD two different status codes

**Body:**

Clause 5 (p. 22): "Where defined, a HEAD request shall function in the same
manner as a GET request, but shall omit the body in the response. The
response code 204 No Content shall be used on success."

Every GET/HEAD operation table of clause 7 lists one response set for both
verbs. For example, 7.3.3.1 "GET/HEAD" (p. 42-44) answers `200 OK` (and
`201 Created` when an EntityMap was (re)created), with no separate row for
HEAD.

An implementation that follows clause 5 answers HEAD with 204, one that
follows the tables answers 200. RFC 9110 9.3.2 expects the server to "send
the same header fields in response to a HEAD request as it would have sent
if the request method had been GET", so 204 also changes what a cache or a
client probing a resource reads from the status line.

Antares answers 200 (owner decision on T-3460), the status the tables and
RFC 9110 give, and takes the HEAD request's @context from its Link header as
6.2.4 requires (`contexts_5_13.rs::ts104176_6_2_4_head_takes_context_from_link`).

Suggest dropping the 204 sentence from clause 5, or, if 204 is meant, adding
a HEAD row with 204 to each GET/HEAD table of clause 7.

---

## 15. [suite] 020_17 to 020_20 delete an Attribute of an Entity that exists only as a temporal evolution

**Title:** 020_17..020_20 set up with `POST /temporal/entities` alone, then
call `DELETE /entities/{id}/attrs/{attrId}`

**Body:**

Delete Attribute (CIM 009 5.6.5.4) raises ResourceNotFound when no Entity
is held locally; a temporal evolution created by 5.6.11 is not one, and
5.6.13 is the operation that deletes from it. The setup creates only the
temporal evolution, so a compliant broker answers 404 and writes no
`deletedAt` instance. Suggest creating the current-state Entity in the
setup and deleting it in the teardown, as the corrected fork does (forge
commits f7c78f94 and 94789a66); Antares carries that fix and answers 404
(`temporal_autorecord.rs::delete_attribute_of_a_temporal_only_entity_is_not_found`).

---

## 16. [suite] 020_19 and 020_20 expect a deleted scope as an empty array

**Title:** the 020_19/020_20 expectations show a deleted scope as `[]`

**Body:**

CIM 009 4.18 reserves `urn:ngsi-ld:null` for deleted scopes, and 4.5.0
uses it for every deleted element of the temporal representation. The
expectation files carry `"value": []` (and `[]` in `temporalValues`).
Suggest `"urn:ngsi-ld:null"`, as the corrected fork does (forge commit
fad5ab03); Antares carries that fix
(`temporal_autorecord.rs::deleted_scope_is_the_ngsi_ld_null_in_the_temporal_representation`).

---

## 17. [spec] TS 104 176 answers Create and Clone Snapshot with 202, CIM 009 with 201, and gives no version to ask for either

**Title:** TS 104 176 13.2.3.1 and 13.4.3.1 change 201 Created to 202
Accepted with no migration rule

**Body:**

CIM 009 V1.9.1 6.36.3.1 and 6.38.3.1 answer "201 Created" with the
Location of the created snapshot. TS 104 176 13.2.3.1 and 13.4.3.1 answer
"202 Accepted" with "the relative path of the snapshot being created". The
snapshot fills after the answer in both texts (5.16.1.4: the status moves
to `ready` when every query has run), so 202 fits the operation, but a
client written against CIM 009 reads 202 as a different outcome, and the
Snapshot TPs assert 201.

A broker could serve both through `Prefer: ngsi-ld=<version>` (4.3.6.8,
TS 104 175 9.5.3), except that TS 104 175 states no `major.minor` of its
own, so no client can prefer it.

Antares answers 201, the CIM 009 status it states conformance to, and
records the conflict here (T-3555). Suggest giving the TS 104 175 family a
version a `Prefer` header can name, or keeping 201 for a snapshot created
synchronously and 202 only where the creation itself is deferred.

---

## 18. [suite] 021_08 expects speed instances that fail the query

**Title:** the 021_08 expectation keeps every instance of the selected
Entity, including those that fail `q`

**Body:**

CIM 009 5.7.4.4, last step: "include in their temporal representation only
the Attribute instances ... which meet the temporal, query and geoquery
restrictions". 021_08 queries `q=speed>90` and its expectation still lists
the speed instances below 90. Suggest dropping them, as the corrected fork
does (forge commit 9ca4213e); Antares carries that fix
(`temporal_autorecord.rs::temporal_query_keeps_only_the_instances_that_meet_q`).
The clause does not say how an instance meets a `q` over several
Attributes; Antares keeps an instance when `q` holds on the Entity with that
Attribute narrowed to that one instance.

---

## 19. [suite] 037_11 and 041_03 ask without Accept and expect ld+json pagination Links

**Title:** 037_11/041_03 expect `type="application/ld+json"` on the
pagination Links of an `application/json` response

**Body:**

CIM 009 6.3.10: the type Link Target Attribute "shall be exactly equal to
the media type resulting from the original request". Both test points query
without an Accept header, so the response is `application/json`, and still
expect `type="application/ld+json"`. Suggest sending
`Accept: application/ld+json`, as the corrected fork does (forge commit
c4cdb0cf, and the same line in its 037_11); Antares carries that fix and
types every pagination Link after the response media type
(`registration_listing_5_10_2.rs::a_registration_page_link_carries_the_requested_media_type`).

---

## 20. [suite] 051_04_01 expects BadRequestData for an unknown @context id with reload=true

**Title:** 051_04_01 expects 400 where 5.13.5.4 raises ResourceNotFound

**Body:**

CIM 009 5.13.5.4, second bullet: "If the @context identifier does not
correspond to any existing entry in the @context storage, then an error of
type ResourceNotFound shall be raised." The reload bullets that follow
describe an existing @context by its kind. 051_04_01 deletes an unknown id
with `reload=true` and expects 400 BadRequestData. Suggest 404
ResourceNotFound, as the corrected fork does (forge commit 39921641);
Antares carries that fix
(`contexts_5_13.rs::clause_5_13_5_delete_and_reload_errors`).

---

## 21. [suite] 008_01 expects a second temporal upsert to overwrite instances at the same observedAt

**Title:** the 008_01 expectation drops instances that the second
`POST /temporal/entities` must add

**Body:**

CIM 009 5.6.11.4: for a known Temporal Evolution "all the Attribute
instances included by the Temporal Evolution shall be added to the existing
Entity as mandated by clause 5.6.12". An instance is identified by its
system-generated instanceId, not by observedAt. The 008_01 expectation reuses
the update payload, so instances that share an observedAt with the first
POST are replaced and duplicates collapse. Suggest the union of both
payloads, as the corrected fork does (forge commit 167fa185); Antares
carries that fix
(`temporal_dedup.rs::a_second_temporal_upsert_at_the_same_instant_appends`).

---

## 22. [suite] 046_22_08 reads a deleted three-instance Attribute as one object

**Title:** 046_22_08 asserts `['name']['value']` on an Attribute whose three
instances were deleted

**Body:**

`name` in building-multi-instances-attributes has the default instance and
two datasetIds, and 046_22_08 deletes all of them. CIM 009 5.8.6 represents
each deleted instance with the NGSI-LD Null and its datasetId, so the
notification carries an Array of three, as it would for the live Attribute.
The assertion indexes a single object. Suggest asserting three instances,
each `urn:ngsi-ld:null`, as the corrected fork does (forge commit
75ba471d); Antares carries that fix
(`notify.rs::whole_attribute_versus_per_instance_deletion`).

---

## 23. [suite] 046_05 percent-encodes the q of a Subscription body

**Title:** the 046_05 Subscription carries `q` as `name=="Eiffel%20Tower"`

**Body:**

CIM 009 Table 5.2.12-1 types a Subscription's `q` as a string holding a
4.9 query, and the Subscription travels as a JSON body, not in a URI, so
4.9's percent-encoding rule for query strings does not apply. Read as
written, the q matches only an Entity whose name is the literal
`Eiffel%20Tower`. Suggest `name=="Eiffel Tower"`, as the corrected fork
does (forge commit cd2ef2ba); Antares carries that fix and reads the q as
written (`antares-matcher::a_subscription_q_is_not_percent_decoded`).

---

## 24. [spec] TS 104 176 9.4.3.1 and 9.4.3.2 carry each other's headings

**Title:** the `csourceSubscriptions` collection binds Create to a clause
headed GET/HEAD and Query to one headed POST

**Body:**

TS 104 176 V1.1.1 clause 9.4.3.1 (p. 95) is headed "GET/HEAD" and reads:
"This method is bound to the operation \"Create Context Source
Registration Subscription\" ... clause 12.4.2, taking the context source
registration subscription to be created from the HTTP request payload
body". Clause 9.4.3.2 (p. 96) is headed "POST" and reads: "This method is
associated to the operation \"Query Context Source Registration
Subscriptions\" ... clause 12.4.5, providing the context source
registration subscription data as part of the HTTP response payload body",
with URL parameters in Table 9.4.3.2-1.

A create takes a request body and a query takes URL parameters, so the
bodies of the two clauses are right and their headings are swapped. Every
other collection resource of the binding uses POST for create and
GET/HEAD for query. Suggest heading 9.4.3.1 "POST" and 9.4.3.2
"GET/HEAD".

Antares serves POST for create and GET for query on
`/ngsi-ld/v1/csourceSubscriptions` (`crates/antares-api/src/lib.rs`); the
operation matrix records the swap (docs/analysis/operation-matrix.md,
open question 9, T-3674).

## 25. [suite] 054_02_02 and 056_02_02 expect 400 for PUT and PATCH on /entities

TS 104 176 clause 6.3.2: "Method Not Allowed" (405) "shall be raised when a client invokes a recognized but unsupported
HTTP verb over a resource". With an empty entity id the request reaches the collection `/entities`, which offers GET,
POST and DELETE (Purge Entities) and no PUT or PATCH. Forge develop `cc8711ce` expects 400; the `integration/all-fixes`
branch already expects 405 (`b8914530`). Antares answers 400 until forge develop moves (T-5584).

## 26. [suite] 003_06_01 and 003_08_01 expect 207 for a batch whose Content-Type and @context disagree

TS 104 176 clause 6.2.4: "if the request payload body (as JSON) contains a @context term, then an HTTP error response of
type BadRequestData shall be raised". The clause answers the request with one error response; a 207 is a partial success.
Forge develop `cc8711ce` expects 207 with the error in `errors[0]`; `integration/all-fixes` expects 400 (`65ab7516`).
Antares answers 207 until forge develop moves (T-5584).

## 27. [suite] the 047 TPs do not consume the initial csource notification

TS 104 175 clause 12.4.7: "If a subscription does not define a timeInterval term, the csource notification shall be sent
on initial subscription and whenever there is a change in a matching csource registration." Forge develop's 047 TPs
create the Subscription before any matching registration and take the first notification they receive as the one the
registration triggers, so a broker that sends the initial notification fails them. `integration/all-fixes` waits for the
initial notification first. Antares skips it while nothing matches until forge develop moves (T-5584).

## 28. [suite] integration/all-fixes TPs that over-specify (found by T-5495, for the branch's maintainers)

- 020_13, 021_15, 021_16 `Check Temporal Pagination Link` matches `>;rel="..."` with no whitespace; RFC 8288 clause 3
  writes `link-value = "<" URI-Reference ">" *( OWS ";" OWS link-param )` (TS 104 176 6.4.7.3 mandates RFC 8288 section 3).
- 019_26_01..06 send `langProperties`, a Coraine extension; TS 104 175 7.2.3.2 defines q, expandValues and jsonKeys only.
- 019_02_08_05 and 019_27_05 expect `jsonProperty[id]` to miss without `jsonKeys`; TS 104 175 5.2.6.4.10 calls a
  JsonProperty's `json` "Raw unexpandable JSON which shall not be interpreted as JSON-LD using the supplied @context".
- D011_01_exc, D011_01_03_inc, D011_02_exc_01 stub the forwarded query only as `pick=...`; TS 104 176 7.2.3.1 still
  defines `attrs`, so the stub has to accept either.
- 020_13_05, 020_14_01/02, 021_15_05 expect 200 for lastN=20 over 20 instances; TS 104 176 6.4.7.3 pages at "the value
  assigned to lastN or the maximum limit configured, whichever is smaller", so the TP needs a configured limit of at
  least 20 (Antares' ETSI stack sets 9 for the vendored pagination TPs).
