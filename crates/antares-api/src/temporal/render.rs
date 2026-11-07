// SPDX-License-Identifier: EUPL-1.2
//! Temporal representation: the 4.11 window, the 6.3.10 cut, paging links and the normalized/temporalValues body.

use super::*;

/// The most periods an aggregated `onChange` Attribute answers, rows and held gaps together.
pub(super) const MAX_HELD_BUCKETS: usize = 10_000;

/// 6.3.10: the ceiling is a CUT, not a label. An Attribute holding more
/// instances than the broker serves at once is truncated to the ceiling in
/// the query direction, and the 206 + Content-Range then describes the
/// instances actually returned. This is what caps a lastN above the ceiling
/// and what caps a request naming no temporal window at all. Aggregated
/// representations (5.7.4.4) are computed over the whole evolution and are
/// complete by construction, so they are never cut.
///
/// The cut is ONE time boundary for the whole entity, not a per-attribute
/// count: the partial content "shall" be the representation the
/// Content-Range describes, so every attribute is trimmed to the tightest
/// ceiling instant among the over-full ones (ties at that instant kept),
/// and a client continuing from the advertised range-end misses no instance
/// of any attribute. An attribute lying entirely beyond the boundary comes
/// back empty on this page. ETSI 020_14 checks exactly that (T-3612; the
/// per-Attribute count of T-3456 failed it on every store).
///
/// An Attribute the boundary cut shorter than the ceiling holds the next
/// page's instances at a smaller offset than the boundary Attribute, so the
/// next offsetN Link moves by the fewest instances a cut Attribute served
/// (`cut_served`): it repeats, never skips. One cut to nothing leaves no
/// offsetN that continues it, and the Link continues by time from the
/// boundary instead (`interval_links`, T-3613).
pub(super) fn truncate(w: &mut Windowed, timeprop: &str, descending: bool, limit: usize) {
    if w.max_per_attr <= limit {
        return;
    }
    w.truncated = true;
    w.more = true;
    w.max_per_attr = limit;
    let key = |inst: &Value| inst.get(timeprop).and_then(Value::as_str).map(dt_key);
    // the tightest ceiling instant: earliest forwards, latest backwards
    let boundary = w
        .attrs
        .values()
        .filter(|insts| insts.len() > limit)
        .filter_map(|insts| insts.get(limit.saturating_sub(1)).and_then(key))
        .reduce(|a, b| if (b < a) != descending { b } else { a });
    let (mut ts_min, mut ts_max) = (None::<String>, None::<String>);
    for instances in w.attrs.values_mut() {
        let before = instances.len();
        match &boundary {
            Some(bd) => instances.retain(|inst| {
                key(inst).is_none_or(|k| if descending { k >= *bd } else { k <= *bd })
            }),
            None => instances.truncate(limit),
        }
        if instances.len() < before {
            w.cut_served = Some(
                w.cut_served
                    .map_or(instances.len(), |c| c.min(instances.len())),
            );
        }
        for inst in instances.iter() {
            if let Some(t) = inst.get(timeprop).and_then(Value::as_str) {
                if ts_min.as_deref().is_none_or(|m| dt_key(t) < dt_key(m)) {
                    ts_min = Some(t.to_owned());
                }
                if ts_max.as_deref().is_none_or(|m| dt_key(t) > dt_key(m)) {
                    ts_max = Some(t.to_owned());
                }
            }
        }
    }
    (w.ts_min, w.ts_max) = (ts_min, ts_max);
}

/// Each instance of `w` replaced by its chain of versions in `chains` (the
/// store's `versions`), the same measurement being the same Attribute,
/// `observedAt` and `datasetId` (ADR-N-054 2.4); a value written without
/// `observedAt` is its own row, by `instanceId` (2.9). An instance the store
/// keeps no version of stays as it is.
pub(super) fn with_versions(w: &mut Windowed, chains: &Map<String, Value>) {
    let key = |i: &Value| {
        (
            i.get("observedAt").and_then(Value::as_str).map(dt_key),
            i.get("datasetId").cloned(),
        )
    };
    for (attr, instances) in &mut w.attrs {
        let Some(chain) = chains.get(attr).and_then(Value::as_array) else {
            continue;
        };
        *instances = instances
            .iter()
            .flat_map(|inst| {
                // a value written without observedAt (ADR-N-054 2.9) is
                // its own row: the window stamps it, its chain row is not
                let own: Vec<Value> = chain
                    .iter()
                    .filter(|v| match v.get("observedAt") {
                        Some(_) => key(v) == key(inst),
                        None => v.get("instanceId") == inst.get("instanceId"),
                    })
                    .cloned()
                    .collect();
                if own.is_empty() {
                    vec![inst.clone()]
                } else {
                    own
                }
            })
            .collect();
    }
}

