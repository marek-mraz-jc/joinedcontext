// SPDX-License-Identifier: EUPL-1.2
//! The caller's residual as a UCAST condition tree (README section 4,
//! platform EP-58): one entry per Entity type with the operations it may
//! call, the Attributes it may project and the conditions a request under
//! it is narrowed by, so a client can filter before it asks.
//!
//! A type no policy names is absent; a prohibition that takes back every
//! operation on a type removes the type rather than showing as a rule
//! (EP-59). `q` travels verbatim beside `where`: splitting an NGSI-LD filter
//! into branches would need a second query grammar.

use crate::access::{actions, selectors, Grants};
use crate::document::Policy;
use crate::odrl::members;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The media type of the condition-tree representation.
pub const GRANT_AST_JSON: &str = "application/vnd.joinedcontext.grant-ast+json";

/// What the grants say about one Entity type.
#[derive(Default)]
struct Type {
    operations: BTreeSet<&'static str>,
    attributes: BTreeSet<String>,
    /// Some grant reaches every Attribute, which subsumes any list.
    all_attributes: bool,
    /// Per grant: its `q` and its condition tree, each absent when the
    /// grant does not narrow by it.
    branches: Vec<(Option<String>, Option<Value>)>,
}

/// The grants as one entry per Entity type.
#[must_use]
pub fn grant_ast(grants: &Grants<'_>, served: &dyn Fn(&str) -> bool) -> Value {
    let mut denied: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for p in &grants.prohibitions {
        let ops = actions(p, served);
        for s in selectors(p) {
            denied.entry(s.entity_type).or_default().extend(&ops);
        }
    }
    let mut granted: BTreeMap<String, Type> = BTreeMap::new();
    for p in &grants.permissions {
        let ops = actions(p, served);
        if ops.is_empty() {
            continue;
        }
        for s in selectors(p) {
            let entry = granted.entry(s.entity_type).or_default();
            entry.operations.extend(&ops);
            if s.attributes.is_empty() {
                entry.all_attributes = true;
            } else {
                entry.attributes.extend(s.attributes);
            }
            entry.branches.push(branch(p));
        }
    }
    let mut doc = Map::new();
    for (ty, mut entry) in granted {
        // a prohibition of every type (`*`) takes its operations off each
        for off in [denied.get(&ty), denied.get("*")].into_iter().flatten() {
            entry.operations.retain(|op| !off.contains(op));
        }
        if entry.operations.is_empty() {
            continue;
        }
        doc.insert(ty, entry.into_value());
    }
    Value::Object(doc)
}

impl Type {
    fn into_value(self) -> Value {
        let mut out = Map::new();
        out.insert("operations".into(), json!(self.operations));
        out.insert(
            "project".into(),
            if self.all_attributes {
                json!("*")
            } else {
                json!(self.attributes)
            },
        );
        // a grant that narrows nothing subsumes every other
        if self
            .branches
            .iter()
            .any(|(q, tree)| q.is_none() && tree.is_none())
        {
            return Value::Object(out);
        }
        let filters: Vec<&str> = self
            .branches
            .iter()
            .filter_map(|(q, _)| q.as_deref())
            .collect();
        // the union is expressible only when every grant carries a `q`
        // (NGSI-LD writes OR as `|`); one grant's `q` alone is its own
        if filters.len() == self.branches.len() {
            out.insert("q".into(), json!(union(&filters)));
        }
        let mut trees: Vec<Value> = self
            .branches
            .into_iter()
            .filter_map(|(_, tree)| tree)
            .collect();
        match trees.len() {
            0 => {}
            1 => {
                out.insert("where".into(), trees.remove(0));
            }
            // several grants are several ways in: a union (R7)
            _ => {
                out.insert("where".into(), compound("or", trees));
            }
        }
        Value::Object(out)
    }
}

/// One grant's residual: its `q` verbatim, and the tree its other
/// conditions make.
fn branch(p: &Policy) -> (Option<String>, Option<Value>) {
    let s = &p.source;
    let mut conditions = Vec::new();
    if let Some(scope) = &s.scope_q {
        conditions.push(field("scope_under", "scope", json!(scope)));
    }
    if let Some(geo) = &s.geo_q {
        conditions.push(geo_condition(geo));
    }
    if let Some(temporal) = &s.temporal_q {
        conditions.extend(temporal_condition(temporal));
    }
    let tree = match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ => Some(compound("and", conditions)),
    };
    (s.q.clone(), tree)
}

