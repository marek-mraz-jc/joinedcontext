// SPDX-License-Identifier: EUPL-1.2
//! Query-filter shapes shared by every backend (the pushdown contract),
//! plus the pure pieces of the geo and temporal query shapes the filters
//! reference. Pure data — no SQL, no I/O; the memory arm consumes these
//! filters too, it just never gets a `decided` outcome.

use serde_json::Value;

pub use antares_ql::geo::{GeoSpec, Rel, LOCATION_IRI};

/// The 4.11 temporal window as the API already validated it.
pub struct InstanceRange<'a> {
    /// `timerel`: `before`, `after` or `between`.
    pub timerel: &'a str,
    /// `timeAt` as a DateTime string.
    pub time_at: &'a str,
    /// `endTimeAt`, present only for `between`.
    pub end_time_at: Option<&'a str>,
    /// The instance timestamp the window applies to (`observedAt`,
    /// `modifiedAt`, `createdAt` or `deletedAt`), expanded.
    pub timeproperty: &'a str,
}

/// Query Entities (5.7.2) predicates as the store seam receives them —
/// every member is optional and already validated by the API layer.
pub struct EntityFilter<'a> {
    /// exact entity ids (`id=` / the ids of a batch query)
    pub ids: Option<&'a [&'a str]>,
    /// A literal every `idPattern` match must carry (5.2.33): the store
    /// narrows on it, the caller's regex still decides. Absent when `id=` is
    /// given (id takes precedence over idPattern).
    pub id_literal: Option<IdLiteral<'a>>,
    /// The id starts with one of these (T-5661), a necessary condition of
    /// the caller's id selection, which still decides. With it a store that
    /// takes it walks the match set and `after_id` in byte order, the order
    /// of the C-collated id index that answers the prefixes.
    pub id_prefixes: Option<&'a [String]>,
    /// The id matches one of these `idPattern`s, each read whole as a policy
    /// rule reads it (`^(?:pattern)$`): an exact condition (T-5826). A store
    /// that cannot apply one exactly ([`id_pattern_sql`]) leaves it to the
    /// caller and reports the outcome undecided.
    pub id_patterns: Option<&'a [String]>,
    /// Entity Type Selection (4.17) as OR-of-AND groups, expanded IRIs
    pub types: Option<&'a [Vec<String>]>,
    /// `attrs=`: the entity must carry at least one, expanded IRIs
    pub attrs: Option<&'a [String]>,
    /// `q=` AST; compiled when its shape is exactly reproducible, else skipped
    pub q: Option<&'a antares_ql::QNode>,
    /// `scopeQ=` verbatim (4.19); compiled over the `scopes` column
    pub scope_q: Option<&'a str>,
    /// `georel`/`geometry`/`coordinates`/`geoproperty` (4.10), compiled over
    /// the extracted `location` column. Every one must hold: the request's
    /// own and a policy narrowing's (ADR-0023 rule 1). Empty = no geoquery.
    pub geo: &'a [GeoSpec<'a>],
    /// term → IRI, the request context's expander (the AST holds terms)
    pub expand: &'a (dyn Fn(&str) -> String + Sync),
    /// The request @context, for a store that evaluates `q` on documents
    /// with the API's own evaluator (`antares_ql::eval::eval_q`). None = such
    /// a store leaves `q` to the caller.
    pub ctx: Option<&'a antares_jsonld::Context>,
    /// Pagination pushdown: applied ONLY when every present predicate
    /// compiled exactly (`decided`) — otherwise the caller's evaluator still
    /// has rows to drop and a SQL LIMIT would page over the wrong set. The
    /// caller passes it only when its own store-invisible filters (idPattern,
    /// federation, orderBy) are absent.
    pub page: Option<Page>,
    /// Projection pushdown (4.21 `pick`, top-level): keep these expanded
    /// attr IRIs + every non-attribute member. Applied only when `decided` —
    /// a projected doc can no longer answer a q= re-check.
    pub keep_attrs: Option<&'a [String]>,
    /// Projection pushdown (`omit`, top-level entries only): drop exactly
    /// these attr IRIs. Same `decided` gate.
    pub drop_attrs: Option<&'a [String]>,
    /// Keyset (T-4683): only the Entities whose id sorts after this one, in
    /// the store's id order. A walk over a whole set passes the last id it
    /// read instead of a growing `offset`, so every page is an index seek and
    /// each row is read once: an OFFSET walk re-reads every row it skips, and
    /// `/types?details=true` over a 25 000-Entity tenant took 57 s. Exact, so
    /// `decided` is untouched.
    pub after_id: Option<&'a str>,
}

impl Default for EntityFilter<'_> {
    fn default() -> Self {
        Self {
            ids: None,
            id_literal: None,
            id_prefixes: None,
            id_patterns: None,
            types: None,
            attrs: None,
            q: None,
            scope_q: None,
            geo: &[],
            expand: &|t: &str| t.to_owned(),
            ctx: None,
            page: None,
            keep_attrs: None,
            drop_attrs: None,
            after_id: None,
        }
    }
}

/// One page: OFFSET/LIMIT in row units, ORDER BY id (the store's stable
/// default order, same as the memory snapshot).
pub struct Page {
    /// Rows to skip.
    pub offset: i64,
    /// Maximum rows to return.
    pub limit: i64,
    /// The client asked for the match total (count=true); otherwise the
    /// store only needs to know whether a next page exists.
    pub count: bool,
}