/// The instant a cut page ends at in its order: the boundary `truncate`
/// kept every Attribute to.
pub(super) fn cut_boundary(w: &Windowed, desc: bool) -> Option<&str> {
    if desc {
        w.ts_min.as_deref()
    } else {
        w.ts_max.as_deref()
    }
}

/// 5.7.4.4 S4/S7: does a scope VALUE (string or array of strings) match the
/// 4.19 Scope query? The 4.5.7 deletion sentinel never matches.
pub(super) fn scope_value_matches(sq: &str, v: &Value) -> bool {
    if v.is_null() || v.as_str() == Some("urn:ngsi-ld:null") {
        return false;
    }
    crate::scope_matches(sq, &serde_json::json!({ "scope": v }))
}

/// 4.18 over a Temporal Evolution: "a given Scope is considered valid from
/// the time it has been set until the time it has been explicitly removed by
/// an update or delete operation" (example: annex C.5.16). Instance-shaped
/// scope arrays become [set-time, next-set-time) validity intervals
/// (set-time = observedAt‖modifiedAt‖createdAt — 4.5.6 mirrors them from the
/// Core API change); a plain string/array scope is valid for all time.
/// Returns only the intervals whose value matches `sq`; "" start = -inf,
/// None end = +inf.
pub(super) fn scope_match_intervals(doc: &Value, sq: &str) -> Vec<(String, Option<String>)> {
    match doc.get("scope") {
        Some(Value::Array(a)) if a.first().is_some_and(Value::is_object) => {
            let mut states: Vec<(&str, &Value)> = a
                .iter()
                .filter_map(|i| {
                    scope_set_time(i).map(|t| (t, i.get("value").unwrap_or(&Value::Null)))
                })
                .collect();
            states.sort_by_key(|(t, _)| dt_key(t));
            // each state with the time the next one was set, the last open
            let next = states.iter().skip(1).map(Some).chain(std::iter::once(None));
            states
                .iter()
                .zip(next)
                .filter(|((_, value), _)| scope_value_matches(sq, value))
                .map(|((t, _), next)| ((*t).to_owned(), next.map(|(t, _)| (*t).to_owned())))
                .collect()
        }
        Some(_) if crate::scope_matches(sq, doc) => vec![(String::new(), None)],
        _ => Vec::new(),
    }
}

/// The time a temporal scope instance was set (4.5.6: observedAt is a copy
/// of modifiedAt on Core-API changes; direct 5.6.11 input may carry any).
pub(super) fn scope_set_time(i: &Value) -> Option<&str> {
    ["observedAt", "modifiedAt", "createdAt"]
        .iter()
        .find_map(|k| i.get(*k).and_then(Value::as_str))
}

