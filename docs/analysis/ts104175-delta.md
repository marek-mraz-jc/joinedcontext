---
title: Antares against the TC DATA NGSI-LD family (TS 104 175 / 104 176 / 104 243 / 104 179 V1.1.1)
task: T-3435
---

# Antares against TS 104 175 / 104 176 / 104 243 / 104 179

Antares' conformance ledger (`docs/spec/`, 947 clauses) is keyed to GS CIM 009 V1.9.1. ETSI TC DATA replaced that document with TS 104 175 V1.1.1 (Core API), TS 104 176 V1.1.1 (HTTP binding), TS 104 243 V1.1.1 (MQTT notification binding) and TS 104 179 V1.1.1 (provenance and integrity), split one file per clause in `/workspace/etsi-specs/`. This page maps the ledger onto them, lists the rules whose text changed, checks the rules named in T-3435 against the code, and names a task for every gap found. The ICS of TS 104 187 is [ics-ts104187.md](ics-ts104187.md).

## 1. Method

- **Clause map** (`python3 dev/clause-map.py`, appendix A): every ledger clause is compared with every clause of the four documents by text, as the share of its word trigrams found in the candidate (containment, so a clause split in two still finds the larger half), plus 0.5 for an equal title unless the title is generic ("Introduction", "General", ...). Numbers are never used. A clause with fewer than 12 words of its own (a heading) is matched on its title alone. Kinds: `moved` (same title, text kept), `edited`, `weak` (0.45 to 0.6 under another title: hand-checked in 2.2), `title` (title only), `heading`, `unmatched`, and `new-only` for a new clause no ledger clause landed on.
- **Rule delta**: for each matched pair the script compares the normative sentences (those with shall, should, may, must) as normalised words and counts the ones only the old clause has (`rules removed`) and only the new one has (`rules added`). Counts only: ETSI text is not copied beyond the short quotes in section 3.
- **Code**: every code location in section 3 comes from the code graph (`cg build --reuse` at `bd5615b`), with the Cypher printed beside it so a reviewer can rerun it.
- **Compliance claims** name the Rust test or the ETSI test-suite id that proves them, or say "none".

## 2. Clause map

| Kind | Ledger clauses |
|---|---|
| moved | 128 |
| edited | 728 |
| weak | 8 |
| heading | 37 |
| unmatched | 46 |
| **total** | **947** |

367 clauses of the four new documents have no ledger clause; 43 of them carry normative sentences (2.3). By document, the ledger lands in TS 104 175 (584 clauses) and TS 104 176 (293), 2 in TS 104 243; none in TS 104 179, which is new matter (3.9).

### 2.1 Unmatched ledger clauses, placed by hand

The script finds no text for these because TS 104 175 rewrote them into another structure: the per-attribute Normalized and Concise representations became data-type definitions (5.2.6.4.x) plus the representation clauses (5.3.2.x, 5.3.3.x), and the availability representations became data types (5.2.6.10.x). The new home below is read from the titles and the text; the ledger keeps its own clause id until the ledger is rekeyed.

| Ledger clause | Title | Status | New home (TS 104 175 unless noted) |
|---|---|---|---|
| 4.5.2.2, 4.5.2.3 | Normalized / Concise Property | implemented | 5.2.6.4.3 Property; 5.3.2.2, 5.3.2.3 |
| 4.5.3.2, 4.5.3.3 | Normalized / Concise Relationship | implemented | 5.2.6.4.4 Relationship; 5.3.2.2, 5.3.2.3 |
| 4.7.3 | Concise GeoProperty | implemented | 5.2.6.4.5 GeoProperty; 5.3.2.3 |
| 4.5.18.1–4.5.18.3 | LanguageProperty representations | implemented | 5.2.6.4.6 LanguageProperty |
| 4.5.20.1–4.5.20.3 | VocabProperty representations | implemented | 5.2.6.4.7 VocabProperty |
| 4.5.21.1–4.5.21.3 | ListProperty representations | implemented | 5.2.6.4.8 ListProperty |
| 4.5.22.1–4.5.22.3 | ListRelationship representations | implemented | 5.2.6.4.9 ListRelationship |
| 4.5.24.1–4.5.24.3 | JsonProperty representations | implemented | 5.2.6.4.10 JsonProperty |
| 4.5.25 | EntityMap Representation | implemented | 5.2.6.5.5 EntityMap |
| 4.5.10 | Entity Type List Representation | implemented | 5.2.6.10.5 EntityTypeList |
| 4.5.11 | Detailed Entity Type List Representation | implemented | 5.2.6.10.3 EntityType (the list's element) |
| 4.5.12 | Entity Type Information Representation | implemented | 5.2.6.10.4 EntityTypeInfo |
| 4.5.13 | Attribute List Representation | implemented | 5.2.6.10.2 AttributeList |
| 4.5.14, 4.5.15 | Detailed Attribute List / Attribute Information | implemented | 5.2.6.10.1 Attribute |
| 4.6.6, 5.5.11.1–5.5.11.5 | Same Entity more than once in an array, per batch operation | implemented | 10.3 batch operations (10.3.1–10.3.6), the rule now inside each operation's behaviour |
| 4.5.0, 4.5.17.0, 5.1, 5.9.1, 6.3.1, D.1 | Introductions and forewords | informative / implemented (5.9.1) | no normative content; 5.9.1 → 12.2.1 |
| 3.3 | Abbreviations | informative | 3.3 (list rewritten) |
| D.2–D.4 | Transformation algorithms ALG1, ALG1.1, ALG1.2 | informative | not in TS 104 175 (no clause names ALG1); the JSON-LD forms are 5.3.2 |
| F | Conventions and syntax guidelines | informative | Annex E (TS 104 175), Annex B (TS 104 176) |
| I | Change history | informative | each new document's own history annex |
| C.5.2.1 | HTTP Request (example) | informative | TS 104 176 annex A examples |

### 2.2 Weak matches, checked by hand

| Ledger clause | Script's match | Verdict |
|---|---|---|
| 1 Scope | TS 104 175 2.1 | wrong: scope is TS 104 175 clause 1 (rewritten for the split into several documents) |
| 4.20 Distributed Operation names | TS 104 175 9.2 Distributed operation names | right |
| 4.5.1 Entity Representation | TS 104 175 5.3.2.2 Entities Normalized | right, with 5.2.6.4.1 Entity |
| 4.5.17.1 Simplified GeoJSON, one Entity | TS 104 175 5.3.2.4 | partly: the GeoJSON form is 5.3.3.5.1 |
| 4.7.1 GeoJSON Geometries | TS 104 175 5.2.6.4.1 Entity | partly: the geometry rules are 5.2.6.4.5 GeoProperty |
| 5.5.11 Same Entity in an array | TS 104 175 10.3.1 | right |
| 6.3.10 Pagination behaviour | TS 104 176 6.4.7.2 General pagination | right |
| G.1.2 Property with a natural language | TS 104 175 F.2.3 | right |

### 2.3 New clauses with normative content and no ledger clause

The ones that change what a broker must do are checked in section 3; the rest are introductions, references or examples.

| New clause | What it adds | Section |
|---|---|---|
| TS 104 175 10.2.6, TS 104 176 7.6.3.2 | Set Attribute Value: `PUT /entities/{entityId}/attrs/{attrId}/value` | 3.10 |
| TS 104 175 10.4.4, TS 104 176 7.5.3.1, 7.6.3.1 | Retrieve Attribute Value: `GET/HEAD` on `…/attrs/{attrId}` and `…/attrs/{attrId}/value` | 3.10 |
| TS 104 175 7.2.3.3, 7.2.3.4 | Query terms, logical operators and query term equality | 3.1 |
| TS 104 175 7.7.4 | Depth of Linked Entity retrieval | 3.3 |
| TS 104 175 9.3.4 | Precedence of distributed operations | 3.6 |
| TS 104 175 9.4.2 | CSR matching algorithm | 3.6 |
| TS 104 175 5.4.2 | Attribute fragments | 3.10 |
| TS 104 176 6.4.7.3 | Temporal pagination | 3.2 |
| TS 104 179 4, 5.x | Provenance and integrity | 3.9 |

## 3. The rules T-3435 names, checked against the code

Each entry: what the old and new texts say (quotes kept to the deciding sentence), where Antares implements it, whether it complies with the **new** text, the test that proves it, and the task for a gap. Code paths are relative to the repository root; spec paths to `/workspace/etsi-specs/`. Compliance is read from code and the named tests; no suite was run for this page.

### 3.1 Query equality and non-existence

- Old: CIM 009 4.9 (`docs/spec/4/4.9.md`). New: TS 104 175 7.2.3.2, 7.2.3.3 and the new 7.2.3.4 "Query Term Equality" (`ts-104175-ngsi-ld-core-api/7/7.2.3.4.md`). Equal must "meet at least one of the conditions defined in clause 7.2.3.4", Unequal "none" of them.
- Changed: conditions 8 and 9 are new. A sub-attribute addressed on a Property, ListProperty or JsonProperty matches when it "Is included in one of the JSON Objects corresponding to the target value, and the latter is an array" (its example: `address[city]==Berlin` against `[{"city":"London"},…,{"city":"Berlin"}]`). Unchanged: the ABNF, non-existence ("If the evaluation of such MemberExpression does not result in a defined value, the target element shall be considered as non-existent"), datatype mismatch, temporal ordering, string comparison.
- Code: `crates/antares-ql/src/eval.rs` `compare` :399, `terminal` :274 (trailing path :315-320), `navigate` :333. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND s.file CONTAINS 'antares-ql' AND s.kind IN ['Function','Method'] AND NOT s.is_test RETURN s.path, s.file, s.line ORDER BY s.file, s.line LIMIT 120`
- Behaviour: a trailing path navigates with `Value::get`, which is `None` on an array, so the target is non-existent; `==` matches nothing, `!=` matches.
- Compliant: **partial**; conditions 1–7, 10, Unequal and non-existence yes, 8 and 9 no.
- Tests: eval.rs `compound_value_trailing_path`, `unequal_matches_on_datatype_mismatch`, `unequal_over_an_array_requires_every_element_to_differ`, `value_list_semantics`, `range_semantics`; lib.rs `existence_and_negation`; robot 019_28_*, 019_29_*. Gap: **T-3455**.

### 3.2 Temporal pagination: `firstN`, `lastN`, `offsetN`

- Old: CIM 009 has `lastN` only (6.18.3.2, `docs/spec/6/6.18.3.2.md`) and pages a truncated answer with 206 and `Content-Range` (6.3.10, `docs/spec/6/6.3.10.md`).
- New: TS 104 175 7.4.2.5 "Temporal pagination can be done in ascending order using the firstN parameter and in descending order using the lastN parameter … offsetN is used to select the respective page in the chosen order"; TS 104 176 6.4.7.3: pages are Links `rel="intervalafter"` / `"intervalbefore"`. Neither document mentions 206 or `Content-Range`. `limit`/`offset` still page Entities; `firstN`/`lastN` cap instances per Attribute.
- Code: `crates/antares-api/src/temporal.rs` `lastN` :880, `window` :413, `content_range` :526, allowlist `check_params` :1738-1777; `paging.rs:898` (`temporalQ.lastN`); `antares_store::filter::TemporalFilter::last_n` (`crates/antares-store/src/filter.rs:116`). Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND (lower(s.name) CONTAINS 'lastn' OR lower(s.name) CONTAINS 'firstn' OR lower(s.name) CONTAINS 'offsetn' OR lower(s.name) CONTAINS 'last_n' OR lower(s.name) CONTAINS 'first_n') RETURN s.path, s.kind, s.file, s.line, s.is_test LIMIT 60`
- Behaviour (T-3456): `firstN` pages oldest first, `lastN` newest first, neither oldest first; `offsetN` skips in that order and past the end returns the last instance (Table 6.4.7.3-2); `firstN` with `lastN` is 400 BadRequestData. The query string and the `temporalQ` body read all three. Each Attribute is cut to the same count (`min(N, 9)`), and the response carries `Link` `rel="intervalafter"`/`"intervalbefore"` with `offsetN` moved by one page and `type` equal to the response media type. The pg store ranks per Attribute and datasetId, ascending for `firstN`, capped at `N + offsetN + 1`. Federation forwards `firstN`/`lastN` as `N + offsetN` and never `offsetN`. Owner decision (2026-10-10): `206` + `Content-Range` stay on a page cut by the ceiling while robot 020_13/021_27 assert them.
- Compliant: yes.
- Tests: `tests/temporal_pagination_7_4_2_5.rs` (6, memory and pg), `clause_6_3_10::the_cut_is_one_count_per_attribute` and `last_n_above_the_ceiling_is_clamped_to_it` (temporal.rs), `first_n_ranks_the_oldest_first` and `last_n_caps_with_rank_per_attr_and_dataset_never_row_number` (antares-sql), robot 020_05_*, 020_13_*, 021_27_*.

### 3.3 `join`, `joinLevel`, `containedBy`

- Old: CIM 009 4.5.23.1, Table 6.4.3.2-1, default 1 (5.2.14.1, 5.2.23). New: TS 104 175 5.2.6.5.1 (unchanged definitions, default 1) and the new 7.7.4 "Depth of retrieval": "whenever a Linked Entity has already been previously encountered during graph traversal at a shallower depth, its Linked Entities shall not undergo further expansion for a second time."
- Code: `crates/antares-api/src/entities.rs` `parse_join` :496, `contained_by` :524; `crates/antares-api/src/repr.rs` `inline_join_walk` :801, `collect_flat_walk` :989; `bounds.rs:109` `MAX_JOIN_LEVEL = 10`. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND (lower(s.name) CONTAINS 'join' OR lower(s.name) CONTAINS 'contained_by' OR lower(s.name) CONTAINS 'linked') RETURN s.path, s.kind, s.file, s.line, s.is_test ORDER BY s.is_test, s.file LIMIT 120`
- Behaviour: default 1, above 10 is 400 (an implementation bound); both walks are depth-first over one `seen` set, so an entity met first on a deep branch is embedded unexpanded when met again at depth 1 (repr.rs:847-855).
- Compliant: **partial**; defaults, domains, `containedBy` and loop prevention yes, the shallowest-depth rule no.
- Tests: entities.rs `flat_join_never_repeats_the_root_entity`, `cyclic_inline_join_stops_instead_of_cascading`, `wide_inline_join_stops_at_the_lookup_budget`, `contained_by_ids_are_not_retrieved_again`; robot 018_11–018_14, 019_13–019_24. Gap: **T-3457**.

### 3.4 Core @context: URL, version, type- and property-scoped terms

- Old: CIM 009 4.4 names `https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld`; its Annex B has 206 top-level terms and one scoped context (`ngsildproof`).
- New: TS 104 175 5.3.2.1 keeps the same URL (no other version string occurs in the document) and "Whereas the core NGSI-LD @context contains JSON-LD Scoped Contexts, the user @context shall not contain JSON-LD Scoped Contexts"; Annex B has 149 top-level terms and 23 type- or property-scoped contexts; 61 old top-level terms are scoped only (`format`, `data`, `status`, `attrs`, `join`, `joinLevel`, `timerel`, `orderBy`, `geometry`, `accept`, …); 5.2.5 lets a user @context override a scoped core term ("this usage is still discouraged"). Counts from parsing both Annex B bodies and the pinned file.
- Code: `crates/antares-jsonld/src/loader.rs` `CORE_CONTEXT` :56, the pinned table :50-53 (`contexts/core-v1.9.jsonld`); `context.rs` `merge_core_object` :72, scoped contexts in a core document dropped :138-141, in a user @context 400 :141-146. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND s.file CONTAINS 'antares-jsonld' AND (s.name IN ['CORE_CONTEXT','core_context','pinned','merge_core_object','expand_key'] OR lower(s.name) CONTAINS 'scoped' OR lower(s.name) CONTAINS 'core') RETURN s.path, s.kind, s.file, s.line, s.is_test ORDER BY s.is_test, s.file, s.line`
- Compliant: yes since T-3458 (owner decision 2026-10-10): the v1.9 URL resolves to the TS 104 175 Annex B document, its scoped contexts apply in expansion and compaction (`Context::within`), a user @context may redefine a scoped term, and scoped contexts in a user @context stay 400.
- Tests: loader.rs `the_implicit_core_context_is_the_one_v1_9_1_names` (asserts the old top-level `orderBy`, `collation`, `aggrMethods`), `core_terms_are_protected_from_user_redefinition`; context.rs `clause_5_5_7_scoped_contexts_rejected_and_fqn_fallback`; robot 557_01, 451_01. Gap: **T-3458** (owner decision: which document the v1.9 URL resolves to).

### 3.5 Loop detection

- Old: CIM 009 4.3.6.4, 6.3.17 ("508 Loop Detected - if the single registered source and tenant is registered to redirect back on to the Context Broker"), 6.3.18.
- New: TS 104 175 9.7 adds "If the requested endpoint is also listed as previously encountered when a Context Source receives a call to that endpoint, this is an indication that a new loop has been detected. A suitable error shall be returned, indicating that the loop has been detected." TS 104 176 6.3.5 keeps 508 scoped to a single exclusive or redirect source; 6.4.11 (Via) unchanged. Table 8.3.2-1 has no LoopDetected type.
- Code: `crates/antares-api/src/federation.rs` `inbound_via` :165, `via_loop` :263, `handle_via_loop` :269, `loop_508` :2409, `matching_regs` :515. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND (lower(s.name) CONTAINS 'via' OR lower(s.name) CONTAINS 'loop') AND s.kind IN ['Function','Method','StaticMethod','EnumMember','Constant'] RETURN s.path, s.file, s.line, s.is_test LIMIT 80`
- Behaviour: 508 for more than 32 hops or one exclusive/redirect registration; otherwise the self-loop is served locally (GET with a 199 warning).
- Compliant: yes since T-3459 (owner decision 2026-10-10): a write whose `Via` names this broker is 508 whatever its registrations; a read is served locally with a `199` warning. Raise #1 (`docs/upstream/etsi-raises.md`) now reports the 6.3.17 / 9.7 disagreement.
- Tests: tests/federation_loop.rs `single_redirect_source_looping_back_is_508`, `clause_9_7_self_in_via_with_inclusive_registration_is_a_detected_loop`, `clause_9_7_self_in_via_without_a_registration_is_a_loop_on_writes`, `suppressed_loop_forward_warns_199_on_get`; federation.rs `a_write_looping_back_is_508_whatever_its_registrations`; robot D018_01–D018_03, 436_04.

### 3.6 Context Source Registration matching

- Old: CIM 009 5.12, 4.3.6.1–4.3.6.3. New: TS 104 175 9.4.1, 9.4.2 "Matching algorithm", 9.3.1/9.3.3/9.3.4, 12.3.3.4, Table 5.2.6.6.5-1.
- Changed: "the specified Attribute identifiers shall match the attributeNames (or the combination of propertyNames and relationshipNames) in the RegistrationInfo"; Table 5.2.6.6.5-1 adds `attributeNames` and deprecates `propertyNames`/`relationshipNames`, "only used if attributeNames is not present". The id, pattern, datasetId, Via and EntityMap rules are unchanged in substance; 9.3.4 states the precedence of distributed operations that 4.3.6 held.
- Code: `crates/antares-api/src/registry.rs` `entity_info_matches` :144, `attrs_match_info` :194, `csr_matches` :262; `federation.rs` `reg_candidate` :448, `matching_regs` :515, `covers_attr` :101; `csource.rs` `norm_registration_info` :33; `crates/antares-sql/src/store/pg/doc.rs` index rows ~:151. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND s.name IN ['matching_infos','entity_info_matches','attrs_match_info','matching_regs','type_matches','can_match_id','covers_attr','active','registration_matches','csr_matches'] RETURN s.path, s.file, s.line, s.end_line`
- Behaviour: entity and id matching implement all five conditions; attributes are matched on `attributeNames`, else on `propertyNames`/`relationshipNames` together (`antares_model::registration_attribute_names`, read by registry.rs, federation.rs, csource.rs, distsub.rs and the pg index rows); `attributeNames` is validated and expanded like the old pair (T-3451).
- Compliant: yes (T-3451).
- Tests: tests/id_routing_5_12.rs `clause_5_12_*` (5), antares-sql `clause_5_12_matching_registrations_narrows_by_type_and_id`; robot 037_*. tests/csr_attribute_names_9_4_2.rs (4) covers `attributeNames`: discovery, the federated query, the 400 on an empty list, precedence over the old pair.

### 3.7 CSourceSubscription parameters

- Old: CIM 009 5.11.2–5.11.7, Table 5.2.12-1. New: TS 104 175 12.4.2.4, 12.4.7, Table 5.2.6.5.2-1, 5.2.6.9.2, 5.2.6.9.4. Behaviour clauses unchanged; no member added or removed; `GeoQuery`→`GeoParams`, `TemporalQuery`→`TemporalParams`.
- Changed in the table: `notificationTrigger`, `q`, `expandValues`, `jsonKeys`, `lang`, `localOnly`, `splitEntities`, `throttling` are "not applicable and shall be ignored" for a CSR subscription; `geoQ` "is matched against the specified Context Source Registration GeoJSON attributes, e.g. location, observationSpace or operationSpace"; `timeInterval` is marked not applicable while 12.4.2.4/12.4.7 still define periodic CSR notification on it (a contradiction inside TS 104 175).
- Code: `crates/antares-api/src/registry.rs` `csr_matches_subscription` :266 (geoQ against `reg.location` :315, checked by hand); `notify.rs` `csource_trigger` :1691, periodic sweep :1560-1600. Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND NOT s.is_test AND (lower(s.name) CONTAINS 'csub' OR lower(s.name) CONTAINS 'csource_sub' OR lower(s.name) CONTAINS 'csr_matches_subscription' OR lower(s.name) CONTAINS 'csource_trigger' OR lower(s.name) CONTAINS 'csource_notif') AND s.kind IN ['Function','Method','StaticMethod'] RETURN s.path, s.file, s.line LIMIT 40`
- Compliant: **partial**; the ignore list holds in code (untested), `geoQ.geoproperty` is ignored, `timeInterval` follows 12.4.7.
- Tests: registry.rs `csource_subscription_csf_and_scope_matching`; notify.rs `trigger_reason_enumeration`; robot 047_01–047_16. Gap: **T-3452** (also files the `timeInterval` contradiction upstream).

### 3.8 HTTP binding: base path, Link header, HEAD, MQTT

- Base path: CIM 009 6.2 and TS 104 176 clause 5 are the same ("The apiName shall be set to "ngsi-ld" and the apiVersion shall be set to "v1""). Antares: `antares_model::API_ROOT = "/ngsi-ld/v1"` (`crates/antares-model/src/lib.rs:15`), nested at `crates/antares-api/src/lib.rs:508-509`. Compliant **yes**. Cypher: `MATCH (c:Symbol)-[r:REFERENCES]->(s:Symbol) WHERE s.name='API_ROOT' AND s.lang='rust' RETURN c.path, c.file, r.line LIMIT 30`
- HEAD: TS 104 176 clause 5 adds "a HEAD request shall function in the same manner as a GET request, but shall omit the body in the response. The response code 204 No Content shall be used on success", while its 7.3.3.1 table still lists 200 for GET/HEAD. Antares answers 200 (`preadoptions_attr_get_value_options_head`, lib.rs:2703). Compliant: per the tables yes, per clause 5 no. Decision: **T-3460**. OPTIONS 204 with Allow: yes (`options_204`, lib.rs:771).
- Link header: TS 104 176 6.2.2–6.2.4, 6.4.10 and TS 104 175 annex C.7. Changed: 6.2.4 names HEAD beside GET and DELETE; RFC 7231 → RFC 9110; informative C.7 EXAMPLE 3 answers a compound request @context with a broker-hosted wrapper. No new link relation and no "Link header extension" occurs in the four documents (searched). Antares: `crates/antares-api/src/negotiate.rs` `link_context` :425, `request_context` :621, `context_link_url` :651, `respond` :674. Compliant: 6.2.2–6.2.4 yes; for a compound request @context answered as json the Link names the core context only (`context_link_url_selection` asserts it), short of informative C.7; recorded here, no task (informative). Tests: negotiate.rs `link_header_context_extraction`, `two_link_headers_naming_different_contexts_are_refused`, `clause_6_3_5_context_source_mixing_rules`; lib.rs `accept_precedence_follows_the_spec_list_not_header_order`; robot 001_04–001_09, 003_04–003_07, 045_01–045_04. HEAD taking its @context from the Link is untested: part of T-3460.
- MQTT (TS 104 243 5.1, 5.2): word for word CIM 009 7.1/7.2 apart from numbering. Antares: `crates/antares-notifier/src/mqtt.rs` `MqttEndpoint::parse` :35, `from_notifier_info` :104, `build_message` :136. Compliant yes, except an mqtt endpoint accepts `accept: application/geo+json` (subscriptions.rs:318-327), a Content-Type Table 5.2-2 does not list. Tests: mqtt.rs `parses_endpoint_variants`, `notifier_info_defaults_and_validation`, `message_wrapper_shape`; `mqtt_notification_carries_metadata_and_body` (antares-broker/tests/mqtt_notify.rs, needs `ANTARES_TEST_MQTT_URL`); robot 058_*. Gap: **T-3453**.

### 3.9 TS 104 179 Provenance and Integrity

New matter: requirements INT-1 to INT-4 (clause 4) and the Atomic Entity / Sealed Attribute design (5.2). Annex A places the API changes in TS 104 175 (5.2.6.4.3: a Property never includes `entityIdSealed`/`entityTypeSealed` unless its name is `ngsildproof`). The broker's share is to parse, keep and never re-sign a Sealed Attribute (INT-4); signing and verification (INT-1, INT-2, 5.2.2, 5.2.5, 5.2.6) belong to producer and client.

- Code: `crates/antares-jsonld/src/expand.rs` `check_prohibited_members` :610-712 (sealed members only on `ngsildproof`, strings, `value` an object), `compact.rs:118-122`. No signing or canonicalisation code (Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND (lower(s.name) CONTAINS 'proof' OR lower(s.name) CONTAINS 'seal' OR lower(s.name) CONTAINS 'canonical' OR lower(s.name) CONTAINS 'signature') RETURN s.path, s.file, s.line LIMIT 40`), which is right for a broker.
- Compliant: the broker's share yes for a top-level `ngsildproof`; INT-3 partial (keyValues drops sub-properties, which clause 4 allows); the nested Sealed Attribute (`color.ngsildproof`) through a merge is **untested, unknown**.
- Tests: expand.rs `ngsildproof_sealed_members_round_trip`, `ngsildproof_shapes_are_validated`, `clause_4_5_3_2_sealed_members_are_carried_by_a_property_only`; robot 452_01, 452_03, 453_01. Gap: **T-3454**.