/// What `query` produced. `decided` = SQL applied every present predicate
/// exactly, so re-evaluation cannot drop a row; `paged` = LIMIT/OFFSET
/// happened in SQL (implies `decided`, except from `query_candidates`),
/// `total` = the pre-LIMIT match count. `R` is the row: a document, or its
/// JSON text from [`crate::CurrentStateDriver::query_entities_text`].
pub struct QueryOutcome<R = Value> {
    /// Matching entity documents.
    pub rows: Vec<R>,
    /// Every present predicate was applied exactly; no re-check needed.
    pub decided: bool,
    /// LIMIT/OFFSET already applied (implies `decided`, except from
    /// `Store::query_candidates`).
    pub paged: bool,
    /// Pre-LIMIT match count, when the backend computed it.
    pub total: Option<i64>,
}

/// Query Temporal Evolution (5.7.4) predicates as the store seam receives
/// them — every member is optional and already validated by the API layer.
pub struct TemporalFilter<'a> {
    /// exact entity ids
    pub ids: Option<&'a [&'a str]>,
    /// flat OR list of expanded type IRIs (temporal query has no AND groups)
    pub types: Option<&'a [String]>,
    /// The id matches one of these `idPattern`s, each read whole as a policy
    /// rule reads it: a policy's narrowing the caller hands over only when
    /// [`id_pattern_sql`] writes every one (T-6052). A store that cannot apply
    /// them returns a superset, as for `ids`, and the caller's rules decide.
    pub id_patterns: Option<&'a [String]>,
    /// `attrs=`: the entity must carry at least one, expanded IRIs
    pub attrs: Option<&'a [String]>,
    /// the 4.11 window; `None` = no instance pruning
    pub range: Option<InstanceRange<'a>>,
    /// `lastN`: per-(attr, datasetId) RANK() cap — ties all kept, so the
    /// per-attr lastN the API applies afterwards always finds its instances
    pub last_n: Option<i64>,
    /// the cap keeps the OLDEST instances instead (firstN, TS 104 176
    /// 6.4.7.3); same RANK() semantics, ascending
    pub oldest_first: bool,
    /// ordering key for the lastN cap (the request's timeproperty)
    pub timeproperty: &'a str,
    /// Entity-level LIMIT/OFFSET pushdown (without it a temporal query
    /// materializes the tenant's ENTIRE history). Passed only when the
    /// caller has no store-invisible entity filters (idPattern, q, geo) —
    /// when honoured, SQL also applies the caller's entity-qualification rule
    /// (≥1 instance, in-window when a range is given), so the paged set is
    /// exactly the set the evaluator would keep.
    pub page: Option<Page>,
    /// With `page`: an Entity qualifies for the page only through an
    /// instance of one of these Attributes (expanded IRIs), the ones a
    /// policy's rules show (T-5616); `None` qualifies through any.
    pub page_attrs: Option<&'a [String]>,
    /// 5.7.4.4 S2 prefilter: the `q=` AST. The Pg arm compiles the leaves it
    /// can reproduce into windowed EXISTS predicates and treats everything
    /// else as TRUE — always a SUPERSET of the eval_q verdict, so the API
    /// arbiter (which always re-runs when q is present) never changes an
    /// answer, only sees fewer rows. Requires the matching `expand`.
    pub q: Option<&'a antares_ql::QNode>,
    /// term → IRI, the request context's expander (the AST holds terms).
    /// `Sync` so a filter alive across an await keeps the handler future Send.
    pub expand: &'a (dyn Fn(&str) -> String + Sync),
    /// 5.7.4.4 S3 prefilter: the geoquery plus the EXPANDED geoproperty IRI
    /// whose windowed instances the EXISTS checks (per-instance rows carry
    /// extracted geometries for EVERY geoproperty, not just `location`).
    /// Superset like `q` — `GeoQuery::matches` stays the arbiter; rows with
    /// an unextracted `geo_value` always survive. Every one must hold, like
    /// [`EntityFilter::geo`]; empty = no geoquery.
    pub geo: &'a [(GeoSpec<'a>, String)],
    /// 4.5.19 aggregated representation computed by the backend: when a
    /// driver can bucket and aggregate the windowed instances itself it
    /// returns per-attribute aggregated objects instead of instance arrays
    /// and sets `TemporalOutcome::aggregated`; a driver that cannot (or a
    /// value class outside the numeric one) ignores this and the API
    /// aggregates over the reconstructed instances as before.
    pub aggregate: Option<Aggregate<'a>>,
}

/// 4.5.19.1: the bucket matrix a backend computes for the numeric value class.
#[derive(Clone, Copy, Debug)]
pub struct Aggregate<'a> {
    /// aggregation methods, each one of [`AGGREGATE_METHODS`]
    pub methods: &'a [String],
    /// bucket width in seconds; `None` = one bucket over the whole range (PT0S)
    pub period_secs: Option<i64>,
    /// bucket origin (the request's timeAt); `None` = each attribute's first instant
    pub anchor: Option<&'a str>,
}

/// Table 4.5.19.1-1: the methods every numeric Property is eligible for —
/// the set a backend may compute in place of the API.
pub const AGGREGATE_METHODS: [&str; 8] = [
    "totalCount",
    "distinctCount",
    "min",
    "max",
    "sum",
    "avg",
    "stddev",
    "sumsq",
];

impl Default for TemporalFilter<'_> {
    fn default() -> Self {
        Self {
            ids: None,
            types: None,
            id_patterns: None,
            attrs: None,
            range: None,
            last_n: None,
            oldest_first: false,
            timeproperty: "observedAt",
            page: None,
            page_attrs: None,
            q: None,
            expand: &|t: &str| t.to_owned(),
            geo: &[],
            aggregate: None,
        }
    }
}