// seven of the request's own plus the Attributes the type records onChange
#[allow(clippy::too_many_arguments)]
pub(super) fn window(
    doc: &Value,
    tq: Option<&TemporalQ>,
    page: InstancePage,
    attrs_filter: Option<&Vec<String>>,
    omit: Option<&Vec<crate::repr::ProjNode>>,
    dataset: Option<&Vec<String>>,
    timeprop: &str,
    held: std::collections::HashSet<String>,
) -> Windowed {
    let mut w = Windowed {
        attrs: std::collections::BTreeMap::new(),
        max_per_attr: 0,
        ts_min: None,
        ts_max: None,
        truncated: false,
        cut_served: None,
        more: false,
        held,
    };
    let Some(obj) = doc.as_object() else { return w };
    for (k, v) in obj {
        // 4.5.6: the Scope of a Temporal Evolution is represented as the
        // temporal representation of a Property — instance-shaped scope
        // arrays window like attributes (plain-string scope stays meta).
        let scope_instances = k == "scope"
            && v.as_array()
                .is_some_and(|a| a.first().is_some_and(Value::is_object));
        if (is_meta(k) && !scope_instances) || !projected(k, attrs_filter, omit) {
            continue;
        }
        let mut instances = in_window(v, tq, dataset);
        if w.held.contains(k) {
            carry_held(&mut instances, v, tq, dataset, timeprop);
        }
        if scope_instances {
            carry_scope(&mut instances, v, tq);
        }
        // Canonicalize before comparing: '.' sorts before 'Z', so a raw
        // string compare puts "…00.5Z" ahead of "…00Z" and lastN keeps the
        // wrong instant. 4.6.3 allows both fraction spellings. Cached: one
        // key per instance, not two per comparison.
        instances
            .sort_by_cached_key(|i| dt_key(i.get(timeprop).and_then(Value::as_str).unwrap_or("")));
        w.more |= page_instances(&mut instances, page);
        if instances.is_empty() {
            continue;
        }
        w.note(&instances, timeprop);
        w.attrs.insert(k.clone(), instances);
    }
    w
}

impl Windowed {
    /// One Attribute's page joins the window: its size and time range.
    fn note(&mut self, instances: &[Value], timeprop: &str) {
        self.max_per_attr = self.max_per_attr.max(instances.len());
        for inst in instances {
            if let Some(t) = inst.get(timeprop).and_then(Value::as_str) {
                if self.ts_min.as_deref().is_none_or(|m| dt_key(t) < dt_key(m)) {
                    self.ts_min = Some(t.to_owned());
                }
                if self.ts_max.as_deref().is_none_or(|m| dt_key(t) > dt_key(m)) {
                    self.ts_max = Some(t.to_owned());
                }
            }
        }
    }
}

/// Whether attrs and omit keep Attribute `k`.
fn projected(
    k: &str,
    attrs_filter: Option<&Vec<String>>,
    omit: Option<&Vec<crate::repr::ProjNode>>,
) -> bool {
    attrs_filter.is_none_or(|want| want.iter().any(|w| w == k))
        && omit.is_none_or(|omit| !omit.iter().any(|n| n.iri == k && n.children.is_none()))
}

fn of_dataset(dataset: Option<&Vec<String>>, inst: &Value) -> bool {
    match (dataset, inst.get("datasetId")) {
        (None, _) => true,
        (Some(want), Some(Value::String(have))) => want.iter().any(|w| w == have),
        (Some(want), None) => want.iter().any(|w| w == "@none"),
        _ => false,
    }
}

/// The instances of one Attribute in the asked datasets and the window.
fn in_window(v: &Value, tq: Option<&TemporalQ>, dataset: Option<&Vec<String>>) -> Vec<Value> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|inst| of_dataset(dataset, inst))
        .filter(|inst| tq.is_none_or(|tq| tq.instance_matches(inst)))
        .cloned()
        .collect()
}

/// ADR-N-054 2.5 (DM-117): an `onChange` Attribute held its last row
/// before the window when the window opened, so that row comes along.
fn carry_held(
    instances: &mut Vec<Value>,
    v: &Value,
    tq: Option<&TemporalQ>,
    dataset: Option<&Vec<String>>,
    timeprop: &str,
) {
    let Some(start) = tq.and_then(|t| match t.timerel.as_str() {
        "after" | "between" => Some(dt_key(&t.time_at)),
        _ => None,
    }) else {
        return;
    };
    let key = |i: &&Value| i.get(timeprop).and_then(Value::as_str).map(dt_key);
    let carry = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|inst| of_dataset(dataset, inst))
        .filter(|i| key(i).is_some_and(|t| t < start))
        .max_by_key(key);
    if let Some(c) = carry {
        instances.push(c.clone());
    }
}

