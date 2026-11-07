# ADR-0023 — Endpoint views: one policy-narrowed NGSI-LD endpoint that stays compliant

Date: 2026-10-10. Status: accepted. Decided by worker-2 under the owner
order of 2026-10-10 ("a design decision is yours: pick the recommended
option"); the owner may override any rule below. Builds on ADR-0020 (the
policy seam) and ADR-0014 (fixed phases); supersedes neither. Task T-3438.

## Context

The joinedcontext platform serves one filtered endpoint per data product
today from a gateway in front of the broker (`joinedcontext-platform`
`crates/context-gateway`): an endpoint slug names a Context Space and its
policies, the gateway rewrites the query, and it projects the answer
(`docs/analysis/gateway-to-broker-map.md`). The owner wants that endpoint
inside the broker (`tasks/GOAL-antares-surfaces.md`, points 1 to 3), as an
NGSI-LD endpoint that a stock client reads like any other.

ADR-0020 already narrows inside the broker: an engine's `Filter` is
conjoined into the query the store runs, on the `antares-ql` AST, and its
`pick`/`omit` go through the projection. What it does not have is a named,
configured endpoint: a fixed predicate and allowlists that hold for every
caller of one address, whatever engine is installed. The operation matrix
(`docs/analysis/operation-matrix.md`) shows where the seam stops today:
discovery (10.6.x) is not in `policy::FILTERABLE`, writes are judged
without the stored Entity, a batch is decided once, and a join target is
fetched without a visibility check (T-3470).

Clause numbers below are TS 104 175 V1.1.1 and TS 104 176 V1.1.1
(`/workspace/etsi-specs/`); the code names CIM 009 clauses, translated in
`docs/analysis/cim009-to-ts104175.md`.

## Decision

A **view** is configuration, not an engine:

```
View {
    id:          a name, [a-z0-9-]{1,63}
    tenant:      the one Tenant it serves
    selector:    entity types (expanded IRIs), at least one
    q:           Option<QNode>        // antares-ql AST, parsed once at load
    scope_q:     Option<String>       // 7.2.4 scopeQ
    geo_q:       Option<GeoQuery>     // 7.2.5 geoQ
    rules:       [Rule]               // types, condition, attributes; rule 11
    operations:  allowlist of TS 104 175 operations (operation-matrix.yaml ids)
}
```

A request to a view runs the ordinary NGSI-LD handler with one more
conjunct: the view's predicate (`selector ∧ q ∧ scope_q ∧ geo_q`) and its
per-rule attribute allowlists (rule 11), composed with whatever the installed engine answers
(`effective = view ∧ engine`, both narrowing, neither widening). The view
is applied before the engine is asked, so `AllowAll` behind a view still
serves only the view. Without the `views` cargo feature none of this is
compiled; with it and no view configured, `/x/views` answers 404 for
every id (fail closed: no view means nothing served, never everything).

### 1. Conjunction on the AST, before paging, ordering and count

The view predicate is conjoined into the store query exactly as an
ADR-0020 `Filter` is: on the `antares-ql` AST, never on the query string.
Paging (7.4), ordering (7.6) and counting (7.5) are computed by the store
on the conjoined query, so `NGSILD-Results-Count` and the `Link`
`rel="next"` pages describe the narrowed result set and nothing else. 7.5
requires the count to be "the number of expected resulting elements", and
the expected elements of a view are the in-view ones.

`policy::Filter` carries a `geo_q` and a request may carry its own `geoQ`;
7.2.5 allows one geoQ per query. The store query takes a list of geo
predicates, all of which must hold (the view's and the request's): the
`antares-store` filters hold a slice of them, the Postgres arms conjoin
one predicate per entry, and the evaluator requires every one (T-3484).
A narrowing holds a list of areas too (`geo_q` and `geo_also`, T-3543), so
a view and an engine that both narrow by area at the same level compose
into both areas, in the `Filter` and in each paired rule, instead of
refusing.

### 2. A hidden attribute does not exist while `q` is evaluated