/// 4.22 transient storage: is this doc/instance past its `expiresAt`?
/// Parses both stamps to instants (so a non-UTC-Z offset expiresAt is judged
/// correctly, matching the SQL `expires_at`/timestamptz path); byte compare is
/// only the fallback when a stamp is unparseable.
///
/// 4.6.3 also allows a comma as the seconds-fraction separator, which RFC 3339
/// does not; a comma cannot appear anywhere else in such a stamp, so the first
/// one is rewritten to a point before parsing. Without that the comma form
/// always fell into the byte fallback, where ',' (0x2C) sorts before both '.'
/// and 'Z' and a live instance reads as expired.
pub fn expired_at(v: &Value, now: &str) -> bool {
    let Some(e) = v.get("expiresAt").and_then(Value::as_str) else {
        return false;
    };
    let instant = |s: &str| chrono::DateTime::parse_from_rfc3339(&canonical_datetime(s)).ok();
    match (instant(e), instant(now)) {
        (Some(exp), Some(n)) => exp < n,
        _ => e < now,
    }
}

/// 4.6.3: "In requests, also a comma instead of a decimal point may be used
/// as separator for compatibility reasons." Nothing downstream of the request
/// boundary accepts it — not RFC 3339 parsing, not PostgreSQL's
/// `::timestamptz` cast, not byte comparison, where ',' (0x2C) sorts before
/// both '.' and 'Z'. A comma cannot appear anywhere else in a DateTime of
/// that shape, so the first one IS the fraction separator and rewriting it
/// is total: a stamp with no comma is returned untouched, and a string that
/// is not a DateTime at all is not made into one.
pub fn canonical_datetime(s: &str) -> std::borrow::Cow<'_, str> {
    match s.find(',') {
        Some(_) => std::borrow::Cow::Owned(s.replacen(',', ".", 1)),
        None => std::borrow::Cow::Borrowed(s),
    }
}

/// Apply 4.22 invalidity to a read: `true` = the ENTITY is expired (caller
/// drops it entirely); otherwise expired attribute INSTANCES are stripped in
/// place (an attribute left with zero instances disappears). The stamp marks
/// "a certain Entity, Property or Relationship", and a sub-Attribute is a
/// Property or a Relationship as well, so the pass recurses through the
/// instances it keeps. Only Attribute names are walked: 5.5.7 expansion makes
/// every one an absolute IRI, so a member without a colon is either a
/// reserved instance member or Entity metadata, and user JSON under `value`
/// or `json` — which may spell `expiresAt` itself — is never entered.
pub fn strip_expired(doc: &mut Value, now: &str) -> bool {
    if expired_at(doc, now) {
        return true;
    }
    prune_expired(doc, now);
    false
}

/// The stored half of [`strip_expired`]: drop the expired instances of every
/// Attribute of `doc` at every depth, an Attribute left with none removed,
/// without judging `doc`'s own `expiresAt` (the Entity reap owns that).
/// Whether `doc` changed. Both store arms sweep with it (4.22 "should be
/// deleted"), so a sweep removes what a read hides.
pub fn prune_expired(doc: &mut Value, now: &str) -> bool {
    let Some(obj) = doc.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    let mut empty: Vec<String> = Vec::new();
    for (k, v) in obj.iter_mut() {
        let Some(arr) = v.as_array_mut().filter(|_| k.contains(':')) else {
            continue;
        };
        let before = arr.len();
        arr.retain_mut(|inst| {
            if expired_at(inst, now) {
                return false;
            }
            changed |= prune_expired(inst, now);
            true
        });
        if arr.len() != before {
            changed = true;
            if arr.is_empty() {
                empty.push(k.clone());
            }
        }
    }
    for k in empty {
        obj.remove(&k);
    }
    changed
}

/// What a temporal query produced. `paged` = LIMIT/OFFSET (and the
/// entity-qualification EXISTS) ran in SQL; `total` = pre-LIMIT match count.
pub struct TemporalOutcome {
    /// Matching temporal documents.
    pub rows: Vec<Value>,
    /// LIMIT/OFFSET already applied.
    pub paged: bool,
    /// Pre-LIMIT match count, when the backend computed it.
    pub total: Option<i64>,
    /// The rows carry the 4.5.19 aggregated attribute objects the filter's
    /// `aggregate` asked for (no instance arrays to window).
    pub aggregated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// F-R2ANT4-008: an expired sub-Attribute goes from the stored doc too,
    /// and the doc reports the change, so a sweep rewrites the row.
    #[test]
    fn a_sweep_prunes_an_expired_sub_attribute() {
        let now = "2026-09-01T12:00:00Z";
        let mut doc = json!({"id": "urn:x", "type": ["T"],
            "https://a/attr": [{"value": 1,
                "https://a/sub": [{"value": 3, "expiresAt": "2026-09-01T11:00:00Z"}]}]});
        assert!(prune_expired(&mut doc, now));
        assert!(doc["https://a/attr"][0].get("https://a/sub").is_none());
        assert_eq!(doc["https://a/attr"][0]["value"], 1);
        assert!(!prune_expired(&mut doc, now), "nothing left to prune");
    }

    const NOW: &str = "2026-08-08T12:00:00.000Z";

    /// Defaults push nothing down: a filter built with `..Default::default()`
    /// must not silently add a predicate, and its expander is the identity
    /// (terms stay terms until a request context replaces it).
    #[test]
    fn entity_filter_default_pushes_nothing_down() {
        let f = EntityFilter::default();
        assert!(f.ids.is_none());
        assert!(f.types.is_none());
        assert!(f.attrs.is_none());
        assert!(f.q.is_none());
        assert!(f.scope_q.is_none());
        assert!(f.geo.is_empty());
        assert!(f.page.is_none(), "no LIMIT until the caller proves decided");
        assert!(f.keep_attrs.is_none());
        assert!(f.drop_attrs.is_none());
        assert_eq!((f.expand)("Vehicle"), "Vehicle");
    }