/// 4.18/C.5.16: the scope valid AT the window start was set at or
/// before it — carry the latest pre-window instance into the
/// representation (a temporal scope stays valid until replaced).
fn carry_scope(instances: &mut Vec<Value>, v: &Value, tq: Option<&TemporalQ>) {
    let Some(start) = tq.and_then(|t| match t.timerel.as_str() {
        "after" | "between" => Some(t.time_at.as_str()),
        _ => None,
    }) else {
        return;
    };
    let carry = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| scope_set_time(i).is_some_and(|t| dt_key(t) < dt_key(start)))
        .max_by_key(|i| scope_set_time(i).map(dt_key))
        .cloned();
    if let Some(c) = carry {
        if !instances.contains(&c) {
            instances.push(c);
        }
    }
}

/// 6.4.7.3: the sorted instances cut to one page; true when the
/// Attribute holds more past it.
fn page_instances(instances: &mut Vec<Value>, page: InstancePage) -> bool {
    // lastN pages newest first, firstN and neither oldest first
    if page.desc {
        instances.reverse();
    }
    // Table 6.4.7.3-2: an offsetN past the end "should be assumed to be
    // equal to the size of the result set, i.e. only the last element
    // of the result set is to be returned"
    let skip = page.offset.min(instances.len().saturating_sub(1));
    instances.drain(..skip);
    match page.n {
        Some(n) if instances.len() > n => {
            instances.truncate(n);
            true
        }
        _ => false,
    }
}

/// `Content-Range: date-time <start>-<end>/<size>` (Scorpio-parity semantics).
pub(super) fn content_range(
    truncated: bool,
    ts_min: Option<&str>,
    ts_max: Option<&str>,
    tq: Option<&TemporalQ>,
    page: InstancePage,
) -> Option<String> {
    if !truncated {
        return None;
    }
    let (data_min, data_max) = (ts_min?, ts_max?);
    // The window bound is the query's own when the query names one; matching
    // on the pair keeps the timerel and the query that produced it together,
    // so no arm can reach for a query that is not there.
    let named = tq.filter(|t| t.timerel != "any");
    let (start, end) = if !page.desc {
        let start = match named.map(|t| (t.timerel.as_str(), t)) {
            Some(("after" | "between", t)) => t.time_at.clone(),
            _ => data_min.to_owned(),
        };
        (start, data_max.to_owned())
    } else {
        let start = match named.map(|t| (t.timerel.as_str(), t)) {
            Some(("before", t)) => t.time_at.clone(),
            Some(("between", t)) => t.end_time_at.clone().unwrap_or_else(|| data_max.to_owned()),
            _ => data_max.to_owned(),
        };
        (start, data_min.to_owned())
    };
    // start/end bound the instances actually returned; the size is the length
    // of the complete representation the client asked for — the requested
    // firstN or lastN, or "*" when the window leaves it unknown.
    let size = page.n.map_or_else(|| "*".to_owned(), |n| n.to_string());
    Some(format!("date-time {start}-{end}/{size}"))
}

/// TS 104 176 6.4.7.3: the Links to the neighbouring instance pages, each
/// the request with offsetN moved by one page and typed with the response
/// media type. Ascending pages run forward in time, so their next page is
/// `intervalafter` and their previous one `intervalbefore`; lastN pages run
/// backward and swap the two. `step` is the page size served per Attribute.
///
/// A page the ceiling cut moves its next Link by `cut`, the fewest
/// instances a cut Attribute served (see `truncate`). When that is none at
/// all, no offsetN continues every Attribute, and the next Link asks for the
/// time past `boundary` instead (the window's end the cut moved): `after`
/// it ascending, which repeats the instances at the boundary instant once
/// (4.11 `after` is inclusive), `before` it descending, which is exact.
pub(super) fn interval_links(
    params: &HashMap<String, String>,
    path: &str,
    accept: Accept,
    page: InstancePage,
    more: bool,
    step: usize,
    cut: Option<(usize, &str)>,
) -> Vec<String> {
    let (forward, back) = match page.desc {
        false => ("intervalafter", "intervalbefore"),
        true => ("intervalbefore", "intervalafter"),
    };
    // the request with `set` replacing its own values of those names
    let link = |set: Vec<(&str, String)>, rel: &str| {
        let mut qp: Vec<String> = params
            .iter()
            .filter(|(k, _)| k.as_str() != "offsetN" && !set.iter().any(|(s, _)| s == k))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(set.iter().map(|(k, v)| (*k, v.as_str())))
            .map(|(k, v)| format!("{k}={}", crate::paging::query_value(v)))
            .collect();
        qp.sort();
        format!(
            "<{path}?{}>; rel=\"{rel}\";type=\"{}\"",
            qp.join("&"),
            crate::paging::link_media_type(accept)
        )
    };
    let offset = |n: usize| vec![("offsetN", n.to_string())];
    let mut links = Vec::new();
    if more {
        match cut {
            None => links.push(link(offset(page.offset.saturating_add(step)), forward)),
            Some((0, boundary)) => {
                links.push(link(resume_after(params, page.desc, boundary), forward))
            }
            Some((served, _)) => {
                links.push(link(
                    offset(page.offset.saturating_add(served.min(step))),
                    forward,
                ));
            }
        }
    }
    if page.offset > 0 {
        links.push(link(offset(page.offset.saturating_sub(step)), back));
    }
    links
}

