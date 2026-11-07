# ADR-0025 — Internal context sources: a registration that names this broker is served in process

Date: 2026-10-10. Status: accepted, rule 8 amended by ADR-0027 (section 9). Decided by worker-2 under the owner
order of 2026-10-10 ("a design decision is yours: pick the recommended
option"); the owner may override any rule below. Builds on ADR-0011 (the
tenant-qualified Via alias), ADR-0016 (bindings behind the sink registry)
and ADR-0020 (the policy seam); supersedes none. Task T-3439.

## Context

Cross-tenant federation inside one broker is a deployment shape Antares
already serves (ADR-0011): tenant A holds a Context Source Registration
whose `endpoint` is this broker and whose `tenant` member names tenant B.
Every such forward leaves the process today. `federation::forward`
(`crates/antares-api/src/federation.rs:858`) builds a `reqwest` request,
passes the egress guard, opens a socket to the broker's own address, and
the request comes back in through the listener, the router and tenant B's
handler. The joinedcontext platform wants one binary to serve Context
Spaces that read each other this way (`tasks/GOAL-antares-surfaces.md`,
point 2), and the socket hop costs a connection slot, a second JSON
serialisation, the egress check and the 8 s forward ceiling for data that
sits in the same store.

What the spec fixes and an in-process path must keep:

- TS 104 175 7.8.1: "When contacting the respective Context Sources, the
  Tenant information from the Context Source Registration has to be used.
  If no Tenant information is present in the Context Source Registration,
  no Tenant information is to be used and thus the default Tenant is
  targeted on the registered Context Source."
- TS 104 175 9.7: "a specific field listing all previously encountered
  Context Sources (e.g. a Via header ...) shall be passed as part of the
  request and this field can be used to exclude duplicated sources from
  matching as Context Source Registration. If the requested endpoint is
  also listed as previously encountered when a Context Source receives a
  call to that endpoint, this is an indication that a new loop has been
  detected. A suitable error shall be returned, indicating that the loop
  has been detected. In the case of multi-tenancy (see clause 7.8) each
  Tenant found within each registered Context Source shall be considered
  separately."
- TS 104 176 6.3.5: "508 Loop Detected - if the single registered source
  and tenant is registered to redirect back on to the Context Broker".

The code map (cg graph built at `4c3610cd5b`; rerun with `cg q -e`):

```cypher
// every caller of the one forward function: the transport choke point
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::federation::forward'})
RETURN DISTINCT c.path;
// -> batch::forward_one, batch::forward_batch, batch::batch_delete,
//    federation::{fed_retrieve, fed_query, fed_retrieve_temporal,
//    fed_query_temporal, fed_entity_maps, forward_part}, distsub::forward_sub
// every reader of the registration matcher
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {name:'matching_regs'}) WHERE NOT c.is_test
RETURN DISTINCT c.path, c.file;
// -> federation::{fed_query, fed_retrieve, fed_query_temporal,
//    fed_retrieve_temporal, write_regs, fed_entity_maps},
//    entity_map::build_retrieve_map, entities::retrieve_entity_inner
// the in-process router entry the façades already use
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::state::AppState::call'})
RETURN DISTINCT c.path, c.file;
// -> examples/plugin-example surface::things, tests/facade_seam_f0.rs
```

So every distributed operation reaches the network through
`federation::forward` (ten callers), every candidate set comes from
`federation::matching_regs` (`federation.rs:524`, eight callers), the Via
pseudonym from `federation::alias_for` (`federation.rs:210`), the loop rule
from `federation::via_loop` / `handle_via_loop` (`federation.rs:265`,
`:276`), and the merge plus the ADR-0020 post-merge filter run in the
callers on the `(status, body, warnings)` triple `forward` returns. And the
broker already owns an in-process entry into its own router:
`AppState::call` (`crates/antares-api/src/state.rs:535`), with a per-task
depth bound (`bounds::MAX_IN_PROCESS_CALL_DEPTH = 8`,
`crates/antares-api/src/bounds.rs:107`).

## Decision

An internal context source is **a transport inside `federation::forward`,
nothing else**. Matching, mode handling, operation lists, Via, loop
detection, merge and policy stay on the one code path they are on today;
only the step that turns a built request into `(status, body, warnings)`
changes.