TS 104 175 7.2.3.3 states every operator in terms of containment:
"Existence (only attribute is specified). A matching entity shall contain
the target element", and each comparison begins "A matching Entity shall
contain the target element". 7.2.3.2 already uses the notion for an
undefined trailing path: "the target element shall be considered as
non-existent for the purpose of query resolution". A view applies the same
notion to every Attribute outside its allowlist: in the caller's `q`, a
term whose Attribute path starts at a hidden Attribute (at any hop of a
`LinkedEntityRelation`) is evaluated as if the Entity did not contain it.

So `q=secret>5`, `q=secret==1`, `q=secret!=1` and `q=secret~="x"` select
nothing, and `q=!secret` selects every in-view Entity, exactly what they
select over Entities that never had the Attribute. No answer differs
between "hidden" and "absent", which is the property that closes the
inference channel. The rewrite is done on the AST before the store sees
it (a hidden term becomes the constant `false`, its negation `true`), so no
store plan, index use or timing depends on the hidden values. `orderBy` on
a hidden Attribute orders as if it were absent (7.6.2.2 puts "Attribute
does not exist" last in the comparison order), and `attrs`/`pick`
naming one yield nothing for it.

Recorded spec defect: 7.2.3.2's ABNF reads `notOp = %x7E ; !`, but `%x7E`
is `~`, while Example 6 is `?q=!isMonitoredBy` (`!` is `%x21`). The broker
keeps accepting `!`; the defect is reported to ETSI with the TS 104 176
heading swap the matrix records (section 8, item 9).

### 3. An out-of-view Entity is not found: 404 ResourceNotFound

Decided (worker-2, owner may override): **404**, because ADR-0020 already
answers a hidden retrieve that way ("a caller cannot tell a hidden entity
from an absent one, and a retrieve of a hidden entity is 404"), because 404
is what a smaller store returns, and because 403 tells the caller the id
exists. Options: 404, or ADR-0020's 403 `AccessDenied`. The 403 stays for
what it is in ADR-0020: the engine's refusal of an operation, about the
caller and not about one Entity. The same 404 answers a retrieve of a
temporal Entity, an Attribute (10.4.4) and a write naming an out-of-view id
(rule 7). A create of an id that exists out of view is the one exception:
409, as decided in the operation matrix (section 8, item 4), since
TS 104 175 10.2.2.4 fixes that status ("If the Entity already exists
locally this shall result in an error of type AlreadyExists").

### 4. A disallowed operation: 405 with `Allow`, 422 only when the method stays

TS 104 176 6.3.2 names the HTTP-level answer: "'Method Not Allowed' (405)
which shall be raised when a client invokes a recognized but unsupported
HTTP verb over a resource. Implementations shall provide the allowed HTTP
methods". A view that does not offer Create Entity does not support `POST`
on `/entities`; 405 with the `Allow` header is the answer a stock HTTP
client understands, and `Allow` lets the client discover the view's shape.

Where one method and path serve several operations and the view allows
some of them (`PATCH /entities/{id}/attrs` is Update Attributes; `DELETE
/entities` with and without the purge parameters), the method stays
allowed and the disallowed variant answers 422 `OperationNotSupported`
(TS 104 175 8.3.2: "The operation is not supported"; 422 per TS 104 176
Table 6.3.2-1, read from the PDF, the clause file lost the status
column). `GET` and `HEAD` are never removed from a resource the view
serves at all: TS 104 176 6.3.2 says "a Context broker shall recognize the
methods GET and HEAD on all endpoints". 501 stays what 6.3.2 makes it, an
unrecognized verb.

### 5. Discovery is computed from the view

`/types`, `/types/{type}`, `/attributes`, `/attributes/{attr}` (10.6.3 to
10.6.8) answer from the Entities the view selects, with the attribute
allowlist applied: a type the selector does not name is absent from the
list and `/types/{t}` answers 404; a hidden Attribute is absent from
`attributeNames`, from `attributeDetails` and from `/attributes`. These
clauses join `policy::FILTERABLE` (CIM 009 5.7.5 to 5.7.10), so an engine
can narrow them too, with `pick`/`omit` read as type and Attribute names.

`/jsonldContexts` (13.3) lists the core `@context` and the contexts the
view's configuration names. A stored `@context` is the tenant's
(ADR-0021), and one may define a term for a hidden Attribute, so a view
does not list the tenant's contexts by default. A view that shows every
name (rule 12) hides no term, so it lists them as the root does
(T-3946). Add, Delete and Reload @context (13.2, 13.5) are writes and
are offered only when allowlisted.
The same holds at the root for a caller the engine narrows (T-4009): List
and Serve @context (13.3, 13.4) take a `Filter` from the engine, read as
"this caller sees fewer names than the tenant holds", and under one the
list is the core `@context` alone and a stored `@context` is not found.
The reference engine answers them with a narrowing unless the caller may
query every Entity whole.

### 6. Joins: an out-of-view target is not embedded, its Relationship stays

`join=inline|flat` and `joinLevel` (7.7) embed a target only if the view
and the engine would select it. A target that fails is treated exactly as
a target that is not stored locally (7.7.1: "Only Relationships targeting a
locally stored Entity ... are considered to be retrievable in this
manner"): not embedded, no error. The Relationship and its `object` URI
stay in the linking Entity unless the Relationship itself is a hidden
Attribute. The check is one shared function over both walks; T-3470 fixes
it for ADR-0020 today, and a view reuses it.

### 7. Writes: pre-image visible, post-image still in view, allowlist only

Every write that names a stored Entity loads it first and evaluates the
view on that pre-image: out of view is 404, as for a read (rule 3). The
proposed result (the stored Entity with the write applied, as the handler
computes it for 10.2.3/10.2.4/10.2.9/10.2.10) is evaluated again before the
commit: a write whose post-image the view would not select is refused with
403 `AccessDenied` ("the write would move the Entity out of this view").
The caller saw the pre-image, so the refusal tells them nothing new, and
nobody can launder an Entity into another view by changing the member the
view keys on.

A write that names an Attribute outside the allowlist is refused whole
(403, never a silent drop of that Attribute: the platform's GW17 rule).
Creation (10.2.2, 10.3.2, the create half of 10.3.3) must produce an
Entity the view selects, or it is refused with 403 before anything is
written. A batch (10.3.x) is decided per Entity and answered with the
`BatchOperationResult` (207 when mixed): every member is its own
single-Entity operation under these rules, which needs the one-`Operation`-
per-member call the matrix recommends (section 8, item 6) inside one engine
call (ADR-0014 rule 1). A Relationship write (any write of a Relationship
Attribute) requires the target Entity to be visible in the view as well.

Batch Upsert (10.3.3) splits each member under the store's lock (T-3594).
A stored Entity in the view is replaced, or with `options=update` updated,
between the pre-image and post-image checks; a replace counts every
Attribute the Entity holds as written, because it deletes the ones it
omits, so an Entity holding a hidden Attribute is refused (threat model
D6). An absent id, and one out of the view, take the create half: the
Entity is created in the view or refused with 403, and an out-of-view one
is never overwritten. Its create answers 409, the exception rule 3 names
for a create of a taken id. Decided (worker-2, owner may override): the
create half for an out-of-view id, because overwriting it would replace an
Entity the caller cannot see and a 404 would deny a create the operation
promises; options: 409 through the create half, 404 as for a write naming
the id.

### 8. Temporal: membership by current state, limits after the allowlist

A temporal request (11.3.2, 11.3.3) serves the history of the Entities the
view selects **now**, with hidden Attributes removed from every instance.
`lastN` (and `firstN`, T-3456) are applied per Attribute after the
allowlist, so a hidden Attribute never uses up a caller's instance budget,
and aggregated values are computed over visible instances only.

Decided (worker-2, owner may override): current state, because a view is a
predicate over Entities and the store can evaluate it in the same query
that selects the Entities; evaluating it per instant would need every
member the predicate reads to be recorded temporally. Options: current
state, membership per instant. The residual: an Entity that enters the
view brings its earlier history of visible Attributes along. A view whose
history must not leak that way keys its predicate on members that do not
change (type, id pattern, `scope`), and the view loader warns when a
temporal-enabled view's `q` names a mutable Property.

For a rule with a condition, rule 11 replaces current-state membership by a
check per instance: an instance is served only where the rule held at its
time, so an Entity entering zone A does not bring its earlier `speed`
along.

### 9. Subscriptions: stored as sent, the view applied at match time

A Subscription created through a view is stored as the client sent it,
with the view id recorded beside the subject (`__view`, under the `__`
prefix ADR-0020 reserves for broker members). At match time the
notification condition is `view ∧ subscription`, so a narrowed or removed
view takes effect on existing Subscriptions without a rewrite. Before
`pre_notify`:

- hidden Attributes are removed from the notified Entities, including
  `previousValue` (`showChanges`, 5.2.6.4.x) and any `join` target (rule 6);
- a notification whose only change was to hidden Attributes is not sent
  at all, and counts as not triggered (no `timesSent`, no
  `lastNotification`, the reading ADR-0020 gives a dropped notification):
  sending an empty notification would leak the timing of hidden writes;
- `watchedAttributes` is intersected with the allowlist at creation; an
  empty intersection is 400 `BadRequestData`, and no `watchedAttributes`
  means the visible Attributes.

### 10. Where views live, how they reload, how one is addressed

Views are data (ADR-0014 rule 3): stored as an internal doc kind under the
reserved tenant (ADR-0012), written through `/q` (the admin surface, the
gateway's to protect, ADR-0020), and loaded at start. A change is
published on the change bus (ADR-0002) and every pod swaps its view table
atomically; no restart. A view that fails validation at load (unknown
type, unparsable `q`, an operation id the matrix does not have) is not
served and `/q/health` names it: never served without its predicate.

A view is addressed by path: `/x/views/{view}/ngsi-ld/v1/...`. TS 104 176
clause 5: "All resource URIs of this API shall have the following root:
{apiRoot}/{apiName}/{apiVersion}/ ... The apiRoot includes the scheme
("http" or "https"), host and optional port, and an optional prefix
string". `/x/views/{view}` is that optional prefix, so a view is a
compliant NGSI-LD API root, and `/x` is where `surface.rs` lets a
deployment mount anything. Per-host addressing is not adopted: hostnames
are DNS and certificate work outside the broker. A view pins its tenant: a
request on a view path that carries `NGSILD-Tenant` naming another tenant
is 400 `BadRequestData` ("a view serves one tenant"), never a silent switch.

### 11. Attributes are allowed per rule: types and a condition, expanded IRIs

Amended 2026-10-10 (T-3528), twice. The owner's first finding: "from cars
only these attributes, from vehicles and users totally different
attributes, with different names". The first version of this ADR gave a
view one flat `attributes` list, which is the platform's T-1862 leak: an
Attribute allowed on `Car` is served on `User` because it is in the one
list, and a query over two types leaks (gap G4,
`docs/analysis/gateway-to-broker-map.md`). The second: "one entity in a
specific filtered area like specific GPS can have different attributes
than entities of the same type elsewhere". So the allowlist is not per
type but per rule, the way the platform's grants already carry their own
types, `q`/scopes (K10), area (K12) and attribute list (K9).

A view's `rules` is a list of at least one rule:

```
Rule {
    types:       Set<type IRI>, at least one
    q:           Option<QNode>      // on the Entity's own members, no linked path
    scope_q:     Option<String>
    geo_q:       Option<GeoQuery>
    attributes:  Set<Attribute IRI>
    every_attribute: bool           // engine rules only, see below
}
```

Example: `{Car, geoQ: within zone A, [speed, location, status]}`,
`{Car, [status]}`, `{User, [name, role]}`. The per-type map of the first
amendment is the special case "one rule per type, no condition".

- **Membership and projection per Entity, on the stored Entity.** An Entity
  is in the view when it matches at least one rule: one of its types is in
  the rule's `types` and the rule's condition holds. It shows the union of
  the `attributes` of the rules it matches and nothing else besides the
  Entity frame (`id`, `type` and the core members ADR-0020 never removes).
  An Entity no rule matches is out of the view (rule 3: 404 on a retrieve,
  absent from a query), never "frame only".
- **Conditions are the policy's, not the caller's.** A rule's condition is
  evaluated on the full stored Entity (it may read an Attribute the caller
  cannot see), with its names expanded against the core `@context`, never
  the request's, so a caller's `@context` cannot change what a rule means.
  A rule `q` with a `LinkedEntityRelation` path never matches: the visible
  set of one Entity must not depend on another Entity the caller may not
  read, and failing closed shows less, never more.
- **Keys are expanded IRIs.** Types and Attributes are expanded through the
  view's `@context` at load. The same short name with two meanings (`Car`
  `name` from one vocabulary, `User` `name` from another) is two IRIs and
  two different Attributes; the same IRI may be visible under one rule and
  hidden under another.
- **Rule 2 per Entity.** The caller's own `q`, `geoQ`, `attrs` and
  `orderBy` are evaluated against the Entity as it is shown, so an
  Attribute a matched rule does not show is absent there (7.2.3.3, as in
  rule 2): `q=speed>100` never finds a Car outside zone A, and `q=!speed`
  finds it. The view's predicate (the OR of its rules) is evaluated on the
  stored Entity and the caller's on the shown one, before paging and count
  (rule 1), so `NGSILD-Results-Count` and the pages describe the same set.
  No warning is added: a warning that appears only when a hidden value
  matched would itself be the oracle rule 2 closes.
- **Every surface.** The rule holds at every join level (rule 6: a target
  by its own rules), in discovery (rule 5: a type lists only the
  Attributes some of its Entities show), in notifications (rule 9: the
  rules are evaluated on the notified state; a notification carries only
  the Attributes visible now, `previousValue` only for those; an Entity
  that left every rule is not notified), and in temporal answers (below).
- **Moving across a boundary.** A read uses the current state: a Car that
  drives out of zone A loses `speed` and `location` from that moment.
- **Temporal, per instance (amends rule 8 for conditional rules).** An
  instance of an Attribute at time `t` (its `observedAt`, else
  `modifiedAt`) is served only if a rule that allows the Attribute held at
  `t`: its condition is evaluated on the Entity's state at `t`,
  reconstructed from the history as the latest instance at or before `t`
  of every Attribute. Where a member the condition reads has no instance at
  or before `t`, or the condition reads `scope`, which has no history, the
  rule does not hold for that instance: the instance is hidden, never
  served. A rule without a condition holds at every `t`. An Entity is
  answered when at least one instance survives.
- **Writes (rule 7 per rule, K22).** The post-image must fall inside at
  least one rule, and every Attribute the write names must be allowed by a
  rule that holds for the post-image: a write sets what the Entity then
  shows, the union of its rules. A write that would move an Entity into a
  zone where it shows more is allowed only if a rule holding there allows
  those Attributes. Decided (worker-5, owner may override, T-4753): the
  union, not one rule covering every name, because a view splits a type's
  list past 256 names over several rules and the one-rule check refused
  every row naming one name of each part (17,025 rows of the BBSK open data
  on prod), and two writes, one per rule, already reached the same state;
  options: union of holding rules, one rule per write.
- **Disclosure, documented.** The attribute set reveals which rule matched:
  seeing `speed` means "in zone A". The view loader reports, in `/q/health`
  and the view's describe output, every rule whose condition reads an
  Attribute that rule does not show, so the admin sees what the visible
  set discloses.

The engine's `policy::Filter` gets the same shape: `rules`, a list of
`policy::Rule`, beside the flat `pick`/`omit` (which still apply to every
Entity; both narrow, so they intersect) and `omit_by_type` (an Attribute
any of the Entity's types omits is removed whatever rule matched: an omit
is a promise that the Attribute is never served on that type). A View
compiles into these rules; `Filter::admits_write` is rule 7's check and
`Rule::discloses` the loader's report.

Decided (worker-1, owner may override): an Entity no rule matches is out
of the view, because the owner's rule reads "in the view if it matches at
least one rule" and an identity-only answer would still disclose that the
Entity exists; options: out of view, frame only (the first amendment).
Decided (worker-1, owner may override): absence for the caller's
predicates instead of dropping the Entity with a 199 warning (the first
amendment's choice), because rule 2 already states absence and the warning
appeared only when a hidden value matched, an oracle; options: absence,
drop with warning, drop silently. Decided (worker-1, owner may override):
no pushdown while rules are live: the store answers the policy's own
predicates and the broker evaluates the rules and the caller's predicates
over that set, so paging and projection pushdown are forfeited
(`ponytail:` a scan; compile the rules' OR to SQL when it shows up);
options: scan, SQL compilation of per-Entity visibility now.

Amended 2026-10-11 (T-3637): an engine's rule may show the whole Entity.
The platform's grant without an attribute list is the whole Entity (map
K9, `no_whitelist_anywhere_means_the_whole_entity`), and a rule with an
empty `attributes` shows its frame alone, so the joinedcontext engine could
not say it. `Rule::every_attribute` set means the rule shows every
Attribute and its `attributes` are not read: an Entity it holds for keeps
everything the stored Entity carries (minus `omit`, `omit_by_type` and
`pick`, which still narrow), a write under it may name any Attribute, it
discloses nothing (it shows what its condition reads), and a temporal
answer keeps every Attribute's instances at the times it holds. Composed
with a view's rule (rule 1), the pair shows the view rule's Attributes.
A view never sets it: a view states every Attribute with rule 12's `*`.
Decided (worker-1, owner may override): a flag beside the list, because a
reserved name in the list would be compared as an Attribute IRI wherever
the list is read; options: a flag, a reserved `*` member, an
`Option<Set>`, which would change every reader of the list.

### 12. A view that narrows nothing: `*`

Added 2026-10-11 (T-3666). The confirmation below runs the ETSI suite
through a view "that selects every type and hides nothing", and
`surfaces-conformance.md` section 1 names it `all`. Rules 10 and 11 cannot
say it: a selector names its types and a rule its Attributes, at most 256
names a list and 64 rules, while the suite makes up types and Attributes
as it goes. So a view may write one wildcard form, and only that one:

```
"selector": ["*"],
"rules": [{"types": ["*"], "attributes": ["*"]}]
```

It selects every Entity type and shows every Attribute; the view's own
`q`, `scopeQ` and `geoQ` still narrow, its `operations` still decide
what it serves, and its Tenant stays pinned (rule 10). It compiles to a
narrowing with no rules, which is what an engine's `Filter` without rules
already means, so every rule above reads it unchanged: `view ∧ engine` is
the engine's answer under the view's conditions.

`*` is never a name: it is not expanded through the view's `@context`, so
a vocabulary that defines a term `*` cannot widen a view. Any other use
of `*` is a load error: beside a name in the same list, in a rule beside
other rules, in a rule with a condition, as a rule's types with named
Attributes or the other way round. Each of those would be a view rule over every type
or every Attribute, which no view asks for; an engine says the whole
Entity with `Rule::every_attribute` (rule 11, amended by T-3637).

Decided (worker-2, owner may override): the whole-view wildcard alone,
because it is what the confirmation and the conformance jobs need and it
adds no member to `policy::Rule`; options: the whole-view wildcard
(chosen), `*` per list (a rule over every type with named Attributes,
named types with every Attribute), a boolean member `narrowsNothing`
(a second way to say what `rules` already says).

## Threats and the rule that answers each

| threat | example | rule |
|---|---|---|
| inference through `q` on hidden Attributes | `q=salary>50000` returns a subset | 2 |
| enumeration of hidden Entities | `GET /entities/{id}` for guessed ids | 3 (404), 6 |
| count and paging leaks | `count=true&limit=0`, `Link` pages | 1 |
| ordering leak | `orderBy=hidden` reorders | 2 |
| discovery leak | `/types`, `/attributes`, `@context` terms | 5 |
| join leak | `join=inline` reaches an out-of-view target | 6, T-3470 |
| write laundering | PATCH `scope` moves an Entity into another view; write to a hidden Attribute | 7 |
| existence oracle on create | 409 on an out-of-view id | 3 (decided 409, matrix item 4) |
| notification timing leak | a notification fires when only a hidden Attribute changed | 9 |
| notification content leak | `previousValue`, joined targets | 9, 6 |
| temporal history leak | history before the Entity entered the view | 8 (residual, documented) |
| tenant switch | `NGSILD-Tenant` on a view path | 10 |
| misconfiguration served wide | a view with a broken predicate | 10 (not served), fail closed |
| cross-type attribute leak (T-1862) | an Attribute allowed on `Car` served on `User`; `q` on it over both types | 11 (per-rule allowlists, no matching rule = out of view) |
| cross-zone attribute leak | `speed` allowed on Cars in zone A served on a Car elsewhere; `q=speed>100` finds it; its history after it left | 11 (union over the matched rules only, rule 2 per Entity, temporal per instance) |

## Consequences

- The broker gains a `views` cargo feature, off by default; the vanilla
  build and its ETSI result are unchanged (GOAL point 1), proven by the
  same `dev/check-no-addon.sh` style gate the ADR-0020 addons use.
- `policy::FILTERABLE` grows by the six discovery clauses; `Operation`
  gains the pre-image the matrix recommends (section 8, item 2); the
  batch handlers decide per member; the store query takes a list of geo
  predicates. Each is its own implementation task (T-3445).
- The platform gateway's NGSI-LD representation is replaced by a view per
  endpoint slug, and its code is removed once every caller uses the view
  (GOAL point 11; the inventory is `gateway-to-broker-map.md`).
- A view and an engine compose; neither replaces the other. A deployment
  may run views with `AllowAll`, an engine without views, or both.

## Confirmation

Each rule has one named test, filed by T-3445 from the T-3444 plan, in
`crates/antares-api/tests/views_*.rs` beside the ADR-0020 ones:

| rule | test |
|---|---|
| 1 | `views_count_and_paging_describe_the_narrowed_set` |
| 2 | `views_a_hidden_attribute_does_not_exist_for_q` (every operator, `!`, linked paths, `orderBy`) |
| 3 | `views_an_out_of_view_entity_is_not_found` (entity, temporal, attribute, write) |
| 4 | `views_a_disallowed_operation_answers_405_with_allow` and `..._422_when_the_method_stays` |
| 5 | `views_discovery_is_computed_from_the_view` |
| 6 | `views_join_omits_out_of_view_targets_and_keeps_the_relationship` |
| 7 | `views_writes_check_pre_and_post_image_and_the_allowlist`, `views_batch_is_decided_per_entity` |
| 8 | `views_temporal_serves_current_members_and_limits_after_the_allowlist` |
| 9 | `views_notifications_drop_hidden_changes_and_strip_previous_value` |
| 10 | `views_reload_without_restart`, `views_a_broken_view_is_not_served`, `views_pin_their_tenant` |
| 11 | `two_types_one_view_each_sees_only_its_own_attributes`, `same_short_name_two_vocabularies_are_different_attributes`, `same_iri_visible_on_car_hidden_on_user`, `same_type_two_zones_two_attribute_sets`, `entity_matching_two_rules_gets_the_union`, `entity_matching_no_rule_is_not_served`, `q_on_speed_never_matches_cars_outside_zone_a`, `count_and_paging_consistent_with_rule_union`, `car_leaving_zone_a_loses_speed_on_next_read`, `notification_after_zone_exit_has_no_hidden_previous_value`, `temporal_speed_history_only_while_in_zone_a`, `join_target_projected_by_its_own_rules`, `discovery_lists_each_types_own_attributes_only`, `no_answer_holds_an_entity_attribute_pair_no_matching_rule_allows` (T-3528; the engine half in `policy_filter_by_type.rs`), `write_that_adds_speed_outside_zone_a_is_refused` (`policy::tests`) |

Plus, for the whole: the ETSI suite run against `/x/views/{all}/ngsi-ld/v1`
where `{all}` selects every type and hides nothing must give the vanilla
result (a view that narrows nothing is the vanilla broker), and the
default build links no view code (`cargo tree` gate).
