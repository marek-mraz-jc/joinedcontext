// SPDX-License-Identifier: EUPL-1.2
//! Registration matching over a registration document: the `CsrSpec`
//! of a request or subscription, the 5.9 information/registration
//! match (4.3.6.1), the csf and scope filters, the temporal interval
//! and expiry of a registration, and the 5.11.2 subscription match.
//! Pure functions over documents; no route and no store.

use antares_jsonld::Context;
use antares_store::Kind;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// 5.9.2.4: a registration whose expiresAt has been reached counts as
/// deleted — lazily filtered on every read/match path (dt_key so fraction
/// spellings cannot misorder, 4.11).
pub fn reg_expired(doc: &Value) -> bool {
    doc.get("expiresAt")
        .and_then(Value::as_str)
        .is_some_and(|e| antares_model::dt_key(e) < antares_model::dt_key(&crate::state::now_iso()))
}

/// Output shaping: compact IRIs.
pub fn present_registration(doc: &Value, ctx: &Context, sys_attrs: bool) -> Value {
    let Some(obj) = doc.as_object() else {
        return doc.clone();
    };
    let mut out = Map::new();
    for (k, v) in obj {
        match k.as_str() {
            // 5.2.9 has no member under this prefix: the creating subject
            // (policy::SUBJECT_MEMBER) stays in the broker, in a response
            // and in a 5.11 notification alike
            k if k.starts_with("__") => continue,
            "createdAt" | "modifiedAt" if !sys_attrs => continue,
            "information" => {
                let infos: Vec<Value> = v
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|info| {
                        let mut ni = info.as_object().cloned().unwrap_or_default();
                        if let Some(es) = ni.get("entities").and_then(Value::as_array) {
                            let nes: Vec<Value> = es
                                .iter()
                                .map(|e| {
                                    let mut ne = e.as_object().cloned().unwrap_or_default();
                                    match ne.get("type") {
                                        Some(Value::String(t)) => {
                                            let c = ctx.compact_iri(t);
                                            ne.insert("type".into(), Value::String(c));
                                        }
                                        Some(Value::Array(ts)) => {
                                            let cs: Vec<Value> = ts
                                                .iter()
                                                .filter_map(Value::as_str)
                                                .map(|t| Value::String(ctx.compact_iri(t)))
                                                .collect();
                                            ne.insert("type".into(), Value::Array(cs));
                                        }
                                        _ => {}
                                    }
                                    Value::Object(ne)
                                })
                                .collect();
                            ni.insert("entities".into(), Value::Array(nes));
                        }
                        for names_key in antares_model::REGISTRATION_ATTRIBUTE_MEMBERS {
                            if let Some(names) = ni.get(names_key).and_then(Value::as_array) {
                                let nn: Vec<Value> = names
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(|n| Value::String(ctx.compact_iri(n)))
                                    .collect();
                                ni.insert(names_key.into(), Value::Array(nn));
                            }
                        }
                        Value::Object(ni)
                    })
                    .collect();
                out.insert("information".into(), Value::Array(infos));
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// 5.10.2.4 temporal matching against observationInterval/managementInterval.
pub(crate) fn temporal_interval_matches(doc: &Value, tq: &crate::temporalq::TemporalQ) -> bool {
    let key = if tq.timeproperty == "observedAt" {
        "observationInterval"
    } else {
        "managementInterval"
    };
    let Some(iv) = doc.get(key).and_then(Value::as_object) else {
        return false; // relevant interval not present ⇒ no match
    };
    // 4.11 comparison on the canonical key — equal instants in different
    // 4.6.3 fraction spellings must hit the bounds exactly.
    let dt = antares_model::dt_key;
    let start = dt(iv.get("startAt").and_then(Value::as_str).unwrap_or(""));
    let end = iv.get("endAt").and_then(Value::as_str).map(dt); // open-ended when absent
    match tq.timerel.as_str() {
        // interval contains times before/after timeAt (037_09, 047_10/11)
        "before" => start < dt(&tq.time_at),
        "after" => end.is_none_or(|e| e > dt(&tq.time_at)),
        "between" => {
            // overlap between [timeAt, endTimeAt] and the interval
            let qe = dt(tq.end_time_at.as_deref().unwrap_or(&tq.time_at));
            dt(&tq.time_at) <= end.unwrap_or_else(|| "9999".into()) && qe >= start
        }
        _ => true,
    }
}

/// The entity/attribute specification matched against registrations (5.12).
#[derive(Default)]
pub struct CsrSpec {
    /// Expanded type IRIs (or raw 4.17 selector expressions).
    pub types: Option<Vec<String>>,
    pub ids: Option<Vec<String>>,
    pub id_pattern: Option<String>,
    /// Expanded attribute IRIs.
    pub attrs: Option<Vec<String>>,
    /// 5.12 datasetId condition (should-level): with both the request and
    /// the CSourceRegistration specifying datasetId, they match only with
    /// "at least one value in common"; one side alone always matches.
    pub dataset_ids: Option<Vec<String>>,
    /// 4.9 Context Source Filter: with a csf present, only registrations
    /// whose Context Source Properties match it are considered (query,
    /// temporal query, purge, entityMaps — 5.7.2.4/5.7.4.4/5.6.21.4).
    pub csf: Option<antares_ql::QNode>,
    /// 5.2.9 location ("Location for which the Context Source may be able
    /// to provide information") + 4.3.6.1: a geo query is only distributed
    /// to registrations whose location geometry matches it.
    pub geo: Option<antares_ql::geo::GeoQuery>,
    /// 5.2.9 observationInterval/managementInterval: "matched against the
    /// observationInterval for overlap" — a temporal read is only
    /// distributed to registrations whose declared interval overlaps the
    /// temporal query; a registration declaring NO interval stays
    /// unconstrained (both members are optional).
    pub temporal: Option<crate::temporalq::TemporalQ>,
}

/// 5.12: does an EntityInfo element match the entity specification?
fn entity_info_matches(spec: &CsrSpec, ei: &Value, ctx: &Context) -> bool {
    if let Some(types) = &spec.types {
        let its = ei_types(ei);
        // EntityInfo without a type restricts only by id/idPattern. Each spec
        // entry is a 4.17 Entity Type Selection over the WHOLE declared type
        // list (a conjunction needs every named type present); a plain
        // expanded IRI is the one-term case of the same evaluation.
        if !its.is_empty()
            && !types.iter().any(|t| {
                its.contains(&t.as_str()) || antares_ql::type_selection_matches(t, &its, ctx)
            })
        {
            return false;
        }
    }
    let ei_id = ei.get("id").and_then(Value::as_str);
    let ei_pat = ei.get("idPattern").and_then(Value::as_str);
    if ei_id.is_none() && ei_pat.is_none() {
        return true;
    }
    if let Some(ids) = &spec.ids {
        if let Some(rid) = ei_id {
            if ids.iter().any(|i| i == rid) {
                return true;
            }
        }
        if let Some(p) = ei_pat {
            if let Ok(re) = antares_ql::regex::compile(p) {
                if ids.iter().any(|i| re.find(i).is_some()) {
                    return true;
                }
            }
        }
    }
    if let Some(qp) = &spec.id_pattern {
        if let Some(rid) = ei_id {
            if antares_ql::regex::compile(qp).is_ok_and(|re| re.find(rid).is_some()) {
                return true;
            }
        }
        if ei_pat.is_some() {
            return true; // both patterns present ⇒ assumed compatible (5.12)
        }
    }
    // no id restriction given by the query side ⇒ EntityInfo id restrictions
    // don't exclude it when the type matched
    spec.ids.is_none() && spec.id_pattern.is_none()
}

fn attrs_match_info(attrs: &Option<Vec<String>>, info: &Value) -> bool {
    let Some(attrs) = attrs else { return true };
    if attrs.is_empty() {
        return true;
    }
    // TS 104 175 9.4.2: attributeNames, else the deprecated pair
    match antares_model::registration_attribute_names(info) {
        None => true,
        Some(names) => names.iter().any(|n| attrs.iter().any(|w| w == n)),
    }
}

/// 5.12: the RegistrationInfo elements of `doc.information` that match `spec`.
pub fn matching_infos<'a>(spec: &CsrSpec, doc: &'a Value, ctx: &Context) -> Vec<&'a Value> {
    let Some(infos) = doc.get("information").and_then(Value::as_array) else {
        return Vec::new();
    };
    infos
        .iter()
        .filter(|info| {
            let entity_ok = match info.get("entities").and_then(Value::as_array) {
                None => true,
                Some(es) => es.iter().any(|ei| entity_info_matches(spec, ei, ctx)),
            };
            entity_ok && attrs_match_info(&spec.attrs, info)
        })
        .collect()
}

/// 5.10.2.4: the context source filter (csf, 4.9) evaluates over the
/// registration document's own Context Source Properties — its members are
/// wrapped as Property instances so the shared 4.9 evaluator applies.
pub(crate) fn csf_matches(csf: &antares_ql::QNode, reg: &Value, ctx: &Context) -> bool {
    let Some(obj) = reg.as_object() else {
        return false;
    };
    let mut pseudo = Map::new();
    pseudo.insert("id".into(), obj.get("id").cloned().unwrap_or(Value::Null));
    pseudo.insert(
        "type".into(),
        serde_json::json!(["ContextSourceRegistration"]),
    );
    for (k, v) in obj {
        // the `__` members are the broker's (policy::SUBJECT_MEMBER), not
        // Context Source Properties a client may filter on (T-3692)
        if ["id", "type", "information", "createdAt", "modifiedAt"].contains(&k.as_str())
            || k.starts_with("__")
        {
            continue;
        }
        // a Context Source Property stored in attribute form (5.2.9) is an
        // instance already — only bare scalars need the Property wrap
        let inst = match v {
            Value::Array(_) => v.clone(),
            Value::Object(o) if o.contains_key("value") || o.contains_key("object") => {
                serde_json::json!([v])
            }
            _ => serde_json::json!([{"type": "Property", "value": v}]),
        };
        pseudo.insert(ctx.expand_key(k), inst);
    }
    antares_ql::eval::eval_q(csf, &Value::Object(pseudo), ctx, &|_| None)
}

pub fn csr_matches(spec: &CsrSpec, doc: &Value, ctx: &Context) -> bool {
    !matching_infos(spec, doc, ctx).is_empty()
}

/// Full 5.11.2.4 match of a registration against a csource subscription:
/// 5.12 entity/attr matching + temporal interval rules + geoQ vs the
/// registration's own `location`.
///
/// Not the same rule as `antares_matcher::selector_match`, which it shares a
/// signature with: that one asks whether an ENTITY satisfies a
/// subscription's `entities` selector, this one asks whether a REGISTRATION
/// does — and a registration carries its selector inside `information`, has
/// an observation/management interval a latest-information subscription must
/// not match, and answers `geoQ` from its own `location` rather than from a
/// GeoProperty of the data. The part that IS the same rule is the selector
/// walk, and it is not written twice: `spec_for_subscription` turns the
/// subscription into a `CsrSpec` and `csr_matches` walks it.
pub fn csr_matches_subscription(sub: &Value, reg: &Value, ctx: &Context) -> bool {
    // An expired registration is no longer a Context Source: it must not be
    // reported as newlyMatching, nor receive a forwarded subscription copy.
    if reg_expired(reg) {
        return false;
    }
    let spec = spec_for_subscription(sub);
    if !csr_matches(&spec, reg, ctx) {
        return false;
    }
    let has_interval =
        reg.get("observationInterval").is_some() || reg.get("managementInterval").is_some();
    match sub.get("temporalQ").and_then(Value::as_object) {
        None => {
            if has_interval {
                return false; // latest-information sources only (5.11.2.4)
            }
        }
        Some(tq) => {
            let mut params: HashMap<String, String> = HashMap::new();
            for k in ["timerel", "timeAt", "endTimeAt", "timeproperty"] {
                if let Some(s) = tq.get(k).and_then(Value::as_str) {
                    params.insert(k.into(), s.into());
                }
            }
            if let Ok(Some(t)) = crate::temporalq::TemporalQ::from_params(&params, false) {
                if t.timerel != "any" && !temporal_interval_matches(reg, &t) {
                    return false;
                }
            }
        }
    }
    if let Some(g) = sub.get("geoQ").and_then(Value::as_object) {
        if let Ok(Some(gq)) =
            antares_ql::geo::GeoQuery::from_params(&antares_matcher::geo_params(g))
        {
            // Table 5.2.6.5.2-1: matched against the registration GeoJSON
            // member `geoproperty` names (location, observationSpace,
            // operationSpace), stored by its compacted name
            let member = if gq.geoproperty.is_empty() {
                "location".to_owned()
            } else {
                ctx.compact_iri(&ctx.expand_key(&gq.geoproperty))
            };
            match reg.get(&member) {
                Some(geom) => {
                    if !gq.matches_geometry(geom) {
                        return false;
                    }
                }
                None => return false,
            }
        }
    }
    // 5.11.2.4: csf vs the registration's Context Source Properties, scopeQ
    // vs its scope property
    if let Some(csf) = sub.get("csf").and_then(Value::as_str) {
        match antares_ql::parse_q(csf) {
            Ok(ast) if csf_matches(&ast, reg, ctx) => {}
            _ => return false,
        }
    }
    if let Some(sq) = sub.get("scopeQ").and_then(Value::as_str) {
        if !antares_ql::scope::scope_matches(sq, reg) {
            return false;
        }
    }
    true
}

/// Build the 5.12 spec for a csource subscription (5.11.2.4): entities
/// selectors + watchedAttributes ∪ notification.attributes.
pub fn spec_for_subscription(sub: &Value) -> CsrSpec {
    let mut spec = CsrSpec::default();
    if let Some(es) = sub.get("entities").and_then(Value::as_array) {
        let mut types = Vec::new();
        let mut ids = Vec::new();
        let mut patterns = Vec::new();
        // a selector naming no id restricts no id: then neither list does
        let mut unrestricted = false;
        for e in es {
            if let Some(t) = e.get("type").and_then(Value::as_str) {
                types.push(t.to_owned());
            }
            let before = ids.len();
            match e.get("id") {
                Some(Value::String(i)) => ids.push(i.clone()),
                Some(Value::Array(a)) => {
                    ids.extend(a.iter().filter_map(Value::as_str).map(str::to_owned));
                }
                _ => {}
            }
            let pattern = e.get("idPattern").and_then(Value::as_str);
            if let Some(p) = pattern {
                patterns.push(format!("(?:{p})"));
            }
            unrestricted |= ids.len() == before && pattern.is_none();
        }
        if !types.is_empty() {
            spec.types = Some(types);
        }
        // every selector's ids and idPatterns (F-ANT3-015), as one id list
        // and one alternation
        if !unrestricted {
            spec.ids = Some(ids).filter(|i| !i.is_empty());
            spec.id_pattern = Some(patterns.join("|")).filter(|p| !p.is_empty());
        }
    }
    let mut attrs: Vec<String> = sub
        .get("watchedAttributes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Some(na) = sub
        .get("notification")
        .and_then(|n| n.get("attributes"))
        .and_then(Value::as_array)
    {
        attrs.extend(na.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if !attrs.is_empty() {
        spec.attrs = Some(attrs);
    }
    spec
}

/// The kind one Registration Subscription id is stored under.
pub(crate) fn csr_kind(id: &str) -> Kind {
    if id.starts_with(INTERNAL_CSR_PREFIX) {
        Kind::DistSub
    } else {
        Kind::CSourceSubscription
    }
}

/// 5.8.1.4: "The mapping of the received subscriptionId with the own
/// Subscription identifier is stored" (inbound), "a mapping of the id of
/// the Context Source Registration to the received subscriptionId is
/// stored" (remotes), and "the mapping of the id of the Subscription to the
/// … Context Source Registration Subscription shall be stored" (csr_sub).
/// All three live in the store (Kind::DistSub) so persistent modes keep the
/// consumer half across restarts: one doc per (tenant, own Subscription id)
/// = {"csr_sub": id, "remotes": {reg_id: [endpoint, remote sub id]}}, plus
/// inbound index docs under the internal "distsub-index" tenant
/// (id = remote subscriptionId, doc = {"tenant", "own"}).
/// 5.8.1.4: the Registration Subscription the distributed half owns is
/// broker plumbing, not a resource a Context Source Subscriber created —
/// 5.11.5.4 lists the subscriptions clients made through 5.11.2, and this
/// one carries the internal `urn:antares:distsub:` endpoint naming the
/// tenant and the owning Subscription. It is stored under `Kind::DistSub`,
/// so the 5.11 endpoints cannot read, patch or delete it, and its id
/// namespace is what tells the two apart on the notification path.
pub(crate) const INTERNAL_CSR_PREFIX: &str = "urn:ngsi-ld:CSourceSubscription:distsub:";

/// 5.2.8: EntityInfo type is a String or String[] — yield every named type.
pub(crate) fn ei_types(ei: &Value) -> Vec<&str> {
    match ei.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use antares_jsonld::Loader;
    use serde_json::json;

    /// F-ANT3-015: a CSR subscription's id arrays and every selector's
    /// idPattern restrict which registrations it watches; a selector with
    /// neither restricts none.
    #[test]
    fn a_subscription_spec_reads_every_selectors_ids_and_patterns() {
        let spec = spec_for_subscription(&json!({"entities": [
            {"type": "T", "id": ["urn:a", "urn:b"]},
            {"type": "T", "idPattern": "^urn:c"},
            {"type": "T", "idPattern": "^urn:d"}
        ]}));
        assert_eq!(spec.ids, Some(vec!["urn:a".to_owned(), "urn:b".to_owned()]));
        assert_eq!(spec.id_pattern.as_deref(), Some("(?:^urn:c)|(?:^urn:d)"));
        let open = spec_for_subscription(&json!({"entities": [
            {"type": "T", "id": "urn:a"}, {"type": "U"}
        ]}));
        assert_eq!((open.ids, open.id_pattern), (None, None));
    }

    /// TS 104 175 Table 5.2.6.5.2-1: a CSR subscription's geoQ "is matched
    /// against the specified Context Source Registration GeoJSON attributes,
    /// e.g. location, observationSpace or operationSpace": `geoproperty`
    /// names which one, `location` when it names none (12.4.2).
    /// A csf reads the Registration's Context Source Properties, never the
    /// broker's `__` members: a client whose `@context` maps a term onto
    /// the member cannot learn the subject another Registration was created
    /// by (T-3692), whatever form a broker member is stored in. The 4.9 grammar refuses `__subject` and a full IRI, so
    /// a term is the only way to name it.
    #[tokio::test]
    async fn csf_never_reads_the_recorded_subject() {
        let core = Loader::new().core();
        let (iri, name) = (
            core.expand_key("__subject"),
            core.expand_key("registrationName"),
        );
        let other = core.expand_key("__other");
        let ctx = Loader::new()
            .resolve(&json!({"s": iri, "o": other, "n": name}))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let reg = json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:s",
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Car"}]}],
            "endpoint": "http://a.example.com",
            "registrationName": "r",
            "__subject": [["X-Subject", "alice"]],
            // any broker member, whatever its form: one in Property form
            // would match if the prefix were read
            "__other": {"value": "alice"}
        });
        for q in ["s", "s[1]==\"alice\"", "s==\"alice\"", "o", "o==\"alice\""] {
            let csf = antares_ql::parse_q(q).unwrap_or_else(|e| panic!("{q}: {e}"));
            assert!(!csf_matches(&csf, &reg, &ctx), "{q}");
        }
        let named = antares_ql::parse_q("n").unwrap_or_else(|e| panic!("{e}"));
        assert!(csf_matches(&named, &reg, &ctx), "a property still matches");
    }

    #[test]
    fn clause_12_4_2_geoq_geoproperty_matches_observation_space() {
        let ctx = Loader::new().core();
        let point = |x: f64| json!({"type": "Point", "coordinates": [x, 0.5]});
        let reg = json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:geo",
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://a.example.com",
            "location": point(5.0),
            "observationSpace": point(0.5)
        });
        let sub = |geoproperty: Option<&str>| {
            let mut geo = json!({"georel": "within", "geometry": "Polygon",
                "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]]});
            if let Some(p) = geoproperty {
                geo["geoproperty"] = json!(p);
            }
            json!({"entities": [{"type": "Building"}], "geoQ": geo})
        };
        assert!(csr_matches_subscription(
            &sub(Some("observationSpace")),
            &reg,
            &ctx
        ));
        assert!(
            csr_matches_subscription(
                &sub(Some("https://uri.etsi.org/ngsi-ld/observationSpace")),
                &reg,
                &ctx
            ),
            "the expanded name names the same member"
        );
        assert!(
            !csr_matches_subscription(&sub(None), &reg, &ctx),
            "location is outside"
        );
        assert!(!csr_matches_subscription(
            &sub(Some("location")),
            &reg,
            &ctx
        ));
        assert!(
            !csr_matches_subscription(&sub(Some("operationSpace")), &reg, &ctx),
            "a member the registration lacks matches nothing"
        );
    }

    /// 5.11.2.4: a csource subscription's csf matches the registration's
    /// own Context Source Properties, and its scopeQ matches the
    /// registration scope.
    #[test]
    fn csource_subscription_csf_and_scope_matching() {
        let ctx = Loader::new().core();
        let reg_a = json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:sub-a",
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://a.example.com",
            "scope": "/Madrid/Centro"
        });
        let reg_b = json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:sub-b",
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://b.example.com",
            "scope": "/Berlin"
        });
        let sub_csf = json!({
            "entities": [{"type": "Building"}],
            "csf": "endpoint==\"http://a.example.com\""
        });
        assert!(csr_matches_subscription(&sub_csf, &reg_a, &ctx));
        assert!(
            !csr_matches_subscription(&sub_csf, &reg_b, &ctx),
            "csf must exclude the other endpoint"
        );
        let sub_scope = json!({
            "entities": [{"type": "Building"}],
            "scopeQ": "/Madrid/#"
        });
        assert!(csr_matches_subscription(&sub_scope, &reg_a, &ctx));
        assert!(
            !csr_matches_subscription(&sub_scope, &reg_b, &ctx),
            "scopeQ must exclude /Berlin"
        );
    }
}