### 1. "This broker" is configured, never guessed

`ANTARES_SELF_URLS`: a comma-separated list of absolute `http`/`https`
base URLs under which this deployment is reachable (the in-cluster service
URL, the public ingress URL with its path prefix). Validated at start:
absolute, no userinfo, no query, no fragment; scheme and host lowercased,
the default port made explicit, a trailing `/` removed. An invalid entry
stops the broker (a misread self URL would send tenant data to the wrong
place, so it is not a warning).

A registration endpoint is internal when, normalised the same way, it
equals a self URL or starts with a self URL followed by `/`. An endpoint
carrying userinfo is never internal (it is going somewhere that wants
credentials). No DNS lookup, no comparison with the pod IP, no use of
`ANTARES_PUBLIC_URL` or `host_alias` as an implicit self URL: the list is
empty by default, and an empty list means every forward is HTTP.

Decided (worker-2, owner may override): explicit list, empty by default,
because a derived self URL (`http://{host_alias}`) names some other
broker in a deployment that reuses the alias, and a DNS answer is input
an attacker who controls a resolver writes. Options: explicit list;
`ANTARES_PUBLIC_URL` as an implicit member; resolve the endpoint host and
compare with local addresses.

### 2. The request is the HTTP request, delivered without a socket

`forward` builds the request exactly as it does today (method, path,
query, `Via` from `outbound_via(headers, &alias_for(..))`, `Link` or
inline `@context`, `NGSILD-Tenant` from the registration only, the
4.3.6.5 `contextSourceInfo` headers minus the subject headers, the
4.3.6.6 content-type rules, the `local=true` of a `localOnly`
registration). Only the last step branches:

- **HTTP** (today): `st.fed_http` sends it, after `egress.check_url`, the
  breaker and the 5.2.34 cooldown.
- **Internal**: the URL's self-URL prefix is cut off (the router serves
  `/ngsi-ld/v1/...` without the ingress prefix) and the request goes to
  the router through `AppState::call`. The 5.2.34 cooldown and the
  per-registration timeout (`reg.timeout_ms`, capped at the same 8 s
  ceiling) apply unchanged; `bounds::FED_INFLIGHT` is still acquired, so
  the fan-out bound is the same. The egress guard and the host:port
  breaker are skipped, as ADR-0016 rule 5 skips them for a sink with
  `network() == false`: no socket is opened, and the breaker models a
  remote peer's latency, which an in-process call does not have.
- The response is read through the same cap as `read_body_capped`
  (`federation.rs:735`), its `NGSILD-Warning` values through the same
  `MAX_PEER_WARNINGS` cut, and the triple returned is the HTTP triple.
  A timeout is 504, as for HTTP; there is no 503 class (no connection to
  refuse).

`AppState::call` copies `NGSILD-Tenant`, `NGSILD-Snapshot` and `Link`
from its `caller` argument when the request lacks them
(`state.rs:604`). A forward must not inherit any of them: a registration
without a `tenant` targets the default tenant (7.8.1), and a snapshot
read is never forwarded. The internal transport therefore passes a
caller map that holds the subject headers only (rule 4); a unit test
holds that a registration without `tenant` lands in the default tenant
when the triggering request named tenant A.

### 3. Loop detection and Via are the HTTP ones, because they are the same code

The inner request carries the `Via` the HTTP request would carry, with
`alias_for(host_alias, A)` appended. Tenant B's handler reads it with
`via_tokens`, `matching_regs` drops every registration whose
`contextSourceAlias` is already in the chain (ADR-0011 rule 4), and
`handle_via_loop` answers 508 only in 6.3.5's case (a single
`exclusive`/`redirect` source back to the same source and tenant). The
alias is `{alias}~{tenant}` for a non-default tenant, so an A → B → A
chain is one hop per tenant pair and a genuine A → A cycle is a loop,
exactly as over HTTP. `MAX_VIA_HOPS = 32` (`federation.rs:249`) holds the
chain length.