/// ponytail: ascending, a page whose boundary instant holds a whole page of
/// one Attribute's instances links to itself; offsetN by time would fix it.
///
/// The request's time window moved past `boundary` in the page order: the
/// window's other end stays (4.11: `between` is inclusive of `timeAt`,
/// exclusive of `endTimeAt`).
pub(super) fn resume_after(
    params: &HashMap<String, String>,
    desc: bool,
    boundary: &str,
) -> Vec<(&'static str, String)> {
    let rel = params.get("timerel").map(String::as_str);
    let (at, end) = (params.get("timeAt"), params.get("endTimeAt"));
    let b = boundary.to_owned();
    let between = |from: String, to: String| {
        vec![
            ("timerel", "between".to_owned()),
            ("timeAt", from),
            ("endTimeAt", to),
        ]
    };
    match (desc, rel, at, end) {
        (false, Some("between"), _, Some(e)) | (false, Some("before"), Some(e), _) => {
            between(b, e.clone())
        }
        (false, ..) => vec![("timerel", "after".to_owned()), ("timeAt", b)],
        (true, Some("between" | "after"), Some(a), _) => between(a.clone(), b),
        (true, ..) => vec![("timerel", "before".to_owned()), ("timeAt", b)],
    }
}

/// Render one temporal entity from its windowed data.
pub(super) fn present_temporal(
    doc: &Value,
    w: &Windowed,
    ctx: &Context,
    r: &TRepr,
    tq: Option<&TemporalQ>,
    timeprop: &str,
) -> Result<Value, NgsiError> {
    let Some(obj) = doc.as_object() else {
        return Ok(doc.clone());
    };
    let mut out = present_meta(obj, ctx, r);
    if r.aggregated {
        for (k, v) in render_aggregated(w, tq, r, ctx, timeprop)? {
            out.insert(k, v);
        }
        return Ok(Value::Object(out));
    }
    for (k, instances) in &w.attrs {
        // Table 6.18.3.2-1 / 6.19.3.1 `lang`: each LanguageProperty
        // instance becomes a Property in the chosen language (4.15) before
        // either representation renders it.
        let reduced: Vec<Value>;
        let instances: &[Value] = match &r.lang {
            Some(lang) => {
                reduced = instances
                    .iter()
                    .map(|inst| {
                        let mut inst = inst.clone();
                        if let Some(o) = inst.as_object_mut() {
                            crate::repr::apply_lang(o, lang);
                        }
                        inst
                    })
                    .collect();
                &reduced
            }
            None => instances,
        };
        let rendered = if instances.is_empty() {
            // gap-cut leftovers render as empty arrays
            Value::Array(vec![])
        } else if r.temporal_values {
            simplified_attr(instances, ctx, timeprop)
        } else {
            Value::Array(
                instances
                    .iter()
                    .map(|inst| normalized_instance(inst, ctx, r.sys))
                    .collect(),
            )
        };
        out.insert(ctx.compact_iri(k), rendered);
    }
    Ok(Value::Object(out))
}

