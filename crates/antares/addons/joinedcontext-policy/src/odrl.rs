// SPDX-License-Identifier: EUPL-1.2
//! The caller's grants as a W3C ODRL 2.2 policy in the `ngsi-ld:` profile
//! (README section 4, platform EP-57, R52): the same grants the `AuthZEN`
//! document shows ([`crate::access`]), in the words a data-space connector
//! negotiates in. CIM 009 operations are actions, Entity types targets, and
//! `q`, `scopeQ`, `geoQ`, `temporalQ` constraint left operands; `geoQ` and
//! `temporalQ` are written decomposed, as the geometry and the interval a
//! plain ODRL reader can act on.

use crate::access::{actions, selectors, Grants};
use crate::document::Policy;
use crate::subject::{Caller, Platform};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// The media type of the JSON-LD serialization.
pub const ODRL_JSON: &str = "application/odrl+json";

/// The media type of the RDF serialization.
pub const TURTLE: &str = "text/turtle";

/// The JSON-LD contexts the document is read with: ODRL's own, then the
/// `ngsi-ld:` profile.
pub const CONTEXTS: [&str; 2] = [
    "http://www.w3.org/ns/odrl.jsonld",
    "https://joinedcontext.com/odrl/ngsi-ld/v1/context.jsonld",
];

/// The grants as an ODRL `Set`. Its `uid` is `access_url` with the SHA-256
/// of the rules after it: the same grants read twice have one identifier,
/// a changed policy set a new one.
#[must_use]
pub fn policy(
    caller: &Caller,
    grants: &Grants<'_>,
    served: &dyn Fn(&str) -> bool,
    access_url: &str,
) -> Value {
    let permission = rules(&grants.permissions, served);
    let prohibition = rules(&grants.prohibitions, served);
    let digest = Sha256::digest(json!([&permission, &prohibition]).to_string().as_bytes());
    let hex = digest.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    let mut doc = Map::new();
    doc.insert("@context".into(), json!(CONTEXTS));
    doc.insert("@type".into(), json!("Set"));
    doc.insert("uid".into(), json!(format!("{access_url}#sha256:{hex}")));
    // one assigner for the document when every rule names the same one
    let shown: Vec<&Policy> = grants
        .permissions
        .iter()
        .chain(&grants.prohibitions)
        .copied()
        .filter(|p| !actions(p, served).is_empty())
        .collect();
    if let Some(first) = shown.first().and_then(|p| p.source.assigner.as_ref()) {
        if shown
            .iter()
            .all(|p| p.source.assigner.as_ref() == Some(first))
        {
            doc.insert("assigner".into(), json!(first));
        }
    }
    doc.insert("assignee".into(), json!(assignee(caller)));
    doc.insert("permission".into(), Value::Array(permission));
    if !prohibition.is_empty() {
        doc.insert("prohibition".into(), Value::Array(prohibition));
    }
    Value::Object(doc)
}

/// The caller as an ODRL party.
fn assignee(caller: &Caller) -> &str {
    match caller {
        Caller::Internal => "internal",
        Caller::Platform(Platform {
            user: Some(user), ..
        }) => user,
        Caller::Platform(Platform {
            client: Some(client),
            ..
        }) => client,
        Caller::Platform(Platform { did: Some(did), .. }) => did,
        Caller::Platform(_) => "public",
    }
}

/// One rule per selector: ODRL targets one thing, a policy may name several.
fn rules(policies: &[&Policy], served: &dyn Fn(&str) -> bool) -> Vec<Value> {
    let mut out = Vec::new();
    for p in policies {
        let names = actions(p, served);
        if names.is_empty() {
            continue;
        }
        let action: Vec<String> = names.iter().map(|n| format!("ngsi-ld:{n}")).collect();
        for s in selectors(p) {
            let mut refinement = Vec::new();
            if !s.attributes.is_empty() {
                refinement.push(constraint("ngsi-ld:attrs", "isAnyOf", json!(s.attributes)));
            }
            if let Some(id) = &s.id {
                refinement.push(constraint("ngsi-ld:id", "eq", json!(id)));
            }
            if let Some(pattern) = &s.id_pattern {
                // ODRL has no regular-expression operator: the profile's
                // left operand says how to read it, and a reader without the
                // profile must not act on a refinement it cannot evaluate
                refinement.push(constraint("ngsi-ld:idPattern", "eq", json!(pattern)));
            }
            let mut target = Map::new();
            target.insert("@type".into(), json!("ngsi-ld:EntityType"));
            target.insert("uid".into(), json!(s.entity_type));
            if !refinement.is_empty() {
                target.insert("refinement".into(), Value::Array(refinement));
            }
            let mut rule = Map::new();
            rule.insert("action".into(), json!(action));
            rule.insert("target".into(), Value::Object(target));
            if let Some(assigner) = &p.source.assigner {
                rule.insert("assigner".into(), json!(assigner));
            }
            let constraints = constraints(p);
            if !constraints.is_empty() {
                rule.insert("constraint".into(), Value::Array(constraints));
            }
            out.push(Value::Object(rule));
        }
    }
    out
}