    /// The default lastN ordering key is `observedAt` (4.11); a different
    /// default would silently rank instances by another time property.
    #[test]
    fn temporal_filter_default_orders_by_observed_at() {
        let f = TemporalFilter::default();
        assert_eq!(f.timeproperty, "observedAt");
        assert!(f.ids.is_none());
        assert!(f.types.is_none());
        assert!(f.attrs.is_none());
        assert!(f.range.is_none(), "no window means no instance pruning");
        assert!(f.last_n.is_none());
        assert!(f.page.is_none());
        assert!(f.q.is_none());
        assert!(f.geo.is_empty());
        assert_eq!((f.expand)("speed"), "speed");
    }

    /// 4.22: expiry has PASSED only when the stamp lies strictly before now —
    /// the same strictness as the `expires_at < now()` reaping predicate, so a
    /// read and the sweep never disagree at the boundary instant.
    #[test]
    fn expiry_exactly_at_now_has_not_passed() {
        assert!(!expired_at(&json!({ "expiresAt": NOW }), NOW));
        // same instant, coarser spelling: still not expired
        assert!(!expired_at(
            &json!({"expiresAt": "2026-08-08T12:00:00Z"}),
            NOW
        ));
        // one millisecond earlier is
        assert!(expired_at(
            &json!({"expiresAt": "2026-08-08T11:59:59.999Z"}),
            NOW
        ));
    }

    /// 4.6.3 DateTime accepts a comma as the fraction separator, so a stored
    /// `expiresAt` may carry one. Judging it by bytes puts ',' (0x2C) before
    /// '.' (0x2E) and calls a still-live stamp expired — the comparison must
    /// stay on instants.
    #[test]
    fn comma_fraction_expiry_is_judged_by_instant() {
        assert!(
            !expired_at(&json!({"expiresAt": "2026-08-08T12:00:00,500Z"}), NOW),
            "half a second in the future is not expired"
        );
        assert!(expired_at(
            &json!({"expiresAt": "2026-08-08T11:59:59,999Z"}),
            NOW
        ));
    }

    /// A comma-fraction expiry still in the future must not cost the instance
    /// its place in the document.
    #[test]
    fn a_live_comma_fraction_instance_is_not_stripped() {
        let mut doc = json!({
            "id": "urn:x", "type": ["T"],
            "https://a/attr": [{"value": 1, "instanceId": "i1",
                                "expiresAt": "2026-08-08T12:00:00,500Z"}]
        });
        assert!(!strip_expired(&mut doc, NOW));
        assert_eq!(doc["https://a/attr"].as_array().map(Vec::len), Some(1));
    }

    /// A non-UTC offset is judged by instant: 13:30+02:00 is 11:30Z, expired
    /// against a 12:00Z now even though its bytes sort after it.
    #[test]
    fn offset_expiry_is_judged_by_instant_not_bytes() {
        assert!(expired_at(
            &json!({"expiresAt": "2026-08-08T13:30:00+02:00"}),
            NOW
        ));
        assert!(!expired_at(
            &json!({"expiresAt": "2026-08-08T11:30:00-02:00"}),
            NOW
        ));
    }

    /// Hostile `expiresAt` shapes decide without panicking: a non-string is no
    /// expiry at all, an unparseable string falls back to the byte compare.
    #[test]
    fn a_hostile_expiry_never_panics() {
        for v in [
            json!({ "expiresAt": 1 }),
            json!({ "expiresAt": null }),
            json!({"expiresAt": {"@value": "2020-01-01T00:00:00Z"}}),
            json!({ "expiresAt": ["2020-01-01T00:00:00Z"] }),
            json!({ "expiresAt": true }),
            json!({}),
        ] {
            assert!(!expired_at(&v, NOW), "a non-string expiresAt is no expiry");
        }
        // unparseable strings stay decidable through the byte fallback
        assert!(expired_at(&json!({ "expiresAt": "" }), NOW));
        assert!(expired_at(
            &json!({"expiresAt": "2026-08-08T11:00:00"}),
            NOW
        ));
        assert!(!expired_at(
            &json!({"expiresAt": "9999-99-99T99:99:99Z"}),
            NOW
        ));
    }

    /// Multi-instance (4.5.5) attribute: the expired instance must be gone
    /// from the document entirely, not merely reordered or emptied.
    #[test]
    fn an_expired_instance_never_survives_a_multi_instance_attribute() {
        let mut doc = json!({
            "id": "urn:x", "type": ["T"],
            "https://a/attr": [
                {"value": 1, "instanceId": "i1", "datasetId": "urn:d:1"},
                {"value": 2, "instanceId": "i2", "datasetId": "urn:d:2",
                 "expiresAt": "2026-08-08T11:00:00Z"}
            ]
        });
        assert!(!strip_expired(&mut doc, NOW));
        let text = serde_json::to_string(&doc).expect("serialize");
        assert!(
            !text.contains("i2"),
            "expired instance still present: {text}"
        );
        assert!(!text.contains("urn:d:2"));
        assert!(text.contains("i1"));
    }

    /// Only attribute arrays are instance-filtered: an already-empty array is
    /// left in place (it lost nothing), and a doc that is not an object is
    /// simply not expired.
    #[test]
    fn strip_expired_leaves_untouched_what_it_must_not_remove() {
        let mut doc = json!({
            "id": "urn:x", "type": ["T"], "scope": ["/a"],
            "https://a/empty": []
        });
        assert!(!strip_expired(&mut doc, NOW));
        assert!(doc.get("https://a/empty").is_some(), "empty array kept");
        assert_eq!(doc["scope"], json!(["/a"]));

        let mut not_an_object = json!(["urn:x"]);
        assert!(!strip_expired(&mut not_an_object, NOW));
        assert_eq!(not_an_object, json!(["urn:x"]));
    }