### 3.10 New attribute operations

- Retrieve Attribute Value (TS 104 175 10.4.4; TS 104 176 7.5.3.1 GET/HEAD on `…/attrs/{attrId}`, 7.6.3.1 on `…/attrs/{attrId}/value`): Antares serves both as pre-adoptions (`crates/antares-api/src/lib.rs:338-349`, `entities::retrieve_entity_attr` / `retrieve_entity_attr_value` → `retrieve_attr_inner` :2303). Compliant yes for the routes; test `preadoptions_attr_get_value_options_head` (lib.rs:2703). Each 10.4.4.4 behaviour bullet is not audited one by one here: part of T-3461.
- Set Attribute Value (TS 104 175 10.2.6; TS 104 176 7.6.3.2 PUT `…/attrs/{attrId}/value`): served since T-3450 (`attrs::set_attr_value`). The stored instance keeps its type and sub-Attributes; the type's value member is replaced after term expansion; observedAt follows the `observedAt` parameter or is removed; `datasetId` and `type` select the instance and Entity. Every 10.2.6.4 error bullet answers as written (BadRequestData, InvalidRequest for null except in a JsonProperty, ResourceNotFound). Gated as Replace Attribute (5.6.19). Forwarded to exclusive, redirect and inclusive sources supporting `replaceAttrs` on their own value resource (9.2 names no operation for 10.2.6). Compliant yes; tests `tests/set_attribute_value_10_2_6.rs` (3) and `policy_pre_image` (two PUT …/value cases).
- Attribute fragments (TS 104 175 5.4.2) are the payload of both; audited with them.
- Cypher: `MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND (s.name CONTAINS 'retrieve_attr' OR s.name CONTAINS 'attribute_value' OR s.name CONTAINS 'set_attr' OR s.name CONTAINS 'attr_value') RETURN s.path, s.file, s.line LIMIT 20`

## 4. Rules whose text changed, not yet re-audited