/// The Entity's own members (id, type, scope, the system attributes) as
/// sysAttrs, pick and omit admit them.
fn present_meta(obj: &Map<String, Value>, ctx: &Context, r: &TRepr) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in obj {
        let scope_instances = k == "scope"
            && v.as_array()
                .is_some_and(|a| a.first().is_some_and(Value::is_object));
        if !is_meta(k) || scope_instances {
            continue;
        }
        match k.as_str() {
            // Table 6.3.11-1: `sysAttrs` is what admits "the system
            // generated temporal attributes createdAt, modifiedAt and
            // the system temporal attribute expiresAt … In the case of
            // temporal representations, also the system generated
            // temporal attribute deletedAt". Without it none of them is
            // in the payload — the same set `repr.rs` gates on the
            // current-state path.
            "createdAt" | "modifiedAt" | "expiresAt" | "deletedAt" if !r.sys => continue,
            _ => {}
        }
        if !crate::repr::meta_projected(r.pick.as_deref(), r.omit.as_deref(), k) {
            continue;
        }
        if k == "type" {
            out.insert("type".into(), antares_jsonld::compact_types(v, ctx));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// 4.5.9 temporalValues: the instances grouped by datasetId, one object
/// per group, the group alone when there is one.
fn simplified_attr(instances: &[Value], ctx: &Context, timeprop: &str) -> Value {
    let mut groups: Vec<(Option<String>, Vec<&Value>)> = Vec::new();
    for inst in instances {
        let ds = inst
            .get("datasetId")
            .and_then(Value::as_str)
            .map(String::from);
        match groups.iter_mut().find(|(g, _)| *g == ds) {
            Some((_, list)) => list.push(inst),
            None => groups.push((ds, vec![inst])),
        }
    }
    let mut rendered: Vec<Value> = groups
        .iter()
        .map(|(ds, list)| simplified_group(ds.as_deref(), list, ctx, timeprop))
        .collect();
    if rendered.len() == 1 {
        rendered.remove(0)
    } else {
        Value::Array(rendered)
    }
}

/// One datasetId's instances as `{type, datasetId?, <member>: [[v, t]]}`.
fn simplified_group(ds: Option<&str>, list: &[&Value], ctx: &Context, timeprop: &str) -> Value {
    let atype = list
        .first()
        .and_then(|i| i.get("type"))
        .cloned()
        .unwrap_or_else(|| Value::String("Property".into()));
    let values: Vec<Value> = list
        .iter()
        .map(|inst| {
            let t = inst.get(timeprop).cloned().unwrap_or(Value::Null);
            Value::Array(vec![simplified_value(inst, ctx), t])
        })
        .collect();
    let mut o = Map::new();
    // 4.5.9: the simplified member name follows the attribute type
    let member = match atype.as_str() {
        Some("Relationship") => "objects",
        Some("LanguageProperty") => "languageMaps",
        Some("VocabProperty") => "vocabs",
        Some("JsonProperty") => "jsons",
        Some("ListProperty") => "valueLists",
        Some("ListRelationship") => "objectLists",
        _ => "values",
    };
    o.insert("type".into(), atype);
    if let Some(ds) = ds {
        o.insert("datasetId".into(), Value::String(ds.to_owned()));
    }
    o.insert(member.into(), Value::Array(values));
    Value::Object(o)
}

/// 4.5.9: Property/Relationship pairs carry the bare value/object; other
/// attribute kinds wrap it under their member name.
fn simplified_value(inst: &Value, ctx: &Context) -> Value {
    if let Some(v) = inst.get("value") {
        // 4.5.9: "the first element shall be a
        // Property value" — the one the instance
        // holds. A f64 round trip would retype an
        // integer and drop a digit past 2^53.
        v.clone()
    } else if let Some(o) = inst.get("object") {
        o.clone()
    } else if let Some(lm) = inst.get("languageMap") {
        serde_json::json!({"languageMap": lm})
    } else if let Some(j) = inst.get("json") {
        serde_json::json!({"json": j})
    } else if let Some(vv) = inst.get("vocab") {
        serde_json::json!({"vocab": compact_vocab(vv, ctx)})
    } else if let Some(l) = inst.get("valueList") {
        // 4.5.9 p.63 EXAMPLE 3: the pair's first element
        // is the BARE ordered array, not a {"valueList"}
        // wrapper — unlike languageMap/
        // json/vocab, which the clause does wrap
        l.clone()
    } else if let Some(l) = inst.get("objectList") {
        // 4.5.9 p.65: same bare form for ListRelationship
        l.clone()
    } else {
        Value::Null
    }
}

fn compact_vocab(vv: &Value, ctx: &Context) -> Value {
    match vv {
        Value::String(iri) => Value::String(ctx.compact_iri(iri)),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|s| match s {
                    Value::String(iri) => Value::String(ctx.compact_iri(iri)),
                    o => o.clone(),
                })
                .collect(),
        ),
        o => o.clone(),
    }
}