/// `q` and `scopeQ` verbatim, geography and time decomposed.
fn constraints(p: &Policy) -> Vec<Value> {
    let s = &p.source;
    let mut out = Vec::new();
    if let Some(q) = &s.q {
        out.push(constraint("ngsi-ld:q", "eq", json!(q)));
    }
    if let Some(scope) = &s.scope_q {
        out.push(constraint("ngsi-ld:scopeQ", "isPartOf", json!(scope)));
    }
    if let Some(geo) = &s.geo_q {
        out.push(geo_constraint(geo));
    }
    if let Some(temporal) = &s.temporal_q {
        out.extend(temporal_constraints(temporal));
    }
    out
}

/// The `key=value` members of a `;`-separated (or `&`) condition string.
pub(crate) fn members(raw: &str) -> impl Iterator<Item = (&str, &str)> {
    raw.split([';', '&'])
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
}

/// `georel=within;geometry=Polygon;coordinates=[[…]]` as a constraint over a
/// geometry: the relation is the operator, the `GeoJSON` the right operand.
fn geo_constraint(geo_q: &str) -> Value {
    let get = |name: &str| members(geo_q).find(|(k, _)| *k == name).map(|(_, v)| v);
    let coordinates = get("coordinates").and_then(|c| serde_json::from_str::<Value>(c).ok());
    match (get("georel"), get("geometry"), coordinates) {
        (Some(relation), Some(geometry), Some(coordinates)) => constraint(
            "ngsi-ld:geoQ",
            &format!("ngsi-ld:{relation}"),
            json!({"@type": format!("geojson:{geometry}"), "coordinates": coordinates}),
        ),
        // a geometry this cannot read is still a constraint: dropping it
        // would describe a wider grant than the one in force
        _ => constraint("ngsi-ld:geoQ", "eq", json!(geo_q)),
    }
}

/// `timerel=after;timeAt=P-1D` as one or two `dateTime` constraints.
fn temporal_constraints(temporal_q: &str) -> Vec<Value> {
    let get = |name: &str| {
        members(temporal_q)
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    };
    match (get("timerel"), get("timeAt"), get("endTimeAt")) {
        (Some("after"), Some(at), _) => vec![constraint("dateTime", "gteq", json!(at))],
        (Some("before"), Some(at), _) => vec![constraint("dateTime", "lteq", json!(at))],
        (Some("between"), Some(at), Some(end)) => vec![
            constraint("dateTime", "gteq", json!(at)),
            constraint("dateTime", "lteq", json!(end)),
        ],
        _ => vec![constraint("ngsi-ld:temporalQ", "eq", json!(temporal_q))],
    }
}

fn constraint(left: &str, operator: &str, right: Value) -> Value {
    let mut c = Map::new();
    c.insert("leftOperand".into(), json!(left));
    c.insert("operator".into(), json!(operator));
    c.insert("rightOperand".into(), right);
    Value::Object(c)
}

/// The same policy as Turtle, written from the JSON-LD so the two cannot
/// say different things.
#[must_use]
pub fn turtle(doc: &Value) -> String {
    let mut out = String::from(
        "@prefix odrl: <http://www.w3.org/ns/odrl/2/> .\n\
         @prefix ngsi-ld: <https://joinedcontext.com/odrl/ngsi-ld/v1#> .\n\
         @prefix geojson: <https://purl.org/geojson/vocab#> .\n\n",
    );
    let uid = doc.get("uid").and_then(Value::as_str).unwrap_or_default();
    let _ = write!(out, "<{uid}> a odrl:Set");
    for party in ["assigner", "assignee"] {
        if let Some(id) = doc.get(party).and_then(Value::as_str) {
            let _ = write!(out, " ;\n  odrl:{party} {}", party_term(id));
        }
    }
    for key in ["permission", "prohibition"] {
        for rule in doc.get(key).and_then(Value::as_array).into_iter().flatten() {
            let _ = write!(out, " ;\n  odrl:{key} {}", rule_node(rule));
        }
    }
    out.push_str(" .\n");
    out
}