/// `georel=within;geometry=Polygon;coordinates=[[…]]` as a `geo_within`
/// leaf; a geometry this cannot read travels as the filter string.
fn geo_condition(geo_q: &str) -> Value {
    let get = |name: &str| members(geo_q).find(|(k, _)| *k == name).map(|(_, v)| v);
    let property = get("geoproperty").unwrap_or("location");
    let coordinates = get("coordinates").and_then(|c| serde_json::from_str::<Value>(c).ok());
    match (get("geometry"), coordinates) {
        (Some(geometry), Some(coordinates)) => field(
            "geo_within",
            property,
            json!({"type": geometry, "coordinates": coordinates}),
        ),
        _ => field(
            "geo_within",
            property,
            json!({"geoQ": geo_q, "georel": get("georel").unwrap_or("within")}),
        ),
    }
}

/// `timerel=…` as a `gte`/`lte` leaf, or `time_between` for a closed
/// window. A duration stays one: a moving bound resolved now would be stale
/// on the client.
fn temporal_condition(temporal_q: &str) -> Option<Value> {
    let get = |name: &str| {
        members(temporal_q)
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    };
    match (get("timerel"), get("timeAt"), get("endTimeAt")) {
        (Some("after"), Some(at), _) => Some(field("gte", "observedAt", instant(at))),
        (Some("before"), Some(at), _) => Some(field("lte", "observedAt", instant(at))),
        (Some("between"), Some(at), Some(end)) => Some(field(
            "time_between",
            "observedAt",
            json!([instant(at), instant(end)]),
        )),
        // the engine refuses any other window at load, so none reaches here
        _ => None,
    }
}

/// An instant as itself, a duration as the moving bound it is.
fn instant(value: &str) -> Value {
    if value.starts_with('P') || value.starts_with("-P") {
        json!({ "relative": value })
    } else {
        json!(value)
    }
}

fn node(kind: &str, operator: &str, field: Option<&str>, value: Value) -> Value {
    let mut n = Map::new();
    n.insert("type".into(), json!(kind));
    n.insert("operator".into(), json!(operator));
    if let Some(field) = field {
        n.insert("field".into(), json!(field));
    }
    n.insert("value".into(), value);
    Value::Object(n)
}

fn field(operator: &str, name: &str, value: Value) -> Value {
    node("field", operator, Some(name), value)
}

fn compound(operator: &str, value: Vec<Value>) -> Value {
    node("compound", operator, None, Value::Array(value))
}