/// One instance in the normalized representation, its system attributes
/// gone unless sysAttrs asked for them.
fn normalized_instance(inst: &Value, ctx: &Context, sys: bool) -> Value {
    let mut ci = inst.clone();
    if let Some(o) = ci.as_object_mut().filter(|_| !sys) {
        o.remove("createdAt");
        o.remove("modifiedAt");
        // 6.3.11: expiresAt is sysAttrs-gated too
        o.remove("expiresAt");
        // ... and so are the sub-Attributes' own (4.8)
        for sub in o.values_mut() {
            let subs = match sub {
                Value::Array(a) => a.iter_mut().collect(),
                other => vec![other],
            };
            for so in subs.into_iter().filter_map(Value::as_object_mut) {
                if so.contains_key("type") {
                    so.remove("createdAt");
                    so.remove("modifiedAt");
                }
            }
        }
    }
    compact_instance(&ci, ctx)
}

pub(crate) fn parse_aggr_period(s: &str) -> Option<AggrPeriod> {
    let d = antares_model::parse_iso_duration(s)?;
    // Buckets are whole seconds and whole months: a fraction (PT0.5S, P1.5M)
    // would be truncated, and PT0.5S would turn into one bucket for the whole
    // range. Refused instead of answered with other buckets than asked.
    if !d.whole {
        return None;
    }
    // saturating: an absurd magnitude must not panic (debug) or wrap
    // (release) — f64→int `as` casts already saturate, guard the ops.
    let months = (d.years as u32)
        .saturating_mul(12)
        .saturating_add(d.months as u32);
    let secs = [
        (d.weeks, 604_800.0),
        (d.days, 86_400.0),
        (d.hours, 3_600.0),
        (d.minutes, 60.0),
        (d.seconds, 1.0),
    ]
    .into_iter()
    .fold(0i64, |acc, (n, per)| acc.saturating_add((n * per) as i64));
    Some(match (months, secs) {
        (0, 0) => AggrPeriod::Whole,
        (0, sc) => AggrPeriod::Seconds(sc),
        (m, sc) => AggrPeriod::Months(m, sc),
    })
}

pub(super) const AGGR_METHODS: &[&str] = &[
    "totalCount",
    "distinctCount",
    "sum",
    "avg",
    "min",
    "max",
    "stddev",
    "sumsq",
];

/// 5.7.3.4 / 5.7.4.4: "If projection attributes are present and indicate the
/// use of Linked Entity retrieval, an error of type BadRequestData shall be
/// raised." Unconditional on both temporal consumption operations, because
/// neither defines a join; only the clause number in the message differs.
pub(super) fn reject_linked_projection(trepr: &TRepr, clause: &str) -> Result<(), NgsiError> {
    let depth = |p: &Option<Vec<crate::repr::ProjNode>>| {
        p.as_deref().map(crate::repr::proj_depth).unwrap_or(0)
    };
    // attrs=owner{object} names a Linked Entity projection too (4.21)
    let linked_attrs = trepr
        .attrs
        .as_ref()
        .is_some_and(|a| a.iter().any(|t| t.contains('{')));
    if depth(&trepr.pick) > 0 || depth(&trepr.omit) > 0 || linked_attrs {
        return Err(NgsiError::BadRequestData(format!(
            "temporal projection must not use Linked Entity selection ({clause})"
        )));
    }
    Ok(())
}