/// An IRI when the identifier is one (a DID, a URL), a literal otherwise.
fn party_term(id: &str) -> String {
    let iri = id.contains(':')
        && !id.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '\\'));
    if iri {
        format!("<{id}>")
    } else {
        literal(id)
    }
}

fn rule_node(rule: &Value) -> String {
    let mut parts = Vec::new();
    let names: Vec<String> = rule
        .get("action")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(term_or_literal)
        .collect();
    if !names.is_empty() {
        parts.push(format!("odrl:action {}", names.join(", ")));
    }
    if let Some(target) = rule.get("target") {
        parts.push(format!("odrl:target {}", target_node(target)));
    }
    if let Some(assigner) = rule.get("assigner").and_then(Value::as_str) {
        parts.push(format!("odrl:assigner {}", party_term(assigner)));
    }
    for item in rule
        .get("constraint")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        parts.push(format!("odrl:constraint {}", constraint_node(item)));
    }
    format!("[\n    {}\n  ]", parts.join(" ;\n    "))
}

fn target_node(target: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = target.get("@type").and_then(Value::as_str) {
        parts.push(format!("a {}", term_or_literal(kind)));
    }
    if let Some(uid) = target.get("uid").and_then(Value::as_str) {
        parts.push(format!("odrl:uid {}", literal(uid)));
    }
    for item in target
        .get("refinement")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        parts.push(format!("odrl:refinement {}", constraint_node(item)));
    }
    format!("[ {} ]", parts.join(" ; "))
}

fn constraint_node(item: &Value) -> String {
    let left = item
        .get("leftOperand")
        .and_then(Value::as_str)
        .map(term_or_literal)
        .unwrap_or_default();
    let operator = item
        .get("operator")
        .and_then(Value::as_str)
        .map(|op| {
            if op.contains(':') {
                term_or_literal(op)
            } else {
                format!("odrl:{op}")
            }
        })
        .unwrap_or_default();
    let right = item
        .get("rightOperand")
        .map_or_else(|| literal(""), right_node);
    format!("[ odrl:leftOperand {left} ; odrl:operator {operator} ; odrl:rightOperand {right} ]")
}

/// A list for a set, a blank node for a geometry, a literal otherwise;
/// numbers and booleans keep their types.
fn right_node(value: &Value) -> String {
    match value {
        Value::String(text) => literal(text),
        Value::Array(values) => values.iter().map(right_node).collect::<Vec<_>>().join(", "),
        Value::Object(object) => {
            let mut parts = Vec::new();
            if let Some(kind) = object.get("@type").and_then(Value::as_str) {
                parts.push(format!("a {}", term_or_literal(kind)));
            }
            for (key, member) in object.iter().filter(|(k, _)| *k != "@type") {
                parts.push(format!("geojson:{key} {}", right_node(member)));
            }
            format!("[ {} ]", parts.join(" ; "))
        }
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => literal(""),
    }
}

/// A prefixed name when the value carries a known prefix and a plain local
/// part, a literal otherwise.
fn term_or_literal(value: &str) -> String {
    match value.split_once(':') {
        Some((prefix, local))
            if matches!(prefix, "ngsi-ld" | "odrl" | "geojson")
                && !local.is_empty()
                && local.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
        {
            format!("{prefix}:{local}")
        }
        _ => literal(value),
    }
}