/// The union of NGSI-LD filters: `|` is OR, every operand parenthesized so
/// none changes how its neighbours group.
fn union(filters: &[&str]) -> String {
    match filters {
        [one] => (*one).to_owned(),
        many => many
            .iter()
            .map(|f| format!("({f})"))
            .collect::<Vec<_>>()
            .join("|"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member at `path` (a JSON pointer), `null` when absent.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }
    use crate::document::compile;
    use crate::subject::Caller;
    use antares_api::policy::Subject;
    use chrono::Utc;

    const AREA: &str =
        "georel=within;geometry=Polygon;coordinates=[[[0,0],[1,0],[1,1],[0,1],[0,0]]]";

    fn ast(policies: &Value) -> Result<Value, String> {
        let t = compile(&json!({ "policies": policies }))?;
        let public = Caller::of(&Subject {
            tenant: antares_model::TenantId::default(),
            headers: vec![("x-jc-roles".into(), String::new())],
        });
        let grants = Grants::of(Some(&t), &public, Utc::now());
        Ok(grant_ast(&grants, &|_| true))
    }

    fn grant(id: &str, ty: &str, extra: &Value) -> Value {
        let mut p = json!({"id": id, "assignee": {"kind": "role", "id": "public"},
                           "operations": ["5.7.1", "5.7.2"],
                           "information": [{"entities": [{"type": ty}], "propertyNames": ["pm10"]}]});
        if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        p
    }

    #[test]
    fn one_entry_per_type_with_its_residual() -> Result<(), String> {
        let a = ast(&json!([grant(
            "a",
            "Air",
            &json!({"q": "pm10>0", "scopeQ": "/SK", "geoQ": AREA,
                                                       "temporalQ": "timerel=after;timeAt=P-1D"})
        )]))?;
        assert_eq!(
            at(&a, "/Air/operations"),
            json!(["queryBatch", "queryEntity", "retrieveEntity"])
        );
        assert_eq!(at(&a, "/Air/project"), json!(["pm10"]));
        assert_eq!(at(&a, "/Air/q"), "pm10>0");
        let w = &at(&a, "/Air/where");
        assert_eq!(at(w, "/operator"), "and");
        assert_eq!(
            at(w, "/value/0"),
            json!({"type": "field", "operator": "scope_under", "field": "scope", "value": "/SK"})
        );
        assert_eq!(at(w, "/value/1/value/type"), "Polygon");
        assert_eq!(at(w, "/value/2/value"), json!({"relative": "P-1D"}));
        Ok(())
    }

    #[test]
    fn grants_on_one_type_are_a_union_and_an_open_one_subsumes_the_rest() -> Result<(), String> {
        let a = ast(&json!([
            grant("a", "Air", &json!({"q": "pm10>0", "scopeQ": "/a"})),
            grant("b", "Air", &json!({"q": "pm25>0", "scopeQ": "/b"}))
        ]))?;
        assert_eq!(at(&a, "/Air/q"), "(pm10>0)|(pm25>0)");
        assert_eq!(at(&a, "/Air/where/operator"), "or");
        let half = ast(&json!([
            grant("a", "Air", &json!({"q": "pm10>0"})),
            grant("b", "Air", &json!({"scopeQ": "/b"}))
        ]))?;
        assert!(
            at(&half, "/Air").get("q").is_none(),
            "a union with an unfiltered grant is not a q"
        );
        let open = ast(&json!([
            grant("a", "Air", &json!({"q": "pm10>0"})),
            grant(
                "b",
                "Air",
                &json!({"information": [{"entities": [{"type": "Air"}]}]})
            )
        ]))?;
        assert_eq!(
            at(&open, "/Air"),
            json!({"operations": ["queryBatch", "queryEntity", "retrieveEntity"], "project": "*"})
        );
        Ok(())
    }

    #[test]
    fn a_prohibition_takes_operations_off_and_a_type_it_empties_is_gone() -> Result<(), String> {
        let a = ast(&json!([
            grant("a", "Air", &json!({})),
            grant("w", "Water", &json!({})),
            grant(
                "p",
                "Air",
                &json!({"effect": "prohibit", "operations": ["5.7.2"]})
            ),
            grant("q", "Water", &json!({"effect": "prohibit"}))
        ]))?;
        assert_eq!(at(&a, "/Air/operations"), json!(["retrieveEntity"]));
        assert!(a.get("Water").is_none(), "{a}");
        let all = ast(&json!([grant("a", "Air", &json!({})),
                              {"id": "x", "effect": "prohibit", "assignee": {"kind": "role", "id": "public"},
                               "operations": ["5.7.1"]}]))?;
        assert_eq!(
            at(&all, "/Air/operations"),
            json!(["queryBatch", "queryEntity"])
        );
        Ok(())
    }

    #[test]
    fn conditions_it_cannot_split_travel_whole() {
        assert_eq!(
            at(
                &geo_condition("georel=near;maxDistance==5;geoproperty=parking"),
                "/field"
            ),
            "parking"
        );
        assert_eq!(
            at(
                &geo_condition("georel=near;maxDistance==5"),
                "/value/georel"
            ),
            "near"
        );
        assert_eq!(temporal_condition("timerel=between;timeAt=x"), None);
        assert_eq!(
            temporal_condition("timerel=between;timeAt=2026-01-01T00:00:00Z;endTimeAt=-P1D"),
            Some(field(
                "time_between",
                "observedAt",
                json!(["2026-01-01T00:00:00Z", {"relative": "-P1D"}])
            ))
        );
        assert_eq!(
            temporal_condition("timerel=before;timeAt=x"),
            Some(field("lte", "observedAt", json!("x")))
        );
        assert_eq!(union(&[]), "");
    }
}