292 matched clauses have at least one normative sentence removed or added (columns 8 and 9 of appendix A); 266 of them are `implemented` or `partial` in the ledger: 50 in chapter 4, 131 in chapter 5, 83 in chapter 6, 2 in the annexes. Section 3 audits the rules T-3435 names. The others keep their ledger status until they are re-read against the new text, which is T-3461: a changed sentence is often a rename (NGSI-LD Property → Property, the new document's clause references), so a count is a reason to read, not a finding.

List them: `python3 dev/clause-map.py | awk -F'\t' '$3!="informative" && ($9>0 || $10>0)'`.

### 4.1 Chapter 4

The 88 implemented or partial clauses of chapter 4 carry their TS 104 175 verdict at the end of `notes` (`grep -h 'TS 104 175' docs/spec/4/*.md`). 87 hold under the new text: most changes are renames and new clause numbers, and the rest move a rule without changing it.

| Ledger clause | TS 104 175 | What changed | Verdict |
|---|---|---|---|
| 4.3.6.4 | 9.7 | every detected loop is an error | compliant since T-3459 (3.5) |
| 4.3.6.7 | 9.6 | the Entity Map rules of CIM 009 5.5.14, 5.7.1.4, 5.7.2.4 gathered into one clause | compliant, no new obligation |
| 4.5.4, 4.5.16.2 | 5.3.2.4, 5.3.3.3.1 | scope, and in GeoJSON expiresAt, named as members | compliant, entity-level members pass through |
| 4.5.5.3 | 8.5.3 | an instance with observedAt wins over every instance without one | compliant since T-3545: `recency()` ranks observed instances first |
| 4.5.6 to 4.5.8 | 5.3.2.5 | one text for every attribute type, scope rules stated | compliant |
| 4.6.5, 4.7.2 | 5.2.6.4.6, 5.2.6.4.5 | data-type tables with an `ngsildProof` member | TS 104 179, outside the ledger (3.9) |
| 4.6.6 | 10.3.x | chronological order became a note | compliant |
| 4.2.2, 4.2.3, 4.4, 4.15 | 4.2.2, 4.2.3, 5.3.2.1, 7.2.7 | shall sentences removed | nothing to do |
| 4.16 | 5.2.3 | two should-rules for @context authors | not a broker rule |
| 4.22, 4.8 | 5.2.4 | expiresAt restated; a Context Source omits an unknown TemporalProperty | compliant |

### 4.2 Chapter 5

The 266 implemented or partial clauses of chapter 5 carry their verdict the same way (`grep -h 'TS 104 175' docs/spec/5/*.md`). 131 have a changed sentence; most are the new clause numbers, "follow" become "comply with", and the operation behaviours restating the exclusive and redirect forwarding per operation. One is non-compliant.

| Ledger clause | TS 104 175 | What changed | Verdict |
|---|---|---|---|
| 5.13.4.4 | 13.4.3, 13.4.4 | Serve @context takes `core` (major.minor): a [user URL, core URL] array; BadRequest when malformed or with `details` | compliant since T-3553 |
| 5.2.5 to 5.2.7, 5.2.32, 5.2.35 to 5.2.38 | 5.2.6.4.x | the prohibited members of CIM 009 4.5.x moved into the data-type tables; `ngsildProof` added | compliant (`check_prohibited_members`); `ngsildProof` is TS 104 179 (3.9) |
| 5.2.12, 5.2.14.1, 5.2.14.2 | 5.2.6.5.2, 5.2.6.7.3 | members a CSourceSubscription and its notification ignore | 3.7 (T-3452) |
| 5.2.10, 5.10.2.4 | 5.2.6.6.5, 12.3.3.4 | attributeNames before propertyNames and relationshipNames | 3.6 (T-3451) |
| 5.12, 5.5.14 | 9.4, 9.6 | matching rules regrouped; Entity Map rules gathered in 9.6 | compliant (4.1) |
| 5.7.3.3, 5.7.4.3 | 11.3.2.3, 11.3.3.3 | firstN | 3.2 (T-3456) |
| 5.8.6 | 10.5.7 | notification bookkeeping and formats restated, ngsildConformance amendment | compliant |
| 5.14.1.5, 5.16.7.5 | 14.2.5, 16.7.5 | the script paired them wrongly; placed by hand | unchanged |

### 4.3 Chapter 6

The 121 implemented or partial clauses of chapter 6 carry their TS 104 176 verdict the same way (`grep -h 'TS 104 176' docs/spec/6/*.md`). The resources moved from 6.4 to 6.38 into chapters 7 to 13, and the common behaviours split over 6.2 to 6.5; the script paired nineteen resource definitions and two methods to the wrong resource, so the pairs follow the resource path and the method. Every GET became GET/HEAD. Two clauses are non-compliant with TS 104 176.

| Ledger clause | TS 104 176 | What changed | Verdict |
|---|---|---|---|
| 6.3.2 | 6.3.2 | an unrecognised or unimplemented verb is 501, 405 only for a recognised one | compliant since T-3554 |
| 6.36.3.1, 6.38.3.1 | 13.2.3.1, 13.4.3.1 | Create and Clone Snapshot answer 202 Accepted, CIM 009 answers 201 | version conflict, Antares keeps 201; T-3555, raise 17 |
| 6.2, every GET | 5, x.x.3.1 | OPTIONS answers 204; HEAD is GET without a body | compliant; the HEAD status is T-3460 (3.8) |
| 6.3.10 | 6.4.7.2, 6.4.7.3 | temporal pagination in a clause of its own, firstN and offsetN | 3.2 (T-3456) |
| 6.12.3.1, 6.12.3.2 | 9.4.3.1, 9.4.3.2 | the published headings GET/HEAD and POST are swapped against their text | editorial, paired by text |
| 6.5.3.4 | 7.3.3.2 | 207 Multi Status for a distributed merge named in the table | compliant, 6.3.17 already required it |
| 6.4.3.3, 6.8.3.2 | 7.2.3.3, 9.2.3.1 | a should-rule on a configurable default; geoproperty and timeproperty rows corrected | compliant |

### 4.4 Chapter 7 and the annexes

7.2 (TS 104 243 5.2) is unchanged in substance. A.2 and A.3 lost sentences and gained none: the lowercase-namespace rule and the identifier structure are gone, which removes obligations and adds none. Annex B is the core @context, non-compliant until T-3458 (3.4). With this, every implemented or partial clause of the ledger carries its TS 104 175, 176 or 243 verdict.

## 5. Gaps and their tasks

| Task | Gap | Kind |
|---|---|---|
| T-3553 | Serve @context `core` parameter (4.2) | compliant (fixed) |
| T-3554 | 501 for an unrecognised HTTP verb (4.3) | compliant (fixed) |
| T-3555 | Create and Clone Snapshot 202 or 201 (4.3) | decided: 201, raise 17 |
| T-3450 | Set Attribute Value `PUT …/attrs/{attrId}/value` (3.10) | compliant |
| T-3451 | `RegistrationInfo.attributeNames` not matched (3.6) | non-compliant, security (over-forwarding) |
| T-3452 | CSR subscription ignores `geoQ.geoproperty`; ignore list untested (3.7) | non-compliant |
| T-3453 | mqtt endpoint accepts geo+json (3.8) | non-compliant |
| T-3454 | nested Sealed Attribute untested (3.9) | unknown, test first |
| T-3455 | q trailing path over an array of JSON objects (3.1) | non-compliant |
| T-3456 | `firstN`, `offsetN`, interval Links (3.2) | compliant (206 kept while the robots assert it) |
| T-3457 | join depth-first, not shallowest depth (3.3) | non-compliant |
| T-3458 | core @context content and scoped terms (3.4) | non-compliant, owner decision |
| T-3459 | 9.7 loop error for inclusive or no registration (3.5) | non-compliant, owner decision |
| T-3460 | HEAD 204 or 200 (3.8) | spec contradiction, owner decision |
| T-3461 | re-audit of every implemented clause (section 4) | done, gaps filed |
| T-3545 | conflicting attributes: observedAt did not take precedence (4.1) | compliant (fixed) |

The ICS ([ics-ts104187.md](ics-ts104187.md)) answers against CIM 009 as TS 104 187 A.3 asks; these gaps reach it when the ledger records them.

## Related

- [ics-ts104187.md](ics-ts104187.md): the ICS of vanilla Antares.
- [../spec/README.md](../spec/README.md): the ledger.
- [../upstream/etsi-raises.md](../upstream/etsi-raises.md): issues raised with ETSI; T-3452, T-3456, T-3459 and T-3460 add to it.
- `tasks/GOAL-antares-surfaces.md`: the epic this analysis opens.

## Appendix A. Clause map

Generated by `python3 dev/clause-map.py --markdown` at `bd5615b`; rerun it to refresh, do not edit the table by hand. Rules removed / added: normative sentences only the old / only the new clause has (counts). `new-only` rows count the new clause's normative sentences under "Rules added".

| CIM 009 | Title | Ledger status | New clause | New title | Kind | Score | Rules removed | Rules added |
|---|---|---|---|---|---|---|---|---|
| [1](../spec/1/1.md) | Scope | informative | TS 104 175 2.1 | Normative references | weak | 0.53 | 0 | 0 |
| [2.1](../spec/2/2.1.md) | Normative references | informative | TS 104 175 2.1 | Normative references | edited | 1.06 | 0 | 0 |
| [2.2](../spec/2/2.2.md) | Informative references | informative | TS 104 175 2.2 | Informative references | edited | 0.99 | 1 | 1 |
| [2](../spec/2/2.md) | References | informative | TS 104 175 1 | Scope | edited | 0.81 | 0 | 0 |
| [3.1](../spec/3/3.1.md) | Terms | informative | TS 104 176 3.1 | Terms | moved | 1.50 | 0 | 0 |
| [3.2](../spec/3/3.2.md) | Symbols | informative | TS 104 175 3.2 | Symbols | edited | 1.00 | 0 | 0 |
| [3.3](../spec/3/3.3.md) | Abbreviations | informative |   |  | unmatched | 0.35 |  |  |
| [3](../spec/3/3.md) | Definition of terms, symbols and abbreviations | informative | TS 104 175 2.2 | Informative references | edited | 0.61 | 0 | 2 |
| [4.1](../spec/4/4.1.md) | Introduction | informative | TS 104 175 4.1 | Introduction | edited | 0.87 | 0 | 0 |
| [4.10](../spec/4/4.10.md) | NGSI-LD Geoquery Language | implemented | TS 104 175 7.2.4 | GeoQuery language | edited | 0.94 | 10 | 10 |
| [4.11](../spec/4/4.11.md) | NGSI-LD Temporal Query Language | implemented | TS 104 175 7.2.6 | Temporal query language | edited | 0.94 | 3 | 3 |
| [4.12](../spec/4/4.12.md) | NGSI-LD Pagination | implemented | TS 104 175 7.4.2.1 | Introduction | edited | 0.91 | 1 | 1 |
| [4.13](../spec/4/4.13.md) | Counting the Number of Results | implemented | TS 104 175 7.5 | Counting results | edited | 0.87 | 2 | 2 |
| [4.14](../spec/4/4.14.md) | Supporting Multiple Tenants | implemented | TS 104 175 7.8.1 | Introduction | edited | 0.99 | 0 | 0 |
| [4.15](../spec/4/4.15.md) | NGSI-LD Language Filter | implemented | TS 104 175 7.2.7 | Language filter | edited | 0.96 | 2 | 2 |
| [4.16](../spec/4/4.16.md) | Supporting Multiple Entity Types | implemented | TS 104 175 5.2.3 | Entity Types | edited | 0.95 | 0 | 2 |
| [4.17](../spec/4/4.17.md) | NGSI-LD Entity Type Selection Language | implemented | TS 104 175 7.2.2 | Entity Type selection language | edited | 0.91 | 1 | 1 |
| [4.18](../spec/4/4.18.md) | NGSI-LD Scopes | implemented | TS 104 175 5.2.7 | Scopes | edited | 0.95 | 0 | 0 |
| [4.19](../spec/4/4.19.md) | NGSI-LD Scope Query Language | implemented | TS 104 175 7.2.5 | Scope query language | edited | 0.94 | 1 | 1 |
| [4.2.1](../spec/4/4.2.1.md) | Introduction | informative | TS 104 175 4.2.1 | Introduction | edited | 0.83 | 2 | 0 |
| [4.2.2](../spec/4/4.2.2.md) | NGSI-LD Meta Model | implemented | TS 104 175 4.2.2 | NGSI-LD meta model | edited | 1.38 | 5 | 0 |
| [4.2.3](../spec/4/4.2.3.md) | Cross Domain Ontology | implemented | TS 104 175 4.2.3 | Cross-domain ontology | edited | 1.15 | 8 | 0 |
| [4.2.4](../spec/4/4.2.4.md) | NGSI-LD domain-specific models and instantiation | informative | TS 104 175 4.2.4 | NGSI-LD domain-specific models and instantiation | moved | 1.44 | 0 | 0 |
| [4.2.5](../spec/4/4.2.5.md) | UML representation | informative | TS 104 175 4.2.5 | UML representation | edited | 0.81 | 0 | 0 |
| [4.2](../spec/4/4.2.md) | NGSI-LD Information Model | informative | TS 104 175 4.2 | NGSI-LD information model | edited | 1.00 | 0 | 0 |
| [4.20](../spec/4/4.20.md) | NGSI-LD Distributed Operation names | implemented | TS 104 175 9.2 | Distributed operation names | weak | 0.48 | 1 | 1 |
| [4.21](../spec/4/4.21.md) | NGSI-LD Attribute Projection Language | implemented | TS 104 175 7.3.2 | Attribute projection language | edited | 0.91 | 1 | 1 |
| [4.22](../spec/4/4.22.md) | Transient Storage of Entities and Attributes | implemented | TS 104 175 5.2.4 | Simple Attributes / temporal Attributes | edited | 0.94 | 1 | 5 |
| [4.23.1](../spec/4/4.23.1.md) | Introduction | implemented | TS 104 175 7.6.2.1 | Introduction | edited | 0.97 | 2 | 2 |
| [4.23.2](../spec/4/4.23.2.md) | Datatype Comparison Order | implemented | TS 104 175 7.6.2.2 | Datatype comparison order | moved | 1.47 | 1 | 1 |
| [4.23.3](../spec/4/4.23.3.md) | NGSI-LD Entity Ordering Language | implemented | TS 104 175 7.6.2.3 | NGSI-LD Entity ordering language | edited | 1.01 | 11 | 3 |
| [4.23](../spec/4/4.23.md) | Entity Ordering | implemented |   |  | heading | 0.00 |  |  |
| [4.3.1](../spec/4/4.3.1.md) | Introduction | informative | TS 104 175 4.3.1 | Introduction | edited | 0.54 | 2 | 1 |
| [4.3.2](../spec/4/4.3.2.md) | Centralized architecture | informative | TS 104 175 4.3.2 | Centralized architecture | moved | 1.42 | 0 | 0 |
| [4.3.3](../spec/4/4.3.3.md) | Distributed architecture | informative | TS 104 175 4.3.3.1 | Introduction | edited | 0.95 | 0 | 0 |
| [4.3.4](../spec/4/4.3.4.md) | Federated architecture | informative | TS 104 175 4.3.3.1 | Introduction | edited | 0.89 | 1 | 1 |
| [4.3.5](../spec/4/4.3.5.md) | NGSI-LD API Structure and Implementation Options | implemented | TS 104 175 6 | API structure | edited | 0.63 | 1 | 1 |
| [4.3.6.1](../spec/4/4.3.6.1.md) | Introduction | implemented | TS 104 175 9.1 | Introduction | edited | 0.86 | 2 | 2 |
| [4.3.6.2](../spec/4/4.3.6.2.md) | Additive Registrations | implemented | TS 104 175 9.3.2 | Additive registrations | moved | 1.46 | 0 | 0 |
| [4.3.6.3](../spec/4/4.3.6.3.md) | Proxied Registrations | implemented | TS 104 175 9.3.3 | Proxied registrations | moved | 1.49 | 0 | 0 |
| [4.3.6.4](../spec/4/4.3.6.4.md) | Limiting Cascading Distributed Operations | implemented | TS 104 175 9.7 | Limiting cascading distributed operations | edited | 1.39 | 2 | 3 |
| [4.3.6.5](../spec/4/4.3.6.5.md) | Extra information to provide when contacting Context Source | implemented | TS 104 175 9.5.1 | Extra information to provide when contacting Context Sources | edited | 0.95 | 1 | 1 |
| [4.3.6.6](../spec/4/4.3.6.6.md) | Additional pre- and post-processing of extra information when contacting Context Source | implemented | TS 104 175 9.5.2 | Additional pre- and post-processing of extra information when contacting Context Sources | edited | 0.94 | 3 | 3 |
| [4.3.6.7](../spec/4/4.3.6.7.md) | Querying and Retrieving Distributed Entities as Unitary Operations | implemented | TS 104 175 9.6 | Querying and retrieving distributed Entities as unitary operations - Entity Maps | edited | 0.92 | 2 | 10 |
| [4.3.6.8](../spec/4/4.3.6.8.md) | Backwards compatibility of Context Source payloads | implemented | TS 104 175 9.5.3 | Backwards compatibility of Context Source payloads | edited | 1.31 | 2 | 2 |
| [4.3.6](../spec/4/4.3.6.md) | Distributed Operations | informative |   |  | heading | 0.00 |  |  |
| [4.3.7](../spec/4/4.3.7.md) | Snapshots | informative | TS 104 175 7.9.1 | Introduction | edited | 0.97 | 1 | 1 |
| [4.3](../spec/4/4.3.md) | NGSI-LD Architectural Considerations | informative |   |  | heading | 0.00 |  |  |
| [4.4](../spec/4/4.4.md) | Core and user NGSI-LD @context | implemented | TS 104 175 5.3.2.1 | Core and User @context | edited | 0.81 | 4 | 3 |
| [4.5.0](../spec/4/4.5.0.md) | Introduction | informative |   |  | unmatched | 0.17 |  |  |
| [4.5.1](../spec/4/4.5.1.md) | NGSI-LD Entity Representation | implemented | TS 104 175 5.3.2.2 | Entities Normalized | weak | 0.55 | 6 | 16 |
| [4.5.10](../spec/4/4.5.10.md) | Entity Type List Representation | implemented |   |  | unmatched | 0.43 |  |  |
| [4.5.11](../spec/4/4.5.11.md) | Detailed Entity Type List Representation | implemented |   |  | unmatched | 0.41 |  |  |
| [4.5.12](../spec/4/4.5.12.md) | Entity Type Information Representation | implemented |   |  | unmatched | 0.36 |  |  |
| [4.5.13](../spec/4/4.5.13.md) | Attribute List Representation | implemented |   |  | unmatched | 0.36 |  |  |
| [4.5.14](../spec/4/4.5.14.md) | Detailed Attribute List Representation | implemented |   |  | unmatched | 0.34 |  |  |
| [4.5.15](../spec/4/4.5.15.md) | Attribute Information Representation | implemented |   |  | unmatched | 0.30 |  |  |
| [4.5.16.0](../spec/4/4.5.16.0.md) | Foreword | informative | TS 104 175 5.3.3.1 | Introduction | edited | 0.79 | 0 | 0 |
| [4.5.16.1](../spec/4/4.5.16.1.md) | Top-level "geometry" field selection algorithm | implemented | TS 104 175 5.3.3.2 | Top-level "geometry" field selection algorithm | moved | 1.44 | 2 | 2 |
| [4.5.16.2](../spec/4/4.5.16.2.md) | GeoJSON Representation of an individual Entity | implemented | TS 104 175 5.3.3.3.1 | Representation of an individual Entity | edited | 0.76 | 1 | 2 |
| [4.5.16.3](../spec/4/4.5.16.3.md) | GeoJSON Representation of Multiple Entities | implemented | TS 104 175 5.3.3.3.2 | Representation of multiple Entities | edited | 0.81 | 1 | 1 |
| [4.5.16](../spec/4/4.5.16.md) | GeoJSON Representation of Entities | informative |   |  | heading | 0.00 |  |  |
| [4.5.17.0](../spec/4/4.5.17.0.md) | Foreword | informative |   |  | unmatched | 0.04 |  |  |
| [4.5.17.1](../spec/4/4.5.17.1.md) | Simplified GeoJSON Representation of an individual Entity | implemented | TS 104 175 5.3.2.4 | Entities Simplified | weak | 0.45 | 1 | 9 |
| [4.5.17.2](../spec/4/4.5.17.2.md) | Simplified GeoJSON Representation of multiple Entities | implemented | TS 104 175 5.3.3.3.2 | Representation of multiple Entities | edited | 0.75 | 1 | 1 |
| [4.5.17](../spec/4/4.5.17.md) | Simplified GeoJSON Representation of Entities | informative |   |  | heading | 0.00 |  |  |
| [4.5.18.1](../spec/4/4.5.18.1.md) | Introduction | implemented |   |  | unmatched | 0.27 |  |  |
| [4.5.18.2](../spec/4/4.5.18.2.md) | Normalized NGSI-LD LanguageProperty | implemented |   |  | unmatched | 0.18 |  |  |
| [4.5.18.3](../spec/4/4.5.18.3.md) | Concise NGSI-LD LanguageProperty | implemented |   |  | unmatched | 0.16 |  |  |
| [4.5.18](../spec/4/4.5.18.md) | NGSI-LD LanguageProperty Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.19.0](../spec/4/4.5.19.0.md) | Foreword | partial | TS 104 175 5.3.2.7 | Temporal Aggregated | edited | 0.91 | 0 | 4 |
| [4.5.19.1](../spec/4/4.5.19.1.md) | Supported behaviours for aggregation functions | implemented | TS 104 175 5.3.2.7 | Temporal Aggregated | edited | 0.95 | 0 | 8 |
| [4.5.19](../spec/4/4.5.19.md) | Aggregated temporal representation of an Entity | informative |   |  | heading | 0.00 |  |  |
| [4.5.2.1](../spec/4/4.5.2.1.md) | Introduction | implemented | TS 104 175 5.3.2.3 | Entities Concise | edited | 0.68 | 1 | 3 |
| [4.5.2.2](../spec/4/4.5.2.2.md) | Normalized NGSI-LD Property | implemented |   |  | unmatched | 0.29 |  |  |
| [4.5.2.3](../spec/4/4.5.2.3.md) | Concise NGSI-LD Property | implemented |   |  | unmatched | 0.25 |  |  |
| [4.5.2](../spec/4/4.5.2.md) | NGSI-LD Property Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.20.1](../spec/4/4.5.20.1.md) | Introduction | implemented |   |  | unmatched | 0.27 |  |  |
| [4.5.20.2](../spec/4/4.5.20.2.md) | Normalized NGSI-LD VocabProperty | implemented |   |  | unmatched | 0.16 |  |  |
| [4.5.20.3](../spec/4/4.5.20.3.md) | Concise NGSI-LD VocabProperty | implemented |   |  | unmatched | 0.17 |  |  |
| [4.5.20](../spec/4/4.5.20.md) | NGSI-LD VocabProperty Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.21.1](../spec/4/4.5.21.1.md) | Introduction | implemented |   |  | unmatched | 0.27 |  |  |
| [4.5.21.2](../spec/4/4.5.21.2.md) | Normalized NGSI-LD ListProperty | implemented |   |  | unmatched | 0.25 |  |  |
| [4.5.21.3](../spec/4/4.5.21.3.md) | Concise NGSI-LD ListProperty | implemented |   |  | unmatched | 0.23 |  |  |
| [4.5.21](../spec/4/4.5.21.md) | NGSI-LD ListProperty Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.22.1](../spec/4/4.5.22.1.md) | Introduction | implemented |   |  | unmatched | 0.27 |  |  |
| [4.5.22.2](../spec/4/4.5.22.2.md) | Normalized NGSI-LD ListRelationship | implemented |   |  | unmatched | 0.20 |  |  |
| [4.5.22.3](../spec/4/4.5.22.3.md) | Concise NGSI-LD ListRelationship | implemented |   |  | unmatched | 0.18 |  |  |
| [4.5.22](../spec/4/4.5.22.md) | NGSI-LD ListRelationship Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.23.1](../spec/4/4.5.23.1.md) | Introduction | implemented | TS 104 175 7.7.1 | Introduction | edited | 0.95 | 0 | 0 |
| [4.5.23.2](../spec/4/4.5.23.2.md) | Inline Linked Entity Representation | implemented | TS 104 175 7.7.2 | Inline Linked Entity representation | moved | 1.46 | 1 | 1 |
| [4.5.23.3](../spec/4/4.5.23.3.md) | Flattened Linked Entity Representation | implemented | TS 104 175 7.7.3 | Flattened Linked Entity representation | edited | 1.31 | 1 | 1 |
| [4.5.23](../spec/4/4.5.23.md) | NGSI-LD Linked Entity Retrieval | informative |   |  | heading | 0.00 |  |  |
| [4.5.24.1](../spec/4/4.5.24.1.md) | Introduction | implemented |   |  | unmatched | 0.27 |  |  |
| [4.5.24.2](../spec/4/4.5.24.2.md) | Normalized NGSI-LD JsonProperty | implemented |   |  | unmatched | 0.13 |  |  |
| [4.5.24.3](../spec/4/4.5.24.3.md) | Concise NGSI-LD JsonProperty | implemented |   |  | unmatched | 0.14 |  |  |
| [4.5.24](../spec/4/4.5.24.md) | NGSI-LD JsonProperty Representations | implemented |   |  | heading | 0.00 |  |  |
| [4.5.25](../spec/4/4.5.25.md) | NGSI-LD EntityMap Representation | implemented |   |  | unmatched | 0.15 |  |  |
| [4.5.3.1](../spec/4/4.5.3.1.md) | Introduction | implemented | TS 104 175 5.3.2.3 | Entities Concise | edited | 0.68 | 1 | 3 |
| [4.5.3.2](../spec/4/4.5.3.2.md) | Normalized NGSI-LD Relationship | implemented |   |  | unmatched | 0.32 |  |  |
| [4.5.3.3](../spec/4/4.5.3.3.md) | Concise NGSI-LD Relationship | implemented |   |  | unmatched | 0.28 |  |  |
| [4.5.3](../spec/4/4.5.3.md) | NGSI-LD Relationship Representations | informative |   |  | heading | 0.00 |  |  |
| [4.5.4](../spec/4/4.5.4.md) | Simplified Representation | implemented | TS 104 175 5.3.2.4 | Entities Simplified | edited | 0.71 | 2 | 4 |
| [4.5.5.1](../spec/4/4.5.5.1.md) | Introduction | implemented | TS 104 175 8.5.1 | Introduction | edited | 0.96 | 0 | 0 |
| [4.5.5.2](../spec/4/4.5.5.2.md) | Processing of Conflicting Transient Entities | implemented | TS 104 175 8.5.2 | Processing of conflicting transient Entities | moved | 1.47 | 1 | 1 |
| [4.5.5.3](../spec/4/4.5.5.3.md) | Processing of Conflicting Attributes | implemented | TS 104 175 8.5.3 | Processing of conflicting Attributes | moved | 1.42 | 1 | 3 |
| [4.5.5](../spec/4/4.5.5.md) | Multi-Attribute Support | informative |   |  | heading | 0.00 |  |  |
| [4.5.6](../spec/4/4.5.6.md) | Temporal Representation of an Entity | implemented | TS 104 175 5.3.2.5 | Temporal Normalized | edited | 0.79 | 2 | 15 |
| [4.5.7](../spec/4/4.5.7.md) | Temporal Representation of a Property | implemented | TS 104 175 5.3.2.5 | Temporal Normalized | edited | 0.83 | 2 | 13 |
| [4.5.8](../spec/4/4.5.8.md) | Temporal Representation of a Relationship | implemented | TS 104 175 5.3.2.5 | Temporal Normalized | edited | 0.78 | 2 | 13 |
| [4.5.9](../spec/4/4.5.9.md) | Simplified temporal representation of an Entity | implemented | TS 104 175 5.3.2.6 | Temporal Simplified | edited | 0.93 | 0 | 1 |
| [4.5](../spec/4/4.5.md) | NGSI-LD Data Representation | informative |   |  | heading | 0.00 |  |  |
| [4.6.1](../spec/4/4.6.1.md) | Supported text encodings | implemented | TS 104 175 5.2.2.2 | Supported text encodings | moved | 1.43 | 1 | 1 |
| [4.6.2](../spec/4/4.6.2.md) | Supported names | implemented | TS 104 175 5.2.2.3 | Supported names | moved | 1.44 | 0 | 0 |
| [4.6.3](../spec/4/4.6.3.md) | Supported data types for Values | implemented | TS 104 175 5.2.2.4 | Supported data types for Values | moved | 1.46 | 6 | 6 |
| [4.6.4](../spec/4/4.6.4.md) | Supported Content | implemented | TS 104 175 5.2.2.5 | Supported content | moved | 1.48 | 0 | 0 |
| [4.6.5](../spec/4/4.6.5.md) | Supported data types for LanguageMaps | implemented | TS 104 175 5.2.6.4.6 | LanguageProperty | edited | 0.67 | 3 | 14 |
| [4.6.6](../spec/4/4.6.6.md) | Ordering of Entities in arrays having more than one instance of the same Entity | implemented |   |  | unmatched | 0.32 |  |  |
| [4.6](../spec/4/4.6.md) | Data Representation Restrictions | informative |   |  | heading | 0.00 |  |  |
| [4.7.1](../spec/4/4.7.1.md) | GeoJSON Geometries | implemented | TS 104 175 5.2.6.4.1 | Entity | weak | 0.50 | 5 | 3 |
| [4.7.2](../spec/4/4.7.2.md) | Representation of GeoJSON Geometries in JSON-LD | implemented | TS 104 175 5.2.6.4.5 | GeoProperty | edited | 0.96 | 1 | 13 |
| [4.7.3](../spec/4/4.7.3.md) | Concise NGSI-LD GeoProperty | implemented |   |  | unmatched | 0.10 |  |  |
| [4.7](../spec/4/4.7.md) | Geospatial Properties | informative |   |  | heading | 0.00 |  |  |
| [4.8](../spec/4/4.8.md) | Temporal Properties | implemented | TS 104 175 5.2.4 | Simple Attributes / temporal Attributes | edited | 0.89 | 3 | 8 |
| [4.9](../spec/4/4.9.md) | NGSI-LD Query Language | implemented | TS 104 175 7.2.3.2 | Query Language Definition | edited | 0.65 | 30 | 14 |
| [4](../spec/4/4.md) | Context Information Management Framework | informative |   |  | heading | 0.00 |  |  |
| [5.1](../spec/5/5.1.md) | Introduction | informative |   |  | unmatched | 0.33 |  |  |
| [5.10.1.1](../spec/5/5.10.1.1.md) | Description | informative | TS 104 175 12.3.2.1 | Description | edited | 0.82 | 0 | 0 |
| [5.10.1.2](../spec/5/5.10.1.2.md) | Use case diagram | informative | TS 104 175 12.3.2.2 | Use case diagram | edited | 1.32 | 0 | 0 |
| [5.10.1.3](../spec/5/5.10.1.3.md) | Input data | implemented | TS 104 175 12.3.2.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.10.1.4](../spec/5/5.10.1.4.md) | Behaviour | implemented | TS 104 175 12.3.2.4 | Behaviour | moved | 1.41 | 1 | 1 |
| [5.10.1.5](../spec/5/5.10.1.5.md) | Output data | implemented | TS 104 175 12.3.2.5 | Output data | edited | 1.31 | 0 | 0 |
| [5.10.1](../spec/5/5.10.1.md) | Retrieve Context Source Registration | informative | TS 104 175 12.3.2 | Retrieve Context Source Registration | edited | 1.00 | 0 | 0 |
| [5.10.2.1](../spec/5/5.10.2.1.md) | Description | informative | TS 104 175 12.3.3.1 | Description | edited | 0.94 | 0 | 0 |
| [5.10.2.2](../spec/5/5.10.2.2.md) | Use case diagram | informative | TS 104 175 12.3.3.2 | Use case diagram | edited | 1.35 | 1 | 1 |
| [5.10.2.3](../spec/5/5.10.2.3.md) | Input data | implemented | TS 104 175 12.3.3.3 | Input data | edited | 1.35 | 0 | 0 |
| [5.10.2.4](../spec/5/5.10.2.4.md) | Behaviour | implemented | TS 104 175 12.3.3.4 | Behaviour | moved | 1.43 | 5 | 5 |
| [5.10.2.5](../spec/5/5.10.2.5.md) | Output data | implemented | TS 104 175 12.3.3.5 | Output data | edited | 1.39 | 0 | 0 |
| [5.10.2](../spec/5/5.10.2.md) | Query Context Source Registrations | informative | TS 104 175 12.3.3 | Query Context Source Registrations | edited | 1.00 | 0 | 0 |
| [5.10](../spec/5/5.10.md) | Context Source Discovery | informative | TS 104 175 12.3 | Context Source discovery | edited | 1.00 | 0 | 0 |
| [5.11.1](../spec/5/5.11.1.md) | Introduction | implemented | TS 104 175 12.4.1 | Introduction | edited | 0.97 | 0 | 0 |
| [5.11.2.1](../spec/5/5.11.2.1.md) | Description | informative | TS 104 175 12.4.2.1 | Description | edited | 0.85 | 0 | 0 |
| [5.11.2.2](../spec/5/5.11.2.2.md) | Use case diagram | informative | TS 104 175 12.4.2.2 | Use case diagram | edited | 1.34 | 0 | 0 |
| [5.11.2.3](../spec/5/5.11.2.3.md) | Input data | implemented | TS 104 175 10.5.2.3 | Input data | edited | 1.38 | 0 | 0 |
| [5.11.2.4](../spec/5/5.11.2.4.md) | Behaviour | implemented | TS 104 175 12.4.2.4 | Behaviour | moved | 1.45 | 3 | 3 |
| [5.11.2.5](../spec/5/5.11.2.5.md) | Output data | implemented | TS 104 175 12.4.2.5 | Output data | edited | 1.32 | 0 | 0 |
| [5.11.2](../spec/5/5.11.2.md) | Create Context Source Registration Subscription | informative | TS 104 175 12.4.2 | Create Context Source Registration Subscription | edited | 1.00 | 0 | 0 |
| [5.11.3.1](../spec/5/5.11.3.1.md) | Description | informative | TS 104 175 12.4.3.1 | Description | edited | 0.85 | 0 | 0 |
| [5.11.3.2](../spec/5/5.11.3.2.md) | Use case diagram | informative | TS 104 175 12.4.3.2 | Use case diagram | edited | 1.33 | 0 | 0 |
| [5.11.3.3](../spec/5/5.11.3.3.md) | Input data | implemented | TS 104 175 12.4.3.3 | Input data | moved | 1.40 | 0 | 0 |
| [5.11.3.4](../spec/5/5.11.3.4.md) | Behaviour | implemented | TS 104 175 12.4.3.4 | Behaviour | edited | 1.37 | 2 | 2 |
| [5.11.3.5](../spec/5/5.11.3.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.11.3](../spec/5/5.11.3.md) | Update Context Source Registration Subscription | informative | TS 104 175 12.4.3 | Update Context Source Registration Subscription | edited | 1.00 | 0 | 0 |
| [5.11.4.1](../spec/5/5.11.4.1.md) | Description | informative | TS 104 175 12.4.4.1 | Description | edited | 0.85 | 0 | 0 |
| [5.11.4.2](../spec/5/5.11.4.2.md) | Use case diagram | informative | TS 104 175 12.4.4.2 | Use case diagram | edited | 1.34 | 0 | 0 |
| [5.11.4.3](../spec/5/5.11.4.3.md) | Input data | implemented | TS 104 175 10.5.4.3 | Input data | edited | 1.36 | 0 | 0 |
| [5.11.4.4](../spec/5/5.11.4.4.md) | Behaviour | implemented | TS 104 175 12.4.4.4 | Behaviour | moved | 1.47 | 0 | 0 |
| [5.11.4.5](../spec/5/5.11.4.5.md) | Output data | implemented | TS 104 175 12.4.4.5 | Output data | edited | 1.39 | 0 | 0 |
| [5.11.4](../spec/5/5.11.4.md) | Retrieve Context Source Registration Subscription | informative | TS 104 175 12.4.4 | Retrieve Context Source Registration Subscription | edited | 1.00 | 0 | 0 |
| [5.11.5.1](../spec/5/5.11.5.1.md) | Description | informative | TS 104 175 12.4.5.1 | Description | edited | 0.83 | 0 | 0 |
| [5.11.5.2](../spec/5/5.11.5.2.md) | Use case diagram | informative | TS 104 175 12.4.5.2 | Use case diagram | edited | 1.34 | 0 | 0 |
| [5.11.5.3](../spec/5/5.11.5.3.md) | Input data | implemented | TS 104 175 12.4.5.3 | Input data | edited | 1.27 | 0 | 0 |
| [5.11.5.4](../spec/5/5.11.5.4.md) | Behaviour | implemented | TS 104 175 12.4.5.4 | Behaviour | edited | 1.33 | 1 | 1 |
| [5.11.5.5](../spec/5/5.11.5.5.md) | Output data | implemented | TS 104 175 10.5.5.5 | Output data | edited | 1.39 | 0 | 0 |
| [5.11.5](../spec/5/5.11.5.md) | Query Context Source Registration Subscriptions | informative | TS 104 175 12.4.5 | Query Context Source Registration Subscriptions | edited | 1.00 | 0 | 0 |
| [5.11.6.1](../spec/5/5.11.6.1.md) | Description | informative | TS 104 175 12.4.6.1 | Description | edited | 0.85 | 0 | 0 |
| [5.11.6.2](../spec/5/5.11.6.2.md) | Use case diagram | informative | TS 104 175 12.4.6.2 | Use case diagram | moved | 1.40 | 0 | 0 |
| [5.11.6.3](../spec/5/5.11.6.3.md) | Input data | implemented | TS 104 175 12.2.3.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.11.6.4](../spec/5/5.11.6.4.md) | Behaviour | implemented | TS 104 175 12.4.6.4 | Behaviour | moved | 1.47 | 0 | 0 |
| [5.11.6.5](../spec/5/5.11.6.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.11.6](../spec/5/5.11.6.md) | Delete Context Source Registration Subscription | informative | TS 104 175 12.4.6 | Delete Context Source Registration Subscription | edited | 1.00 | 0 | 0 |
| [5.11.7](../spec/5/5.11.7.md) | Notification behaviour | implemented | TS 104 175 12.4.7 | Notification behaviour | moved | 1.46 | 3 | 3 |
| [5.11](../spec/5/5.11.md) | Context Source Registration Subscription | informative | TS 104 175 12.4 | Context Source Registration Subscription | edited | 1.00 | 0 | 0 |
| [5.12](../spec/5/5.12.md) | Matching Context Source Registrations | implemented | TS 104 175 9.4 | Matching Context Source registrations | edited | 0.50 | 5 | 0 |
| [5.13.1](../spec/5/5.13.1.md) | Introduction | implemented | TS 104 175 13.1 | Introduction | edited | 0.97 | 1 | 1 |
| [5.13.2.1](../spec/5/5.13.2.1.md) | Description | informative | TS 104 175 13.2.1 | Description | edited | 0.96 | 0 | 0 |
| [5.13.2.2](../spec/5/5.13.2.2.md) | Use case diagram | informative | TS 104 175 13.2.2 | Use case diagram | edited | 1.38 | 0 | 0 |
| [5.13.2.3](../spec/5/5.13.2.3.md) | Input data | implemented | TS 104 175 13.2.3 | Input data | moved | 1.42 | 1 | 1 |
| [5.13.2.4](../spec/5/5.13.2.4.md) | Behaviour | implemented | TS 104 175 13.2.4 | Behaviour | moved | 1.43 | 1 | 1 |
| [5.13.2.5](../spec/5/5.13.2.5.md) | Output data | implemented | TS 104 175 13.2.5 | Output data | moved | 1.44 | 0 | 0 |
| [5.13.2](../spec/5/5.13.2.md) | Add @context | informative | TS 104 175 13.2 | Add @context | edited | 1.00 | 0 | 0 |
| [5.13.3.1](../spec/5/5.13.3.1.md) | Description | informative | TS 104 175 13.3.1 | Description | edited | 0.99 | 0 | 0 |
| [5.13.3.2](../spec/5/5.13.3.2.md) | Use case diagram | informative | TS 104 175 13.3.2 | Use case diagram | edited | 1.37 | 0 | 0 |
| [5.13.3.3](../spec/5/5.13.3.3.md) | Input data | implemented | TS 104 175 13.3.3 | Input data | moved | 1.48 | 0 | 0 |
| [5.13.3.4](../spec/5/5.13.3.4.md) | Behaviour | implemented | TS 104 175 13.3.4 | Behaviour | moved | 1.47 | 1 | 1 |
| [5.13.3.5](../spec/5/5.13.3.5.md) | Output data | implemented | TS 104 175 13.3.5 | Output data | moved | 1.48 | 0 | 0 |
| [5.13.3](../spec/5/5.13.3.md) | List @contexts | informative | TS 104 175 13.3 | List @contexts | edited | 1.00 | 0 | 0 |
| [5.13.4.1](../spec/5/5.13.4.1.md) | Description | informative | TS 104 175 13.4.1 | Description | edited | 0.82 | 0 | 0 |
| [5.13.4.2](../spec/5/5.13.4.2.md) | Use case diagram | informative | TS 104 175 13.4.2 | Use case diagram | edited | 1.39 | 0 | 0 |
| [5.13.4.3](../spec/5/5.13.4.3.md) | Input data | implemented | TS 104 175 13.4.3 | Input data | moved | 1.43 | 0 | 0 |
| [5.13.4.4](../spec/5/5.13.4.4.md) | Behaviour | implemented | TS 104 175 13.4.4 | Behaviour | moved | 1.45 | 2 | 6 |
| [5.13.4.5](../spec/5/5.13.4.5.md) | Output data | implemented | TS 104 175 13.4.5 | Output data | edited | 1.35 | 0 | 0 |
| [5.13.4](../spec/5/5.13.4.md) | Serve @context | informative | TS 104 175 13.4 | Serve @context | edited | 1.00 | 0 | 0 |
| [5.13.5.1](../spec/5/5.13.5.1.md) | Description | informative | TS 104 175 13.5.1 | Description | edited | 0.99 | 1 | 1 |
| [5.13.5.2](../spec/5/5.13.5.2.md) | Use case diagram | informative | TS 104 175 13.5.2 | Use case diagram | moved | 1.40 | 0 | 0 |
| [5.13.5.3](../spec/5/5.13.5.3.md) | Input data | implemented | TS 104 175 13.5.3 | Input data | moved | 1.48 | 0 | 0 |
| [5.13.5.4](../spec/5/5.13.5.4.md) | Behaviour | implemented | TS 104 175 13.5.4 | Behaviour | moved | 1.45 | 2 | 2 |
| [5.13.5.5](../spec/5/5.13.5.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.13.5](../spec/5/5.13.5.md) | Delete and Reload @context | informative | TS 104 175 13.5 | Delete and reload @context | edited | 1.00 | 0 | 0 |
| [5.13](../spec/5/5.13.md) | Storing, Managing and Serving @contexts | informative |   |  | heading | 0.00 |  |  |
| [5.14.1.1](../spec/5/5.14.1.1.md) | Description | informative | TS 104 175 14.2.1 | Description | edited | 0.68 | 0 | 0 |
| [5.14.1.2](../spec/5/5.14.1.2.md) | Use case diagram | informative | TS 104 175 14.2.2 | Use case diagram | edited | 1.09 | 0 | 0 |
| [5.14.1.3](../spec/5/5.14.1.3.md) | Input data | implemented | TS 104 175 10.4.2.3 | Input data | edited | 0.83 | 0 | 0 |
| [5.14.1.4](../spec/5/5.14.1.4.md) | Behaviour | implemented | TS 104 175 14.2.4 | Behaviour | edited | 1.26 | 2 | 2 |
| [5.14.1.5](../spec/5/5.14.1.5.md) | Output data | implemented | TS 104 175 10.4.2.5 | Output data | edited | 1.13 | 0 | 9 |
| [5.14.1](../spec/5/5.14.1.md) | Retrieve EntityMap | informative |   |  | heading | 0.00 |  |  |
| [5.14.2.1](../spec/5/5.14.2.1.md) | Description | informative | TS 104 175 14.3.1 | Description | edited | 0.81 | 0 | 0 |
| [5.14.2.2](../spec/5/5.14.2.2.md) | Use case diagram | informative | TS 104 175 14.3.2 | Use case diagram | edited | 1.08 | 0 | 0 |
| [5.14.2.3](../spec/5/5.14.2.3.md) | Input data | implemented | TS 104 175 10.2.11.3 | Input data | edited | 0.82 | 0 | 0 |
| [5.14.2.4](../spec/5/5.14.2.4.md) | Behaviour | implemented | TS 104 175 14.3.4 | Behaviour | edited | 1.30 | 2 | 2 |
| [5.14.2.5](../spec/5/5.14.2.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.14.2](../spec/5/5.14.2.md) | Update EntityMap | informative |   |  | heading | 0.00 |  |  |
| [5.14.3.1](../spec/5/5.14.3.1.md) | Description | informative | TS 104 175 10.2.7.1 | Description | edited | 0.64 | 0 | 1 |
| [5.14.3.2](../spec/5/5.14.3.2.md) | Use case diagram | informative | TS 104 175 14.4.2 | Use case diagram | edited | 1.11 | 0 | 0 |
| [5.14.3.3](../spec/5/5.14.3.3.md) | Input data | implemented | TS 104 175 10.4.2.3 | Input data | edited | 0.83 | 0 | 0 |
| [5.14.3.4](../spec/5/5.14.3.4.md) | Behaviour | implemented | TS 104 175 14.4.4 | Behaviour | edited | 1.23 | 3 | 3 |
| [5.14.3.5](../spec/5/5.14.3.5.md) | Output data | implemented | TS 104 175 10.6.4.5 | Output data | edited | 0.75 | 0 | 0 |
| [5.14.3](../spec/5/5.14.3.md) | Delete EntityMap | informative |   |  | heading | 0.00 |  |  |
| [5.14.4.1](../spec/5/5.14.4.1.md) | Description | informative | TS 104 175 14.5.1 | Description | edited | 0.84 | 0 | 0 |
| [5.14.4.2](../spec/5/5.14.4.2.md) | Use case diagram | informative | TS 104 175 14.5.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.14.4.3](../spec/5/5.14.4.3.md) | Input data | implemented | TS 104 175 14.5.3 | Input data | edited | 1.36 | 2 | 2 |
| [5.14.4.4](../spec/5/5.14.4.4.md) | Behaviour | implemented | TS 104 175 14.5.4 | Behaviour | edited | 1.38 | 5 | 4 |
| [5.14.4.5](../spec/5/5.14.4.5.md) | Output data | implemented | TS 104 175 14.5.5 | Output data | edited | 1.19 | 1 | 1 |
| [5.14.4](../spec/5/5.14.4.md) | Create EntityMap for Query Entities | informative |   |  | heading | 0.00 |  |  |
| [5.14.5.1](../spec/5/5.14.5.1.md) | Description | informative | TS 104 175 14.6.1 | Description | edited | 0.88 | 0 | 0 |
| [5.14.5.2](../spec/5/5.14.5.2.md) | Use case diagram | informative | TS 104 175 14.6.2 | Use case diagram | edited | 1.36 | 0 | 0 |
| [5.14.5.3](../spec/5/5.14.5.3.md) | Input data | implemented | TS 104 175 14.6.3 | Input data | edited | 1.36 | 1 | 1 |
| [5.14.5.4](../spec/5/5.14.5.4.md) | Behaviour | implemented | TS 104 175 14.6.4 | Behaviour | moved | 1.41 | 10 | 9 |
| [5.14.5.5](../spec/5/5.14.5.5.md) | Output Data | implemented | TS 104 175 14.5.5 | Output data | edited | 1.27 | 1 | 1 |
| [5.14.5](../spec/5/5.14.5.md) | Create EntityMap for Query Temporal Evolution of Entities | informative |   |  | heading | 0.00 |  |  |
| [5.14](../spec/5/5.14.md) | Context Source Entity Mapping | informative |   |  | heading | 0.00 |  |  |
| [5.15.1.1](../spec/5/5.15.1.1.md) | Description | informative | TS 104 175 15.2.1 | Description | edited | 0.84 | 0 | 0 |
| [5.15.1.2](../spec/5/5.15.1.2.md) | Use case diagram | informative | TS 104 175 15.2.2 | Use case diagram | edited | 1.32 | 0 | 0 |
| [5.15.1.3](../spec/5/5.15.1.3.md) | Input data | implemented | TS 104 175 12.2.3.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.15.1.4](../spec/5/5.15.1.4.md) | Behaviour | implemented | TS 104 175 15.2.4 | Behaviour | edited | 1.39 | 0 | 0 |
| [5.15.1.5](../spec/5/5.15.1.5.md) | Output data | implemented | TS 104 175 15.2.5 | Output data | edited | 1.32 | 0 | 0 |
| [5.15.1](../spec/5/5.15.1.md) | Retrieve Context Source Identity Information | informative | TS 104 175 15.2 | Retrieve Context Source identity information | edited | 1.00 | 0 | 0 |
| [5.15](../spec/5/5.15.md) | Context Source Identity Information | informative |   |  | heading | 0.00 |  |  |
| [5.16.1.1](../spec/5/5.16.1.1.md) | Description | informative | TS 104 175 10.2.2.1 | Description | edited | 0.60 | 0 | 0 |
| [5.16.1.2](../spec/5/5.16.1.2.md) | Use case diagram | informative | TS 104 175 16.2.2 | Use case diagram | edited | 1.02 | 0 | 0 |
| [5.16.1.3](../spec/5/5.16.1.3.md) | Input data | implemented | TS 104 175 16.2.3 | Input data | edited | 1.31 | 0 | 0 |
| [5.16.1.4](../spec/5/5.16.1.4.md) | Behaviour | implemented | TS 104 175 16.2.4 | Behaviour | moved | 1.44 | 3 | 3 |
| [5.16.1.5](../spec/5/5.16.1.5.md) | Output data | implemented | TS 104 175 16.2.5 | Output data | edited | 1.23 | 0 | 0 |
| [5.16.1](../spec/5/5.16.1.md) | Create Snapshot | informative | TS 104 175 16.2 | Create Snapshot | edited | 1.00 | 0 | 0 |
| [5.16.2.1](../spec/5/5.16.2.1.md) | Description | informative | TS 104 175 16.3.1 | Description | edited | 0.80 | 0 | 0 |
| [5.16.2.2](../spec/5/5.16.2.2.md) | Use case diagram | informative | TS 104 175 16.3.2 | Use case diagram | edited | 1.28 | 0 | 0 |
| [5.16.2.3](../spec/5/5.16.2.3.md) | Input data | implemented | TS 104 175 16.3.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.16.2.4](../spec/5/5.16.2.4.md) | Behaviour | implemented | TS 104 175 16.3.4 | Behaviour | moved | 1.45 | 1 | 1 |
| [5.16.2.5](../spec/5/5.16.2.5.md) | Output data | implemented | TS 104 175 16.2.5 | Output data | moved | 1.41 | 0 | 0 |
| [5.16.2](../spec/5/5.16.2.md) | Clone Snapshot | informative | TS 104 175 16.3 | Clone Snapshot | edited | 1.00 | 0 | 0 |
| [5.16.3.1](../spec/5/5.16.3.1.md) | Description | informative | TS 104 175 16.4.1 | Description | edited | 0.83 | 0 | 0 |
| [5.16.3.2](../spec/5/5.16.3.2.md) | Use case diagram | informative | TS 104 175 16.4.2 | Use case diagram | edited | 1.29 | 0 | 0 |
| [5.16.3.3](../spec/5/5.16.3.3.md) | Input data | implemented | TS 104 175 16.4.3 | Input data | edited | 1.31 | 0 | 0 |
| [5.16.3.4](../spec/5/5.16.3.4.md) | Behaviour | implemented | TS 104 175 16.4.4 | Behaviour | moved | 1.45 | 0 | 0 |
| [5.16.3.5](../spec/5/5.16.3.5.md) | Output data | implemented | TS 104 175 16.4.5 | Output data | edited | 1.29 | 0 | 0 |
| [5.16.3](../spec/5/5.16.3.md) | Retrieve Snapshot Status | informative | TS 104 175 16.4 | Retrieve Snapshot status | edited | 1.00 | 0 | 0 |
| [5.16.4.1](../spec/5/5.16.4.1.md) | Description | informative | TS 104 175 16.5.1 | Description | edited | 0.70 | 0 | 0 |
| [5.16.4.2](../spec/5/5.16.4.2.md) | Use case diagram | informative | TS 104 175 16.5.2 | Use case diagram | edited | 1.28 | 0 | 0 |
| [5.16.4.3](../spec/5/5.16.4.3.md) | Input data | implemented | TS 104 175 16.5.3 | Input data | edited | 1.17 | 0 | 0 |
| [5.16.4.4](../spec/5/5.16.4.4.md) | Behaviour | implemented | TS 104 175 16.5.4 | Behaviour | edited | 1.37 | 3 | 3 |
| [5.16.4.5](../spec/5/5.16.4.5.md) | Output data | implemented | TS 104 175 16.4.5 | Output data | edited | 1.39 | 0 | 0 |
| [5.16.4](../spec/5/5.16.4.md) | Update Snapshot Status | informative | TS 104 175 16.5 | Update Snapshot status | edited | 1.00 | 0 | 0 |
| [5.16.5.1](../spec/5/5.16.5.1.md) | Description | informative | TS 104 175 16.6.1 | Description | edited | 0.70 | 0 | 0 |
| [5.16.5.2](../spec/5/5.16.5.2.md) | Use case diagram | informative | TS 104 175 16.6.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.16.5.3](../spec/5/5.16.5.3.md) | Input data | implemented | TS 104 175 16.6.3 | Input data | edited | 1.23 | 0 | 0 |
| [5.16.5.4](../spec/5/5.16.5.4.md) | Behaviour | implemented | TS 104 175 16.6.4 | Behaviour | moved | 1.44 | 0 | 0 |
| [5.16.5.5](../spec/5/5.16.5.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.16.5](../spec/5/5.16.5.md) | Delete Snapshot | informative | TS 104 175 16.6 | Delete Snapshot | edited | 1.00 | 0 | 0 |
| [5.16.6](../spec/5/5.16.6.md) | Snapshot status notification behaviour | implemented | TS 104 175 16.8 | Snapshot status notification behaviour | moved | 1.45 | 1 | 1 |
| [5.16.7.1](../spec/5/5.16.7.1.md) | Description | informative |   |  | heading | 0.00 |  |  |
| [5.16.7.2](../spec/5/5.16.7.2.md) | Use case diagram | informative | TS 104 175 16.7.2 | Use case diagram | edited | 1.36 | 0 | 0 |
| [5.16.7.3](../spec/5/5.16.7.3.md) | Input data | implemented | TS 104 175 16.7.3 | Input data | edited | 1.32 | 0 | 0 |
| [5.16.7.4](../spec/5/5.16.7.4.md) | Behaviour | implemented | TS 104 175 16.7.4 | Behaviour | edited | 1.39 | 1 | 1 |
| [5.16.7.5](../spec/5/5.16.7.5.md) | Output data | implemented | TS 104 176 5 | Global definitions and resource structure | edited | 0.73 | 3 | 6 |
| [5.16.7](../spec/5/5.16.7.md) | Purge Snapshots | informative | TS 104 175 16.7 | Purge Snapshots | edited | 1.00 | 0 | 0 |
| [5.16](../spec/5/5.16.md) | Snapshot Functionality | informative |   |  | heading | 0.00 |  |  |
| [5.2.1](../spec/5/5.2.1.md) | Introduction | implemented | TS 104 175 5.2.6.1 | Introduction | edited | 0.87 | 3 | 3 |
| [5.2.10](../spec/5/5.2.10.md) | RegistrationInfo | implemented | TS 104 175 5.2.6.6.5 | RegistrationInfo | edited | 1.23 | 4 | 5 |
| [5.2.11](../spec/5/5.2.11.md) | TimeInterval | implemented | TS 104 175 5.2.6.7.1 | TimeIntervalParams | edited | 0.64 | 1 | 1 |
| [5.2.12](../spec/5/5.2.12.md) | Subscription | implemented | TS 104 175 5.2.6.5.2 | Subscription | edited | 1.36 | 10 | 18 |
| [5.2.13](../spec/5/5.2.13.md) | GeoQuery | implemented | TS 104 175 5.2.6.7.2 | GeoParams | edited | 0.81 | 1 | 1 |
| [5.2.14.1](../spec/5/5.2.14.1.md) | NotificationParams data type definition | implemented | TS 104 175 5.2.6.7.3 | NotificationParams | edited | 0.87 | 4 | 13 |
| [5.2.14.2](../spec/5/5.2.14.2.md) | Output only members | implemented | TS 104 175 5.2.6.7.3 | NotificationParams | edited | 0.91 | 1 | 8 |
| [5.2.14](../spec/5/5.2.14.md) | NotificationParams | informative | TS 104 175 5.2.6.7.3 | NotificationParams | edited | 1.00 | 0 | 13 |
| [5.2.15](../spec/5/5.2.15.md) | Endpoint | implemented | TS 104 175 5.2.6.6.1 | Endpoint | edited | 1.38 | 3 | 3 |
| [5.2.16](../spec/5/5.2.16.md) | BatchOperationResult | implemented | TS 104 175 5.2.6.8.1 | OperationResult | edited | 0.71 | 1 | 1 |
| [5.2.17](../spec/5/5.2.17.md) | BatchEntityError | implemented | TS 104 175 5.2.6.8.2 | EntityError | edited | 0.80 | 1 | 1 |
| [5.2.18](../spec/5/5.2.18.md) | UpdateResult | implemented | TS 104 175 5.2.6.8.5 | UpdateResult | edited | 1.30 | 1 | 1 |
| [5.2.19](../spec/5/5.2.19.md) | NotUpdatedDetails | implemented | TS 104 175 5.2.6.8.4 | NotUpdatedResult | edited | 0.78 | 1 | 1 |
| [5.2.2](../spec/5/5.2.2.md) | Common members | implemented | TS 104 175 5.2.6.2 | Common members | edited | 1.23 | 1 | 1 |
| [5.2.20](../spec/5/5.2.20.md) | EntityTemporal | implemented | TS 104 175 5.2.6.4.2 | EntityTemporal | edited | 1.24 | 1 | 1 |
| [5.2.21](../spec/5/5.2.21.md) | TemporalQuery | implemented | TS 104 175 5.2.6.7.4 | TemporalParams | edited | 0.82 | 3 | 3 |
| [5.2.22](../spec/5/5.2.22.md) | KeyValuePair | implemented | TS 104 175 5.2.6.6.4 | KeyValuePair | moved | 1.40 | 1 | 1 |
| [5.2.23](../spec/5/5.2.23.md) | Query | implemented | TS 104 175 5.2.6.5.1 | Query | edited | 1.35 | 9 | 8 |
| [5.2.24](../spec/5/5.2.24.md) | EntityTypeList | implemented | TS 104 175 5.2.6.10.5 | EntityTypeList | edited | 1.25 | 1 | 1 |
| [5.2.25](../spec/5/5.2.25.md) | EntityType | implemented | TS 104 175 5.2.6.10.3 | EntityType | edited | 1.27 | 2 | 2 |
| [5.2.26](../spec/5/5.2.26.md) | EntityTypeInfo | implemented | TS 104 175 5.2.6.10.4 | EntityTypeInfo | edited | 1.27 | 2 | 2 |
| [5.2.27](../spec/5/5.2.27.md) | AttributeList | implemented | TS 104 175 5.2.6.10.2 | AttributeList | edited | 1.25 | 1 | 1 |
| [5.2.28](../spec/5/5.2.28.md) | Attribute | implemented | TS 104 175 5.2.6.10.1 | Attribute | edited | 1.28 | 2 | 2 |
| [5.2.29](../spec/5/5.2.29.md) | Feature | implemented | TS 104 175 5.2.6.11.1 | Feature | edited | 1.27 | 2 | 2 |
| [5.2.3](../spec/5/5.2.3.md) | @context | implemented | TS 104 175 5.2.6.3 | @context | edited | 1.39 | 1 | 1 |
| [5.2.30](../spec/5/5.2.30.md) | FeatureCollection | implemented | TS 104 175 5.2.6.11.2 | FeatureCollection | edited | 1.33 | 2 | 2 |
| [5.2.31](../spec/5/5.2.31.md) | FeatureProperties | implemented | TS 104 175 5.2.6.11.3 | FeatureProperties | edited | 0.95 | 0 | 1 |
| [5.2.32](../spec/5/5.2.32.md) | LanguageProperty | implemented | TS 104 175 5.2.6.4.6 | LanguageProperty | edited | 1.20 | 6 | 11 |
| [5.2.33](../spec/5/5.2.33.md) | EntitySelector | implemented | TS 104 175 5.2.6.6.3 | EntitySelector | edited | 1.38 | 2 | 2 |
| [5.2.34](../spec/5/5.2.34.md) | RegistrationManagementInfo | implemented | TS 104 175 5.2.6.6.6 | RegistrationManagementInfo | edited | 1.39 | 3 | 3 |
| [5.2.35](../spec/5/5.2.35.md) | VocabProperty | implemented | TS 104 175 5.2.6.4.7 | VocabProperty | edited | 1.20 | 4 | 7 |
| [5.2.36](../spec/5/5.2.36.md) | ListProperty | implemented | TS 104 175 5.2.6.4.8 | ListProperty | edited | 1.18 | 4 | 6 |
| [5.2.37](../spec/5/5.2.37.md) | ListRelationship | implemented | TS 104 175 5.2.6.4.9 | ListRelationship | edited | 1.18 | 4 | 7 |
| [5.2.38](../spec/5/5.2.38.md) | JsonProperty | implemented | TS 104 175 5.2.6.4.10 | JsonProperty | edited | 1.20 | 4 | 7 |
| [5.2.39](../spec/5/5.2.39.md) | EntityMap | implemented | TS 104 175 5.2.6.5.5 | EntityMap | edited | 1.32 | 4 | 4 |
| [5.2.4](../spec/5/5.2.4.md) | Entity | implemented | TS 104 175 5.2.6.4.1 | Entity | edited | 0.96 | 1 | 3 |
| [5.2.40](../spec/5/5.2.40.md) | Context Source Identity | implemented | TS 104 175 5.2.6.5.6 | ContextSourceIdentity | edited | 0.75 | 4 | 4 |
| [5.2.41](../spec/5/5.2.41.md) | Snapshot | implemented | TS 104 175 5.2.6.5.4 | Snapshot | edited | 1.34 | 7 | 7 |
| [5.2.42](../spec/5/5.2.42.md) | ExecutionResultDetails | implemented | TS 104 175 5.2.6.8.3 | ExecutionResult | edited | 0.80 | 2 | 2 |
| [5.2.43](../spec/5/5.2.43.md) | OrderingParams | implemented | TS 104 175 5.2.6.7.5 | OrderingParams | edited | 1.37 | 4 | 4 |
| [5.2.44](../spec/5/5.2.44.md) | AggregationParams | implemented | TS 104 175 5.2.6.7.6 | AggregationParams | edited | 1.34 | 1 | 1 |
| [5.2.5](../spec/5/5.2.5.md) | Property | implemented | TS 104 175 5.2.6.4.3 | Property | edited | 1.25 | 5 | 15 |
| [5.2.6](../spec/5/5.2.6.md) | Relationship | implemented | TS 104 175 5.2.6.4.4 | Relationship | edited | 1.24 | 3 | 13 |
| [5.2.7](../spec/5/5.2.7.md) | GeoProperty | implemented | TS 104 175 5.2.6.4.5 | GeoProperty | edited | 1.23 | 4 | 14 |
| [5.2.8](../spec/5/5.2.8.md) | EntityInfo | implemented | TS 104 175 5.2.6.6.2 | EntityInfo | edited | 1.36 | 2 | 2 |
| [5.2.9](../spec/5/5.2.9.md) | CSourceRegistration | implemented | TS 104 175 5.2.6.5.3 | CSourceRegistration | edited | 1.38 | 10 | 9 |
| [5.2](../spec/5/5.2.md) | Data Types | informative | TS 104 175 5.2.6 | Data types | edited | 1.00 | 0 | 0 |
| [5.3.1](../spec/5/5.3.1.md) | Notification | implemented | TS 104 175 5.2.6.9.1 | Notification | edited | 1.36 | 2 | 2 |
| [5.3.2](../spec/5/5.3.2.md) | CSourceNotification | implemented | TS 104 175 5.2.6.9.2 | CSourceNotification | edited | 1.25 | 1 | 2 |
| [5.3.3](../spec/5/5.3.3.md) | TriggerReasonEnumeration | implemented | TS 104 175 5.2.6.9.4 | TriggerReasonEnumeration | moved | 1.47 | 0 | 0 |
| [5.3.4](../spec/5/5.3.4.md) | SnapshotNotification | implemented | TS 104 175 5.2.6.9.3 | SnapshotNotification | edited | 1.12 | 5 | 4 |
| [5.3](../spec/5/5.3.md) | Notification data types | informative |   |  | heading | 0.00 |  |  |
| [5.4](../spec/5/5.4.md) | NGSI-LD Fragments | implemented | TS 104 175 5.4.1 | Element fragments | edited | 0.81 | 1 | 2 |
| [5.5.1](../spec/5/5.5.1.md) | Introduction | implemented | TS 104 175 8.1 | Introduction | edited | 0.81 | 1 | 1 |
| [5.5.10](../spec/5/5.5.10.md) | Multi-Tenant Behaviour | implemented | TS 104 175 7.8.2 | Multi-Tenant Behaviour | edited | 1.36 | 1 | 1 |
| [5.5.11.0](../spec/5/5.5.11.0.md) | Foreword | implemented | TS 104 175 10.3.1 | Introduction | edited | 0.77 | 0 | 0 |
| [5.5.11.1](../spec/5/5.5.11.1.md) | Batch Entity Creation case | implemented |   |  | unmatched | 0.16 |  |  |
| [5.5.11.2](../spec/5/5.5.11.2.md) | Batch Entity Creation or Update (Upsert) case | implemented |   |  | unmatched | 0.07 |  |  |
| [5.5.11.3](../spec/5/5.5.11.3.md) | Batch Entity Update case | implemented |   |  | unmatched | 0.05 |  |  |
| [5.5.11.4](../spec/5/5.5.11.4.md) | Batch Entity Delete case | implemented |   |  | unmatched | 0.15 |  |  |
| [5.5.11.5](../spec/5/5.5.11.5.md) | Batch Entity Merge case | implemented |   |  | unmatched | 0.08 |  |  |
| [5.5.11](../spec/5/5.5.11.md) | More than one instance of the same Entity in an Entity array | informative | TS 104 175 10.3.1 | Introduction | weak | 0.46 | 0 | 2 |
| [5.5.12](../spec/5/5.5.12.md) | Merge Patch Behaviour | implemented | TS 104 175 8.4.3 | Merge patch behaviour | moved | 1.45 | 2 | 2 |
| [5.5.13](../spec/5/5.5.13.md) | Limiting operations to local scope | implemented | TS 104 175 8.6 | Limiting operations to local scope | edited | 1.40 | 1 | 1 |
| [5.5.14](../spec/5/5.5.14.md) | Distributed Transactional Behaviour | implemented | TS 104 175 9.6 | Querying and retrieving distributed Entities as unitary operations - Entity Maps | edited | 0.76 | 8 | 10 |
| [5.5.15](../spec/5/5.5.15.md) | Snapshot Behaviour | implemented | TS 104 175 7.9.2 | Snapshot Behaviour | edited | 1.39 | 1 | 1 |
| [5.5.2](../spec/5/5.5.2.md) | Error types | implemented | TS 104 175 8.3.2 | Error types | moved | 1.45 | 1 | 1 |
| [5.5.3](../spec/5/5.5.3.md) | Error response payload body | implemented | TS 104 175 8.3.3 | Error response payload body | moved | 1.42 | 2 | 2 |
| [5.5.4](../spec/5/5.5.4.md) | General NGSI-LD validation | implemented | TS 104 175 8.2.3 | General NGSI-LD validation | moved | 1.41 | 5 | 5 |
| [5.5.5](../spec/5/5.5.5.md) | Default @context assignment | implemented | TS 104 175 8.2.2 | Default @context assignment | moved | 1.45 | 1 | 1 |
| [5.5.6](../spec/5/5.5.6.md) | Operation execution and generic error handling | implemented | TS 104 175 8.3.4 | Operation execution and generic error handling | moved | 1.47 | 2 | 2 |
| [5.5.7](../spec/5/5.5.7.md) | Term to URI expansion or compaction | implemented | TS 104 175 8.2.4 | Term to URI expansion or compaction | moved | 1.41 | 3 | 3 |
| [5.5.8](../spec/5/5.5.8.md) | Partial Update Patch Behaviour | implemented | TS 104 175 8.4.2 | Partial update patch behaviour | moved | 1.45 | 2 | 2 |
| [5.5.9.1](../spec/5/5.5.9.1.md) | General Pagination Behaviour | implemented | TS 104 175 7.4.2.2 | General pagination behaviour | moved | 1.49 | 1 | 1 |
| [5.5.9.2](../spec/5/5.5.9.2.md) | Pagination option using limit and offset | implemented | TS 104 175 7.4.2.3 | Pagination option using limit and offset | edited | 1.40 | 0 | 0 |
| [5.5.9.3](../spec/5/5.5.9.3.md) | Pagination with Entity maps | implemented | TS 104 175 7.4.2.4 | Pagination with Entity maps | moved | 1.48 | 0 | 0 |
| [5.5.9](../spec/5/5.5.9.md) | Pagination Behaviour | informative |   |  | heading | 0.00 |  |  |
| [5.5](../spec/5/5.5.md) | Common Behaviours | informative | TS 104 176 6 | Common behaviours | edited | 1.00 | 0 | 0 |
| [5.6.1.1](../spec/5/5.6.1.1.md) | Description | informative | TS 104 175 10.2.2.1 | Description | edited | 0.75 | 0 | 0 |
| [5.6.1.2](../spec/5/5.6.1.2.md) | Use case diagram | informative | TS 104 175 10.2.2.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.6.1.3](../spec/5/5.6.1.3.md) | Input data | implemented | TS 104 175 10.2.2.3 | Input data | edited | 1.30 | 0 | 0 |
| [5.6.1.4](../spec/5/5.6.1.4.md) | Behaviour | implemented | TS 104 175 10.2.2.4 | Behaviour | moved | 1.42 | 3 | 3 |
| [5.6.1.5](../spec/5/5.6.1.5.md) | Output data | implemented | TS 104 175 10.2.2.5 | Output data | edited | 1.23 | 0 | 0 |
| [5.6.1](../spec/5/5.6.1.md) | Create Entity | informative | TS 104 175 10.2.2 | Create Entity | edited | 1.00 | 0 | 0 |
| [5.6.10.1](../spec/5/5.6.10.1.md) | Description | informative | TS 104 175 10.3.6.1 | Description | edited | 0.77 | 0 | 0 |
| [5.6.10.2](../spec/5/5.6.10.2.md) | Use case diagram | informative | TS 104 175 10.3.6.2 | Use case diagram | edited | 1.28 | 0 | 0 |
| [5.6.10.3](../spec/5/5.6.10.3.md) | Input data | implemented | TS 104 175 10.3.6.3 | Input data | edited | 0.88 | 0 | 0 |
| [5.6.10.4](../spec/5/5.6.10.4.md) | Behaviour | implemented | TS 104 175 10.3.6.4 | Behaviour | moved | 1.46 | 3 | 3 |
| [5.6.10.5](../spec/5/5.6.10.5.md) | Output data | implemented | TS 104 175 10.3.6.5 | Output data | moved | 1.43 | 0 | 0 |
| [5.6.10](../spec/5/5.6.10.md) | Batch Entity Delete | informative | TS 104 175 10.3.6 | Batch Entity delete | edited | 1.00 | 0 | 0 |
| [5.6.11.1](../spec/5/5.6.11.1.md) | Description | informative | TS 104 175 11.2.2.1 | Description | edited | 0.85 | 0 | 0 |
| [5.6.11.2](../spec/5/5.6.11.2.md) | Use case diagram | informative | TS 104 175 11.2.2.2 | Use case diagram | edited | 1.37 | 0 | 0 |
| [5.6.11.3](../spec/5/5.6.11.3.md) | Input data | implemented | TS 104 175 11.2.2.3 | Input data | edited | 1.32 | 0 | 0 |
| [5.6.11.4](../spec/5/5.6.11.4.md) | Behaviour | implemented | TS 104 175 11.2.2.4 | Behaviour | moved | 1.44 | 4 | 4 |
| [5.6.11.5](../spec/5/5.6.11.5.md) | Output data | implemented | TS 104 175 11.2.2.5 | Output data | edited | 1.34 | 0 | 0 |
| [5.6.11](../spec/5/5.6.11.md) | Create or Update (Upsert) Temporal Evolution of an Entity | informative | TS 104 175 11.2.2 | Create or update (upsert) Temporal Evolution of an Entity | edited | 1.20 | 0 | 0 |
| [5.6.12.1](../spec/5/5.6.12.1.md) | Description | informative | TS 104 175 11.2.3.1 | Description | edited | 0.83 | 0 | 0 |
| [5.6.12.2](../spec/5/5.6.12.2.md) | Use case diagram | informative | TS 104 175 11.2.3.2 | Use case diagram | edited | 1.34 | 0 | 0 |
| [5.6.12.3](../spec/5/5.6.12.3.md) | Input data | implemented | TS 104 175 11.2.3.3 | Input data | moved | 1.43 | 0 | 0 |
| [5.6.12.4](../spec/5/5.6.12.4.md) | Behaviour | implemented | TS 104 175 11.2.3.4 | Behaviour | moved | 1.41 | 3 | 4 |
| [5.6.12.5](../spec/5/5.6.12.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.12](../spec/5/5.6.12.md) | Add Attributes to Temporal Evolution of an Entity | informative | TS 104 175 11.2.3 | Add Attributes to Temporal Evolution of an Entity | edited | 1.00 | 0 | 0 |
| [5.6.13.1](../spec/5/5.6.13.1.md) | Description | informative | TS 104 175 11.2.4.1 | Description | edited | 0.75 | 0 | 0 |
| [5.6.13.2](../spec/5/5.6.13.2.md) | Use case diagram | informative | TS 104 175 11.2.4.2 | Use case diagram | edited | 1.33 | 0 | 0 |
| [5.6.13.3](../spec/5/5.6.13.3.md) | Input data | implemented | TS 104 175 11.2.4.3 | Input data | edited | 1.17 | 0 | 0 |
| [5.6.13.4](../spec/5/5.6.13.4.md) | Behaviour | implemented | TS 104 175 11.2.4.4 | Behaviour | moved | 1.47 | 2 | 2 |
| [5.6.13.5](../spec/5/5.6.13.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.13](../spec/5/5.6.13.md) | Delete Attribute from Temporal Evolution of an Entity | informative | TS 104 175 11.2.4 | Delete Attribute from Temporal Evolution of an Entity | edited | 1.00 | 0 | 0 |
| [5.6.14.1](../spec/5/5.6.14.1.md) | Description | informative | TS 104 175 11.2.5.1 | Description | edited | 0.81 | 0 | 0 |
| [5.6.14.2](../spec/5/5.6.14.2.md) | Use case diagram | informative | TS 104 175 11.2.5.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.6.14.3](../spec/5/5.6.14.3.md) | Input data | implemented | TS 104 175 11.2.5.3 | Input data | edited | 1.39 | 0 | 0 |
| [5.6.14.4](../spec/5/5.6.14.4.md) | Behaviour | implemented | TS 104 175 11.2.5.4 | Behaviour | moved | 1.47 | 2 | 2 |
| [5.6.14.5](../spec/5/5.6.14.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.14](../spec/5/5.6.14.md) | Modify Attribute instance in Temporal Evolution of an Entity | informative | TS 104 175 11.2.5 | Modify Attribute instance in Temporal Evolution of an Entity | edited | 1.20 | 0 | 0 |
| [5.6.15.1](../spec/5/5.6.15.1.md) | Description | informative | TS 104 175 11.2.6.1 | Description | edited | 0.85 | 0 | 0 |
| [5.6.15.2](../spec/5/5.6.15.2.md) | Use case diagram | informative | TS 104 175 11.2.6.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.6.15.3](../spec/5/5.6.15.3.md) | Input data | implemented | TS 104 175 11.2.6.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.6.15.4](../spec/5/5.6.15.4.md) | Behaviour | implemented | TS 104 175 11.2.6.4 | Behaviour | moved | 1.45 | 4 | 2 |
| [5.6.15.5](../spec/5/5.6.15.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.15](../spec/5/5.6.15.md) | Delete Attribute instance from Temporal Evolution of an Entity | informative | TS 104 175 11.2.6 | Delete Attribute instance from Temporal Evolution of an Entity | edited | 1.20 | 0 | 0 |
| [5.6.16.1](../spec/5/5.6.16.1.md) | Description | informative | TS 104 175 11.2.7.1 | Description | edited | 0.77 | 0 | 0 |
| [5.6.16.2](../spec/5/5.6.16.2.md) | Use case diagram | informative | TS 104 175 11.2.7.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.6.16.3](../spec/5/5.6.16.3.md) | Input data | implemented | TS 104 175 11.2.4.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.6.16.4](../spec/5/5.6.16.4.md) | Behaviour | implemented | TS 104 175 11.2.7.4 | Behaviour | moved | 1.48 | 1 | 1 |
| [5.6.16.5](../spec/5/5.6.16.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.16](../spec/5/5.6.16.md) | Delete Temporal Evolution of an Entity | informative | TS 104 175 11.2.7 | Delete Temporal Evolution of an Entity | edited | 1.00 | 0 | 0 |
| [5.6.17.1](../spec/5/5.6.17.1.md) | Description | informative | TS 104 175 10.2.9.1 | Description | edited | 0.87 | 0 | 0 |
| [5.6.17.2](../spec/5/5.6.17.2.md) | Use case diagram | informative | TS 104 175 10.2.9.2 | Use case diagram | edited | 1.29 | 0 | 0 |
| [5.6.17.3](../spec/5/5.6.17.3.md) | Input data | implemented | TS 104 175 10.2.9.3 | Input data | edited | 1.38 | 0 | 0 |
| [5.6.17.4](../spec/5/5.6.17.4.md) | Behaviour | implemented | TS 104 175 10.2.9.4 | Behaviour | moved | 1.42 | 5 | 6 |
| [5.6.17.5](../spec/5/5.6.17.5.md) | Output data | implemented | TS 104 175 10.2.9.5 | Output data | edited | 1.38 | 0 | 0 |
| [5.6.17](../spec/5/5.6.17.md) | Merge Entity | informative | TS 104 175 10.2.9 | Merge Entity | edited | 1.00 | 0 | 0 |
| [5.6.18.1](../spec/5/5.6.18.1.md) | Description | informative | TS 104 175 10.2.10.1 | Description | edited | 0.87 | 0 | 0 |
| [5.6.18.2](../spec/5/5.6.18.2.md) | Use case diagram | informative | TS 104 175 10.2.10.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.6.18.3](../spec/5/5.6.18.3.md) | Input data | implemented | TS 104 175 10.2.10.3 | Input data | edited | 1.32 | 0 | 0 |
| [5.6.18.4](../spec/5/5.6.18.4.md) | Behaviour | implemented | TS 104 175 10.2.10.4 | Behaviour | edited | 1.38 | 2 | 3 |
| [5.6.18.5](../spec/5/5.6.18.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.18](../spec/5/5.6.18.md) | Replace Entity | informative | TS 104 175 10.2.10 | Replace Entity | edited | 1.00 | 0 | 0 |
| [5.6.19.1](../spec/5/5.6.19.1.md) | Description | informative | TS 104 175 10.2.11.1 | Description | edited | 0.85 | 0 | 0 |
| [5.6.19.2](../spec/5/5.6.19.2.md) | Use case diagram | informative | TS 104 175 10.2.11.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.6.19.3](../spec/5/5.6.19.3.md) | Input data | implemented | TS 104 175 10.2.11.3 | Input data | edited | 1.25 | 0 | 0 |
| [5.6.19.4](../spec/5/5.6.19.4.md) | Behaviour | implemented | TS 104 175 10.2.11.4 | Behaviour | moved | 1.40 | 3 | 4 |
| [5.6.19.5](../spec/5/5.6.19.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.19](../spec/5/5.6.19.md) | Replace Attribute | informative | TS 104 175 10.2.11 | Replace Attribute | edited | 1.00 | 0 | 0 |
| [5.6.2.1](../spec/5/5.6.2.1.md) | Description | informative | TS 104 175 10.2.3.1 | Description | edited | 0.88 | 0 | 0 |
| [5.6.2.2](../spec/5/5.6.2.2.md) | Use case diagram | informative | TS 104 175 10.2.3.2 | Use case diagram | edited | 1.26 | 0 | 0 |
| [5.6.2.3](../spec/5/5.6.2.3.md) | Input data | implemented | TS 104 175 10.2.3.3 | Input data | edited | 1.32 | 0 | 0 |
| [5.6.2.4](../spec/5/5.6.2.4.md) | Behaviour | implemented | TS 104 175 10.2.3.4 | Behaviour | moved | 1.41 | 6 | 6 |
| [5.6.2.5](../spec/5/5.6.2.5.md) | Output data | implemented | TS 104 175 10.2.3.5 | Output data | edited | 1.39 | 0 | 0 |
| [5.6.2](../spec/5/5.6.2.md) | Update Attributes | informative | TS 104 175 10.2.3 | Update Attributes | edited | 1.00 | 0 | 0 |
| [5.6.20.1](../spec/5/5.6.20.1.md) | Description | informative | TS 104 175 10.3.5.1 | Description | edited | 0.87 | 0 | 0 |
| [5.6.20.2](../spec/5/5.6.20.2.md) | Use case diagram | informative | TS 104 175 10.3.5.2 | Use case diagram | edited | 1.28 | 0 | 0 |
| [5.6.20.3](../spec/5/5.6.20.3.md) | Input data | implemented | TS 104 175 10.3.3.3 | Input data | edited | 0.98 | 0 | 2 |
| [5.6.20.4](../spec/5/5.6.20.4.md) | Behaviour | implemented | TS 104 175 10.3.5.4 | Behaviour | moved | 1.44 | 3 | 3 |
| [5.6.20.5](../spec/5/5.6.20.5.md) | Output data | implemented | TS 104 175 10.3.5.5 | Output data | moved | 1.43 | 0 | 0 |
| [5.6.20](../spec/5/5.6.20.md) | Batch Entity Merge | informative | TS 104 175 10.3.5 | Batch Entity merge | edited | 1.00 | 0 | 0 |
| [5.6.21.1](../spec/5/5.6.21.1.md) | Description | informative | TS 104 175 10.2.12.1 | Description | edited | 0.84 | 0 | 0 |
| [5.6.21.2](../spec/5/5.6.21.2.md) | Use case diagram | informative | TS 104 175 10.2.12.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.6.21.3](../spec/5/5.6.21.3.md) | Input data | implemented | TS 104 175 10.2.12.3 | Input data | edited | 1.38 | 0 | 0 |
| [5.6.21.4](../spec/5/5.6.21.4.md) | Behaviour | implemented | TS 104 175 10.2.12.4 | Behaviour | moved | 1.40 | 6 | 6 |
| [5.6.21.5](../spec/5/5.6.21.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.21](../spec/5/5.6.21.md) | Purge Entities | informative | TS 104 175 10.2.12 | Purge Entities | edited | 1.00 | 0 | 0 |
| [5.6.3.1](../spec/5/5.6.3.1.md) | Description | informative | TS 104 175 10.2.4.1 | Description | edited | 0.83 | 0 | 0 |
| [5.6.3.2](../spec/5/5.6.3.2.md) | Use case diagram | informative | TS 104 175 10.2.4.2 | Use case diagram | edited | 1.29 | 0 | 0 |
| [5.6.3.3](../spec/5/5.6.3.3.md) | Input data | implemented | TS 104 175 10.2.4.3 | Input data | edited | 1.34 | 0 | 0 |
| [5.6.3.4](../spec/5/5.6.3.4.md) | Behaviour | implemented | TS 104 175 10.2.4.4 | Behaviour | moved | 1.41 | 6 | 7 |
| [5.6.3.5](../spec/5/5.6.3.5.md) | Output data | implemented | TS 104 175 10.2.4.5 | Output data | edited | 1.40 | 0 | 0 |
| [5.6.3](../spec/5/5.6.3.md) | Append Attributes | informative | TS 104 175 10.2.4 | Append Attributes | edited | 1.00 | 0 | 0 |
| [5.6.4.1](../spec/5/5.6.4.1.md) | Description | informative | TS 104 175 10.2.5.1 | Description | edited | 0.94 | 0 | 0 |
| [5.6.4.2](../spec/5/5.6.4.2.md) | Use case diagram | informative | TS 104 175 10.2.5.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.6.4.3](../spec/5/5.6.4.3.md) | Input data | implemented | TS 104 175 10.2.5.3 | Input data | edited | 1.25 | 0 | 0 |
| [5.6.4.4](../spec/5/5.6.4.4.md) | Behaviour | implemented | TS 104 175 10.2.5.4 | Behaviour | moved | 1.41 | 3 | 3 |
| [5.6.4.5](../spec/5/5.6.4.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.4](../spec/5/5.6.4.md) | Partial Attribute update | informative | TS 104 175 10.2.5 | Partial Attribute update | edited | 1.00 | 0 | 0 |
| [5.6.5.1](../spec/5/5.6.5.1.md) | Description | informative | TS 104 175 10.2.7.1 | Description | edited | 0.88 | 0 | 0 |
| [5.6.5.2](../spec/5/5.6.5.2.md) | Use case diagram | informative | TS 104 175 10.2.7.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.6.5.3](../spec/5/5.6.5.3.md) | Input data | implemented | TS 104 175 10.2.7.3 | Input data | edited | 1.35 | 0 | 0 |
| [5.6.5.4](../spec/5/5.6.5.4.md) | Behaviour | implemented | TS 104 175 10.2.7.4 | Behaviour | moved | 1.44 | 1 | 1 |
| [5.6.5.5](../spec/5/5.6.5.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.5](../spec/5/5.6.5.md) | Delete Attribute | informative | TS 104 175 10.2.7 | Delete Attribute | edited | 1.00 | 0 | 0 |
| [5.6.6.1](../spec/5/5.6.6.1.md) | Description | informative | TS 104 175 10.2.8.1 | Description | edited | 0.73 | 0 | 0 |
| [5.6.6.2](../spec/5/5.6.6.2.md) | Use case diagram | informative | TS 104 175 10.2.8.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.6.6.3](../spec/5/5.6.6.3.md) | Input data | implemented | TS 104 175 10.2.8.3 | Input data | edited | 1.29 | 0 | 0 |
| [5.6.6.4](../spec/5/5.6.6.4.md) | Behaviour | implemented | TS 104 175 10.2.8.4 | Behaviour | moved | 1.44 | 2 | 2 |
| [5.6.6.5](../spec/5/5.6.6.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.6.6](../spec/5/5.6.6.md) | Delete Entity | informative | TS 104 175 10.2.8 | Delete Entity | edited | 1.00 | 0 | 0 |
| [5.6.7.1](../spec/5/5.6.7.1.md) | Description | informative | TS 104 175 10.3.2.1 | Description | edited | 0.77 | 0 | 0 |
| [5.6.7.2](../spec/5/5.6.7.2.md) | Use case diagram | informative | TS 104 175 10.3.2.2 | Use case diagram | edited | 1.30 | 0 | 0 |
| [5.6.7.3](../spec/5/5.6.7.3.md) | Input data | implemented | TS 104 175 10.3.2.3 | Input data | edited | 0.96 | 0 | 0 |
| [5.6.7.4](../spec/5/5.6.7.4.md) | Behaviour | implemented | TS 104 175 10.3.2.4 | Behaviour | moved | 1.43 | 5 | 4 |
| [5.6.7.5](../spec/5/5.6.7.5.md) | Output data | implemented | TS 104 175 10.3.2.5 | Output data | moved | 1.42 | 0 | 0 |
| [5.6.7](../spec/5/5.6.7.md) | Batch Entity Creation | informative | TS 104 175 10.3.2 | Batch Entity creation | edited | 1.00 | 0 | 0 |
| [5.6.8.1](../spec/5/5.6.8.1.md) | Description | informative | TS 104 175 10.3.3.1 | Description | edited | 0.91 | 0 | 0 |
| [5.6.8.2](../spec/5/5.6.8.2.md) | Use case diagram | informative | TS 104 175 10.3.3.2 | Use case diagram | edited | 1.30 | 0 | 0 |
| [5.6.8.3](../spec/5/5.6.8.3.md) | Input data | implemented | TS 104 175 10.3.3.3 | Input data | edited | 1.15 | 0 | 0 |
| [5.6.8.4](../spec/5/5.6.8.4.md) | Behaviour | implemented | TS 104 175 10.3.3.4 | Behaviour | moved | 1.45 | 3 | 4 |
| [5.6.8.5](../spec/5/5.6.8.5.md) | Output data | implemented | TS 104 175 10.3.3.5 | Output data | moved | 1.45 | 0 | 0 |
| [5.6.8](../spec/5/5.6.8.md) | Batch Entity Creation or Update (Upsert) | informative | TS 104 175 10.3.3 | Batch Entity creation or update (upsert) | edited | 1.00 | 0 | 0 |
| [5.6.9.1](../spec/5/5.6.9.1.md) | Description | informative | TS 104 175 10.3.4.1 | Description | edited | 0.77 | 0 | 0 |
| [5.6.9.2](../spec/5/5.6.9.2.md) | Use case diagram | informative | TS 104 175 10.3.4.2 | Use case diagram | edited | 1.28 | 0 | 0 |
| [5.6.9.3](../spec/5/5.6.9.3.md) | Input data | implemented | TS 104 175 10.3.4.3 | Input data | edited | 1.15 | 0 | 0 |
| [5.6.9.4](../spec/5/5.6.9.4.md) | Behaviour | implemented | TS 104 175 10.3.4.4 | Behaviour | moved | 1.45 | 3 | 3 |
| [5.6.9.5](../spec/5/5.6.9.5.md) | Output data | implemented | TS 104 175 10.3.4.5 | Output data | moved | 1.42 | 0 | 0 |
| [5.6.9](../spec/5/5.6.9.md) | Batch Entity Update | informative | TS 104 175 10.3.4 | Batch Entity update | edited | 1.00 | 0 | 0 |
| [5.6](../spec/5/5.6.md) | Context Information Provision | informative | TS 104 175 10.2 | Context information provision | edited | 1.00 | 0 | 0 |
| [5.7.1.1](../spec/5/5.7.1.1.md) | Description | informative | TS 104 175 10.4.2.1 | Description | edited | 0.73 | 0 | 0 |
| [5.7.1.2](../spec/5/5.7.1.2.md) | Use case diagram | informative | TS 104 175 10.4.2.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.7.1.3](../spec/5/5.7.1.3.md) | Input data | implemented | TS 104 175 10.4.2.3 | Input data | edited | 1.34 | 0 | 0 |
| [5.7.1.4](../spec/5/5.7.1.4.md) | Behaviour | implemented | TS 104 175 10.4.2.4 | Behaviour | moved | 1.41 | 5 | 5 |
| [5.7.1.5](../spec/5/5.7.1.5.md) | Output data | implemented | TS 104 175 10.4.2.5 | Output data | moved | 1.43 | 3 | 3 |
| [5.7.1](../spec/5/5.7.1.md) | Retrieve Entity | informative | TS 104 175 10.4.2 | Retrieve Entity | edited | 1.00 | 0 | 0 |
| [5.7.10.1](../spec/5/5.7.10.1.md) | Description | informative | TS 104 175 10.6.8.1 | Description | edited | 0.96 | 0 | 0 |
| [5.7.10.2](../spec/5/5.7.10.2.md) | Use case diagram | informative | TS 104 175 10.6.8.2 | Use case diagram | edited | 1.32 | 0 | 0 |
| [5.7.10.3](../spec/5/5.7.10.3.md) | Input data | implemented | TS 104 175 10.6.8.3 | Input data | edited | 1.36 | 0 | 0 |
| [5.7.10.4](../spec/5/5.7.10.4.md) | Behaviour | implemented | TS 104 175 10.6.8.4 | Behaviour | edited | 1.27 | 0 | 0 |
| [5.7.10.5](../spec/5/5.7.10.5.md) | Output data | implemented | TS 104 175 10.6.8.5 | Output data | edited | 1.31 | 0 | 0 |
| [5.7.10](../spec/5/5.7.10.md) | Retrieve Available Attribute Information | informative | TS 104 175 10.6.8 | Retrieve available Attribute information | edited | 1.00 | 0 | 0 |
| [5.7.11](../spec/5/5.7.11.md) | Architecture-related aspects of retrieval of Entity Types and Attributes | implemented | TS 104 175 10.6.2 | Architecture-related aspects of retrieval of Entity Types and Attributes | edited | 1.38 | 2 | 2 |
| [5.7.2.1](../spec/5/5.7.2.1.md) | Description | informative | TS 104 175 10.4.3.1 | Description | edited | 0.73 | 0 | 0 |
| [5.7.2.2](../spec/5/5.7.2.2.md) | Use case diagram | informative | TS 104 175 10.4.3.2 | Use case diagram | edited | 1.31 | 0 | 0 |
| [5.7.2.3](../spec/5/5.7.2.3.md) | Input data | implemented | TS 104 175 10.4.3.3 | Input data | edited | 1.38 | 0 | 0 |
| [5.7.2.4](../spec/5/5.7.2.4.md) | Behaviour | implemented | TS 104 175 10.4.3.4 | Behaviour | moved | 1.41 | 13 | 13 |
| [5.7.2.5](../spec/5/5.7.2.5.md) | Output data | implemented | TS 104 175 10.4.3.5 | Output data | moved | 1.43 | 4 | 4 |
| [5.7.2](../spec/5/5.7.2.md) | Query Entities | informative | TS 104 175 10.4.3 | Query Entities | edited | 1.00 | 0 | 0 |
| [5.7.3.1](../spec/5/5.7.3.1.md) | Description | informative | TS 104 175 11.3.2.1 | Description | edited | 0.77 | 0 | 0 |
| [5.7.3.2](../spec/5/5.7.3.2.md) | Use case diagram | informative | TS 104 175 11.3.2.2 | Use case diagram | edited | 1.33 | 0 | 0 |
| [5.7.3.3](../spec/5/5.7.3.3.md) | Input data | implemented | TS 104 175 11.3.2.3 | Input data | edited | 1.22 | 0 | 1 |
| [5.7.3.4](../spec/5/5.7.3.4.md) | Behaviour | implemented | TS 104 175 11.3.2.4 | Behaviour | moved | 1.42 | 4 | 4 |
| [5.7.3.5](../spec/5/5.7.3.5.md) | Output data | implemented | TS 104 175 11.3.2.5 | Output data | moved | 1.40 | 0 | 0 |
| [5.7.3](../spec/5/5.7.3.md) | Retrieve Temporal Evolution of an Entity | informative | TS 104 175 11.3.2 | Retrieve Temporal Evolution of an Entity | edited | 1.00 | 0 | 0 |
| [5.7.4.1](../spec/5/5.7.4.1.md) | Description | informative | TS 104 175 11.3.3.1 | Description | edited | 0.79 | 0 | 0 |
| [5.7.4.2](../spec/5/5.7.4.2.md) | Use case diagram | informative | TS 104 175 11.3.3.2 | Use case diagram | edited | 1.34 | 0 | 0 |
| [5.7.4.3](../spec/5/5.7.4.3.md) | Input data | implemented | TS 104 175 11.3.3.3 | Input data | edited | 1.37 | 0 | 1 |
| [5.7.4.4](../spec/5/5.7.4.4.md) | Behaviour | implemented | TS 104 175 11.3.3.4 | Behaviour | moved | 1.43 | 15 | 15 |
| [5.7.4.5](../spec/5/5.7.4.5.md) | Output Data | implemented | TS 104 175 11.3.3.5 | Output Data | edited | 1.36 | 1 | 1 |
| [5.7.4](../spec/5/5.7.4.md) | Query Temporal Evolution of Entities | informative | TS 104 175 11.3.3 | Query Temporal Evolution of Entities | edited | 1.00 | 0 | 0 |
| [5.7.5.1](../spec/5/5.7.5.1.md) | Description | informative | TS 104 175 10.6.3.1 | Description | edited | 0.88 | 0 | 0 |
| [5.7.5.2](../spec/5/5.7.5.2.md) | Use case diagram | informative | TS 104 175 10.6.3.2 | Use case diagram | edited | 1.30 | 0 | 0 |
| [5.7.5.3](../spec/5/5.7.5.3.md) | Input data | implemented | TS 104 175 10.2.10.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.7.5.4](../spec/5/5.7.5.4.md) | Behaviour | implemented | TS 104 175 10.6.3.4 | Behaviour | edited | 1.26 | 0 | 0 |
| [5.7.5.5](../spec/5/5.7.5.5.md) | Output data | implemented | TS 104 175 10.6.3.5 | Output data | edited | 1.32 | 0 | 0 |
| [5.7.5](../spec/5/5.7.5.md) | Retrieve Available Entity Types | informative | TS 104 176 A.4.1 | Retrieve available Entity Types | edited | 1.00 | 0 | 0 |
| [5.7.6.1](../spec/5/5.7.6.1.md) | Description | informative | TS 104 175 10.6.4.1 | Description | edited | 0.95 | 0 | 0 |
| [5.7.6.2](../spec/5/5.7.6.2.md) | Use case diagram | informative | TS 104 175 10.6.4.2 | Use case diagram | edited | 1.33 | 0 | 0 |
| [5.7.6.3](../spec/5/5.7.6.3.md) | Input data | implemented | TS 104 175 10.2.10.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.7.6.4](../spec/5/5.7.6.4.md) | Behaviour | implemented | TS 104 175 10.6.4.4 | Behaviour | edited | 1.28 | 0 | 0 |
| [5.7.6.5](../spec/5/5.7.6.5.md) | Output data | implemented | TS 104 175 10.6.4.5 | Output data | edited | 1.33 | 0 | 0 |
| [5.7.6](../spec/5/5.7.6.md) | Retrieve Details of Available Entity Types | informative | TS 104 176 A.4.2 | Retrieve details of available Entity Types | edited | 1.00 | 0 | 0 |
| [5.7.7.1](../spec/5/5.7.7.1.md) | Description | informative | TS 104 175 10.6.5.1 | Description | edited | 0.96 | 0 | 0 |
| [5.7.7.2](../spec/5/5.7.7.2.md) | Use case diagram | informative | TS 104 175 10.6.5.2 | Use case diagram | edited | 0.98 | 0 | 0 |
| [5.7.7.3](../spec/5/5.7.7.3.md) | Input data | implemented | TS 104 175 10.6.5.3 | Input data | edited | 1.35 | 0 | 0 |
| [5.7.7.4](../spec/5/5.7.7.4.md) | Behaviour | implemented | TS 104 175 10.6.5.4 | Behaviour | edited | 1.24 | 0 | 0 |
| [5.7.7.5](../spec/5/5.7.7.5.md) | Output data | implemented | TS 104 175 10.6.5.5 | Output data | edited | 1.33 | 0 | 0 |
| [5.7.7](../spec/5/5.7.7.md) | Retrieve Available Entity Type Information | informative | TS 104 175 10.6.5 | Retrieve available Entity Type information | edited | 1.00 | 0 | 0 |
| [5.7.8.1](../spec/5/5.7.8.1.md) | Description | informative | TS 104 175 10.6.6.1 | Description | edited | 0.88 | 0 | 0 |
| [5.7.8.2](../spec/5/5.7.8.2.md) | Use case diagram | informative | TS 104 175 10.6.6.2 | Use case diagram | edited | 1.29 | 0 | 0 |
| [5.7.8.3](../spec/5/5.7.8.3.md) | Input data | implemented | TS 104 175 10.2.10.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.7.8.4](../spec/5/5.7.8.4.md) | Behaviour | implemented | TS 104 175 10.6.6.4 | Behaviour | edited | 1.26 | 0 | 0 |
| [5.7.8.5](../spec/5/5.7.8.5.md) | Output data | implemented | TS 104 175 10.6.6.5 | Output data | edited | 1.31 | 0 | 0 |
| [5.7.8](../spec/5/5.7.8.md) | Retrieve Available Attributes | informative | TS 104 175 10.6.6 | Retrieve available attributes | edited | 1.00 | 0 | 0 |
| [5.7.9.1](../spec/5/5.7.9.1.md) | Description | informative | TS 104 175 10.6.7.1 | Description | edited | 0.95 | 0 | 0 |
| [5.7.9.2](../spec/5/5.7.9.2.md) | Use case diagram | informative | TS 104 175 10.6.7.2 | Use case diagram | edited | 1.32 | 0 | 0 |
| [5.7.9.3](../spec/5/5.7.9.3.md) | Input data | implemented | TS 104 175 10.2.10.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.7.9.4](../spec/5/5.7.9.4.md) | Behaviour | implemented | TS 104 175 10.6.7.4 | Behaviour | edited | 1.31 | 0 | 0 |
| [5.7.9.5](../spec/5/5.7.9.5.md) | Output data | implemented | TS 104 175 10.6.7.5 | Output data | edited | 1.31 | 0 | 0 |
| [5.7.9](../spec/5/5.7.9.md) | Retrieve Details of Available Attributes | informative | TS 104 176 A.5.2 | Retrieve details of available Attributes | edited | 1.00 | 0 | 0 |
| [5.7](../spec/5/5.7.md) | Context Information Consumption | informative | TS 104 175 10.4 | Context information consumption | edited | 1.00 | 0 | 0 |
| [5.8.1.1](../spec/5/5.8.1.1.md) | Description | informative | TS 104 175 10.5.2.1 | Description | edited | 0.70 | 0 | 0 |
| [5.8.1.2](../spec/5/5.8.1.2.md) | Use case diagram | informative | TS 104 175 10.5.2.2 | Use case diagram | edited | 1.29 | 0 | 0 |
| [5.8.1.3](../spec/5/5.8.1.3.md) | Input data | implemented | TS 104 175 10.5.2.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.8.1.4](../spec/5/5.8.1.4.md) | Behaviour | implemented | TS 104 175 10.5.2.4 | Behaviour | moved | 1.46 | 6 | 6 |
| [5.8.1.5](../spec/5/5.8.1.5.md) | Output data | implemented | TS 104 175 12.4.2.5 | Output data | edited | 1.23 | 0 | 0 |
| [5.8.1](../spec/5/5.8.1.md) | Create Subscription | informative | TS 104 175 10.5.2 | Create Subscription | edited | 1.00 | 0 | 0 |
| [5.8.2.1](../spec/5/5.8.2.1.md) | Description | informative | TS 104 175 10.5.3.1 | Description | edited | 0.70 | 0 | 0 |
| [5.8.2.2](../spec/5/5.8.2.2.md) | Use case diagram | informative | TS 104 175 10.5.3.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.8.2.3](../spec/5/5.8.2.3.md) | Input data | implemented | TS 104 175 10.5.3.3 | Input data | edited | 1.33 | 0 | 0 |
| [5.8.2.4](../spec/5/5.8.2.4.md) | Behaviour | implemented | TS 104 175 10.5.3.4 | Behaviour | moved | 1.43 | 4 | 4 |
| [5.8.2.5](../spec/5/5.8.2.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.8.2](../spec/5/5.8.2.md) | Update Subscription | informative | TS 104 175 10.5.3 | Update Subscription | edited | 1.00 | 0 | 0 |
| [5.8.3.1](../spec/5/5.8.3.1.md) | Description | informative | TS 104 175 10.5.4.1 | Description | edited | 0.70 | 0 | 0 |
| [5.8.3.2](../spec/5/5.8.3.2.md) | Use case diagram | informative | TS 104 175 10.5.4.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.8.3.3](../spec/5/5.8.3.3.md) | Input data | implemented | TS 104 175 10.5.4.3 | Input data | edited | 1.29 | 0 | 0 |
| [5.8.3.4](../spec/5/5.8.3.4.md) | Behaviour | implemented | TS 104 175 10.5.4.4 | Behaviour | moved | 1.45 | 0 | 0 |
| [5.8.3.5](../spec/5/5.8.3.5.md) | Output data | implemented | TS 104 175 12.4.4.5 | Output data | edited | 1.34 | 0 | 0 |
| [5.8.3](../spec/5/5.8.3.md) | Retrieve Subscription | informative | TS 104 175 10.5.4 | Retrieve Subscription | edited | 1.00 | 0 | 0 |
| [5.8.4.1](../spec/5/5.8.4.1.md) | Description | informative |   |  | heading | 0.00 |  |  |
| [5.8.4.2](../spec/5/5.8.4.2.md) | Use case diagram | informative | TS 104 175 10.5.5.2 | Use case diagram | edited | 1.27 | 0 | 0 |
| [5.8.4.3](../spec/5/5.8.4.3.md) | Input data | implemented | TS 104 175 10.5.5.3 | Input data | edited | 1.18 | 0 | 0 |
| [5.8.4.4](../spec/5/5.8.4.4.md) | Behaviour | implemented | TS 104 175 10.5.5.4 | Behaviour | edited | 1.38 | 1 | 1 |
| [5.8.4.5](../spec/5/5.8.4.5.md) | Output data | implemented | TS 104 175 10.5.5.5 | Output data | edited | 1.35 | 0 | 0 |
| [5.8.4](../spec/5/5.8.4.md) | Query Subscriptions | informative | TS 104 175 10.5.5 | Query Subscriptions | edited | 1.00 | 0 | 0 |
| [5.8.5.1](../spec/5/5.8.5.1.md) | Description | informative | TS 104 175 10.5.6.1 | Description | edited | 0.70 | 0 | 0 |
| [5.8.5.2](../spec/5/5.8.5.2.md) | Use case diagram | informative | TS 104 175 10.5.6.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.8.5.3](../spec/5/5.8.5.3.md) | Input data | implemented | TS 104 175 12.2.3.3 | Input data | edited | 1.00 | 0 | 0 |
| [5.8.5.4](../spec/5/5.8.5.4.md) | Behaviour | implemented | TS 104 175 10.5.6.4 | Behaviour | moved | 1.42 | 1 | 1 |
| [5.8.5.5](../spec/5/5.8.5.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.8.5](../spec/5/5.8.5.md) | Delete Subscription | informative | TS 104 175 10.5.6 | Delete Subscription | edited | 1.00 | 0 | 0 |
| [5.8.6](../spec/5/5.8.6.md) | Notification behaviour | implemented | TS 104 175 10.5.7 | Notification behaviour | moved | 1.44 | 5 | 13 |
| [5.8](../spec/5/5.8.md) | Context Information Subscription | informative | TS 104 175 10.5 | Context Information Subscription | edited | 1.00 | 0 | 0 |
| [5.9.1](../spec/5/5.9.1.md) | Introduction | implemented |   |  | unmatched | 0.14 |  |  |
| [5.9.2.1](../spec/5/5.9.2.1.md) | Description | informative | TS 104 175 12.2.2.1 | Description | edited | 0.87 | 0 | 0 |
| [5.9.2.2](../spec/5/5.9.2.2.md) | Use case diagram | informative | TS 104 175 12.2.2.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.9.2.3](../spec/5/5.9.2.3.md) | Input data | implemented | TS 104 175 12.2.2.3 | Input data | edited | 1.35 | 0 | 0 |
| [5.9.2.4](../spec/5/5.9.2.4.md) | Behaviour | partial | TS 104 175 12.2.2.4 | Behaviour | moved | 1.44 | 4 | 4 |
| [5.9.2.5](../spec/5/5.9.2.5.md) | Output data | implemented | TS 104 175 12.2.2.5 | Output data | edited | 1.32 | 0 | 0 |
| [5.9.2](../spec/5/5.9.2.md) | Register Context Source | informative | TS 104 175 12.2.2 | Register Context Source | edited | 1.00 | 0 | 0 |
| [5.9.3.1](../spec/5/5.9.3.1.md) | Description | informative | TS 104 175 12.2.3.1 | Description | edited | 0.88 | 0 | 0 |
| [5.9.3.2](../spec/5/5.9.3.2.md) | Use case diagram | informative | TS 104 175 12.2.3.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.9.3.3](../spec/5/5.9.3.3.md) | Input data | implemented | TS 104 175 12.2.3.3 | Input data | moved | 1.42 | 0 | 0 |
| [5.9.3.4](../spec/5/5.9.3.4.md) | Behaviour | implemented | TS 104 175 12.2.3.4 | Behaviour | moved | 1.41 | 5 | 5 |
| [5.9.3.5](../spec/5/5.9.3.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.9.3](../spec/5/5.9.3.md) | Update Context Source Registration | informative | TS 104 175 12.2.3 | Update Context Source Registration | edited | 1.00 | 0 | 0 |
| [5.9.4.1](../spec/5/5.9.4.1.md) | Description | informative | TS 104 175 12.2.4.1 | Description | edited | 0.88 | 0 | 0 |
| [5.9.4.2](../spec/5/5.9.4.2.md) | Use case diagram | informative | TS 104 175 12.2.4.2 | Use case diagram | edited | 1.35 | 0 | 0 |
| [5.9.4.3](../spec/5/5.9.4.3.md) | Input data | implemented | TS 104 175 12.2.4.3 | Input data | edited | 1.38 | 0 | 0 |
| [5.9.4.4](../spec/5/5.9.4.4.md) | Behaviour | implemented | TS 104 175 12.2.4.4 | Behaviour | moved | 1.47 | 0 | 0 |
| [5.9.4.5](../spec/5/5.9.4.5.md) | Output data | implemented | TS 104 175 12.2.3.5 | Output data | edited | 1.00 | 0 | 0 |
| [5.9.4](../spec/5/5.9.4.md) | Delete Context Source Registration | informative | TS 104 175 12.2.4 | Delete Context Source Registration | edited | 1.00 | 0 | 0 |
| [5.9](../spec/5/5.9.md) | Context Source Registration | informative | TS 104 175 12.2 | Context Source Registration | edited | 1.00 | 0 | 0 |
| [5](../spec/5/5.md) | API Operation Definition | informative | TS 104 175 7.6.2.3 | NGSI-LD Entity ordering language | edited | 0.87 | 1 | 5 |
| [6.1](../spec/6/6.1.md) | Introduction | informative | TS 104 176 6.1 | Introduction | edited | 0.83 | 0 | 0 |
| [6.10.1](../spec/6/6.10.1.md) | Description | informative | TS 104 176 7.13.1 | Description | edited | 0.85 | 0 | 0 |
| [6.10.2](../spec/6/6.10.2.md) | Resource definition | implemented | TS 104 176 13.4.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.10.3.1](../spec/6/6.10.3.1.md) | POST | implemented | TS 104 176 7.13.3.2 | POST | edited | 1.35 | 1 | 1 |
| [6.10.3.2](../spec/6/6.10.3.2.md) | GET | implemented | TS 104 176 7.13.3.1 | GET/HEAD | edited | 0.83 | 2 | 2 |
| [6.10.3](../spec/6/6.10.3.md) | Resource methods | informative | TS 104 176 13.3.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.10](../spec/6/6.10.md) | Resource: subscriptions/ | informative | TS 104 176 7.13 | Resource: subscriptions/ | edited | 1.00 | 0 | 0 |
| [6.11.1](../spec/6/6.11.1.md) | Description | informative | TS 104 176 7.14.1 | Description | edited | 0.85 | 0 | 0 |
| [6.11.2](../spec/6/6.11.2.md) | Resource definition | implemented | TS 104 176 7.14.2 | Resource definition | edited | 1.32 | 0 | 0 |
| [6.11.3.1](../spec/6/6.11.3.1.md) | GET | implemented | TS 104 176 7.14.3.1 | GET/HEAD | edited | 0.87 | 1 | 1 |
| [6.11.3.2](../spec/6/6.11.3.2.md) | PATCH | implemented | TS 104 176 7.14.3.2 | PATCH | edited | 1.33 | 1 | 1 |
| [6.11.3.3](../spec/6/6.11.3.3.md) | DELETE | implemented | TS 104 176 7.14.3.3 | DELETE | edited | 1.37 | 1 | 1 |
| [6.11.3](../spec/6/6.11.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.11](../spec/6/6.11.md) | Resource: subscriptions/{subscriptionId} | informative | TS 104 176 7.14 | Resource: subscriptions/{subscriptionId} | edited | 1.00 | 0 | 0 |
| [6.12.1](../spec/6/6.12.1.md) | Description | informative | TS 104 176 9.4.1 | Description | edited | 0.88 | 0 | 0 |
| [6.12.2](../spec/6/6.12.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.12.3.1](../spec/6/6.12.3.1.md) | POST | implemented | TS 104 176 7.13.3.2 | POST | edited | 1.26 | 2 | 2 |
| [6.12.3.2](../spec/6/6.12.3.2.md) | GET | implemented | TS 104 176 9.4.3.2 | POST | edited | 0.88 | 2 | 2 |
| [6.12.3](../spec/6/6.12.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.12](../spec/6/6.12.md) | Resource: csourceSubscriptions/ | informative | TS 104 176 9.4 | Resource: csourceSubscriptions/ | edited | 1.00 | 0 | 0 |
| [6.13.1](../spec/6/6.13.1.md) | Description | informative | TS 104 176 9.5.1 | Description | edited | 0.89 | 0 | 0 |
| [6.13.2](../spec/6/6.13.2.md) | Resource definition | implemented | TS 104 176 9.5.2 | Resource definition | edited | 1.33 | 0 | 0 |
| [6.13.3.1](../spec/6/6.13.3.1.md) | GET | implemented | TS 104 176 9.5.3.1 | GET/HEAD | edited | 0.88 | 1 | 1 |
| [6.13.3.2](../spec/6/6.13.3.2.md) | PATCH | implemented | TS 104 176 9.5.3.2 | PATCH | edited | 1.34 | 1 | 1 |
| [6.13.3.3](../spec/6/6.13.3.3.md) | DELETE | implemented | TS 104 176 9.5.3.3 | DELETE | edited | 1.38 | 1 | 1 |
| [6.13.3](../spec/6/6.13.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.13](../spec/6/6.13.md) | Resource: csourceSubscriptions/{subscriptionId} | informative | TS 104 176 9.5 | Resource: csourceSubscriptions/{subscriptionId} | edited | 1.00 | 0 | 0 |
| [6.14.1](../spec/6/6.14.1.md) | Description | informative | TS 104 176 7.7.1 | Description | edited | 0.90 | 0 | 0 |
| [6.14.2](../spec/6/6.14.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.14.3.1](../spec/6/6.14.3.1.md) | POST | implemented | TS 104 176 7.7.3.1 | POST | moved | 1.41 | 1 | 1 |
| [6.14.3](../spec/6/6.14.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.14](../spec/6/6.14.md) | Resource: entityOperations/create | informative | TS 104 176 7.7 | Resource: entityOperations/create | edited | 1.00 | 0 | 0 |
| [6.15.1](../spec/6/6.15.1.md) | Description | informative | TS 104 176 7.8.1 | Description | edited | 0.91 | 0 | 0 |
| [6.15.2](../spec/6/6.15.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.15.3.1](../spec/6/6.15.3.1.md) | POST | implemented | TS 104 176 7.8.3.1 | POST | moved | 1.44 | 1 | 1 |
| [6.15.3](../spec/6/6.15.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.15](../spec/6/6.15.md) | Resource: entityOperations/upsert | informative | TS 104 176 7.8 | Resource: entityOperations/upsert | edited | 1.00 | 0 | 0 |
| [6.16.1](../spec/6/6.16.1.md) | Description | informative | TS 104 176 7.9.1 | Description | edited | 0.90 | 0 | 0 |
| [6.16.2](../spec/6/6.16.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.16.3.1](../spec/6/6.16.3.1.md) | POST | implemented | TS 104 176 7.9.3.1 | POST | moved | 1.42 | 1 | 1 |
| [6.16.3](../spec/6/6.16.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.16](../spec/6/6.16.md) | Resource: entityOperations/update | informative | TS 104 176 7.9 | Resource: entityOperations/update | edited | 1.00 | 0 | 0 |
| [6.17.1](../spec/6/6.17.1.md) | Description | informative | TS 104 176 7.11.1 | Description | edited | 0.90 | 0 | 0 |
| [6.17.2](../spec/6/6.17.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.17.3.1](../spec/6/6.17.3.1.md) | POST | implemented | TS 104 176 7.11.3.1 | POST | moved | 1.42 | 1 | 1 |
| [6.17.3](../spec/6/6.17.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.17](../spec/6/6.17.md) | Resource: entityOperations/delete | informative | TS 104 176 7.11 | Resource: entityOperations/delete | edited | 1.00 | 0 | 0 |
| [6.18.1](../spec/6/6.18.1.md) | Description | informative | TS 104 176 8.2.1 | Description | edited | 0.88 | 0 | 0 |
| [6.18.2](../spec/6/6.18.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.18.3.1](../spec/6/6.18.3.1.md) | POST | implemented | TS 104 176 8.2.3.2 | POST | edited | 1.33 | 1 | 1 |
| [6.18.3.2](../spec/6/6.18.3.2.md) | GET | implemented | TS 104 176 8.2.3.1 | GET/HEAD | edited | 0.88 | 15 | 15 |
| [6.18.3](../spec/6/6.18.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.18](../spec/6/6.18.md) | Resource: temporal/entities/ | informative | TS 104 176 8.2 | Resource: temporal/entities/ | edited | 1.00 | 0 | 0 |
| [6.19.1](../spec/6/6.19.1.md) | Description | informative | TS 104 176 8.3.1 | Description | edited | 0.89 | 0 | 0 |
| [6.19.2](../spec/6/6.19.2.md) | Resource definition | implemented | TS 104 176 8.3.2 | Resource definition | edited | 1.33 | 0 | 0 |
| [6.19.3.1](../spec/6/6.19.3.1.md) | GET | implemented | TS 104 176 8.3.3.1 | GET/HEAD | edited | 0.87 | 7 | 7 |
| [6.19.3.2](../spec/6/6.19.3.2.md) | DELETE | implemented | TS 104 176 8.3.3.2 | DELETE | edited | 1.38 | 1 | 1 |
| [6.19.3](../spec/6/6.19.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.19](../spec/6/6.19.md) | Resource: temporal/entities/{entityId} | informative | TS 104 176 8.3 | Resource: temporal/entities/{entityId} | edited | 1.00 | 0 | 0 |
| [6.2](../spec/6/6.2.md) | Global Definitions and Resource Structure | implemented | TS 104 176 5 | Global definitions and resource structure | edited | 1.06 | 3 | 6 |
| [6.20.1](../spec/6/6.20.1.md) | Description | informative | TS 104 176 8.4.1 | Description | edited | 0.61 | 0 | 0 |
| [6.20.2](../spec/6/6.20.2.md) | Resource definition | implemented | TS 104 176 8.4.2 | Resource definition | edited | 1.33 | 0 | 0 |
| [6.20.3.1](../spec/6/6.20.3.1.md) | POST | implemented | TS 104 176 8.4.3.1 | POST | edited | 1.40 | 1 | 1 |
| [6.20.3](../spec/6/6.20.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.20](../spec/6/6.20.md) | Resource: temporal/entities/{entityId}/attrs/ | informative | TS 104 176 8.4 | Resource: temporal/entities/{entityId}/attrs/ | edited | 1.00 | 0 | 0 |
| [6.21.1](../spec/6/6.21.1.md) | Description | informative | TS 104 176 8.6.1 | Description | edited | 0.76 | 0 | 0 |
| [6.21.2](../spec/6/6.21.2.md) | Resource definition | implemented | TS 104 176 8.5.2 | Resource definition | edited | 1.36 | 0 | 0 |
| [6.21.3.1](../spec/6/6.21.3.1.md) | DELETE | implemented | TS 104 176 8.5.3.1 | DELETE | edited | 1.32 | 1 | 1 |
| [6.21.3](../spec/6/6.21.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.21](../spec/6/6.21.md) | Resource: temporal/entities/{entityId}/attrs/{attrId} | informative | TS 104 176 8.5 | Resource: temporal/entities/{entityId}/attrs/{attrId} | edited | 1.00 | 0 | 0 |
| [6.22.1](../spec/6/6.22.1.md) | Description | informative | TS 104 176 8.6.1 | Description | edited | 0.89 | 0 | 0 |
| [6.22.2](../spec/6/6.22.2.md) | Resource definition | implemented | TS 104 176 8.6.2 | Resource definition | edited | 1.38 | 0 | 0 |
| [6.22.3.1](../spec/6/6.22.3.1.md) | PATCH | implemented | TS 104 176 8.6.3.1 | PATCH | edited | 1.40 | 1 | 1 |
| [6.22.3.2](../spec/6/6.22.3.2.md) | DELETE | implemented | TS 104 176 8.6.3.2 | DELETE | edited | 1.39 | 1 | 1 |
| [6.22.3](../spec/6/6.22.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.22](../spec/6/6.22.md) | Resource: temporal/entities/{entityId}/attrs/{attrId}/ {instanceId} | informative | TS 104 176 8.5.3.1 | DELETE | edited | 0.87 | 1 | 1 |
| [6.23.1](../spec/6/6.23.1.md) | Description | informative | TS 104 176 7.12.1 | Description | edited | 0.89 | 0 | 0 |
| [6.23.2](../spec/6/6.23.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.23.3.1](../spec/6/6.23.3.1.md) | POST | implemented | TS 104 176 7.12.3.1 | POST | edited | 1.38 | 2 | 2 |
| [6.23.3](../spec/6/6.23.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.23](../spec/6/6.23.md) | Resource: entityOperations/query | informative | TS 104 176 7.12 | Resource: entityOperations/query | edited | 1.00 | 0 | 0 |
| [6.24.1](../spec/6/6.24.1.md) | Description | informative | TS 104 176 8.7.1 | Description | edited | 0.89 | 0 | 0 |
| [6.24.2](../spec/6/6.24.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.24.3.1](../spec/6/6.24.3.1.md) | POST | implemented | TS 104 176 8.7.3.1 | POST | moved | 1.41 | 1 | 1 |
| [6.24.3](../spec/6/6.24.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.24](../spec/6/6.24.md) | Resource: temporal/entityOperations/query | informative | TS 104 176 8.7 | Resource: temporal/entityOperations/query | edited | 1.00 | 0 | 0 |
| [6.25.1](../spec/6/6.25.1.md) | Description | informative | TS 104 176 7.15.1 | Description | edited | 0.86 | 0 | 0 |
| [6.25.2](../spec/6/6.25.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.25.3.1](../spec/6/6.25.3.1.md) | GET | implemented | TS 104 176 7.15.3.1 | GET/HEAD | edited | 0.82 | 2 | 2 |
| [6.25.3](../spec/6/6.25.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.25](../spec/6/6.25.md) | Resource: types/ | informative | TS 104 176 7.15 | Resource: types/ | edited | 1.00 | 0 | 0 |
| [6.26.1](../spec/6/6.26.1.md) | Description | informative | TS 104 176 7.16.1 | Description | edited | 0.90 | 0 | 0 |
| [6.26.2](../spec/6/6.26.2.md) | Resource definition | implemented | TS 104 176 7.16.2 | Resource definition | moved | 1.41 | 0 | 0 |
| [6.26.3.1](../spec/6/6.26.3.1.md) | GET | implemented | TS 104 176 7.16.3.1 | GET/HEAD | edited | 0.87 | 1 | 1 |
| [6.26.3](../spec/6/6.26.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.26](../spec/6/6.26.md) | Resource: types/{type} | informative | TS 104 176 7.16 | Resource: types/{type} | edited | 1.00 | 0 | 0 |
| [6.27.1](../spec/6/6.27.1.md) | Description | informative | TS 104 176 7.17.1 | Description | edited | 0.85 | 0 | 0 |
| [6.27.2](../spec/6/6.27.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.27.3.1](../spec/6/6.27.3.1.md) | GET | implemented | TS 104 176 7.17.3.1 | GET/HEAD | edited | 0.82 | 2 | 2 |
| [6.27.3](../spec/6/6.27.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.27](../spec/6/6.27.md) | Resource: attributes/ | informative | TS 104 176 7.17 | Resource: attributes/ | edited | 1.00 | 0 | 0 |
| [6.28.1](../spec/6/6.28.1.md) | Description | informative | TS 104 176 7.18.1 | Description | edited | 0.89 | 0 | 0 |
| [6.28.2](../spec/6/6.28.2.md) | Resource definition | implemented | TS 104 176 7.18.2 | Resource definition | moved | 1.41 | 0 | 0 |
| [6.28.3.1](../spec/6/6.28.3.1.md) | GET | implemented | TS 104 176 7.18.3.1 | GET/HEAD | edited | 0.87 | 1 | 1 |
| [6.28.3](../spec/6/6.28.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.28](../spec/6/6.28.md) | Resource: attributes/{attrId} | informative | TS 104 176 7.18 | Resource: attributes/{attrId} | edited | 1.00 | 0 | 0 |
| [6.29.1](../spec/6/6.29.1.md) | Description | informative | TS 104 176 10.2.1 | Description | edited | 0.85 | 0 | 0 |
| [6.29.2](../spec/6/6.29.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.29.3.1](../spec/6/6.29.3.1.md) | POST | implemented | TS 104 176 10.2.3.2 | POST | edited | 1.34 | 1 | 1 |
| [6.29.3.2](../spec/6/6.29.3.2.md) | GET | implemented | TS 104 176 10.2.3.1 | GET/HEAD | edited | 0.84 | 2 | 2 |
| [6.29.3](../spec/6/6.29.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.29](../spec/6/6.29.md) | Resource: jsonldContexts/ | informative | TS 104 176 10.2 | Resource: jsonldContexts/ | edited | 1.00 | 0 | 0 |
| [6.3.1](../spec/6/6.3.1.md) | Introduction | informative |   |  | unmatched | 0.12 |  |  |
| [6.3.10](../spec/6/6.3.10.md) | Pagination behaviour | implemented | TS 104 176 6.4.7.2 | General pagination | weak | 0.59 | 15 | 9 |
| [6.3.11](../spec/6/6.3.11.md) | Including system Attributes | implemented | TS 104 176 6.4.5 | Including system Attributes | edited | 1.37 | 1 | 1 |
| [6.3.12](../spec/6/6.3.12.md) | Simplified or aggregated temporal representation of Entities | implemented | TS 104 176 6.4.3 | Simplified or aggregated temporal representation of Entities | edited | 1.33 | 5 | 5 |
| [6.3.13](../spec/6/6.3.13.md) | Counting number of results | implemented | TS 104 176 6.4.6 | Counting number of results | edited | 1.40 | 1 | 1 |
| [6.3.14](../spec/6/6.3.14.md) | Tenant specification | implemented | TS 104 176 6.4.8 | Tenant specification | edited | 1.35 | 0 | 0 |
| [6.3.15](../spec/6/6.3.15.md) | GeoJSON representation of spatially bound entities | implemented | TS 104 176 6.4.4 | GeoJSON representation of spatially bound Entities | edited | 1.31 | 1 | 1 |
| [6.3.16](../spec/6/6.3.16.md) | Expiration time for cached @contexts | implemented | TS 104 176 6.3.4 | Expiration time for cached @contexts | edited | 1.40 | 2 | 2 |
| [6.3.17](../spec/6/6.3.17.md) | Distributed Operations Caching and Timeout Behaviour | implemented | TS 104 176 6.3.5 | Distributed operations caching and timeout behaviour | moved | 1.47 | 3 | 3 |
| [6.3.18](../spec/6/6.3.18.md) | Limiting Distributed Operations | implemented | TS 104 176 6.4.11 | Limiting distributed operations | edited | 1.33 | 4 | 4 |
| [6.3.19](../spec/6/6.3.19.md) | Extra information to provide when contacting Context Source | implemented | TS 104 176 6.5.4 | Extra information to provide when contacting Context Sources | edited | 0.84 | 1 | 1 |
| [6.3.2](../spec/6/6.3.2.md) | Error Types | implemented | TS 104 176 6.3.2 | Error types | moved | 1.44 | 3 | 5 |
| [6.3.20](../spec/6/6.3.20.md) | Invalid parameters | implemented | TS 104 176 6.3.6 | Invalid parameters | moved | 1.43 | 1 | 1 |
| [6.3.21](../spec/6/6.3.21.md) | Optional profile information regarding versioning and datatype conformance | implemented | TS 104 176 6.4.10 | Optional profile information regarding versioning and datatype conformance | edited | 1.33 | 1 | 1 |
| [6.3.22](../spec/6/6.3.22.md) | Snapshot specification | implemented | TS 104 176 6.4.9 | Snapshot specification | edited | 1.32 | 0 | 0 |
| [6.3.3](../spec/6/6.3.3.md) | Reporting errors | implemented | TS 104 176 6.3.3 | Reporting errors | edited | 1.36 | 0 | 0 |
| [6.3.4](../spec/6/6.3.4.md) | HTTP request preconditions | implemented | TS 104 176 6.2.2 | HTTP request preconditions | moved | 1.42 | 3 | 3 |
| [6.3.5](../spec/6/6.3.5.md) | JSON-LD @context resolution | implemented | TS 104 176 6.2.4 | JSON-LD @context resolution | moved | 1.46 | 3 | 3 |
| [6.3.6](../spec/6/6.3.6.md) | HTTP response common requirements | implemented | TS 104 176 6.2.3 | HTTP response common requirements | moved | 1.42 | 5 | 5 |
| [6.3.7](../spec/6/6.3.7.md) | Representation of Entities | implemented | TS 104 176 6.4.2 | Representation of Entities | edited | 1.22 | 7 | 7 |
| [6.3.8](../spec/6/6.3.8.md) | Notification behaviour | implemented | TS 104 176 6.5.2 | Notification behaviour | moved | 1.42 | 5 | 5 |
| [6.3.9](../spec/6/6.3.9.md) | Csource Notification behaviour | implemented | TS 104 176 6.5.3 | Csource notification behaviour | edited | 1.39 | 4 | 4 |
| [6.3](../spec/6/6.3.md) | Common Behaviours | informative | TS 104 176 6 | Common behaviours | edited | 1.00 | 0 | 0 |
| [6.30.1](../spec/6/6.30.1.md) | Description | informative | TS 104 176 10.3.1 | Description | edited | 0.88 | 0 | 0 |
| [6.30.2](../spec/6/6.30.2.md) | Resource definition | implemented | TS 104 176 10.3.2 | Resource definition | edited | 1.39 | 0 | 0 |
| [6.30.3.1](../spec/6/6.30.3.1.md) | GET | implemented | TS 104 176 10.3.3.1 | GET/HEAD | edited | 0.83 | 2 | 2 |
| [6.30.3.2](../spec/6/6.30.3.2.md) | DELETE | implemented | TS 104 176 10.3.3.2 | DELETE | edited | 1.37 | 2 | 2 |
| [6.30.3](../spec/6/6.30.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.30](../spec/6/6.30.md) | Resource: jsonldContexts/{contextId} | informative | TS 104 176 10.3 | Resource: jsonldContexts/{contextId} | edited | 1.00 | 0 | 0 |
| [6.31.1](../spec/6/6.31.1.md) | Description | informative | TS 104 176 7.10.1 | Description | edited | 0.90 | 0 | 0 |
| [6.31.2](../spec/6/6.31.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.31.3.1](../spec/6/6.31.3.1.md) | POST | implemented | TS 104 176 7.10.3.1 | POST | moved | 1.40 | 1 | 1 |
| [6.31.3](../spec/6/6.31.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.31](../spec/6/6.31.md) | Resource: entityOperations/merge | informative | TS 104 176 7.10 | Resource: entityOperations/merge | edited | 1.00 | 0 | 0 |
| [6.32.1](../spec/6/6.32.1.md) | Description | informative | TS 104 176 11.4.1 | Description | edited | 0.62 | 0 | 0 |
| [6.32.2](../spec/6/6.32.2.md) | Resource definition | implemented | TS 104 176 11.4.2 | Resource definition | edited | 1.34 | 0 | 0 |
| [6.32.3.1](../spec/6/6.32.3.1.md) | GET | implemented | TS 104 176 11.4.3.1 | GET/HEAD | edited | 0.80 | 1 | 1 |
| [6.32.3.2](../spec/6/6.32.3.2.md) | PATCH | implemented | TS 104 176 11.4.3.2 | PATCH | edited | 1.38 | 1 | 1 |
| [6.32.3.3](../spec/6/6.32.3.3.md) | DELETE | implemented | TS 104 176 11.4.3.3 | DELETE | edited | 1.33 | 1 | 1 |
| [6.32.3](../spec/6/6.32.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.32](../spec/6/6.32.md) | Resource: entityMaps/{entityMapId} | informative | TS 104 176 11.4 | Resource: entityMaps/{entityMapId} | edited | 1.00 | 0 | 0 |
| [6.33.1](../spec/6/6.33.1.md) | Description | informative | TS 104 176 12.2.1 | Description | edited | 0.83 | 0 | 0 |
| [6.33.2](../spec/6/6.33.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.33.3.1](../spec/6/6.33.3.1.md) | GET | implemented | TS 104 176 12.2.3.1 | GET/HEAD | edited | 0.78 | 1 | 1 |
| [6.33.3](../spec/6/6.33.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.33](../spec/6/6.33.md) | Resource: info/sourceIdentity | informative | TS 104 176 12.2 | Resource: info/sourceIdentity | edited | 1.00 | 0 | 0 |
| [6.34.1](../spec/6/6.34.1.md) | Description | informative | TS 104 176 11.2.1 | Description | edited | 0.85 | 0 | 0 |
| [6.34.2](../spec/6/6.34.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.34.3.1](../spec/6/6.34.3.1.md) | GET | implemented | TS 104 176 11.2.3.1 | GET/HEAD | edited | 0.85 | 3 | 3 |
| [6.34.3.2](../spec/6/6.34.3.2.md) | POST | implemented | TS 104 176 11.2.3.2 | POST | moved | 1.41 | 1 | 1 |
| [6.34.3](../spec/6/6.34.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.34](../spec/6/6.34.md) | Resource: entityMaps | informative | TS 104 176 11.2 | Resource: entityMaps/ | edited | 1.00 | 0 | 0 |
| [6.35.1](../spec/6/6.35.1.md) | Description | informative | TS 104 176 11.3.1 | Description | edited | 0.89 | 0 | 0 |
| [6.35.2](../spec/6/6.35.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.35.3.1](../spec/6/6.35.3.1.md) | GET | implemented | TS 104 176 11.3.3.1 | GET/HEAD | edited | 0.87 | 2 | 2 |
| [6.35.3.2](../spec/6/6.35.3.2.md) | POST | implemented | TS 104 176 11.3.3.2 | POST | edited | 1.39 | 1 | 1 |
| [6.35.3](../spec/6/6.35.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.35](../spec/6/6.35.md) | Resource: temporal/entityMaps | informative | TS 104 176 11.3 | Resource: temporal/entityMaps | edited | 1.00 | 0 | 0 |
| [6.36.1](../spec/6/6.36.1.md) | Description | informative | TS 104 176 13.2.1 | Description | edited | 0.83 | 0 | 0 |
| [6.36.2](../spec/6/6.36.2.md) | Resource definition | implemented | TS 104 176 7.6.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.36.3.1](../spec/6/6.36.3.1.md) | POST | implemented | TS 104 176 13.2.3.1 | POST | edited | 1.33 | 2 | 2 |
| [6.36.3.2](../spec/6/6.36.3.2.md) | DELETE | implemented | TS 104 176 13.2.3.2 | DELETE | edited | 1.36 | 2 | 2 |
| [6.36.3](../spec/6/6.36.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.36](../spec/6/6.36.md) | Resource: snapshots | informative | TS 104 176 13.2 | Resource: snapshots/ | edited | 1.00 | 0 | 0 |
| [6.37.1](../spec/6/6.37.1.md) | Description | informative | TS 104 176 13.3.1 | Description | edited | 0.83 | 0 | 0 |
| [6.37.2](../spec/6/6.37.2.md) | Resource definition | implemented | TS 104 176 13.3.2 | Resource definition | edited | 1.30 | 0 | 0 |
| [6.37.3.1](../spec/6/6.37.3.1.md) | GET | implemented | TS 104 176 13.3.3.1 | GET/HEAD | edited | 0.87 | 1 | 1 |
| [6.37.3.2](../spec/6/6.37.3.2.md) | PATCH | implemented | TS 104 176 13.3.3.2 | PATCH | edited | 1.37 | 1 | 1 |
| [6.37.3.3](../spec/6/6.37.3.3.md) | DELETE | implemented | TS 104 176 13.3.3.3 | DELETE | edited | 1.37 | 1 | 1 |
| [6.37.3](../spec/6/6.37.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.37](../spec/6/6.37.md) | Resource: snapshots/{snapshotId} | informative | TS 104 176 13.3 | Resource: snapshots/{snapshotId} | edited | 1.00 | 0 | 0 |
| [6.38.1](../spec/6/6.38.1.md) | Description | informative | TS 104 176 13.4.1 | Description | edited | 0.87 | 0 | 0 |
| [6.38.2](../spec/6/6.38.2.md) | Resource definition | implemented | TS 104 176 13.4.2 | Resource definition | edited | 1.34 | 0 | 0 |
| [6.38.3.1](../spec/6/6.38.3.1.md) | POST | implemented | TS 104 176 13.4.3.1 | POST | edited | 0.82 | 4 | 2 |
| [6.38.3](../spec/6/6.38.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.38](../spec/6/6.38.md) | Resource: snapshots/{snapshotId}/clone | informative | TS 104 176 13.4 | Resource: snapshots/{snapshotId}/clone | edited | 1.00 | 0 | 0 |
| [6.4.1](../spec/6/6.4.1.md) | Description | informative | TS 104 176 7.2.1 | Description | edited | 0.85 | 0 | 0 |
| [6.4.2](../spec/6/6.4.2.md) | Resource definition | implemented | TS 104 176 13.4.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.4.3.1](../spec/6/6.4.3.1.md) | POST | implemented | TS 104 176 7.2.3.2 | POST | edited | 1.37 | 2 | 2 |
| [6.4.3.2](../spec/6/6.4.3.2.md) | GET | implemented | TS 104 176 7.2.3.1 | GET/HEAD | edited | 0.89 | 14 | 14 |
| [6.4.3.3](../spec/6/6.4.3.3.md) | DELETE | implemented | TS 104 176 7.2.3.3 | DELETE | edited | 1.36 | 9 | 10 |
| [6.4.3](../spec/6/6.4.3.md) | Resource methods | informative | TS 104 176 13.3.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.4](../spec/6/6.4.md) | Resource: entities/ | informative | TS 104 176 7.2 | Resource: entities/ | edited | 1.00 | 0 | 0 |
| [6.5.1](../spec/6/6.5.1.md) | Description | informative | TS 104 176 7.3.1 | Description | edited | 0.85 | 0 | 0 |
| [6.5.2](../spec/6/6.5.2.md) | Resource definition | implemented | TS 104 176 7.3.2 | Resource definition | edited | 1.33 | 0 | 0 |
| [6.5.3.1](../spec/6/6.5.3.1.md) | GET | implemented | TS 104 176 7.3.3.1 | GET/HEAD | edited | 0.90 | 4 | 4 |
| [6.5.3.2](../spec/6/6.5.3.2.md) | DELETE | implemented | TS 104 176 7.3.3.4 | DELETE | edited | 1.33 | 2 | 2 |
| [6.5.3.3](../spec/6/6.5.3.3.md) | PUT | implemented | TS 104 176 7.3.3.3 | PUT | edited | 1.39 | 2 | 2 |
| [6.5.3.4](../spec/6/6.5.3.4.md) | PATCH | implemented | TS 104 176 7.3.3.2 | PATCH | edited | 1.39 | 4 | 4 |
| [6.5.3](../spec/6/6.5.3.md) | Resource methods | informative | TS 104 176 13.3.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.5](../spec/6/6.5.md) | Resource: entities/{entityId} | informative | TS 104 176 7.3 | Resource: entities/{entityId} | edited | 1.00 | 0 | 0 |
| [6.6.1](../spec/6/6.6.1.md) | Description | informative | TS 104 176 7.4.1 | Description | edited | 0.88 | 0 | 0 |
| [6.6.2](../spec/6/6.6.2.md) | Resource definition | implemented | TS 104 176 7.4.2 | Resource definition | edited | 1.32 | 0 | 0 |
| [6.6.3.1](../spec/6/6.6.3.1.md) | POST | implemented | TS 104 176 7.4.3.1 | POST | edited | 1.39 | 3 | 3 |
| [6.6.3.2](../spec/6/6.6.3.2.md) | PATCH | implemented | TS 104 176 7.4.3.2 | PATCH | edited | 1.38 | 2 | 2 |
| [6.6.3](../spec/6/6.6.3.md) | Resource methods | informative | TS 104 176 13.3.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.6](../spec/6/6.6.md) | Resource: entities/{entityId}/attrs/ | informative | TS 104 176 7.4 | Resource: entities/{entityId}/attrs/ | edited | 1.00 | 0 | 0 |
| [6.7.1](../spec/6/6.7.1.md) | Description | informative | TS 104 176 7.5.1 | Description | edited | 0.87 | 0 | 0 |
| [6.7.2](../spec/6/6.7.2.md) | Resource definition | implemented | TS 104 176 7.5.2 | Resource definition | edited | 1.35 | 0 | 0 |
| [6.7.3.1](../spec/6/6.7.3.1.md) | PATCH | implemented | TS 104 176 7.5.3.2 | PATCH | edited | 1.28 | 2 | 2 |
| [6.7.3.2](../spec/6/6.7.3.2.md) | DELETE | implemented | TS 104 176 7.5.3.4 | DELETE | edited | 1.30 | 1 | 1 |
| [6.7.3.3](../spec/6/6.7.3.3.md) | PUT | implemented | TS 104 176 7.5.3.3 | PUT | edited | 1.32 | 2 | 2 |
| [6.7.3](../spec/6/6.7.3.md) | Resource methods | informative | TS 104 176 13.3.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.7](../spec/6/6.7.md) | Resource: entities/{entityId}/attrs/{attrId} | informative | TS 104 176 7.5 | Resource: entities/{entityId}/attrs/{attrId} | edited | 1.00 | 0 | 0 |
| [6.8.1](../spec/6/6.8.1.md) | Description | informative | TS 104 176 9.2.1 | Description | edited | 0.87 | 0 | 0 |
| [6.8.2](../spec/6/6.8.2.md) | Resource definition | implemented | TS 104 176 13.4.2 | Resource definition | edited | 1.00 | 0 | 0 |
| [6.8.3.1](../spec/6/6.8.3.1.md) | POST | implemented | TS 104 176 9.2.3.2 | POST | edited | 1.31 | 1 | 1 |
| [6.8.3.2](../spec/6/6.8.3.2.md) | GET | implemented | TS 104 176 9.2.3.1 | GET/HEAD | edited | 0.87 | 6 | 5 |
| [6.8.3](../spec/6/6.8.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.8](../spec/6/6.8.md) | Resource: csourceRegistrations/ | informative | TS 104 176 9.2 | Resource: csourceRegistrations/ | edited | 1.00 | 0 | 0 |
| [6.9.1](../spec/6/6.9.1.md) | Description | informative | TS 104 176 9.3.1 | Description | edited | 0.89 | 0 | 0 |
| [6.9.2](../spec/6/6.9.2.md) | Resource definition | implemented | TS 104 176 9.3.2 | Resource definition | edited | 1.32 | 0 | 0 |
| [6.9.3.1](../spec/6/6.9.3.1.md) | GET | implemented | TS 104 176 9.3.3.1 | GET/HEAD | edited | 0.85 | 1 | 1 |
| [6.9.3.2](../spec/6/6.9.3.2.md) | PATCH | implemented | TS 104 176 9.3.3.2 | PATCH | edited | 1.36 | 1 | 1 |
| [6.9.3.3](../spec/6/6.9.3.3.md) | DELETE | implemented | TS 104 176 9.3.3.3 | DELETE | edited | 1.39 | 1 | 1 |
| [6.9.3](../spec/6/6.9.3.md) | Resource methods | informative | TS 104 176 7.5.3 | Resource methods | edited | 1.00 | 0 | 0 |
| [6.9](../spec/6/6.9.md) | Resource: csourceRegistrations/{registrationId} | informative | TS 104 176 9.3 | Resource: csourceRegistrations/{registrationId} | edited | 1.00 | 0 | 0 |
| [6](../spec/6/6.md) | API HTTP Binding | informative | TS 104 175 16.7.4 | Behaviour | edited | 0.75 | 2 | 2 |
| [7.1](../spec/7/7.1.md) | Introduction | informative | TS 104 243 5.1 | Introduction | edited | 0.74 | 0 | 0 |
| [7.2](../spec/7/7.2.md) | Notification behaviour | implemented | TS 104 243 5.2 | Notification behaviour | edited | 1.06 | 9 | 2 |
| [7](../spec/7/7.md) | API MQTT Notification Binding | informative | TS 104 176 13.4.3.1 | POST | edited | 0.83 | 1 | 2 |
| [A.1](../spec/A/A.1.md) | Introduction | informative | TS 104 175 A.1 | Introduction | edited | 0.95 | 0 | 0 |
| [A.2](../spec/A/A.2.md) | Entity identifiers | implemented | TS 104 175 A.2 | Entity identifiers | moved | 1.50 | 0 | 0 |
| [A.3](../spec/A/A.3.md) | NGSI-LD namespace | implemented | TS 104 175 A.3 | NGSI-LD namespace | edited | 0.95 | 1 | 0 |
| [A](../spec/A/A.md) | Annex A (normative): NGSI-LD identifier considerations | informative | TS 104 175 A | Annex A (normative): NGSI-LD identifier considerations | edited | 1.00 | 0 | 0 |
| [B](../spec/B/B.md) | Annex B (normative): Core NGSI-LD @context definition | implemented | TS 104 175 B | Annex B (normative): Core NGSI-LD @context definition | edited | 1.17 | 0 | 0 |
| [C.1](../spec/C/C.1.md) | Introduction | informative | TS 104 175 C.1 | Introduction | edited | 0.97 | 0 | 0 |
| [C.10](../spec/C/C.10.md) | ValueType datatype utilization clarifications | informative | TS 104 175 C.5 | ValueType datatype utilization clarifications | moved | 1.48 | 0 | 2 |
| [C.11](../spec/C/C.11.md) | Entity with digital signature for a Property | informative | TS 104 175 C.9 | Entity with digital signature for a Property | edited | 0.84 | 2 | 0 |
| [C.2.1](../spec/C/C.2.1.md) | Property Graph | informative | TS 104 175 C.2.1 | Property graph | moved | 1.47 | 0 | 0 |
| [C.2.2](../spec/C/C.2.2.md) | Vehicle Entity | informative | TS 104 175 C.2.2 | Vehicle Entity | edited | 0.50 | 1 | 0 |
| [C.2.3](../spec/C/C.2.3.md) | Parking Entity | informative | TS 104 175 C.2.3 | Parking Entity | edited | 0.51 | 0 | 0 |
| [C.2.4](../spec/C/C.2.4.md) | @context | informative | TS 104 175 C.2.4 | @context | moved | 1.49 | 0 | 0 |
| [C.2](../spec/C/C.2.md) | Entity Representation | informative | TS 104 175 C.2 | Entity representation | edited | 1.00 | 0 | 0 |
| [C.3](../spec/C/C.3.md) | Context Source Registration | informative | TS 104 175 C.3 | Context Source Registration | edited | 1.39 | 0 | 0 |
| [C.4](../spec/C/C.4.md) | Context Subscription | informative | TS 104 175 C.4 | Context Subscription | moved | 1.50 | 0 | 0 |
| [C.5.1](../spec/C/C.5.1.md) | Introduction | informative | TS 104 176 A.1 | Introduction | edited | 0.95 | 0 | 0 |
| [C.5.10.1](../spec/C/C.5.10.1.md) | Introduction | informative | TS 104 176 A.5.1.1 | Introduction | edited | 0.90 | 0 | 0 |
| [C.5.10.2](../spec/C/C.5.10.2.md) | HTTP Request | informative | TS 104 176 A.4.3.2 | HTTP request | edited | 0.85 | 0 | 0 |
| [C.5.10.3](../spec/C/C.5.10.3.md) | HTTP Response | informative | TS 104 176 A.5.1.3 | HTTP response | edited | 0.77 | 0 | 0 |
| [C.5.10](../spec/C/C.5.10.md) | Retrieve Available Attributes | informative | TS 104 175 10.6.6 | Retrieve available attributes | edited | 1.00 | 0 | 0 |
| [C.5.11.1](../spec/C/C.5.11.1.md) | Introduction | informative | TS 104 176 A.5.2.1 | Introduction | edited | 0.91 | 0 | 0 |
| [C.5.11.2](../spec/C/C.5.11.2.md) | HTTP Request | informative | TS 104 176 A.5.2.2 | HTTP request | edited | 0.86 | 0 | 0 |
| [C.5.11.3](../spec/C/C.5.11.3.md) | HTTP Response | informative | TS 104 176 A.5.2.3 | HTTP response | edited | 0.86 | 0 | 0 |
| [C.5.11](../spec/C/C.5.11.md) | Retrieve Details of Available Attributes | informative | TS 104 176 A.5.2 | Retrieve details of available Attributes | edited | 1.00 | 0 | 0 |
| [C.5.12.1](../spec/C/C.5.12.1.md) | Introduction | informative | TS 104 176 A.5.3.1 | Introduction | edited | 0.90 | 0 | 0 |
| [C.5.12.2](../spec/C/C.5.12.2.md) | HTTP Request | informative | TS 104 176 A.5.3.2 | HTTP request | edited | 0.89 | 0 | 0 |
| [C.5.12.3](../spec/C/C.5.12.3.md) | HTTP Response | informative | TS 104 176 A.5.3.3 | HTTP response | edited | 0.64 | 0 | 0 |
| [C.5.12](../spec/C/C.5.12.md) | Retrieve Available Attribute Information | informative | TS 104 175 10.6.8 | Retrieve available Attribute information | edited | 1.00 | 0 | 0 |
| [C.5.13.1](../spec/C/C.5.13.1.md) | Introduction | informative | TS 104 176 A.2.4.1 | Introduction | edited | 0.93 | 0 | 0 |
| [C.5.13.2](../spec/C/C.5.13.2.md) | HTTP Request | informative | TS 104 176 A.2.4.2 | HTTP request | edited | 0.83 | 0 | 0 |
| [C.5.13.3](../spec/C/C.5.13.3.md) | HTTP Response | informative | TS 104 176 A.2.4.3 | HTTP response | edited | 0.80 | 0 | 0 |
| [C.5.13](../spec/C/C.5.13.md) | Query Entities (Natural Language Filtering) | informative | TS 104 176 A.2.4 | Query Entities (natural language filtering) | edited | 1.00 | 0 | 0 |
| [C.5.14.1](../spec/C/C.5.14.1.md) | Introduction | informative | TS 104 176 A.3.3.1 | Introduction | edited | 0.93 | 0 | 0 |
| [C.5.14.2](../spec/C/C.5.14.2.md) | HTTP Request | informative | TS 104 176 A.3.3.2 | HTTP request | edited | 0.81 | 0 | 0 |
| [C.5.14.3](../spec/C/C.5.14.3.md) | HTTP Response | informative | TS 104 176 A.3.3.3 | HTTP response | edited | 0.87 | 0 | 0 |
| [C.5.14](../spec/C/C.5.14.md) | Temporal Query (Aggregated Representation) | informative | TS 104 176 A.3.3 | Temporal Query (aggregated representation) | edited | 1.00 | 0 | 0 |
| [C.5.15.1](../spec/C/C.5.15.1.md) | Introduction | informative | TS 104 176 A.2.6.1 | Introduction | edited | 0.86 | 0 | 0 |
| [C.5.15.2](../spec/C/C.5.15.2.md) | HTTP Request | informative | TS 104 176 A.2.6.2 | HTTP request | edited | 0.80 | 0 | 0 |
| [C.5.15.3](../spec/C/C.5.15.3.md) | HTTP Response | informative | TS 104 176 A.2.6.3 | HTTP response | edited | 0.92 | 0 | 0 |
| [C.5.15](../spec/C/C.5.15.md) | Scope Queries | informative | TS 104 175 C.10 | Scope Queries | edited | 1.00 | 0 | 0 |
| [C.5.16.1](../spec/C/C.5.16.1.md) | Introduction | informative | TS 104 176 A.3.4.1 | Introduction | edited | 0.96 | 0 | 0 |
| [C.5.16.2](../spec/C/C.5.16.2.md) | HTTP Request | informative | TS 104 176 A.3.4.2 | HTTP request | edited | 0.86 | 0 | 0 |
| [C.5.16.3](../spec/C/C.5.16.3.md) | HTTP Response | informative | TS 104 176 A.3.4.3 | HTTP response | edited | 0.94 | 0 | 0 |
| [C.5.16](../spec/C/C.5.16.md) | Temporal Scope Queries | informative | TS 104 176 A.3.4 | Temporal Scope Queries | edited | 1.00 | 0 | 0 |
| [C.5.2.1](../spec/C/C.5.2.1.md) | HTTP Request | informative |   |  | unmatched | 0.26 |  |  |
| [C.5.2.2](../spec/C/C.5.2.2.md) | HTTP Response | informative | TS 104 176 A.2.1.3 | HTTP response | edited | 0.50 | 0 | 0 |
| [C.5.2](../spec/C/C.5.2.md) | Create Entity of Type Vehicle | informative | TS 104 176 A.2.1 | Create Entity of Type Vehicle | edited | 1.00 | 0 | 0 |
| [C.5.3.1](../spec/C/C.5.3.1.md) | Introduction | informative | TS 104 176 A.2.2.1 | Introduction | edited | 0.91 | 0 | 0 |
| [C.5.3.2](../spec/C/C.5.3.2.md) | HTTP Request | informative | TS 104 176 A.2.4.2 | HTTP request | edited | 0.76 | 0 | 0 |
| [C.5.3.3](../spec/C/C.5.3.3.md) | HTTP Response | informative | TS 104 176 A.2.2.3.1 | JSON-LD Response | edited | 0.77 | 0 | 0 |
| [C.5.3](../spec/C/C.5.3.md) | Query Entities | informative | TS 104 175 10.4.3 | Query Entities | edited | 1.00 | 0 | 0 |
| [C.5.4.1](../spec/C/C.5.4.1.md) | Introduction | informative | TS 104 176 A.2.3.1 | Introduction | edited | 0.89 | 0 | 0 |
| [C.5.4.2](../spec/C/C.5.4.2.md) | HTTP Request | informative | TS 104 176 A.2.4.2 | HTTP request | edited | 0.80 | 0 | 0 |
| [C.5.4.3](../spec/C/C.5.4.3.md) | HTTP Response | informative | TS 104 176 A.3.4.3 | HTTP response | edited | 0.49 | 0 | 0 |
| [C.5.4](../spec/C/C.5.4.md) | Query Entities (Pagination) | informative | TS 104 176 A.2.3 | Query Entities (pagination) | edited | 1.00 | 0 | 0 |
| [C.5.5.1](../spec/C/C.5.5.1.md) | Introduction | informative | TS 104 176 A.3.1.1 | Introduction | edited | 0.95 | 0 | 0 |
| [C.5.5.2](../spec/C/C.5.5.2.md) | HTTP Request #1 | informative | TS 104 176 A.3.1.2 | HTTP request #1 | edited | 1.29 | 0 | 0 |
| [C.5.5.3](../spec/C/C.5.5.3.md) | HTTP Response #1 | informative | TS 104 176 A.3.1.3 | HTTP response #1 | edited | 1.36 | 0 | 0 |
| [C.5.5.4](../spec/C/C.5.5.4.md) | HTTP Request #2 | informative | TS 104 176 A.3.1.4 | HTTP request #2 | edited | 1.19 | 0 | 0 |
| [C.5.5.5](../spec/C/C.5.5.5.md) | HTTP Response #2 | informative | TS 104 176 A.3.1.5 | HTTP response #2 | edited | 1.36 | 0 | 0 |
| [C.5.5](../spec/C/C.5.5.md) | Temporal Query | informative | TS 104 176 A.3.1 | Temporal Query | edited | 1.00 | 0 | 0 |
| [C.5.6.1](../spec/C/C.5.6.1.md) | Introduction | informative | TS 104 176 A.3.2.1 | Introduction | edited | 0.93 | 0 | 0 |
| [C.5.6.2](../spec/C/C.5.6.2.md) | HTTP Request | informative | TS 104 176 A.3.2.2 | HTTP request | edited | 0.79 | 0 | 0 |
| [C.5.6.3](../spec/C/C.5.6.3.md) | HTTP Response | informative | TS 104 176 A.3.2.3 | HTTP response | edited | 0.85 | 0 | 0 |
| [C.5.6](../spec/C/C.5.6.md) | Temporal Query (Simplified Representation) | informative | TS 104 176 A.3.2 | Temporal Query (simplified representation) | edited | 1.00 | 0 | 0 |
| [C.5.7.1](../spec/C/C.5.7.1.md) | Introduction | informative | TS 104 176 A.4.1.1 | Introduction | edited | 0.86 | 0 | 0 |
| [C.5.7.2](../spec/C/C.5.7.2.md) | HTTP Request | informative | TS 104 176 A.4.1.2 | HTTP request | edited | 0.85 | 0 | 0 |
| [C.5.7.3](../spec/C/C.5.7.3.md) | HTTP Response | informative | TS 104 176 A.4.1.3 | HTTP response | edited | 0.76 | 0 | 0 |
| [C.5.7](../spec/C/C.5.7.md) | Retrieve Available Entity Types | informative | TS 104 176 A.4.1 | Retrieve available Entity Types | edited | 1.00 | 0 | 0 |
| [C.5.8.1](../spec/C/C.5.8.1.md) | Introduction | informative | TS 104 176 A.4.2.1 | Introduction | edited | 0.88 | 0 | 0 |
| [C.5.8.2](../spec/C/C.5.8.2.md) | HTTP Request | informative | TS 104 176 A.4.2.2 | HTTP request | edited | 0.86 | 0 | 0 |
| [C.5.8.3](../spec/C/C.5.8.3.md) | HTTP Response | informative | TS 104 176 A.4.2.3 | HTTP response | edited | 0.84 | 0 | 0 |
| [C.5.8](../spec/C/C.5.8.md) | Retrieve Details of Available Entity Types | informative | TS 104 176 A.4.2 | Retrieve details of available Entity Types | edited | 1.00 | 0 | 0 |
| [C.5.9.1](../spec/C/C.5.9.1.md) | Introduction | informative | TS 104 176 A.4.3.1 | Introduction | edited | 0.88 | 0 | 0 |
| [C.5.9.2](../spec/C/C.5.9.2.md) | HTTP Request | informative | TS 104 176 A.4.3.2 | HTTP request | edited | 0.90 | 0 | 0 |
| [C.5.9.3](../spec/C/C.5.9.3.md) | HTTP Response | informative | TS 104 176 A.4.3.3 | HTTP response | edited | 0.80 | 0 | 0 |
| [C.5.9](../spec/C/C.5.9.md) | Retrieve Available Entity Type Information | informative | TS 104 175 10.6.5 | Retrieve available Entity Type information | edited | 1.00 | 0 | 0 |
| [C.5](../spec/C/C.5.md) | HTTP REST API Examples | informative |   |  | heading | 0.00 |  |  |
| [C.6](../spec/C/C.6.md) | Date Representation | informative | TS 104 175 C.5 | ValueType datatype utilization clarifications | edited | 0.96 | 2 | 2 |
| [C.7](../spec/C/C.7.md) | @context utilization clarifications | informative | TS 104 175 C.6 | @context utilization clarifications | moved | 1.48 | 0 | 0 |
| [C.8](../spec/C/C.8.md) | Link header utilization clarifications | informative | TS 104 175 C.7 | JSON-LD Link header utilization clarifications | edited | 0.93 | 0 | 0 |
| [C.9](../spec/C/C.9.md) | @context processing clarifications | informative | TS 104 175 C.8 | @context processing clarifications | moved | 1.47 | 2 | 2 |
| [C](../spec/C/C.md) | Annex C (informative): Examples of using the API | informative | TS 104 175 C | Annex C (informative): Examples of using the API | edited | 1.00 | 0 | 0 |
| [D.1](../spec/D/D.1.md) | Introduction | informative |   |  | unmatched | 0.39 |  |  |
| [D.2](../spec/D/D.2.md) | Algorithm for transforming an NGSI-LD Entity into a JSON-LD document (ALG1) | informative |   |  | unmatched | 0.04 |  |  |
| [D.3](../spec/D/D.3.md) | Algorithm for transforming an NGSI-LD Property into JSON-LD (ALG1.1) | informative |   |  | unmatched | 0.04 |  |  |
| [D.4](../spec/D/D.4.md) | Algorithm for transforming an NGSI-LD Relationship into JSON-LD (ALG1.2) | informative |   |  | unmatched | 0.07 |  |  |
| [D](../spec/D/D.md) | Annex D (informative): Transformation Algorithms | informative |   |  | heading | 0.00 |  |  |
| [E](../spec/E/E.md) | Annex E (informative): RDF-compatible specification of NGSI-LD meta-model | informative | TS 104 175 E | Annex E (informative): Conventions and syntax guidelines | edited | 0.88 | 0 | 0 |
| [F](../spec/F/F.md) | Annex F (informative): Conventions and syntax guidelines | informative |   |  | unmatched | 0.43 |  |  |
| [G.0](../spec/G/G.0.md) | Foreword | informative | TS 104 175 F.1 | Introduction | edited | 0.92 | 1 | 1 |
| [G.1.0](../spec/G/G.1.0.md) | Foreword | informative | TS 104 175 F.2.1 | Introduction | edited | 0.88 | 0 | 0 |
| [G.1.1](../spec/G/G.1.1.md) | Associating an Entity with a Natural Language | informative | TS 104 175 F.2.2 | Associating an Entity with a natural language | moved | 1.46 | 0 | 0 |
| [G.1.2](../spec/G/G.1.2.md) | Associating a Property with a Natural Language | informative | TS 104 175 F.2.3 | Associating an Attribute with a natural language | weak | 0.50 | 1 | 1 |
| [G.1.3](../spec/G/G.1.3.md) | Associating as equivalent entity | informative | TS 104 175 F.2.4 | Associating as equivalent Entity | edited | 1.38 | 2 | 2 |
| [G.1](../spec/G/G.1.md) | Introduction | informative |   |  | heading | 0.00 |  |  |
| [G.2.0](../spec/G/G.2.0.md) | Foreword | informative | TS 104 175 F.3.1 | Introduction | edited | 0.84 | 0 | 0 |
| [G.2.1](../spec/G/G.2.1.md) | Maintain collations as metadata | informative | TS 104 175 F.3.2 | Maintain collations as metadata | moved | 1.45 | 0 | 0 |
| [G.2.2](../spec/G/G.2.2.md) | Route language sensitive queries via a proxy | informative | TS 104 175 F.3.3 | Route language sensitive queries via a proxy | moved | 1.47 | 0 | 0 |
| [G.2](../spec/G/G.2.md) | Natural Language Collation Support | informative | TS 104 175 F.3 | Natural language collation support | edited | 1.00 | 0 | 0 |
| [G.3.0](../spec/G/G.3.0.md) | Foreword | informative | TS 104 175 F.4.1 | Introduction | edited | 0.96 | 0 | 0 |
| [G.3.1](../spec/G/G.3.1.md) | Localizing Dates | informative | TS 104 175 F.4.2 | Localizing dates | edited | 0.68 | 2 | 0 |
| [G.3](../spec/G/G.3.md) | Localization of Dates, Currency formats, etc. | informative | TS 104 175 F.4 | Localization of dates, currency formats, etc. | edited | 1.00 | 0 | 0 |
| [G](../spec/G/G.md) | Annex G (informative): Localization and Internationalization Support | informative |   |  | heading | 0.00 |  |  |
| [H.1](../spec/H/H.1.md) | Actuators and feedback to the consumer | informative | TS 104 175 G.1 | Actuators and feedback to the consumer | moved | 1.50 | 0 | 0 |
| [H.2](../spec/H/H.2.md) | Architecture for actuation | informative | TS 104 175 G.2 | Architecture for actuation | moved | 1.48 | 0 | 0 |
| [H.3.0](../spec/H/H.3.0.md) | Introduction | informative | TS 104 175 G.3.1 | Introduction | edited | 0.96 | 0 | 0 |
| [H.3.1](../spec/H/H.3.1.md) | Property for listing available commands | informative | TS 104 175 G.3.2 | Property for listing available commands | moved | 1.44 | 0 | 0 |
| [H.3.2](../spec/H/H.3.2.md) | Properties for command endpoints | informative | TS 104 175 G.3.3 | Properties for command endpoints | moved | 1.48 | 0 | 0 |
| [H.3](../spec/H/H.3.md) | Structure of Commands and additional Properties | informative | TS 104 175 G.3 | Structure of commands and additional Properties | edited | 1.00 | 0 | 0 |
| [H.4.1](../spec/H/H.4.1.md) | Possible communication models | informative | TS 104 175 G.4.1 | Possible communication models | moved | 1.45 | 0 | 0 |
| [H.4.2](../spec/H/H.4.2.md) | Subscription/notification model | informative | TS 104 175 G.4.2 | Subscription/notification model | moved | 1.46 | 0 | 0 |
| [H.4.3](../spec/H/H.4.3.md) | Forwarding model | informative | TS 104 175 G.4.3 | Forwarding model | moved | 1.43 | 0 | 0 |
| [H.4](../spec/H/H.4.md) | Communication model | informative | TS 104 175 G.4 | Communication model | edited | 1.00 | 0 | 0 |
| [H.5](../spec/H/H.5.md) | Implementation of the subscription-based actuation workflow | informative | TS 104 175 G.5 | Implementation of the subscription-based actuation workflow | moved | 1.45 | 1 | 1 |
| [H.6](../spec/H/H.6.md) | Implementation of the registration-based actuation workflow | informative | TS 104 175 G.6 | Implementation of the registration-based actuation workflow | edited | 0.86 | 2 | 0 |
| [H](../spec/H/H.md) | Annex H (informative): Suggested actuation workflows | informative |   |  | heading | 0.00 |  |  |
| [I](../spec/I/I.md) | Annex I (informative): Change history | informative |   |  | unmatched | 0.01 |  |  |
|  |  |  | TS 104 175 10.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.10.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.11.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.12.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.5.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.6.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.6.2 | Use case diagram | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.6.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.6.4 | Behaviour | new-only |  |  | 8 |
|  |  |  | TS 104 175 10.2.6.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.6 | Set Attribute Value | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.7.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.2.8.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.3.5.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.3 | Context information provision - batch operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.4.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.4.4.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.4.4.2 | Use case diagram | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.4.4.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.4.4.4 | Behaviour | new-only |  |  | 11 |
|  |  |  | TS 104 175 10.4.4.5 | Output data | new-only |  |  | 8 |
|  |  |  | TS 104 175 10.4.4 | Retrieve Attribute Value | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.2.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.3.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.4.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.5.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.6.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.5.6.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.3.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.3 | Retrieve available Entity Types | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.4.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.4 | Retrieve details of available Entity Types | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.6.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.7.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6.7 | Retrieve details of available Attributes | new-only |  |  | 0 |
|  |  |  | TS 104 175 10.6 | Context information availability | new-only |  |  | 0 |
|  |  |  | TS 104 175 10 | Core API operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.3.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.4.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.5.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.6.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.7.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2.7.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.2 | Temporal context information provision | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 11.3 | Temporal context information consumption | new-only |  |  | 0 |
|  |  |  | TS 104 175 11 | Temporal API operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.2.1 | Introduction | new-only |  |  | 1 |
|  |  |  | TS 104 175 12.2.4.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.2.3 | Input | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.3.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.4.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.5.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.6.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12.4.6.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 12 | Registry API operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 13.5.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 13 | Storing, managing and serving @contexts operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.2.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.2.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.2 | Retrieve Entity Map | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.3.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.3.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.3 | Update Entity Map | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.4.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.4.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.4.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.4 | Delete Entity Map | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.5 | Create Entity Map for query Entities | new-only |  |  | 0 |
|  |  |  | TS 104 175 14.6.5 | Output Data | new-only |  |  | 1 |
|  |  |  | TS 104 175 14.6 | Create Entity Map for query Temporal Evolution of Entities | new-only |  |  | 0 |
|  |  |  | TS 104 175 14 | Context Source Entity mapping operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 15.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 15.2.3 | Input data | new-only |  |  | 0 |
|  |  |  | TS 104 175 15 | Context Source identity information operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.2.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.3.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.5.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.6.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.7.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 175 16.7.5 | Output data | new-only |  |  | 0 |
|  |  |  | TS 104 175 16 | Snapshot functionality operations | new-only |  |  | 0 |
|  |  |  | TS 104 175 2 | References | new-only |  |  | 0 |
|  |  |  | TS 104 175 3.1 | Terms | new-only |  |  | 1 |
|  |  |  | TS 104 175 3.3 | Abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 175 3 | Definition of terms, symbols and abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3.2 | Registrations | new-only |  |  | 3 |
|  |  |  | TS 104 175 4.3.3.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3.3.2 | Matching registrations | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3.3.3 | Contacting context sources | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3.3.4 | Aggregating and processing of results | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3.3 | Distributed request execution | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.3 | Distributed and federated architectures | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.4.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.4.2 | Expansion and compaction | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.4.3 | Core @context | new-only |  |  | 1 |
|  |  |  | TS 104 175 4.3.4.4 | User @context | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3.4 | JSON-LD | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.3 | NGSI-LD architectures | new-only |  |  | 0 |
|  |  |  | TS 104 175 4.4 | API overview | new-only |  |  | 0 |
|  |  |  | TS 104 175 4 | NGSI-LD conceptual overview (informative) | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.2 | Data representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.5 | Matrix of all Attributes | new-only |  |  | 2 |
|  |  |  | TS 104 175 5.2.6.10 | Availability data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.11 | Geographical data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.4 | Representational data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.5 | Operational data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.6 | Structural data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.7 | Parameter data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.8 | Results and errors data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2.6.9 | Notificational data types | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.2 | NGSI-LD modelling, data types and data representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.1 | Introduction | new-only |  |  | 2 |
|  |  |  | TS 104 175 5.3.2 | JSON-LD | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3.3 | Entities Normalized | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3.4.1 | Representation of an individual Entity | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3.4.2 | Representation of multiple Entities | new-only |  |  | 1 |
|  |  |  | TS 104 175 5.3.3.4 | Entities Concise | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3.5.1 | Representation of an individual Entity | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3.5.2 | Representation of multiple Entities | new-only |  |  | 1 |
|  |  |  | TS 104 175 5.3.3.5 | Entities Simplified | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3.3 | GeoJSON | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.3 | Data interchange formats | new-only |  |  | 0 |
|  |  |  | TS 104 175 5.4.2 | Attribute fragments | new-only |  |  | 11 |
|  |  |  | TS 104 175 5.4 | NGSI-LD fragments | new-only |  |  | 0 |
|  |  |  | TS 104 175 5 | Context information and representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.2.3.1 | Introduction | new-only |  |  | 1 |
|  |  |  | TS 104 175 7.2.3.3 | Query Terms and Logical Operators | new-only |  |  | 15 |
|  |  |  | TS 104 175 7.2.3.4 | Query Term Equality | new-only |  |  | 2 |
|  |  |  | TS 104 175 7.2.3 | Query Language | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.2 | Filtering | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.3.3 | Attribute filtering (deprecated) | new-only |  |  | 1 |
|  |  |  | TS 104 175 7.3 | Projection | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.4.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.4.2.5 | Temporal pagination | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.4.2 | Pagination according to Elements | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.4 | Pagination | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.6.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.6.2 | Entity ordering language | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.6 | Ordering | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.7.4 | Depth of retrieval | new-only |  |  | 3 |
|  |  |  | TS 104 175 7.7 | Linked Entity retrieval | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.8 | Tenants | new-only |  |  | 0 |
|  |  |  | TS 104 175 7.9 | Snapshots | new-only |  |  | 0 |
|  |  |  | TS 104 175 7 | API functionality | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.2.5 | Implicit creation of @context | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.2 | JSON-LD processing | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.3 | Errors and error handling | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.4.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.4 | Common operation behaviours | new-only |  |  | 0 |
|  |  |  | TS 104 175 8.5 | Multi-Attribute behaviour | new-only |  |  | 0 |
|  |  |  | TS 104 175 8 | Common API behaviours | new-only |  |  | 0 |
|  |  |  | TS 104 175 9.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 175 9.3.4 | Precedence of distributed operations | new-only |  |  | 5 |
|  |  |  | TS 104 175 9.3 | Registrations | new-only |  |  | 0 |
|  |  |  | TS 104 175 9.4.1 | Introduction | new-only |  |  | 1 |
|  |  |  | TS 104 175 9.4.2 | Matching algorithm | new-only |  |  | 8 |
|  |  |  | TS 104 175 9.5 | Contacting Context Sources | new-only |  |  | 0 |
|  |  |  | TS 104 175 9 | Distributed behaviour | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.1.1 | Normalized Entity example | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.1.2 | Normalized Entity when inline Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.1.3 | Normalized Entities when flattened Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.1.4 | Normalized Entity when language filter is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.1 | Normalized format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.2.1 | Concise Entity example | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.2.2 | Concise Entity when inline Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.2.3 | Concise Entities when flattened Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.2.4 | Concise Entity when Language Filter is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.2 | Concise format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.3.1 | Simplified Entity example | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.3.2 | Simplified Entity when inline Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.3.3 | Simplified Entities when flattened Linked Entity retrieval is used | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.3.4 | Simplified Entity with natural language Attributes retrieval | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.3 | Simplified format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.4.1 | Normalized multi-Attribute example | new-only |  |  | 1 |
|  |  |  | TS 104 175 C.2.2.4.2 | Simplified multi-Attribute example | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.2.4.3 | Normalized Temporal multi-Attribute example | new-only |  |  | 1 |
|  |  |  | TS 104 175 C.2.2.4.4 | Simplified Temporal multi-Attribute example | new-only |  |  | 1 |
|  |  |  | TS 104 175 C.2.2.4.5 | Aggregated Temporal multi-Attribute example | new-only |  |  | 1 |
|  |  |  | TS 104 175 C.2.2.4 | Multi-Attribute | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.1.1 | Normalized JSON-LD representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.1.2 | Normalized GeoJSON representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.1 | Normalized format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.2.1 | Concise JSON-LD representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.2 | Concise format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.3.1 | Simplified JSON-LD representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.3.2 | Simplified GeoJSON representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.2.3.3 | Simplified format | new-only |  |  | 0 |
|  |  |  | TS 104 175 C.3.2.2.2 | Concise GeoJSON representation | new-only |  |  | 0 |
|  |  |  | TS 104 175 D | Annex D (informative): RDF-compatible specification of NGSI-LD meta-model | new-only |  |  | 0 |
|  |  |  | TS 104 175 F.2 | Localization and internationalization | new-only |  |  | 0 |
|  |  |  | TS 104 175 F | Annex F (informative): Localization and internationalization support | new-only |  |  | 0 |
|  |  |  | TS 104 175 G | Annex G (informative): Suggested actuation workflows | new-only |  |  | 0 |
|  |  |  | TS 104 176 1 | Scope | new-only |  |  | 0 |
|  |  |  | TS 104 176 10.1 | Introduction | new-only |  |  | 1 |
|  |  |  | TS 104 176 10.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 10.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 10.3.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 10 | Storing, Managing and Serving @contexts Resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.3.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.3.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 11.4.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 11 | Context Source Entity mapping resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 12.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 12.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 12.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 12 | Context Source identity information resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 13.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 13.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 13.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 13.4.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 13 | Snapshot functionality resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 2.1 | Normative references | new-only |  |  | 1 |
|  |  |  | TS 104 176 2.2 | Informative references | new-only |  |  | 2 |
|  |  |  | TS 104 176 2 | References | new-only |  |  | 0 |
|  |  |  | TS 104 176 3.2 | Symbols | new-only |  |  | 0 |
|  |  |  | TS 104 176 3.3 | Abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 176 3 | Definition of terms, symbols and abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 176 4 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.2.1 | Introduction | new-only |  |  | 1 |
|  |  |  | TS 104 176 6.2 | General mapping to HTTP | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.3.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.3 | Errors and error handling | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.4.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.4.7.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.4.7.3 | Temporal pagination | new-only |  |  | 14 |
|  |  |  | TS 104 176 6.4.7 | Pagination | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.4 | Mapping NGSI-LD features to URI parameters and HTTP headers | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.5.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 6.5 | Common operation behaviours | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.10.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.10.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.11.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.11.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.12.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.12.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.13.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.13.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.14.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.15.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.15.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.16.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.17.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.17.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.18.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.3.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.4.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.5.3.1 | GET/HEAD | new-only |  |  | 7 |
|  |  |  | TS 104 176 7.6.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.6.3.1 | GET/HEAD | new-only |  |  | 7 |
|  |  |  | TS 104 176 7.6.3.2 | PUT | new-only |  |  | 5 |
|  |  |  | TS 104 176 7.6.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.6 | Resource: entities/{entityId}/attrs/{attrId}/value | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.7.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.7.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.8.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.8.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.9.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 7.9.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 7 | Core API resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.3.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.4.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.5.1 | Description | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.5.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.6.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.6 | Resource: temporal/entities/{entityId}/attrs/{attrId}/{instanceId} | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.7.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 8.7.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 8 | Temporal API resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.2.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.2.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.3.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.4.2 | Resource definition | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.4.3.1 | GET/HEAD | new-only |  |  | 3 |
|  |  |  | TS 104 176 9.4.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 9.5.3 | Resource methods | new-only |  |  | 0 |
|  |  |  | TS 104 176 9 | Registry API resources | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.1.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.1.2.1 | Creation using a JSON-LD body and no Link header | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.1.2.2 | Creation using a JSON body and Link Header | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.1.2 | HTTP request | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2.2.1 | Query requesting a JSON-LD Response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2.2.2 | Query requesting a JSON Response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2.2 | HTTP request | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2.3.2 | JSON Response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2.3 | HTTP response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.2 | Query Entities | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.3.2 | HTTP request | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.3.3 | HTTP response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.5.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.5.2 | HTTP request | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.5.3 | HTTP response | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.5 | Query Entities (preference) | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2.6 | Scope Queries | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.2 | Entity Examples | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.3.1.6 | HTTP request #3 | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.3.1.7 | HTTP response #3 | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.3 | Temporal Entity Examples | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.4.3 | Retrieve available Entity Type information | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.4 | Type Examples | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.5.1.2 | HTTP request | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.5.1 | Retrieve available Attributes | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.5.3 | Retrieve available Attribute information | new-only |  |  | 0 |
|  |  |  | TS 104 176 A.5 | Attribute Examples | new-only |  |  | 0 |
|  |  |  | TS 104 176 A | Annex A (informative): Examples of using the HTTP binding of the API | new-only |  |  | 0 |
|  |  |  | TS 104 176 B | Annex B (informative): Conventions and syntax guidelines | new-only |  |  | 1 |
|  |  |  | TS 104 243 1 | Scope | new-only |  |  | 0 |
|  |  |  | TS 104 243 2.1 | Normative references | new-only |  |  | 1 |
|  |  |  | TS 104 243 2.2 | Informative references | new-only |  |  | 2 |
|  |  |  | TS 104 243 2 | References | new-only |  |  | 0 |
|  |  |  | TS 104 243 3.1 | Terms | new-only |  |  | 0 |
|  |  |  | TS 104 243 3.2 | Symbols | new-only |  |  | 0 |
|  |  |  | TS 104 243 3.3 | Abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 243 3 | Definition of terms, symbols and abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 243 4 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 243 5 | API MQTT Notification Binding | new-only |  |  | 0 |
|  |  |  | TS 104 243 A | Annex A (informative): Change history | new-only |  |  | 0 |
|  |  |  | TS 104 179 1 | Scope | new-only |  |  | 0 |
|  |  |  | TS 104 179 2.1 | Normative references | new-only |  |  | 2 |
|  |  |  | TS 104 179 2.2 | Informative references | new-only |  |  | 2 |
|  |  |  | TS 104 179 2 | References | new-only |  |  | 0 |
|  |  |  | TS 104 179 3.1 | Terms | new-only |  |  | 0 |
|  |  |  | TS 104 179 3.2 | Symbols | new-only |  |  | 0 |
|  |  |  | TS 104 179 3.3 | Abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 179 3 | Definition of terms, symbols and abbreviations | new-only |  |  | 0 |
|  |  |  | TS 104 179 4 | Requirements | new-only |  |  | 5 |
|  |  |  | TS 104 179 5.1.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.1.2 | Overview of W3C® Data Integrity specification | new-only |  |  | 1 |
|  |  |  | TS 104 179 5.1 | Fulfilling requirements | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.2.1 | Introduction | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.2.2 | Atomic Entity | new-only |  |  | 1 |
|  |  |  | TS 104 179 5.2.3 | Sealed Attribute | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.2.4 | Derivation Process | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.2.5 | Reconstruction Process | new-only |  |  | 0 |
|  |  |  | TS 104 179 5.2.6 | Workflow | new-only |  |  | 1 |
|  |  |  | TS 104 179 5.2 | Data integrity and provenance for NGSI-LD | new-only |  |  | 0 |
|  |  |  | TS 104 179 5 | Specification | new-only |  |  | 0 |
|  |  |  | TS 104 179 A | Annex A (informative): Changes to the NGSI-LD API | new-only |  |  | 0 |
|  |  |  | TS 104 179 B | Annex B (informative): Example digital signature workflow | new-only |  |  | 0 |
|  |  |  | TS 104 179 C | Annex C (informative): Change history | new-only |  |  | 0 |
