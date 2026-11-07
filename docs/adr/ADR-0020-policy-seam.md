# ADR-0020 — The policy seam: one trait, one built-in engine, every engine an addon

Date: 2026-09-02. Status: accepted, amended (the narrowing marker header).
Reverses the "no policy code in the
broker" reading of the standing PEP decision (SECURITY.md,
`docs/roadmap-1.0.md`, `docs/policies.md`), which otherwise stands.

## Context

The standing decision puts authentication, authorization, rate limiting
and quotas in the gateway in front of the broker, and `docs/policies.md`
is the design that follows it: APISIX as the PEP, OPA as the PDP,
Keycloak and a VC verifier for identity. That division holds for
everything a gateway can decide from the request line and the response
bytes.

It stops holding where the decision is about data the gateway never sees
as data:

- **A query the broker runs.** "This consumer sees only entities in scope
  `/BB/Traffic`" has to narrow `GET /entities` before the store answers.
  A gateway can only rewrite `q=` on the way in — string surgery on a
  grammar with `;`, `|` and parentheses, where a wrong parenthesisation
  widens the result instead of narrowing it — or filter the response
  after the fact, which has already paid for the rows, and which leaves
  `NGSILD-Results-Count` and the paging links describing a result set the
  caller is not allowed to see.
- **One subscription's notification.** 5.8.6 delivery is broker-initiated:
  the notification goes from the broker to the subscriber's endpoint. A
  gateway in front of the broker is not on that path at all.
- **A federated merge.** 4.3.6 merges the local answer with what the
  Context Sources returned. Only the broker holds the merged result
  before it is rendered.

The deployment that forces the question is the three-organization
federation: a registration forwarded to a peer must carry the narrowing,
or the peer answers more than the consumer may see, and 4.3.6.1's own
narrowing rules then travel with a predicate the gateway never wrote.

## Decision

The broker gains a policy **seam**, and no policy engine.

**Module `antares-api::policy`** — the trait, the types and one built-in
`AllowAll` engine. Core code: always compiled, always tested, always
shipped, no feature flag.

```
PolicyEngine: Send + Sync {
    fn name(&self) -> &str;
    fn decide(&self, subject: &Subject, op: &Operation) -> DecisionFuture;
    fn pre_notify(&self, subject: &Subject, sub: &Value,
                  notification: &mut Value) -> NotifyDecision;
}
```

`Subject { tenant, headers }` carries the identity headers named by
`ANTARES_POLICY_SUBJECT_HEADERS`, opaque to the broker — it parses no
credential and validates no token. `Operation` is the expanded request:
clause, ids, types, attrs, `q`, `scopeQ`, geo, body, and for a write by
id the stored Entity's image (`stored`: types, scope, Attribute names;
ADR-0023 rule 7). `Decision` is
`Allow`, `Deny(String)`, `Filter(Filter)` or `AllowIf(Filter)` (gap G12,
T-3634); `Filter` carries a `q`, a
`scopeQ`, a list of areas (`geo_q` and `geo_also`, all of which must hold;
T-3543), `omit`, `pick` and `restricted`.

**Two phases**, both already in ADR-0014's table:

| phase | when | granularity |
|---|---|---|
| `on_request` | after negotiation and expansion, before the operation and before any fan-out | once per request |
| `pre_notify` | notification built, before the egress check and the send | once per notification document per subscription |

**The answers:**