pub(crate) fn parse_trepr(
    params: &HashMap<String, String>,
    ctx: &Context,
) -> Result<TRepr, NgsiError> {
    let mut r = TRepr {
        lang: params.get("lang").cloned(),
        ..TRepr::default()
    };
    if let Some(opts) = params.get("options") {
        apply_options(&mut r, opts)?;
    }
    // format wins over options on conflict (6.3.7)
    if let Some(f) = params.get("format") {
        apply_format(&mut r, f)?;
    }
    crate::repr::check_projection_exclusive(params)?;
    if let Some(pck) = params.get("pick") {
        r.pick = Some(crate::repr::parse_projection(pck, ctx)?);
    }
    if let Some(o) = params.get("omit") {
        r.omit = Some(crate::repr::parse_projection(o, ctx)?);
    }
    if let Some(a) = params.get("attrs") {
        r.attrs = Some(a.split(',').map(|t| ctx.expand_key(t.trim())).collect());
    }
    r.dataset_id = params
        .get("datasetId")
        .map(|s| s.split(',').map(|d| d.trim().to_owned()).collect());
    r.page = InstancePage::parse(params)?;
    if let Some(m) = params.get("aggrMethods") {
        r.aggr_methods = parse_aggr_methods(m)?;
        // aggrMethods implies aggregation UNLESS an explicit format says otherwise
        if !params.contains_key("format") {
            r.aggregated = true;
        }
    }
    if r.aggregated && r.aggr_methods.is_empty() {
        return Err(NgsiError::BadRequestData(
            "aggregatedValues requires aggrMethods (4.5.19)".into(),
        ));
    }
    if let Some(d) = params.get("aggrPeriodDuration") {
        r.aggr_period = checked_aggr_period(d)?;
    }
    Ok(r)
}

fn apply_options(r: &mut TRepr, opts: &str) -> Result<(), NgsiError> {
    for o in opts.split(',') {
        match o.trim() {
            "sysAttrs" => r.sys = true,
            "temporalValues" => r.temporal_values = true,
            "aggregatedValues" => r.aggregated = true,
            "normalized" => {}
            other => {
                return Err(NgsiError::InvalidRequest(format!(
                    "unsupported options value {other:?}"
                )))
            }
        }
    }
    Ok(())
}

fn apply_format(r: &mut TRepr, f: &str) -> Result<(), NgsiError> {
    (r.temporal_values, r.aggregated) = match f {
        "temporalValues" => (true, false),
        "aggregatedValues" => (false, true),
        "normalized" => (false, false),
        other => {
            return Err(NgsiError::InvalidRequest(format!(
                "unsupported format value {other:?}"
            )))
        }
    };
    Ok(())
}

fn parse_aggr_methods(m: &str) -> Result<Vec<String>, NgsiError> {
    m.split(',')
        .map(|method| {
            let method = method.trim();
            if !AGGR_METHODS.contains(&method) {
                return Err(NgsiError::BadRequestData(format!(
                    "invalid aggrMethods value {method:?} (4.5.19)"
                )));
            }
            Ok(method.to_owned())
        })
        .collect()
}

/// 4.11: the value space ends where duration arithmetic does —
/// beyond ~100 years chrono::Duration::seconds is out of bounds
/// (a panic, i.e. a remote 500), so such periods are rejected.
/// Both components are bounded at ~100 years, the months in their own
/// unit since a month is not a fixed number of seconds.
fn checked_aggr_period(d: &str) -> Result<AggrPeriod, NgsiError> {
    let p = parse_aggr_period(d)
        .ok_or_else(|| NgsiError::BadRequestData(format!("invalid aggrPeriodDuration {d:?}")))?;
    let (months, secs) = match p {
        AggrPeriod::Whole => (0, 0),
        AggrPeriod::Seconds(sc) => (0, sc),
        AggrPeriod::Months(m, sc) => (m, sc),
    };
    if months > 1200 || secs > 86_400 * 366 * 100 {
        return Err(NgsiError::BadRequestData(format!(
            "aggrPeriodDuration {d:?} is out of range"
        )));
    }
    Ok(p)
}

/// The attribute-selection set for windowing: attrs= or pick=.
pub(super) fn selection(r: &TRepr) -> Option<Vec<String>> {
    if let Some(a) = &r.attrs {
        return Some(a.clone());
    }
    // core-member picks (id/type/…) are presentation-only, not attr selection
    r.pick.as_ref().map(|p| {
        p.iter()
            .filter(|n| !is_meta(&n.raw))
            .map(|n| n.iri.clone())
            .collect()
    })
}