In-process depth has its own ceiling, `MAX_IN_PROCESS_CALL_DEPTH = 8`,
held per task by `AppState::call`. A forward that would exceed it is not
made, and it is answered the way 6.3.5 answers a loop: code 199 ("No
response was received from the registration endpoint within the specified
timeout period or a registration loop has been detected") on the merged
answer, or 508 when that registration is the single `exclusive` or
`redirect` source (`handle_via_loop`'s case). It never falls back to
HTTP: an HTTP hop to a self URL would arrive without the subject and
without the grant check of rule 5.

### 4. Tenant B's policy decides, about the caller who asked

ADR-0020: "The subject never travels" — the subject headers are stripped
from every forwarded request. That rule is about leaving the process. An
internal forward does not leave it, so it carries the subject: the
caller map handed to `AppState::call` holds the
`ANTARES_POLICY_SUBJECT_HEADERS` values of the triggering request, and
`policy::subject_of` (`crates/antares-api/src/policy.rs:490`) in tenant
B's handler builds `Subject { tenant: B, headers }` from them. Tenant B's
engine therefore decides about the real caller, under tenant B, exactly
as it would for a direct request to tenant B.

A refusal in B is a 403 to the forward; the merge reports it as 6.3.5's
code 299 ("An error response (such as 403 - Forbidden) was received from
the registration endpoint") without B's `detail` (threat C10) and serves
tenant A's own data. Over HTTP the same registration reaches B with no
subject at all, which an engine that fails closed refuses and `AllowAll`
serves.

The subject headers of the forward are always overwritten from the
caller, never taken from the registration: `AppState::call` keeps a value
the inner request already carries (`state.rs:604-622`), so the transport
removes every subject header from the built request before the call, and
a `contextSourceInfo` key naming `NGSILD-Tenant` is refused at 12.2.2
with 400 `BadRequestData`, as TS 104 175 9.5.1 states: "As Tenant
information, if applicable, is directly specified in the
CSourceRegistration, it shall not be part of contextSourceInfo" (threat
C3).

Decided (worker-2, owner may override): the subject travels in process,
because the target tenant's engine has to decide about someone, and the
caller is the only honest answer; deciding about nobody either refuses a
legitimate grant or serves an anonymous one. Options: carry the subject
in process; strip it as over HTTP.

### 5. The target tenant consents: an export grant

The engine alone is not enough. Under `AllowAll` nothing judges the
forward, and in the platform the gateway is what keeps a client out of
other tenants (it refuses Context Source registration through an
Endpoint, `docs/analysis/gateway-to-broker-map.md` S16); in process there
is no gateway between A and B. A registration is tenant A's data (12.2.2),
so the broker reading B on A's behalf is a confused deputy unless B has
agreed (`docs/analysis/surfaces-threat-model.md`, C1).

This ADR adopts the threat model's decision C1 and C6 to C8: an
internal forward into tenant B needs an **export grant** held by B.

```
ExportGrant {
    target:  B (the tenant that holds the grant)
    source:  A (the tenant whose registrations may read B)
    view:    Option<view id>   // ADR-0023 view of B narrowing what B exports
    access:  read | read-write // read unless it says read-write (C6)
}
```

- A grant is an internal doc kind under the reserved tenant (ADR-0012),
  written and removed through `/q` (the operator surface; ADR-0020 leaves
  `/q` to the deployment to protect), never by a tenant-A request.
- It is checked when a registration with an internal endpoint is created
  or updated (12.2.2, 12.2.4: no grant is 403 `AccessDenied`, before
  anything is stored) and again on every forward and every csource
  notification, so a revoked grant stops data at once (C7). A forward
  without a grant is not made and is reported as 299.
- A write forward (create, update, append, merge, delete, the batch
  forms) needs `read-write`; with `read` it is refused like an engine
  refusal (C6).
- The grant's view narrows on top of B's engine:
  `effective = grant view ∧ B's engine` (ADR-0023), and B's view rules
  (pre-image, post-image, allowlist) judge a forwarded write.
- An internal tenant (`snap-<uuid>`, the reserved tenant) is never a
  target; only a client-addressable tenant can hold a grant (C8).
- B's log line for a forwarded request names the registration id and the
  source tenant, never the subject (C9).

Decided (worker-2, owner may override, carried over from T-3447 C1): the
grant is held by the target, because only B can consent to B's data
leaving B. Options: grant held by the target (chosen); trust the
registrant (today's HTTP behaviour); admin-only registrations.

Views (ADR-0023) on the source side compose without a new rule: a view
pins tenant A, and the merged answer passes the view's predicate and
allowlist after the merge, as every federated answer does (ADR-0020's
post-merge filter).

### 6. Modes and operation lists: unchanged by construction

`inclusive`, `exclusive`, `redirect` and `auxiliary` (9.3.2, 9.3.3), the
`operations` list (`FedReg::supports`, 4.3.6.1), `localOnly`, the
`contextSourceInfo` keys, Entity Maps (9.6) and precedence (9.3.4) are
decided before `forward` is called and do not know the transport. No
branch on "internal" exists outside `forward`; a `cg` query in CI holds
that (Confirmation).

### 7. Distributed subscriptions and their notifications

`distsub::forward_sub` sends the 5.8.1.4 subscription copy through
`forward`, so the copy is created in tenant B in process, under the
grant check of rule 5. Its notification endpoint is `ANTARES_PUBLIC_URL`
plus the ADR-0019 receiver path. When that URL is a self URL, the HTTP
sink delivers the notification through the same transport, after the
grant is read again (C7); the 5.8.6 retry and dead-letter policy
(ADR-0015) is unchanged, since a delivery attempt is still one call that
answers a status. Notification delivery never carries a subject (it is
broker-initiated, ADR-0020), in process or not.

### 8. Crate `antares-icsr`, feature `internal-csr`

GOAL point 6 puts every surface in its own crate. `antares-api` gains one
core trait and no implementation:

```rust
/// A transport that can serve some forwards without a socket.
pub trait ForwardTransport: Send + Sync {
    /// `Some(answer)` when this transport serves `url`; `None` sends the
    /// forward over HTTP as before.
    fn forward<'a>(&'a self, st: &'a AppState, req: BuiltForward<'a>)
        -> Option<ForwardFuture<'a>>;
}
```

`AppState` holds `Option<Arc<dyn ForwardTransport>>`, `None` by default,
set by `with_forward_transport` at the composition root exactly as
`with_sink` adds a binding (ADR-0016). `federation::forward` asks it after
building the request and before the egress check; `None` or a `None`
answer is today's HTTP path. `BuiltForward` is the request `forward`
built (method, URL, query, headers, body, the `FedReg`, the tenant and
the caller's subject headers), so the transport cannot change what is
sent, only how.

The crate `antares-icsr` implements it: self-URL parsing and matching
(rule 1), the export grant doc kind, its `/q` routes and its checks
(rule 5), the call through `AppState::call` with the subject-only caller
map (rules 2 and 4), the depth rule (rule 3). `antares-broker` depends on
it behind the cargo feature `internal-csr`, off by default and in no
default feature set, and registers it when `ANTARES_SELF_URLS` is
non-empty. Without the feature `ANTARES_SELF_URLS` is not read (a set
value logs one warning at start naming the missing feature). With the
feature and an empty list nothing is registered, and every forward is
HTTP.

Decided (worker-2, owner may override): a trait in `antares-api` and the
logic in `antares-icsr`, because a module inside `antares-api` would put
grant storage and self-URL parsing into the vanilla crate behind `cfg`,
which the vanilla-proof gate of GOAL point 1 would have to see through;
options: crate behind a core trait (chosen), `cfg` module in
`antares-api`.

## Consequences

- One binary serves Context Spaces that federate with each other without
  a socket per hop; HTTP federation with other brokers is unchanged.
- A deployment lists its self URLs and writes one grant per
  (target, source) pair it allows. A missing self URL costs only the
  socket (that forward goes over HTTP as today); a missing grant stops
  the internal forward with a 299 warning.
- Under a subject-aware engine an internal forward can return data an
  HTTP forward to the same broker would not (the caller holds a grant in
  B's engine), and it can refuse what `AllowAll` behind an HTTP hop
  served. Both are the target tenant deciding; the difference is
  documented, not hidden.
- The egress guard no longer sees self-forwards. A self URL is the
  deployment's own address, which ADR-0010 already allows as private.
- `antares-api` gains `ForwardTransport` and one optional `AppState`
  member, and `12.2.2` refuses a `contextSourceInfo` key naming
  `NGSILD-Tenant` in every build (a 9.5.1 conformance fix, not a feature).

## Equivalence test plan

Three layers, each a named test filed by T-3445:

1. **Same request.** `icsr_builds_the_http_request`
   (`antares-icsr` unit test over `BuiltForward`): for one
   `FedReg` per mode, with and without `tenant`, `localOnly`,
   `jsonldContext`, `contentType`, `urn:ngsi-ld:request` copies, an inline
   `@context` and an inbound Via chain, the `http::Request` handed to
   `AppState::call` equals the `reqwest::Request` the HTTP branch builds:
   method, path after the self prefix, query, every header except `Host`,
   `Content-Length` and the subject headers, and body bytes.
2. **Same answer.** `crates/antares-icsr/tests/icsr_equivalence.rs`
   runs every scenario below twice against one broker: once with tenant B
   reached over a bound loopback listener (feature on, `ANTARES_SELF_URLS`
   empty, so HTTP) and once in process (the listener URL listed as a self
   URL, and a read-write grant in B for A). Status, body (after `@context` normalisation), `NGSILD-Warning`
   values and the state left in both tenants must be equal. The scenarios
   are the federated ETSI test purposes, rebuilt as tenant-to-tenant
   cases because the suite's own Context Source is a mock HTTP server
   (`ngsi-ld-test-suite/resources/variables.py`, `context_source_host`)
   that an in-process transport cannot replace:

   | ETSI TPs (`ngsi-ld-test-suite/TP/NGSI-LD/DistributedOperations/`) | operation |
   |---|---|
   | D001_01_exc, D001_01_inc, D001_01_red, D001_02_exc, D001_02_inc, D001_03_01_inc, D001_03_02_inc, D001_03_03_inc, D001_04_inc, 5614_01, 436_02, 436_06 | Create Entity |
   | D002_01_exc, D002_01_inc, D002_01_red, D002_02_01_inc, D002_02_02_inc, D002_02_exc, D002_03_inc | Delete Entity |
   | D003_01_exc, D003_01_inc, D003_01_red, D003_02_exc, D003_02_inc, D003_02_red | Append Entity Attributes |
   | D004_01_exc, D004_01_inc, D004_01_red | Update Entity Attributes |
   | D005_01_exc, D005_01_inc, D005_01_red | Partial Attribute Update |
   | D006_01_exc, D006_01_inc, D006_01_red, D006_02_exc, D006_02_inc, D006_02_red | Delete Entity Attribute |
   | D007_01_exc, D007_01_inc, D007_01_red | Replace Entity |
   | D008_01_exc, D008_01_inc, D008_01_red | Merge Entity |
   | D009_01_exc, D009_01_inc, D009_01_red | Replace Entity Attribute |
   | D010_01_aux, D010_01_exc, D010_01_inc, D010_01_red, D010_02_exc, D010_02_inc, D010_02_red, D010_03_inc, 436_01, 436_04, 436_05, 6317_01 | Retrieve Entity |
   | D011_01_01_inc .. D011_01_05_inc, D011_01_aux, D011_01_exc, D011_01_red, D011_02_exc, D011_02_inc, D011_02_red, D011_03_inc, D011_04_inc, D011_05_inc, 5724_01, 5724_02, 6317_02 | Query Entities |
   | D012_01_exc, D012_01_inc, D012_01_red | Create Batch |
   | D013_01_exc, D013_01_inc, D013_01_red, D013_02_exc, D013_02_inc, D013_02_red | Upsert Batch |
   | D014_01_exc, D014_01_inc, D014_01_red, D014_02_exc, D014_02_inc, D014_02_red | Update Batch |
   | D015_01_exc, D015_01_inc, D015_01_red (and the `DeleteBatchEntities/D015_01_inc` duplicate id) | Delete Batch |
   | D016_01_exc, D016_01_inc, D016_01_red | Merge Batch |
   | D017_01_exc, D017_01_inc, D017_01_red | Purge Entities |
   | D018_01, D018_02, D018_03 | Loop detection (D018_01 keeps its recorded suite defect, ADR-0011) |
   | 5814_01, 5814_02 | Distributed subscription and its notification (rule 7) |

   The TP ids come from `find ngsi-ld-test-suite/TP/NGSI-LD/DistributedOperations -name '*.robot'`
   at `4c3610cd5b`; a CI step diffs that list against the test's scenario
   table, so a TP added upstream fails the job until it has a case.
3. **Same suite result.** The ETSI DistributedOperations directory runs
   against the `full` binary with `ANTARES_SELF_URLS` set to the broker's
   own URL (the mock stays an HTTP source, since it is not a self URL);
   the per-TP verdicts must equal the vanilla run. This proves the feature
   leaves foreign HTTP federation alone.

## Security

Threat ids are those of `docs/analysis/surfaces-threat-model.md` section 8.

| threat | rule | named test |
|---|---|---|
| C1: a registration in A reads tenant B without B's consent | 5 | `crates/antares-icsr/tests/icsr_grant.rs::icsr_a_registration_into_a_tenant_that_granted_nothing_is_refused` (403 at 12.2.2, and a forward of a row written around the API is not made: 299) |
| C1, C2: cross-tenant read without a grant in B's engine | 4 | `icsr_gate.rs::icsr_cross_tenant_read_without_an_engine_grant_is_refused`: grant present, a test engine allows subject `alice` in tenant A only; `alice` queries A and gets A's Entities, no B Entity, and a 299 `NGSILD-Warning` |
| C2: the forward is judged under someone else | 4 | `icsr_gate.rs::icsr_the_target_engine_is_asked_about_the_caller` |
| C3: the tenant or subject comes from registration data | 2, 4 | `icsr_gate.rs::icsr_the_subject_is_the_callers_and_never_comes_from_the_registration`; `csource_tenant_in_csi_9_5_1.rs::a_tenant_key_in_context_source_info_is_refused` |
| the forward inherits the caller's tenant or snapshot | 2 | `icsr_gate.rs::icsr_without_tenant_targets_the_default_tenant`, `icsr_gate.rs::icsr_never_forwards_a_snapshot` |
| a spoofed self URL (userinfo, other port, prefix trick `https://self.example.evil`) | 1 | `antares-icsr` unit test `self_url_matching_is_exact` (equal or `self + "/"` only; userinfo never internal; no DNS) |
| C4: an A → B → A chain, stack exhaustion | 3 | `icsr_loops.rs::icsr_a_two_tenant_cycle_terminates_like_http`, `icsr_loops.rs::icsr_past_the_depth_bound_no_forward_is_made_and_none_goes_over_http` |
| C5: fan-out amplification | 2 | `icsr_loops.rs::icsr_in_process_forwards_count_against_the_federation_bounds` |
| C6: a write forwarded on a read grant | 5 | `icsr_grant.rs::icsr_a_write_forward_needs_a_read_write_grant` |
| C7: data after revocation | 5, 7 | `icsr_grant.rs::icsr_a_revoked_grant_stops_forwards`, `icsr_notify.rs::icsr_a_revoked_grant_stops_notifications` |
| C8: an internal tenant as a target | 5 | `icsr_grant.rs::icsr_an_internal_tenant_is_not_a_target` |
| C9: B cannot tell what A read | 5 | `icsr_gate.rs::icsr_a_forward_is_logged_in_the_target_with_the_registration_id` |
| C10: B's error text reaches A | 4 | `icsr_gate.rs::icsr_a_target_error_reaches_the_source_without_its_detail` |
| the subject leaves the process | 4, 7 | `policy_subject_on_the_wire_p6.rs` keeps passing for HTTP forwards; `icsr_notify.rs::icsr_a_notification_carries_no_subject` |

## Confirmation

- The tests above, in `crates/antares-icsr/tests/` and the crate's unit
  modules, run by the feature-matrix job with `--features internal-csr`.
- CI `cg` gate (T-3447 module rules): `AppState::call` is reached from
  federation only through the transport trait; this query must return no
  row:
  `MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::state::AppState::call'}) WHERE (c.path STARTS WITH 'antares_api::federation' OR c.path STARTS WITH 'antares_api::batch' OR c.path STARTS WITH 'antares_api::distsub') AND NOT c.is_test RETURN c.path`
- The default build links no `antares_icsr::` symbol (the vanilla symbol
  gate of the surfaces architecture ADR), and the ETSI
  DistributedOperations verdicts of the default build are unchanged.