/// A Turtle string literal with the escapes the grammar requires.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member at `path` (a JSON pointer), `null` when absent.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }
    use crate::document::compile;
    use antares_api::policy::Subject;
    use chrono::Utc;

    const AREA: &str =
        "georel=within;geometry=Polygon;coordinates=[[[0,0],[1,0],[1,1],[0,1],[0,0]]]";

    fn caller(headers: &[(&str, &str)]) -> Caller {
        Caller::of(&Subject {
            tenant: antares_model::TenantId::default(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        })
    }

    fn doc(policies: &Value, who: &Caller) -> Result<Value, String> {
        let t = compile(&json!({ "policies": policies }))?;
        let grants = Grants::of(Some(&t), who, Utc::now());
        Ok(policy(
            who,
            &grants,
            &|_| true,
            "https://b.example/x/views/v/access",
        ))
    }

    fn air(extra: &Value) -> Value {
        let mut p = json!({"id": "air", "assigner": "did:web:city.example.org",
            "assignee": {"kind": "role", "id": "public"}, "operations": ["5.7.1", "5.7.2"],
            "information": [{"entities": [{"type": "AirQualityObserved", "idPattern": "^urn:a:.*$"},
                                          {"type": "Sensor", "id": "urn:ngsi-ld:Sensor:1"}],
                             "propertyNames": ["pm10"]}]});
        if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        p
    }

    #[test]
    fn a_rule_per_selector_with_its_refinements_and_conditions() -> Result<(), String> {
        let public = caller(&[("x-jc-roles", "")]);
        let d = doc(
            &json!([air(&json!({"q": "pm10>=0", "scopeQ": "/SK", "geoQ": AREA,
                                         "temporalQ": "timerel=between;timeAt=2026-01-01T00:00:00Z;endTimeAt=2026-02-01T00:00:00Z"}))]),
            &public,
        )?;
        assert_eq!(at(&d, "/@type"), "Set");
        assert_eq!(at(&d, "/assigner"), "did:web:city.example.org");
        assert_eq!(at(&d, "/assignee"), "public");
        let rules = at(&d, "/permission");
        assert_eq!(rules.as_array().map_or(0, Vec::len), 2);
        assert_eq!(
            at(&rules, "/0/action"),
            json!([
                "ngsi-ld:queryBatch",
                "ngsi-ld:queryEntity",
                "ngsi-ld:retrieveEntity"
            ])
        );
        assert_eq!(at(&rules, "/0/target/uid"), "AirQualityObserved");
        assert_eq!(
            at(&rules, "/0/target/refinement"),
            json!([{"leftOperand": "ngsi-ld:attrs", "operator": "isAnyOf", "rightOperand": ["pm10"]},
                   {"leftOperand": "ngsi-ld:idPattern", "operator": "eq", "rightOperand": "^urn:a:.*$"}])
        );
        assert_eq!(
            at(&rules, "/1/target/refinement/1/leftOperand"),
            "ngsi-ld:id"
        );
        let c = at(&rules, "/0/constraint");
        assert_eq!(
            at(&c, "/0"),
            json!({"leftOperand": "ngsi-ld:q", "operator": "eq", "rightOperand": "pm10>=0"})
        );
        assert_eq!(at(&c, "/1/operator"), "isPartOf");
        assert_eq!(at(&c, "/2/operator"), "ngsi-ld:within");
        assert_eq!(at(&c, "/2/rightOperand/@type"), "geojson:Polygon");
        assert_eq!(
            at(&c, "/3"),
            json!({"leftOperand": "dateTime", "operator": "gteq", "rightOperand": "2026-01-01T00:00:00Z"})
        );
        assert_eq!(at(&c, "/4/operator"), "lteq");
        assert!(d.get("prohibition").is_none());
        Ok(())
    }

    #[test]
    fn a_participant_holds_its_agreements_grants_and_not_the_publics() -> Result<(), String> {
        let did = "did:web:consumer.example";
        let agreed = air(&json!({"id": "agreed", "assignee": {"kind": "did", "id": did}}));
        let participant = caller(&[("x-jc-did", did)]);
        let d = doc(&json!([agreed, air(&json!({}))]), &participant)?;
        assert_eq!(at(&d, "/assignee"), did);
        assert_eq!(at(&d, "/permission").as_array().map_or(0, Vec::len), 2);
        let other = caller(&[("x-jc-did", "did:web:other.example")]);
        let d = doc(
            &json!([
                air(&json!({"id": "agreed", "assignee": {"kind": "did", "id": did}})),
                air(&json!({}))
            ]),
            &other,
        )?;
        assert_eq!(
            at(&d, "/permission"),
            json!([]),
            "neither the agreement nor public: {d}"
        );
        Ok(())
    }

    #[test]
    fn the_uid_is_the_digest_of_the_rules() -> Result<(), String> {
        let public = caller(&[("x-jc-roles", "")]);
        let one = doc(&json!([air(&json!({}))]), &public)?;
        let again = doc(&json!([air(&json!({}))]), &public)?;
        let other = doc(&json!([air(&json!({"q": "pm10>1"}))]), &public)?;
        let uid = at(&one, "/uid");
        let uid = uid.as_str().ok_or("uid")?;
        assert!(
            uid.starts_with("https://b.example/x/views/v/access#sha256:"),
            "{uid}"
        );
        assert_eq!(
            uid.len(),
            "https://b.example/x/views/v/access#sha256:".len() + 64
        );
        assert_eq!(at(&one, "/uid"), at(&again, "/uid"));
        assert_ne!(at(&one, "/uid"), at(&other, "/uid"));
        Ok(())
    }

    #[test]
    fn assigners_that_differ_stay_on_their_rules_and_others_grants_are_absent() -> Result<(), String>
    {
        let ana = caller(&[("x-jc-subject", "ana"), ("x-jc-roles", "r")]);
        let d = doc(
            &json!([
                air(&json!({"assignee": {"kind": "role", "id": "r"}})),
                air(
                    &json!({"id": "b", "assigner": "did:web:other", "assignee": {"kind": "user", "id": "ana"},
                                        "effect": "prohibit", "operations": ["5.7.1"]})
                ),
                air(
                    &json!({"id": "c", "assignee": {"kind": "user", "id": "bob"},
                                        "information": [{"entities": [{"type": "Secret"}]}]})
                )
            ]),
            &ana,
        )?;
        assert!(d.get("assigner").is_none());
        assert_eq!(at(&d, "/assignee"), "ana");
        assert_eq!(at(&d, "/prohibition/0/assigner"), "did:web:other");
        assert_eq!(
            at(&d, "/prohibition/0/action"),
            json!(["ngsi-ld:retrieveEntity"])
        );
        assert!(!d.to_string().contains("Secret"));
        Ok(())
    }

    #[test]
    fn an_unreadable_condition_travels_whole() {
        assert_eq!(
            at(
                &geo_constraint("georel=near;maxDistance==5"),
                "/rightOperand"
            ),
            "georel=near;maxDistance==5"
        );
        assert_eq!(
            at(
                &json!(temporal_constraints("timerel=between;timeAt=P-1D")),
                "/0/leftOperand"
            ),
            "ngsi-ld:temporalQ"
        );
        assert_eq!(
            at(
                &json!(temporal_constraints("timerel=before;timeAt=P-1D")),
                "/0/operator"
            ),
            "lteq"
        );
    }

    #[test]
    fn the_turtle_says_what_the_json_says_and_escapes_what_it_quotes() -> Result<(), String> {
        let public = caller(&[("x-jc-roles", "")]);
        let d = doc(
            &json!([
                air(&json!({"q": "name==\"a\\b\"", "geoQ": AREA})),
                air(&json!({"id": "p", "effect": "prohibit", "operations": ["5.7.1"]}))
            ]),
            &public,
        )?;
        let ttl = turtle(&d);
        assert!(ttl.starts_with("@prefix odrl:"));
        assert!(ttl.contains("odrl:assigner <did:web:city.example.org>"));
        assert!(ttl.contains("odrl:assignee \"public\""));
        assert!(ttl.contains(
            "odrl:action ngsi-ld:queryBatch, ngsi-ld:queryEntity, ngsi-ld:retrieveEntity"
        ));
        assert!(ttl.contains("odrl:prohibition ["));
        assert!(
            ttl.contains(r#"odrl:rightOperand "name==\"a\\b\"""#),
            "{ttl}"
        );
        assert!(ttl.contains("a geojson:Polygon ; geojson:coordinates 0, 0, 1, 0"));
        assert!(ttl.trim_end().ends_with(" ."));
        assert_eq!(literal("a\tb\nc\r"), "\"a\\tb\\nc\\r\"");
        assert_eq!(party_term("did:web:x y"), "\"did:web:x y\"");
        assert_eq!(party_term("did:web:x>"), "\"did:web:x>\"");
        assert_eq!(term_or_literal("ngsi-ld:a b"), "\"ngsi-ld:a b\"");
        assert_eq!(right_node(&json!(true)), "true");
        assert_eq!(right_node(&Value::Null), "\"\"");
        Ok(())
    }
}