    /// An entity expiry outranks the instance pass: the caller drops the whole
    /// document, so a live instance inside it must never reach a response.
    #[test]
    fn an_expired_entity_is_dropped_before_any_instance_survives() {
        let mut doc = json!({
            "id": "urn:x", "type": ["T"], "expiresAt": "2026-08-08T11:00:00Z",
            "https://a/attr": [{"value": 1, "instanceId": "i1",
                                "expiresAt": "2999-01-01T00:00:00Z"}]
        });
        assert!(strip_expired(&mut doc, NOW));
    }

    /// 4.6.3: the comma is a request-side separator only — every consumer
    /// past that boundary wants the point form. The rewrite is first-comma
    /// only, and leaves anything that is not a DateTime alone: a `datasetId`
    /// or any other URI travels through the same code paths.
    #[test]
    fn canonical_datetime_rewrites_only_the_fraction_separator() {
        assert_eq!(
            canonical_datetime("2026-01-01T00:00:00,500Z"),
            "2026-01-01T00:00:00.500Z"
        );
        assert_eq!(
            canonical_datetime("2026-01-01T00:00:00.500Z"),
            "2026-01-01T00:00:00.500Z"
        );
        assert_eq!(
            canonical_datetime("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(canonical_datetime(""), "");
        // only the first: a second comma is not a fraction separator, so the
        // string stays something the cast will reject rather than becoming a
        // stamp that was never sent.
        assert_eq!(canonical_datetime("a,b,c"), "a.b,c");
        // borrowed when there is nothing to do, so the common path allocates
        // nothing
        assert!(matches!(
            canonical_datetime("2026-01-01T00:00:00Z"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn expired_entity_is_dropped_whole() {
        let mut doc = serde_json::json!({
            "id": "urn:x", "type": ["T"], "expiresAt": "2026-08-08T11:00:00Z",
            "https://a/attr": [{"value": 1, "instanceId": "i1"}]
        });
        assert!(strip_expired(&mut doc, NOW));
    }

    #[test]
    fn expired_instances_are_stripped_and_empty_attrs_disappear() {
        let mut doc = serde_json::json!({
            "id": "urn:x", "type": ["T"], "expiresAt": "2026-08-09T00:00:00Z",
            "https://a/keep": [
                {"value": 1, "instanceId": "i1"},
                {"value": 2, "instanceId": "i2", "expiresAt": "2026-08-08T11:00:00Z"}
            ],
            "https://a/gone": [{"value": 3, "instanceId": "i3",
                                "expiresAt": "2026-08-08T00:00:00Z"}]
        });
        assert!(!strip_expired(&mut doc, NOW));
        assert_eq!(doc["https://a/keep"].as_array().map(Vec::len), Some(1));
        assert!(doc.get("https://a/gone").is_none(), "emptied attr removed");
        // meta arrays (type) are never instance-filtered
        assert_eq!(doc["type"].as_array().map(Vec::len), Some(1));
    }

    /// 4.22 draws no line at depth 1: a sub-Attribute is a Property or a
    /// Relationship, so its own `expiresAt` takes it out of the served
    /// document while its live siblings and the Attribute carrying it stay.
    /// Only expanded Attribute names are walked, so a `value` that happens to
    /// spell `expiresAt` is user JSON and survives untouched.
    #[test]
    fn expired_sub_attributes_leave_and_user_json_stays() {
        let mut doc = serde_json::json!({
            "id": "urn:x", "type": ["T"],
            "https://a/attr": [{
                "value": [{"expiresAt": "2026-08-08T00:00:00Z", "keep": 1}],
                "instanceId": "i1",
                "https://a/gone": [{"value": 2,
                                    "expiresAt": "2026-08-08T11:00:00Z"}],
                "https://a/live": [{"value": 3,
                                    "expiresAt": "2999-01-01T00:00:00Z"}]
            }]
        });
        assert!(!strip_expired(&mut doc, NOW));
        let inst = &doc["https://a/attr"][0];
        assert!(inst.get("https://a/gone").is_none(), "expired sub stripped");
        assert_eq!(inst["https://a/live"][0]["value"], 3, "live sub kept");
        assert_eq!(
            inst["value"],
            serde_json::json!([{"expiresAt": "2026-08-08T00:00:00Z", "keep": 1}]),
            "user JSON is not an Attribute"
        );
    }

    #[test]
    fn no_expiry_means_untouched() {
        let mut doc = serde_json::json!({
            "id": "urn:x", "type": ["T"],
            "https://a/attr": [{"value": 1, "instanceId": "i1"}]
        });
        let before = doc.clone();
        assert!(!strip_expired(&mut doc, NOW));
        assert_eq!(doc, before);
    }
}

/// The longest literal an `idPattern` regex forces on every match: a prefix
/// when the pattern is anchored (`^urn:x:…`), an infix otherwise. A
/// necessary condition only — the regex is still evaluated on every row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdLiteral<'a> {
    /// the characters every match carries
    pub text: &'a str,
    /// true = the match starts with `text`; false = it contains `text`
    pub anchored: bool,
}

/// Extract the literal of `pattern`, or `None` when the pattern forces no
/// literal at its start (alternation anywhere, a leading class or group,
/// an escape, an empty literal). A quantifier after the literal applies to
/// its last character, which is therefore not required.
pub fn id_pattern_literal(pattern: &str) -> Option<IdLiteral<'_>> {
    if pattern.contains('|') {
        return None;
    }
    let (body, anchored) = match pattern.strip_prefix('^') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let start = pattern.len() - body.len();
    let mut end = start;
    let mut stop = None;
    for (i, c) in body.char_indices() {
        if ".^$*+?()[]{}|\\".contains(c) {
            stop = Some(c);
            break;
        }
        end = start + i + c.len_utf8();
    }
    if matches!(stop, Some('*' | '+' | '?' | '{')) {
        end -= pattern[start..end].chars().last()?.len_utf8();
    }
    (end > start).then_some(IdLiteral {
        text: &pattern[start..end],
        anchored,
    })
}

/// A literal every id a policy rule's `pattern` selects carries, the
/// longest one after the start, else the prefix,
/// read as the rule reads it, anchored at both ends (`^(?:pattern)$`); the
/// text and whether ids start with it. Unlike [`id_pattern_literal`] it
/// takes a literal from anywhere in the top-level sequence, so
/// `^urn:x:KPI:[a-z]+:helsinki:stat-[a-z]+$` narrows on `:helsinki:stat-`
/// and not on the prefix every KPI shares (T-5661). `None` for a pattern
/// that does not parse or forces no literal (alternation, a case-insensitive
/// group, only classes). A necessary condition only: the regex still decides.
pub fn id_pattern_required(pattern: &str) -> Option<(String, bool)> {
    use regex_syntax::hir::{HirKind, Look};
    let hir = regex_syntax::Parser::new()
        .parse(&format!("^(?:{pattern})$"))
        .ok()?;
    let HirKind::Concat(parts) = hir.kind() else {
        return None;
    };
    let body = parts
        .iter()
        .skip_while(|h| matches!(h.kind(), HirKind::Look(Look::Start)));
    let mut best: Option<(String, bool)> = None;
    for (i, h) in body.enumerate() {
        let HirKind::Literal(l) = h.kind() else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&l.0) else {
            continue;
        };
        // an inner literal beats the prefix, which in `urn:ngsi-ld:{Type}:`
        // ids repeats the type the read already narrows on
        let rank = |len: usize, anchored: bool| (!anchored, len);
        if best
            .as_ref()
            .is_none_or(|(b, a)| rank(b.len(), *a) < rank(text.len(), i == 0))
        {
            best = Some((text.to_owned(), i == 0));
        }
    }
    best
}

/// An anchored `idPattern` read as `lead`, then a run of characters none of
/// which is `sep`, then `tail` (which starts with `sep`), as in
/// `^urn:ngsi-ld:KPI:[a-z0-9.-]+:helsinki:stat-…`. Every id it selects
/// starts with `lead + s + tail`, `s` being the text between `lead` and the
/// first `sep` after it, so the few values of `s` a Tenant's ids show turn
/// the pattern into exact id prefixes (T-5661).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdSegments {
    /// the literal every match starts with
    pub lead: String,
    /// the first character of `tail`, which the run never holds
    pub sep: char,
    /// the literal right after the run
    pub tail: String,
}

/// [`IdSegments`] of `pattern` read as a policy rule reads it
/// (`^(?:pattern)$`); `None` unless its top-level sequence starts with a
/// literal, a class or a repetition of one that excludes an ASCII `sep`,
/// and a literal starting with `sep`.
pub fn id_pattern_segments(pattern: &str) -> Option<IdSegments> {
    use regex_syntax::hir::{Class, HirKind, Look};
    let hir = regex_syntax::Parser::new()
        .parse(&format!("^(?:{pattern})$"))
        .ok()?;
    let HirKind::Concat(parts) = hir.kind() else {
        return None;
    };
    let mut body = parts
        .iter()
        .skip_while(|h| matches!(h.kind(), HirKind::Look(Look::Start)));
    let text = |h: Option<&regex_syntax::hir::Hir>| match h.map(|h| h.kind()) {
        Some(HirKind::Literal(l)) => std::str::from_utf8(&l.0).ok().map(str::to_owned),
        _ => None,
    };
    let lead = text(body.next())?;
    let run = body.next()?;
    let tail = text(body.next())?;
    let sep = tail.chars().next().filter(char::is_ascii)?;
    let class = match run.kind() {
        HirKind::Repetition(r) => r.sub.kind(),
        k => k,
    };
    let holds_sep = match class {
        HirKind::Class(Class::Unicode(c)) => c
            .ranges()
            .iter()
            .any(|r| (r.start()..=r.end()).contains(&sep)),
        HirKind::Class(Class::Bytes(c)) => {
            let b = u8::try_from(sep).ok()?;
            c.ranges()
                .iter()
                .any(|r| (r.start()..=r.end()).contains(&b))
        }
        _ => return None,
    };
    (!holds_sep).then_some(IdSegments { lead, sep, tail })
}

/// `pattern` read as a policy rule reads it (`^(?:pattern)$`), written as a
/// PostgreSQL advanced regular expression with the same matches, so a store
/// counts a dataset view's Entities in one statement where the API judged
/// 164 000 of them per row and the gateway cut the read at 10 s (T-5826).
/// `None` outside a subset both engines read alike: ASCII literals, classes
/// of printable ASCII, groups, alternation, greedy repetition up to 255
/// (PostgreSQL's `RE_DUP_MAX`), `^` and `$`. Membership of a regular language
/// does not depend on the engine's match preference, so within it the two
/// select the same ids.
/// ponytail: no `\d`, `.` or non-ASCII folding (Unicode classes differ); add one
/// here with a parity case when a seeded pattern needs it.
pub fn id_pattern_sql(pattern: &str) -> Option<String> {
    let hir = regex_syntax::Parser::new()
        .parse(&format!("^(?:{pattern})$"))
        .ok()?;
    let mut out = String::new();
    are(&hir, &mut out)?;
    // a long pattern may pass `regex`'s bound and still be too complex for
    // PostgreSQL, which would fail the statement instead of the pushdown
    (out.len() <= 1024).then_some(out)
}

/// One printable ASCII character as an ARE atom: `\` keeps punctuation
/// literal, inside a bracket expression too.
fn are_char(c: u8, out: &mut String) -> Option<()> {
    if !c.is_ascii_graphic() {
        return None;
    }
    if !c.is_ascii_alphanumeric() {
        out.push('\\');
    }
    out.push(char::from(c));
    Some(())
}

fn are(h: &regex_syntax::hir::Hir, out: &mut String) -> Option<()> {
    use regex_syntax::hir::{Class, HirKind, Look};
    match h.kind() {
        HirKind::Literal(l) => l.0.iter().try_for_each(|&b| are_char(b, out))?,
        HirKind::Class(Class::Unicode(c)) => {
            out.push('[');
            for r in c.ranges() {
                let (s, e) = (u8::try_from(r.start()).ok()?, u8::try_from(r.end()).ok()?);
                let kin = |a: u8, b: u8| {
                    (a.is_ascii_digit() && b.is_ascii_digit())
                        || (a.is_ascii_lowercase() && b.is_ascii_lowercase())
                        || (a.is_ascii_uppercase() && b.is_ascii_uppercase())
                };
                if s < e && kin(s, e) {
                    out.push(char::from(s));
                    out.push('-');
                    out.push(char::from(e));
                } else {
                    (s..=e).try_for_each(|b| are_char(b, out))?;
                }
            }
            out.push(']');
        }
        HirKind::Look(Look::Start) => out.push('^'),
        HirKind::Look(Look::End) => out.push('$'),
        HirKind::Repetition(r) if r.greedy && !matches!(r.sub.kind(), HirKind::Look(_)) => {
            let n = |v: u32| (v <= 255).then_some(v);
            out.push_str("(?:");
            are(&r.sub, out)?;
            out.push(')');
            let q = match (n(r.min)?, r.max.map(n)) {
                (0, None) => "*".to_owned(),
                (1, None) => "+".to_owned(),
                (0, Some(Some(1))) => "?".to_owned(),
                (m, None) => format!("{{{m},}}"),
                (m, Some(Some(x))) if m == x => format!("{{{m}}}"),
                (m, Some(Some(x))) => format!("{{{m},{x}}}"),
                (_, Some(None)) => return None,
            };
            out.push_str(&q);
        }
        HirKind::Capture(c) => {
            out.push_str("(?:");
            are(&c.sub, out)?;
            out.push(')');
        }
        HirKind::Concat(hs) => hs.iter().try_for_each(|h| are(h, out))?,
        HirKind::Alternation(hs) => {
            out.push_str("(?:");
            for (i, h) in hs.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                are(h, out)?;
            }
            out.push(')');
        }
        _ => return None,
    }
    Some(())
}

/// The least string that sorts, byte by byte, after every string starting
/// with `prefix`; `None` when nothing does (an empty prefix, or one of
/// `char::MAX` only). With it `[prefix, end)` is exactly the strings that
/// start with `prefix` under `COLLATE "C"`.
pub fn prefix_end(prefix: &str) -> Option<String> {
    let mut s = prefix.to_owned();
    while let Some(c) = s.pop() {
        let next = (u32::from(c).saturating_add(1)..=u32::from(char::MAX)).find_map(char::from_u32);
        if let Some(n) = next {
            s.push(n);
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod id_literal_tests {
    use super::*;

    /// T-5826: the seeded dataset patterns translate, each to an ARE that
    /// keeps punctuation literal; what both engines may read apart does not.
    #[test]
    fn id_pattern_sql_writes_the_seeded_patterns_and_refuses_the_rest() {
        let sql = id_pattern_sql;
        assert_eq!(
            sql("^urn:ngsi-ld:Tree:[a-z0-9.-]+:praha-mesto:green-[a-z-]+-[0-9a-f-]{36}$")
                .as_deref(),
            Some(
                "^^urn\\:ngsi\\-ld\\:Tree\\:(?:[\\-\\.0-9a-z])+\\:praha\\-mesto\\:green\\-\
                 (?:[\\-a-z])+\\-(?:[\\-0-9a-f]){36}$$"
            )
        );
        assert_eq!(
            sql("^urn:x:(1|2|both)-[0-9]{1,4}(-[a-z]+)?$").as_deref(),
            Some("^^urn\\:x\\:(?:(?:1|2|both))\\-(?:[0-9]){1,4}(?:(?:\\-(?:[a-z])+))?$$")
        );
        for refused in [
            "urn:x:.*",
            "urn:x:\\d+",
            "(?i)urn:k",
            "urn:x:[^a]",
            "urn:x:a+?",
            "urn:x:a{256}",
            "urn:x:\\bz",
            "urn:é",
            "urn:x |",
            "(unclosed",
        ] {
            assert_eq!(sql(refused), None, "{refused}");
        }
    }

    /// T-5661: a rule's pattern narrows on its most selective literal; each
    /// result is a substring (a prefix when anchored) of every id it selects.
    #[test]
    fn required_literal_is_the_longest_the_rule_pattern_forces() {
        let req = id_pattern_required;
        let kpi = "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:helsinki:stat-environment-[A-Za-z0-9._-]+$";
        assert_eq!(
            req(kpi),
            Some((":helsinki:stat-environment-".to_owned(), false))
        );
        assert_eq!(req("urn:x:abc.*"), Some(("urn:x:abc".to_owned(), true)));
        // a quantifier binds the last character
        assert_eq!(req("^abc*$"), Some(("ab".to_owned(), true)));
        assert_eq!(req("a|b"), None);
        assert_eq!(req("urn:(a|b)"), Some(("urn:".to_owned(), true)));
        // case-insensitive letters are classes; only the `:` is forced
        assert_eq!(req("(?i)urn:a"), Some((":".to_owned(), false)));
        assert_eq!(req("[ab]+"), None);
        assert_eq!(req("(unclosed"), None);
        assert_eq!(req(""), None);
        for (p, id) in [
            (
                kpi,
                "urn:ngsi-ld:KeyPerformanceIndicator:x.y:helsinki:stat-environment-a1",
            ),
            ("urn:x:abc.*", "urn:x:abc9"),
            ("^abc*$", "ab"),
            ("urn:(a|b)", "urn:b"),
        ] {
            let re = regex::Regex::new(&format!("^(?:{p})$")).expect("re");
            assert!(re.is_match(id), "{p} vs {id}");
            let (text, anchored) = req(p).expect("literal");
            assert!(
                if anchored {
                    id.starts_with(&text)
                } else {
                    id.contains(&text)
                },
                "{p}: {id}"
            );
        }
    }

    /// 5.2.33 idPattern narrowing: the literal is a necessary condition of
    /// the regex, never a sufficient one, so it may only ever shrink the
    /// candidate set the regex then decides.
    #[test]
    fn literal_is_a_necessary_condition_of_the_pattern() {
        let lit = id_pattern_literal;
        let some = |text, anchored| Some(IdLiteral { text, anchored });
        assert_eq!(
            lit("^urn:ngsi-ld:Vehicle:.*"),
            some("urn:ngsi-ld:Vehicle:", true)
        );
        assert_eq!(
            lit("urn:ngsi-ld:Sensor:t7:7.*"),
            some("urn:ngsi-ld:Sensor:t7:7", false)
        );
        assert_eq!(lit("urn:x:abc"), some("urn:x:abc", false));
        // a quantifier binds the last character: `c` is optional in `abc*`
        assert_eq!(lit("^abc*"), some("ab", true));
        assert_eq!(lit("a?bc"), None);
        assert_eq!(lit("^a{2}"), None);
        // nothing is forced at the start
        assert_eq!(lit(".*0$"), None);
        assert_eq!(lit("^"), None);
        assert_eq!(lit(""), None);
        assert_eq!(lit("(?i)abc"), None);
        assert_eq!(lit("[ab]c"), None);
        assert_eq!(lit("\\d+"), None);
        // alternation anywhere: the literal would exclude the other branch
        assert_eq!(lit("^urn:a|urn:b"), None);
        assert_eq!(lit("^urn:(a|b)"), None);
        // the extracted text is a substring of every regex match
        for (p, sample) in [
            ("^urn:ngsi-ld:Vehicle:.*", "urn:ngsi-ld:Vehicle:1"),
            ("t7:7.*", "urn:ngsi-ld:Sensor:t7:71"),
            ("^abc*", "abd"),
        ] {
            let l = lit(p).expect("literal");
            assert!(
                regex::Regex::new(p).expect("re").is_match(sample),
                "{p} vs {sample}"
            );
            assert!(
                if l.anchored {
                    sample.starts_with(l.text)
                } else {
                    sample.contains(l.text)
                },
                "{p}: {sample}"
            );
        }
    }

    /// T-5661: a pattern's segments are exact prefixes of every id it
    /// selects, and `prefix_end` bounds exactly the strings with a prefix.
    #[test]
    fn segments_split_a_pattern_at_its_first_run() {
        let kpi = "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:helsinki:stat-environment-[A-Za-z0-9._-]+$";
        let s = id_pattern_segments(kpi).expect("segments");
        assert_eq!(s.lead, "urn:ngsi-ld:KeyPerformanceIndicator:");
        assert_eq!(s.sep, ':');
        assert_eq!(s.tail, ":helsinki:stat-environment-");
        let id = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki:stat-environment-a1";
        let re = regex::Regex::new(&format!("^(?:{kpi})$")).expect("re");
        assert!(re.is_match(id));
        let run = &id[s.lead.len()..];
        let seg = &run[..run.find(s.sep).expect("sep")];
        assert!(id.starts_with(&format!("{}{seg}{}", s.lead, s.tail)));
        assert_eq!(
            id_pattern_segments("urn:x:[a-z]:t").map(|s| s.tail),
            Some(":t".to_owned())
        );
        // the run may hold the separator: no split
        assert_eq!(id_pattern_segments("urn:x:.+:t"), None);
        assert_eq!(id_pattern_segments("urn:x:[a-z:]+:t"), None);
        // no literal after the run, a run first, alternation, case folding
        assert_eq!(id_pattern_segments("urn:x:[a-z]+"), None);
        assert_eq!(id_pattern_segments("[a-z]+:t"), None);
        assert_eq!(id_pattern_segments("urn:(a|b):t"), None);
        assert_eq!(id_pattern_segments("(?i)urn:[a-z]+:t"), None);
        assert_eq!(id_pattern_segments("(unclosed"), None);

        assert_eq!(prefix_end("ab:").as_deref(), Some("ab;"));
        assert_eq!(prefix_end("a\u{10FFFF}").as_deref(), Some("b"));
        assert_eq!(prefix_end("a\u{D7FF}").as_deref(), Some("a\u{E000}"));
        assert_eq!(prefix_end(""), None);
        assert_eq!(prefix_end("\u{10FFFF}"), None);
        let end = prefix_end("ab:").expect("end");
        for (s, inside) in [
            ("ab:", true),
            ("ab:\u{10FFFF}z", true),
            ("ab;", false),
            ("ab", false),
            ("ab9", false),
        ] {
            assert_eq!(
                s.as_bytes() >= b"ab:".as_slice() && s.as_bytes() < end.as_bytes(),
                inside,
                "{s}"
            );
        }
    }
}