| decision | the broker does |
|---|---|
| `Allow` | nothing |
| `Deny` | 403 with ProblemDetails whose `type` is an Antares URI — Table 6.3.2-1 has no access-denied type, so this is an Antares decision with its own `AntaresSpecificTests` and a 6.3.2 ledger note |
| `Filter` | narrows: the `q` and the `scopeQ` are conjoined into the query the store runs and travel on forwards; the `geo_q` joins the request's own geoquery in the store query's list of geo predicates, all of which must hold (ADR-0023 rule 1, T-3484), and is checked again after a federated merge; `omit`/`pick` go through the 5.2.14.1 projection the notification path already has, and a merged federated result is filtered after the merge |
| `Filter { restricted: true }` | the same, plus `Antares-Results-Restricted: true` (see the amendment) |
| `Filter` on anything outside `policy::FILTERABLE` | answered as `Deny` — there is no narrowed form of "delete everything", and no handler outside that list reads the filter at all |
| `AllowIf` on a write by id (`policy::WRITE_BY_ID`) | the write is applied only if the stored Entity matches the filter's `q`, `scopeQ`, areas and the types and conditions of its rules, judged inside the store's own read-modify-write on the document it is about to change; an Entity it does not select answers 404 with the body an absent one gets; a batch judges each member by its own Entity; a write that would be forwarded to a Context Source is refused (403), since no copy there can be judged here (T-3634) |
| `AllowIf` anywhere else, or with a projection (`pick`, `omit`, `omit_by_type`, `temporal`, a rule's Attribute list or windows) | answered as `Deny`: a create and Delete Temporal Evolution (5.6.16, the temporal seam has no conditional delete) have no stored Entity to judge in their write, and a write is performed whole or not at all; an empty condition is `Allow`. Batch Upsert is a write by id since T-3668: a conditioned member updates or replaces the Entity its condition selects and never creates, so an absent one answers the same 404 as one the condition does not select. So are the temporal writes 5.6.12 to 5.6.15 (T-3668): the condition is judged inside the temporal store's read-modify-write on the evolution's latest instances, one per Attribute and `datasetId` (greatest `observedAt`, else `createdAt`), and one it does not select answers the 404 an absent evolution gets; under a view they stay refused (rule 7 has no temporal write guard) |

The conjunction is made on the `antares-ql` AST, never on the query
string, so the precedence trap a gateway rewrite has to distribute around
cannot occur. Narrowing is silent: a caller cannot tell a hidden entity
from an absent one, and a retrieve of a hidden entity is 404.

A narrowing reaches every document the answer carries, not only the one the
request named: 4.5.23 `join=` appends Entities reached over a Relationship,
and a member the subject may not see is not less hidden one hop away. The
policy projection is therefore kept apart from the request's own — a 4.21
projection belongs to one level, where a bare `omit` name deliberately
leaves that member alone on a joined document — and it travels down the
join walk unchanged. A Relationship the narrowing removes is not traversed
either.

A narrowing is refused where it cannot be applied. Only the reads in
`policy::FILTERABLE` hand the `Filter` to the code that serves the answer:
retrieve and query Entities, their two temporal twins, the EntityMap
creations those queries page through, and discovery (5.7.5 to 5.7.10,
which ADR-0023 rule 5 computes from the Entities the narrowing selects).
Every other operation is performed
whole — a create writes its Entity or does not — so a `Filter` there would
be dropped, and the broker would do in full what the engine believed it had
narrowed. That is the one direction the seam may not fail in, so those
clauses answer a narrowing the way 5.6.21 does: `Deny`. An engine that
means to let such an operation through says `Allow`.

The tenant an operation is judged under is the caller's. 6.3.22 answers a
request from a Snapshot's frozen copy by swapping the tenant for the
internal one that copy lives under, and a rule set is keyed by tenant, so a
snapshot-scoped read judged under `snap-<uuid>` would match no rule any
deployment ever wrote. The gate resolves that internal tenant back to its
owner before it asks.

A `pick`/`omit` name is one Attribute name (6.5.3.1), not a 4.21 projection
expression, because that grammar reads a dot as the sub-attribute path
separator and would truncate an IRI at the first dot of its authority. It is
read in the CORE `@context` and never in the request's own: `expand_key`
consults the term map before it decides a name is already an IRI, so a
caller that binds the rule's term — or the rule's IRI, as a term, which an
inline `@context` on a Query body is enough to do — would move the rule off
its target and walk out of the narrowing. A rule belongs to the deployment
and means the same thing whatever the caller sends. Write it as the IRI and
it passes through unchanged; write it as a short name and it is read the way
a name the request does not define is read anywhere else, which covers the
default `@context` and nothing more.

**What a notification answer may be.** `pre_notify` narrows by projection
only. `Drop` is not a failed delivery: 5.11.7 moves `notification.timesSent`
and `lastNotification` for a notification that "shall be sent", and one the
engine dropped never was — the same reading the broker already applies to a
cooldown (5.2.15) and an open circuit. A `Filter` carrying `q` or `scopeQ`
is refused as a `Drop`: the entities were selected by the subscription's own
conditions long before the seam sees them, there is nothing left to re-run a
query against, and delivering unfiltered would report a narrowing that never
happened.

**A broker-held record outlives its request.** An EntityMap (5.14) and a
Snapshot (5.16) are both a stored answer to one query, reused by later
requests, so each records the subject it was made for in `__subject`. 5.5.14
already says what to do with a map this subject cannot access — "no inference
can be made … and a new one shall be created" — so a map presented by another
subject is answered exactly as an expired one is, and the two cannot be told
apart; retrieving or deleting it is `ResourceNotFound`, because its
`entityMap` member is the id set a narrowing exists to withhold. The one
allowance the clause makes in the other direction stands: other components
"shall only be allowed to update the expiry timestamp", so 5.14.2 Update is
open to a subject that did not build the map. A snapshot's fill runs after
its request has been answered, which is why the subject is recorded there
too — and why 5.16.1, 5.16.2 and 5.16.7 sit in the whole-tenant list with
5.6.21: there is no narrowed form of "copy everything these queries match".

**Whose subject a notification is under.** 5.8.6 delivery is
broker-initiated, so no request is in flight when it is decided. The subject
is the subscriber's, taken from the creating request and stored with the
subscription in `__subject` — a broker-internal member beside the `__context`
that same clause already keeps there. The `__` prefix as a whole is the
broker's: a client can set no member under it, no served representation
carries one (5.8.3/5.8.4 serve the 5.2.12 data type, which defines none), and
the 5.8.1.4 copy forwarded to a Context Source is stripped of all of them.

**Fail closed.** An engine error, panic or timeout
(`ANTARES_POLICY_TIMEOUT_MS`) is `Deny`. `AllowAll` has no error path. An
engine has two places to panic and both deny: the call that builds the
future `decide` returns, and the future itself — a synchronous engine
decides entirely in the first, so a guard around the second alone would
guard the half that does nothing. The timeout can only race the future:
work done before the future exists holds the executor thread and no timer
in the same task can interrupt it, so an engine that blocks is bounded by
the request timeout in front of the broker and not by this seam.

**The subject never travels.** The subject headers are stripped from
every forwarded request in `federation::forward`, and never enter a
notification, a log line, a dead letter or a peer registration. That
stripping has to be active rather than incidental: 4.3.6.5 lets a
registration name a header to copy from the triggering request
(`"urn:ngsi-ld:request"`), and a registration is client-supplied, so
whoever can register a Context Source could otherwise read the identity of
every request that fans out to it. The names in
`ANTARES_POLICY_SUBJECT_HEADERS` are dropped from that copy — dropped, not
refused at 5.9.2, because which headers a deployment made subject headers
is not something a client gets to probe by watching which registrations
fail. The one place the subject does travel is the broker's own
subscription mirror: 5.8.6 delivery is broker-initiated, and a pod that
mirrored a subscription without its subject would decide every notification
under nobody. That is the broker's own state, the same value the store row
holds, not a peer. `/q` is outside the seam: the admin surface is the
gateway's to protect.

**Every engine is an addon**, in the sense `plugin-example` is: a crate
outside `crates/`, behind an off-by-default `antares-broker` feature,
built and run by `examples.yml` alone. The shipped image, `ci`, `full`,
`strict` and every ETSI cell run `AllowAll`. No core crate names an
engine crate.

A policy engine's rule language, identity model and data source are
deployment choices. A broker that shipped one would make conformance a
function of its configuration — the argument `surface.rs` already makes
for the `/x/` prefixes. Conformance is therefore asserted against the
built-in engine, and a deployment running another engine makes no
conformance claim.

## Amendment: the narrowing marker is an Antares header

This decision first named the marker `NGSILD-Results-Restricted`. It is
sent as `Antares-Results-Restricted` instead.

`NGSILD-` is ETSI's prefix, and clause 6.3 defines what it carries:
`NGSILD-Tenant` (6.3.1), `NGSILD-EntityMap`, `NGSILD-Results-Count` and
`NGSILD-Warning`. A broker-invented header under that prefix claims a name
CIM 009 has not assigned, and a later version that assigns it to something
else turns every Antares deployment into a client-visible conflict. The
same reasoning already puts the access-denied error type on an Antares URI
rather than under `https://uri.etsi.org/ngsi-ld/errors/`: where the
standard is silent, Antares answers in its own namespace and says so.

Nothing else changes — the header is still advisory, still sent only when
the engine sets `restricted`, and a narrowing is otherwise silent.

## Amendment: rule documents through `/q/policy`, one image per engine

An engine has no route of its own (surfaces live under `/x/`,
`crates/antares-api/src/surface.rs`) and, before this amendment, no hook
to receive data, so a rule table could only be compiled into the engine
or read by it from somewhere the broker does not control. ADR-0014 rule 3
asks for configuration "reloaded at runtime from the stores the broker
already has", and every pod must serve the same revision.

The seam therefore gains one hook and one document table:

- `PolicyEngine::load(tenant, &Value) -> Result<(), String>` hands the
  engine the rule document of one Tenant (`Value::Null` unloads it), and
  `PolicyEngine::loads_documents() -> bool` says whether the engine reads
  documents at all. Both default to no: `load` refuses, and the built-in
  `AllowAll` and a broker without an engine answer `/q/policy` 404. A
  refused document leaves the engine's state as it was, so the previous
  revision keeps deciding.
- `/q/policy` (GET, the table) and `/q/policy/{tenant}` (GET, PUT,
  DELETE), served by `antares-ops` beside `/q/views` and behind the same
  admin credential. The document is opaque JSON: the engine owns its
  schema, so the broker bounds it (1 MiB since T-5717, depth and element count) and
  validates nothing else. A PUT the engine refuses answers 400 with the
  engine's reason and is not stored.
- Documents are stored under the reserved tenant `antares-policy`, with a
  revision the broker counts, and reloaded with the view table (every
  `ANTARES_VIEWS_RELOAD_MS` and after every write on the pod that took
  it). A stored revision the engine refuses on reload (written by another
  version, or by hand) is listed in `/q/health` under
  `policy.documents.refused` by tenant and revision only. The document,
  and the engine's reason, never reach a log or `/q/health`: a rule
  document says who may read what.

The table needs the `views` feature, as `/q/views` does. A vanilla broker
has neither the routes nor the reserved tenant's documents, and its tree
is unchanged.

The sentence "built and run by `examples.yml` alone" above is narrowed: a
deployment that runs an engine may build its own image from the same
Dockerfile with the engine's feature, one image per engine. That image is
never published as `antares-broker`, the Antares release image and every
conformance cell keep `AllowAll`, and the deployment image claims no
conformance, as the paragraph above already says of any engine.

## Amendment: the view a request came through

An engine that binds its rules per endpoint has to know which view a
request came through, and a client must not be able to name one: a view
whose Endpoint binds fewer Policies than the Tenant holds would otherwise
be a way around a prohibition the plain root applies (platform T-3702).

`Subject.headers` therefore carries one reserved pair, `("@view", id)`,
when the request reached the broker through `/x/views/{id}/` (any view
surface; `antares-ops` runs the inner request inside that scope).
`Subject::view()` returns it. `@` is no header-name character (RFC 9110
`tchar`), so no request, inner or outer, can carry the pair: the broker
adds it from the in-process call alone, after the headers a deployment
named. The pair is persisted with the rest of the subject (a
Subscription, an EntityMap, a Snapshot, a registration), so a
notification is decided for the view the Subscription was created
through, and two subjects that differ only in their view are different
creators. It is never served: the stored subject stays a `__` member.

An engine that does not bind per view ignores the pair, as it ignores
every header it was not told about; `AllowAll` is unchanged.

## Consequences

- ADR-0014's phase table gains no row: the seam is a named user of two
  phases that already exist, with fail-closed policy per rule 2.
- It does take a stated exception to ADR-0014's rule 1. `pre_notify`
  fires once per matched subscription, not once per drained batch,
  because the decision is *about* the subscription: the subscriber is the
  subject, and one verdict for a batch of subscriptions with different
  subscribers has no meaning. The marshalling cost that rule 1 protects
  against is bounded by the delivery it precedes — the seam is crossed
  once per notification the broker was going to serialise and send over
  the network anyway.
- The gateway keeps everything it had. Authentication, token validation,
  rate limiting and quotas do not move; the broker still takes no
  authorization decision of its own, it asks the engine and obeys.
  `docs/policies.md` stays the reference for what an engine outside the
  tree looks like.
- Deployments that run the built-in engine — the default, the image,
  every gate — attach no engine at all: `AppState::policy` is `None`, and
  a gate with nothing to ask returns the empty `Filter` without building
  the operation, copying the subject headers, boxing a future or arming
  the timeout timer. One branch per request and one per notification.
  Selecting `allow-all` by name is the same thing: the broker skips the
  install, `/q/health` still reports `allow-all`, and every route answers
  what it answers with the engine attached by hand
  (`the_built_in_answer_is_the_bypassed_answer`).

## Confirmation

- Every Notification body passes `pre_notify` once, in
  `notify::deliver_as`, its only caller outside tests (cg: `MATCH
  (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::policy::pre_notify'})
  WHERE NOT c.is_test RETURN c.path`). The 5.8.6 inbound path
  (`distsub::remote_notify`) delivers through it under the ORIGINAL
  Subscription's stored subject
  (`distsub::tests::a_remote_notification_is_narrowed_for_the_original_subscriber`).
  Two sends stay outside the seam on purpose, because neither carries an
  Entity: the `urn:antares:distsub:` endpoint, which hands Context Source
  Notification data to the broker's own handler and reaches no receiver,
  and the 5.16.6 SnapshotNotification (`snapshots::send_notification`),
  whose body is the snapshot's status and the per-query details of the
  creator's own gated request, sent to the endpoint that creator named
  (operation matrix section 8, items 11 and 13).
- `run_policy_contract` (behind `test-kit`, the contract an addon
  engine's own tests call): a `Filter` never widens — the engine's answer
  is run through the projection and asserted a subset of the unfiltered
  one; the whole-tenant operations answer `Deny`; the timeout path
  answers `Deny`.
- A counting engine, reachable from no socket, walks every route of
  `router()` and asserts each operation hits the gate exactly once. A
  handler that skips the gate is a red test, not a review finding.
- `dev/check-no-addon.sh` is the release gate: `cargo tree -p
  antares-broker -e normal --locked` with the default features may name no
  crate under `examples/` and no engine or façade crate. It runs in
  `workspace.yml` (so on every `ci` and `full`), immediately before the
  release binary build in `full.yml`, and before the image is built in
  `build-image.yml`; it refuses a suspiciously small tree and self-tests
  by proving its pattern DOES match a build carrying the addon, so it can
  never be a grep that has never matched anything. `k8s-smoke.yml` asserts
  the other half from the deployed image: `/q/health` reports the policy
  engine `allow-all` and no surface beyond `admin`.
  `policy_shelf_tests` in `antares-broker` asserts the shelf of a build
  without the addon feature is exactly `allow-all`, that no selection is
  that engine, and that an unknown name is fatal rather than wide open.
- The seam is proven from outside `crates/`, which is what makes it a seam:
  `examples/plugin-example/src/policy.rs` is an engine written the way a
  deployment's would be — static per-tenant rules from a JSON document —
  and `examples/plugin-example/tests/policy.rs` runs `run_policy_contract`
  against it and then answers real requests through `router()`: a denied
  Entity type is 403 `urn:antares:error:AccessDenied` while the next type
  in the same tenant is 201, and the engine's `q` and omit list narrow a
  query the store ran. `AntaresSpecificTests/policy_engine.robot` repeats
  it against a running broker built with that engine, adding the
  notification projection and the Context Source forward; `examples.yml`
  runs the conformance suite on `allow-all` and that folder on the addon,
  in the same job.
- `tests/policy_subject_on_the_wire_p6.rs`: a registration that asks for
  the subject header by the 4.3.6.5 copy value does not get it while an
  ordinary header it asks for still arrives; a 5.8.1.4 forwarded
  subscription copy carries no internal member and no subject; a change
  event carries none; no response echoes one back; and the subscription
  mirror keeps it, on purpose, with the reason in the test.
- `/q/health` names the active engine and its timeout.
- `tests/policy_maps_snapshots_5_14_5_16.rs`: a map built for one subject is
  not reused by another and is answered like an expired one; retrieve and
  delete of another subject's map are `ResourceNotFound` while the 5.5.14
  expiry update stays open; no served map or snapshot carries an internal
  member; a narrowing answer to 5.16.1, 5.16.2, 5.16.7 or 5.6.21 is a 403
  while an empty filter is an allow.
- `tests/policy_notify_5_8_6.rs`: a dropped notification is no attempt at
  all (no POST, no `timesSent`, no `lastNotification`, no `lastFailure`)
  while a delivered one still counts; the projection removes what the engine
  named and nothing else, named as a short name or as an IRI; a `Filter`
  carrying a `q` is dropped; the engine is asked about the subscriber and
  not about whoever's write triggered the notification; and the stored
  subject appears in no representation and can be set by no client.
- `tests/policy_filter_5_7.rs`: what the raw read answers, what the
  narrowed one answers, and what is not in it — over the entity query, the
  retrieve, the attribute retrieve, the batch query, the temporal query and
  retrieve, the EntityMap candidate set and a merged federated result; the
  marker header on each read it narrowed.
