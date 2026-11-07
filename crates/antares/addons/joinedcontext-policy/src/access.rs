// SPDX-License-Identifier: EUPL-1.2
//! What the caller may do through one view, read from the table the engine
//! decides with (README section 4, platform EP-55 to EP-60). It answers
//! "what may I do here" and never "what is here": a type only somebody else
//! may touch is absent, not listed as denied.

use crate::decide::{held, refuses_types, withholds, POLICY_VOCABULARY};
use crate::document::{context, Policy, Selector, Table};
use crate::names::{self, OPERATIONS};
use crate::subject::{Caller, Platform};
use antares_api::policy::{Rule, FILTERABLE};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

/// The policies of one caller in force now, split by effect.
pub struct Grants<'t> {
    /// The permissions, in document order.
    pub permissions: Vec<&'t Policy>,
    /// The prohibitions, in document order.
    pub prohibitions: Vec<&'t Policy>,
}

impl<'t> Grants<'t> {
    /// What `caller` holds in `table` at `now`. The cluster's own caller and
    /// a Tenant with no document hold nothing.
    #[must_use]
    pub fn of(table: Option<&'t Table>, caller: &'t Caller, now: DateTime<Utc>) -> Self {
        let (prohibitions, permissions) = match (table, caller) {
            (Some(table), Caller::Platform(p)) => held(table, p, now).partition(|p| p.prohibit),
            _ => (Vec::new(), Vec::new()),
        };
        Self {
            permissions,
            prohibitions,
        }
    }
}

/// The Table 4.20-1 names of the clauses `p` lists whose rows the view
/// serves (`served` answers by operation-matrix row), sorted.
#[must_use]
pub fn actions(p: &Policy, served: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = OPERATIONS
        .iter()
        .filter(|(name, rows)| {
            names::clause(name).is_some_and(|c| p.clauses.contains(c))
                && rows.iter().any(|r| served(r))
        })
        .map(|(name, _)| *name)
        .collect();
    out.sort_unstable();
    out
}

/// The selectors a policy shows: its own, or the type `*` for a policy
/// without `information`, which grants every type.
#[must_use]
pub fn selectors(p: &Policy) -> Vec<Selector> {
    if p.source.selectors.is_empty() {
        vec![Selector {
            entity_type: "*".into(),
            ..Selector::default()
        }]
    } else {
        p.source.selectors.clone()
    }
}

/// The policy's conditions as written (GW11).
#[must_use]
pub fn constraints(p: &Policy) -> Map<String, Value> {
    let s = &p.source;
    [
        ("q", &s.q),
        ("scopeQ", &s.scope_q),
        ("geoQ", &s.geo_q),
        ("temporalQ", &s.temporal_q),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.as_ref().map(|v| (k.to_owned(), json!(v))))
    .collect()
}

/// `attributes` of an entry: the names, or `"*"` for the whole Entity.
fn attributes(s: &Selector) -> Value {
    if s.attributes.is_empty() {
        json!("*")
    } else {
        json!(s.attributes)
    }
}

/// The `AuthZEN` resource-search document (EP-55).
#[must_use]
pub fn authzen(
    caller: &Caller,
    view: &str,
    grants: &Grants<'_>,
    served: &dyn Fn(&str) -> bool,
) -> Value {
    json!({
        "subject": subject(caller),
        "resource": {"type": "view", "id": view},
        "permissions": entries(&grants.permissions, served),
        "prohibitions": entries(&grants.prohibitions, served),
    })
}

/// One entry per selector of each policy that leaves an action through the
/// view.
fn entries(policies: &[&Policy], served: &dyn Fn(&str) -> bool) -> Vec<Value> {
    let mut out = Vec::new();
    for p in policies {
        let actions = actions(p, served);
        if actions.is_empty() {
            continue;
        }
        for s in selectors(p) {
            let mut resource = Map::new();
            resource.insert("type".into(), json!(s.entity_type));
            if let Some(pattern) = &s.id_pattern {
                resource.insert("idPatterns".into(), json!([pattern]));
            }
            if let Some(id) = &s.id {
                resource.insert("id".into(), json!(id));
            }
            out.push(json!({
                "resource": resource,
                "actions": actions,
                "attributes": attributes(&s),
                "constraints": constraints(p),
            }));
        }
    }
    out
}

/// The caller as the document names them: never more than they know.
fn subject(caller: &Caller) -> Value {
    match caller {
        Caller::Internal => json!({"type": "internal"}),
        Caller::Platform(Platform {
            user: Some(user), ..
        }) => json!({"type": "user", "id": user}),
        Caller::Platform(Platform {
            client: Some(client),
            ..
        }) => json!({"type": "serviceAccount", "id": client}),
        Caller::Platform(Platform { did: Some(did), .. }) => json!({"type": "did", "id": did}),
        Caller::Platform(_) => json!({"type": "role", "id": "public"}),
    }
}

/// What decided one prospective request (README section 3, steps 1 to 3):
/// the permission that matched, the prohibition that ended it, or nothing.
#[derive(Debug, Clone, Copy)]
pub enum Verdict<'t> {
    /// A permission listing the action's clause and covering the type (and
    /// the Attribute, when one is asked about).
    Granted(&'t Policy),
    /// A prohibition listing the action's clause and covering the type,
    /// which ends it whatever any permission says (GW8). An entry naming no
    /// Attribute refuses its whole type; one naming Attributes refuses only
    /// those, as `decide` does (T-5842).
    Prohibited(&'t Policy),
    /// A permission covers the type but none lists the Attribute asked
    /// about: the request goes through without it.
    NotInGrant,
    /// No permission grants it, or the view does not serve the action, or
    /// the action is no Table 4.20-1 name.
    NoGrant,
}

/// The [`Verdict`] on the operation named `action` on `entity_type`, and on
/// its Attribute `attribute` (a short name or an IRI) when one is named,
/// through a view that serves the operation-matrix rows `served` answers
/// for. [`check`], [`explain`] and [`attributes_of`] all read it, so they
/// cannot disagree.
#[must_use]
pub fn verdict<'t>(
    grants: &Grants<'t>,
    action: &str,
    entity_type: Option<&str>,
    attribute: Option<&str>,
    served: &dyn Fn(&str) -> bool,
) -> Verdict<'t> {
    let Some(clause) = names::clause(action) else {
        return Verdict::NoGrant;
    };
    let through_view = OPERATIONS
        .iter()
        .any(|(n, rows)| *n == action && rows.iter().any(|r| served(r)));
    if !through_view {
        return Verdict::NoGrant;
    }
    let ctx = context();
    let types: Vec<String> = entity_type.map(|t| ctx.expand_key(t)).into_iter().collect();
    let attribute = attribute.map(|a| ctx.expand_key(a));
    let write = clause.starts_with("5.6.");
    let lists = |p: &&&Policy| p.clauses.contains(clause);
    if let Some(p) = grants.prohibitions.iter().filter(lists).find(|p| {
        // a read naming no type is narrowed, as decide reads it (F-R2ANT1-015)
        refuses_types(p, &types, FILTERABLE.contains(&clause))
            || attribute
                .as_deref()
                .is_some_and(|a| withholds(p, &types, a))
    }) {
        return Verdict::Prohibited(p);
    }
    if write && attribute.as_deref().is_some_and(is_access_control) {
        return Verdict::NoGrant;
    }
    let rules = |p: &'t Policy| {
        p.selectors
            .iter()
            .filter(|s| types.is_empty() || s.types.iter().any(|t| types.contains(t)))
            .collect::<Vec<_>>()
    };
    let mut in_type = false;
    // a window is about reading history; it admits no write (`decide::admits`)
    for p in grants
        .permissions
        .iter()
        .filter(lists)
        .filter(|p| !write || p.conditions.temporal.is_empty())
    {
        let shows = |s: &Rule| {
            attribute
                .as_ref()
                .is_none_or(|a| s.every_attribute || s.attributes.contains(a))
        };
        if p.selectors.is_empty() {
            return Verdict::Granted(p);
        }
        let covering = rules(p);
        in_type |= !covering.is_empty();
        if covering.into_iter().any(shows) {
            return Verdict::Granted(p);
        }
    }
    if in_type {
        Verdict::NotInGrant
    } else {
        Verdict::NoGrant
    }
}

/// An Attribute name the engine refuses in every write (platform GW29).
fn is_access_control(iri: &str) -> bool {
    let ctx = context();
    POLICY_VOCABULARY
        .iter()
        .any(|name| *name == iri || ctx.expand_key(name) == iri)
}

/// The Attribute a check asks about: `resource.properties.attribute`.
#[must_use]
pub fn attribute_of(query: &Value) -> Option<&str> {
    query
        .pointer("/resource/properties/attribute")
        .and_then(Value::as_str)
}

/// One `AuthZEN` decision for one prospective request (platform R51): the
/// operation named `action` on `entity_type` (and its Attribute
/// `attribute`), decided as the engine decides (README section 3, steps 1
/// to 3). A permission may name its assigner, since the caller holds it; a
/// refusal names nothing (GW6).
#[must_use]
pub fn check(
    grants: &Grants<'_>,
    action: &str,
    entity_type: Option<&str>,
    attribute: Option<&str>,
    served: &dyn Fn(&str) -> bool,
) -> Value {
    match verdict(grants, action, entity_type, attribute, served) {
        Verdict::Granted(p) => {
            let mut context = Map::new();
            context.insert("reason".into(), json!("policy_grant_matched"));
            if let Some(assigner) = &p.source.assigner {
                context.insert("assigner".into(), json!(assigner));
            }
            json!({"decision": true, "context": context})
        }
        Verdict::Prohibited(_) | Verdict::NotInGrant | Verdict::NoGrant => {
            json!({"decision": false})
        }
    }
}

/// The [`check`] decision with why, for the platform's access simulator
/// (README section 4, EP-103): the reason, and the policy that decided it
/// with its assigner, a refusal's prohibition included. Its reader is an
/// administrator, who may read every policy.
#[must_use]
pub fn explain(
    grants: &Grants<'_>,
    action: &str,
    entity_type: Option<&str>,
    attribute: Option<&str>,
    served: &dyn Fn(&str) -> bool,
) -> Value {
    let named = |p: &Policy, reason: &str| {
        let mut context = Map::new();
        context.insert("reason".into(), json!(reason));
        context.insert("policy".into(), json!(p.id));
        if let Some(assigner) = &p.source.assigner {
            context.insert("assigner".into(), json!(assigner));
        }
        Value::Object(context)
    };
    match verdict(grants, action, entity_type, attribute, served) {
        Verdict::Granted(p) => {
            json!({"decision": true, "context": named(p, "policy_grant_matched")})
        }
        Verdict::Prohibited(p) => json!({"decision": false, "context": named(p, "prohibited")}),
        Verdict::NotInGrant => json!({"decision": false, "context": {"reason": "not_in_grant"}}),
        Verdict::NoGrant => json!({"decision": false, "context": {"reason": "no_grant"}}),
    }
}

/// The actions the per-Attribute table answers, as `(member, action)`: a
/// read of the Entity, a partial update and a delete of the Attribute.
pub const ATTRIBUTE_ACTIONS: [(&str, &str); 3] = [
    ("read", "retrieveEntity"),
    ("write", "updateAttrs"),
    ("delete", "deleteAttrs"),
];

/// The caller's decisions on each Attribute of `names` of `entity_type`
/// (platform API/02 section 7b): `read`, `write` and `delete`, and one
/// `reason`, the strongest of the three verdicts: `prohibited`, then
/// `grant` (or `formula`, a grant with a condition the stored Entity must
/// meet), then `not_in_grant`, then `no_grant`. A reason names no policy
/// (GW6).
#[must_use]
pub fn attributes_of(
    grants: &Grants<'_>,
    entity_type: &str,
    names: &[&str],
    served: &dyn Fn(&str) -> bool,
) -> Value {
    let rows: Vec<Value> = names
        .iter()
        .map(|name| {
            let mut row = Map::new();
            row.insert("name".into(), json!(name));
            let mut reason = 0;
            for (member, action) in ATTRIBUTE_ACTIONS {
                let v = verdict(grants, action, Some(entity_type), Some(name), served);
                row.insert(member.into(), json!(matches!(v, Verdict::Granted(_))));
                reason = reason.max(match v {
                    Verdict::Prohibited(_) => 4,
                    Verdict::Granted(p) if p.is_conditional() => 3,
                    Verdict::Granted(_) => 2,
                    Verdict::NotInGrant => 1,
                    Verdict::NoGrant => 0,
                });
            }
            let reason = ["no_grant", "not_in_grant", "grant", "formula", "prohibited"]
                .get(reason)
                .copied()
                .unwrap_or("no_grant");
            row.insert("reason".into(), json!(reason));
            Value::Object(row)
        })
        .collect();
    json!({"type": entity_type, "attributes": rows})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member at `path` (a JSON pointer), `null` when absent.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }
    use crate::document::compile;
    use antares_api::policy::{Decision, Operation, Subject};

    fn table() -> Result<Table, String> {
        compile(&json!({"policies": [
            {"id": "read-air", "assigner": "did:web:city.example.org",
             "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2", "5.7.4"],
             "information": [{"entities": [{"type": "AirQualityObserved",
                                            "idPattern": "^urn:ngsi-ld:AirQualityObserved:.*$"}],
                              "propertyNames": ["pm25", "pm10"], "relationshipNames": ["location"]}],
             "q": "pm10>=0", "scopeQ": "/geo/SK/BB"},
            {"id": "steward-writes", "assignee": {"kind": "role", "id": "steward"},
             "operations": ["5.6.1", "5.6.4"],
             "information": [{"entities": [{"type": "InternalIncident"}],
                              "propertyNames": ["severity"]}]},
            {"id": "no-history", "effect": "prohibit",
             "assignee": {"kind": "role", "id": "public"}, "operations": ["5.7.4"],
             "information": [{"entities": [{"type": "AirQualityObserved"}]}]},
            {"id": "expired", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1"], "validity": {"to": "2020-01-01T00:00:00Z"},
             "information": [{"entities": [{"type": "Expired"}]}]}
        ]}))
    }

    fn caller(headers: &[(&str, &str)]) -> Caller {
        Caller::of(&Subject {
            tenant: antares_model::TenantId::default(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        })
    }

    fn every(_: &str) -> bool {
        true
    }

    #[test]
    fn the_document_lists_the_callers_grants_and_nothing_of_anyone_else() -> Result<(), String> {
        let t = table()?;
        let public = caller(&[("x-jc-roles", "")]);
        let doc = authzen(
            &public,
            "air",
            &Grants::of(Some(&t), &public, Utc::now()),
            &every,
        );
        assert_eq!(
            at(&doc, "/subject"),
            json!({"type": "role", "id": "public"})
        );
        assert_eq!(at(&doc, "/resource"), json!({"type": "view", "id": "air"}));
        let grants = at(&doc, "/permissions");
        assert_eq!(grants.as_array().map_or(0, Vec::len), 1, "{doc}");
        let g = &at(&grants, "/0");
        assert_eq!(at(g, "/resource/type"), "AirQualityObserved");
        assert_eq!(
            at(g, "/actions"),
            json!([
                "queryBatch",
                "queryEntity",
                "queryTemporal",
                "retrieveEntity"
            ])
        );
        assert_eq!(at(g, "/attributes"), json!(["location", "pm10", "pm25"]));
        assert_eq!(
            at(g, "/constraints"),
            json!({"q": "pm10>=0", "scopeQ": "/geo/SK/BB"})
        );
        assert!(at(g, "/resource/idPatterns/0").is_string());
        let p = at(&doc, "/prohibitions");
        assert_eq!(p.as_array().map_or(0, Vec::len), 1);
        assert_eq!(at(&p, "/0/actions"), json!(["queryTemporal"]));
        assert_eq!(at(&p, "/0/attributes"), json!("*"));
        let text = doc.to_string();
        for hidden in [
            "InternalIncident",
            "severity",
            "steward",
            "Expired",
            "read-air",
        ] {
            assert!(!text.contains(hidden), "{hidden} in {text}");
        }
        Ok(())
    }

    #[test]
    fn the_view_takes_off_the_actions_it_does_not_serve() -> Result<(), String> {
        let t = table()?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        let doc = authzen(&public, "air", &grants, &|r| r == "query_entities");
        assert_eq!(at(&doc, "/permissions/0/actions"), json!(["queryEntity"]));
        assert_eq!(
            at(&doc, "/prohibitions"),
            json!([]),
            "a view without history"
        );
        let none = authzen(&public, "air", &grants, &|_| false);
        assert_eq!(at(&none, "/permissions"), json!([]));
        Ok(())
    }

    #[test]
    fn each_caller_is_named_by_what_it_is_and_the_cluster_holds_nothing() -> Result<(), String> {
        let t = table()?;
        for (headers, want) in [
            (
                vec![("x-jc-subject", "ana")],
                json!({"type": "user", "id": "ana"}),
            ),
            (
                vec![("x-jc-client", "app")],
                json!({"type": "serviceAccount", "id": "app"}),
            ),
            (
                vec![("x-jc-did", "did:web:consumer.example")],
                json!({"type": "did", "id": "did:web:consumer.example"}),
            ),
            (vec![], json!({"type": "internal"})),
        ] {
            let c = caller(&headers);
            let doc = authzen(&c, "v", &Grants::of(Some(&t), &c, Utc::now()), &every);
            assert_eq!(at(&doc, "/subject"), want);
            assert_eq!(at(&doc, "/permissions"), json!([]), "{headers:?}");
        }
        let steward = caller(&[("x-jc-subject", "bo"), ("x-jc-roles", "steward")]);
        let doc = authzen(
            &steward,
            "v",
            &Grants::of(Some(&t), &steward, Utc::now()),
            &every,
        );
        assert_eq!(
            at(&doc, "/permissions/0/actions"),
            json!(["createEntity", "updateAttrs"])
        );
        let none = authzen(
            &steward,
            "v",
            &Grants::of(None, &steward, Utc::now()),
            &every,
        );
        assert_eq!(
            at(&none, "/permissions"),
            json!([]),
            "a Tenant without a document"
        );
        Ok(())
    }

    #[test]
    fn a_policy_without_information_shows_every_type() -> Result<(), String> {
        let t = compile(
            &json!({"policies": [{"id": "all", "assignee": {"kind": "role", "id": "public"},
                                              "operations": ["5.7.2"]}]}),
        )?;
        let public = caller(&[("x-jc-roles", "")]);
        let doc = authzen(
            &public,
            "v",
            &Grants::of(Some(&t), &public, Utc::now()),
            &every,
        );
        assert_eq!(at(&doc, "/permissions/0/resource"), json!({"type": "*"}));
        assert_eq!(at(&doc, "/permissions/0/constraints"), json!({}));
        Ok(())
    }

    #[test]
    fn the_check_answers_one_prospective_request_and_a_refusal_says_nothing() -> Result<(), String>
    {
        let t = table()?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        let yes = check(
            &grants,
            "queryEntity",
            Some("AirQualityObserved"),
            None,
            &every,
        );
        assert_eq!(
            yes,
            json!({"decision": true, "context": {"reason": "policy_grant_matched",
                                                 "assigner": "did:web:city.example.org"}})
        );
        assert_eq!(
            at(
                &check(&grants, "queryEntity", None, None, &every),
                "/decision"
            ),
            true
        );
        for (action, ty) in [
            ("deleteEntity", Some("AirQualityObserved")),
            ("queryEntity", Some("InternalIncident")),
            // prohibited although a permission lists it (GW8)
            ("queryTemporal", Some("AirQualityObserved")),
            ("queryTemporal", None),
            ("purgeEntity", None),
            ("nosuch", None),
            ("retrieveEntity", Some("Expired")),
        ] {
            assert_eq!(
                check(&grants, action, ty, None, &every),
                json!({"decision": false}),
                "{action}"
            );
        }
        let not_served = check(&grants, "queryEntity", None, None, &|r| {
            r == "retrieve_entity"
        });
        assert_eq!(not_served, json!({"decision": false}));
        let internal = caller(&[]);
        let none = Grants::of(Some(&t), &internal, Utc::now());
        assert_eq!(
            at(
                &check(&none, "queryEntity", None, None, &every),
                "/decision"
            ),
            false
        );
        Ok(())
    }

    #[test]
    fn explain_names_the_deciding_policy_and_agrees_with_the_check() -> Result<(), String> {
        let t = table()?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        assert_eq!(
            explain(
                &grants,
                "queryEntity",
                Some("AirQualityObserved"),
                None,
                &every
            ),
            json!({"decision": true, "context": {"reason": "policy_grant_matched",
                   "policy": "read-air", "assigner": "did:web:city.example.org"}})
        );
        // the prohibition that ended it is named, a refusal's reason included (EP-103)
        assert_eq!(
            explain(
                &grants,
                "queryTemporal",
                Some("AirQualityObserved"),
                None,
                &every
            ),
            json!({"decision": false, "context": {"reason": "prohibited", "policy": "no-history"}})
        );
        for (action, ty) in [
            ("queryEntity", Some("InternalIncident")),
            ("deleteEntity", None),
            ("retrieveEntity", Some("Expired")),
            ("nosuch", None),
        ] {
            assert_eq!(
                explain(&grants, action, ty, None, &every),
                json!({"decision": false, "context": {"reason": "no_grant"}}),
                "{action}"
            );
        }
        let not_served = explain(&grants, "queryEntity", None, None, &|r| {
            r == "retrieve_entity"
        });
        assert_eq!(at(&not_served, "/context/reason"), "no_grant");

        // one verdict: the two answers never disagree on the decision
        let steward = caller(&[("x-jc-subject", "eva"), ("x-jc-roles", "steward")]);
        for who in [&public, &steward, &caller(&[])] {
            let held = Grants::of(Some(&t), who, Utc::now());
            for (action, _) in OPERATIONS {
                for ty in [None, Some("AirQualityObserved"), Some("InternalIncident")] {
                    assert_eq!(
                        at(&explain(&held, action, ty, None, &every), "/decision"),
                        at(&check(&held, action, ty, None, &every), "/decision"),
                        "{action} {ty:?}"
                    );
                }
            }
        }
        Ok(())
    }

    /// One policy document of `policies` as the engine compiles it.
    fn compiled(policies: &Value) -> Result<Table, String> {
        compile(&json!({ "policies": policies }))
    }

    /// The decision `check` gives on `attribute` of `Car` for `action`.
    fn may(grants: &Grants<'_>, action: &str, attribute: &str) -> Value {
        at(
            &check(grants, action, Some("Car"), Some(attribute), &every),
            "/decision",
        )
    }

    #[test]
    fn an_attribute_is_granted_only_when_a_covering_selector_lists_it_or_none() -> Result<(), String>
    {
        let t = compiled(&json!([
            {"id": "speed", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.6.4"],
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]},
            {"id": "whole-bus", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1"], "information": [{"entities": [{"type": "Bus"}]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        assert_eq!(may(&grants, "retrieveEntity", "speed"), true);
        assert_eq!(may(&grants, "updateAttrs", "speed"), true);
        // an IRI names the same Attribute
        let speed = context().expand_key("speed");
        assert_eq!(may(&grants, "retrieveEntity", &speed), true);
        assert_eq!(may(&grants, "retrieveEntity", "colour"), false);
        assert_eq!(may(&grants, "deleteAttrs", "speed"), false);
        assert_eq!(
            explain(
                &grants,
                "retrieveEntity",
                Some("Car"),
                Some("colour"),
                &every
            ),
            json!({"decision": false, "context": {"reason": "not_in_grant"}})
        );
        // a selector listing no Attribute is the whole Entity
        assert_eq!(
            at(
                &check(
                    &grants,
                    "retrieveEntity",
                    Some("Bus"),
                    Some("colour"),
                    &every
                ),
                "/decision"
            ),
            true
        );
        // without an attribute the check answers as before
        assert_eq!(
            at(
                &check(&grants, "retrieveEntity", Some("Car"), None, &every),
                "/decision"
            ),
            true
        );
        Ok(())
    }

    #[test]
    fn a_prohibition_naming_attributes_refuses_only_those() -> Result<(), String> {
        let t = compiled(&json!([
            {"id": "read-car", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2"], "information": [{"entities": [{"type": "Car"}]}]},
            {"id": "no-speed", "effect": "prohibit", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2"],
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        // T-5842: the prohibition names `speed` and takes back only `speed`, as the real read
        assert_eq!(may(&grants, "retrieveEntity", "speed"), false);
        assert_eq!(may(&grants, "retrieveEntity", "colour"), true);
        assert_eq!(
            at(
                &explain(
                    &grants,
                    "retrieveEntity",
                    Some("Car"),
                    Some("speed"),
                    &every
                ),
                "/context/reason"
            ),
            "prohibited"
        );
        assert_eq!(
            at(
                &check(&grants, "retrieveEntity", Some("Car"), None, &every),
                "/decision"
            ),
            true
        );
        let ctx = context();
        let types = vec![ctx.expand_key("Car")];
        let op = Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        let Decision::Filter(f) = crate::decide::decide(Some(&t), &public, &op, Utc::now()) else {
            return Err("a read narrowed by an Attribute".into());
        };
        assert_eq!(
            f.omit_by_type.get(&ctx.expand_key("Car")),
            Some(&vec![ctx.expand_key("speed")])
        );
        Ok(())
    }

    /// F-R2ANT1-015: a read naming no type is narrowed by a prohibition naming Attributes, in
    /// the check as in decide.
    #[test]
    fn an_untyped_read_check_agrees_with_decide() -> Result<(), String> {
        let t = compiled(&json!([
            {"id": "read-car", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2"], "information": [{"entities": [{"type": "Car"}]}]},
            {"id": "no-speed", "effect": "prohibit", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2"],
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        let decided =
            crate::decide::decide(Some(&t), &public, &Operation::new("5.7.2"), Utc::now());
        assert!(!matches!(decided, Decision::Deny(_)), "{decided:?}");
        assert_eq!(
            at(
                &check(&grants, "queryEntity", None, None, &every),
                "/decision"
            ),
            true
        );
        Ok(())
    }

    #[test]
    fn the_attribute_table_answers_each_action_and_a_reason_naming_no_policy() -> Result<(), String>
    {
        let t = compiled(&json!([
            {"id": "editor", "assigner": "did:web:city.example.org",
             "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.6.4"],
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]},
            {"id": "if-red", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1"], "q": "colour==\"red\"",
             "information": [{"entities": [{"type": "Car"}], "propertyNames": ["colour"]}]},
            {"id": "never-plate", "effect": "prohibit", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.6.5"], "information": [{"entities": [{"type": "Car"}]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        let table = attributes_of(
            &grants,
            "Car",
            &["speed", "colour", "owner", "plate"],
            &every,
        );
        assert_eq!(
            table,
            json!({"type": "Car", "attributes": [
                {"name": "speed", "read": true, "write": true, "delete": false, "reason": "prohibited"},
                {"name": "colour", "read": true, "write": false, "delete": false, "reason": "prohibited"},
                {"name": "owner", "read": false, "write": false, "delete": false, "reason": "prohibited"},
                {"name": "plate", "read": false, "write": false, "delete": false, "reason": "prohibited"}
            ]})
        );
        let without = Grants {
            permissions: grants.permissions.clone(),
            prohibitions: Vec::new(),
        };
        let table = attributes_of(&without, "Car", &["speed", "colour", "plate"], &every);
        let reasons: Vec<Value> = ["speed", "colour", "plate"]
            .iter()
            .enumerate()
            .map(|(i, _)| at(&table, &format!("/attributes/{i}/reason")))
            .collect();
        assert_eq!(
            reasons,
            vec![json!("grant"), json!("formula"), json!("not_in_grant")]
        );
        let other = attributes_of(&without, "Bus", &["speed"], &every);
        assert_eq!(at(&other, "/attributes/0/reason"), "no_grant");
        // deny by default: nothing held is false everywhere
        let none = Grants::of(None, &public, Utc::now());
        assert_eq!(
            attributes_of(&none, "Car", &["speed"], &every),
            json!({"type": "Car", "attributes": [
                {"name": "speed", "read": false, "write": false, "delete": false, "reason": "no_grant"}]})
        );
        let text = attributes_of(&grants, "Car", &["speed"], &every).to_string();
        for hidden in ["editor", "never-plate", "if-red", "did:web"] {
            assert!(!text.contains(hidden), "{hidden} in {text}");
        }
        // a write of access-control vocabulary is refused by the engine (GW29)
        assert_eq!(
            attributes_of(&without, "Car", &["owner"], &|_| true),
            json!({"type": "Car", "attributes": [
                {"name": "owner", "read": false, "write": false, "delete": false, "reason": "not_in_grant"}]})
        );
        Ok(())
    }

    #[test]
    fn a_write_of_access_control_vocabulary_is_never_granted() -> Result<(), String> {
        let t = compiled(&json!([
            {"id": "edit-car", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.6.4"], "information": [{"entities": [{"type": "Car"}]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        assert_eq!(may(&grants, "updateAttrs", "speed"), true);
        // GW29: the whole-type grant still admits no write of `owner`
        assert_eq!(may(&grants, "updateAttrs", "owner"), false);
        let owner = context().expand_key("owner");
        assert_eq!(may(&grants, "updateAttrs", &owner), false);
        assert_eq!(may(&grants, "retrieveEntity", "owner"), true);
        Ok(())
    }

    #[test]
    fn a_grant_with_a_time_window_admits_no_write() -> Result<(), String> {
        let t = compiled(&json!([
            {"id": "last-day", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.6.4"], "temporalQ": "timerel=after;timeAt=P-1D",
             "information": [{"entities": [{"type": "Car"}]}]}
        ]))?;
        let public = caller(&[("x-jc-roles", "")]);
        let grants = Grants::of(Some(&t), &public, Utc::now());
        // as `decide::admits`: the window is about reading history
        assert_eq!(
            at(
                &check(&grants, "updateAttrs", Some("Car"), None, &every),
                "/decision"
            ),
            false
        );
        assert_eq!(may(&grants, "updateAttrs", "speed"), false);
        assert_eq!(may(&grants, "retrieveEntity", "speed"), true);
        Ok(())
    }

    /// What the engine decides on a real request: `action` on `attribute` of
    /// one `Car`, shown (a read) or allowed (a write).
    fn real(t: &Table, who: &Caller, action: &str, attribute: &str) -> bool {
        let Some(clause) = names::clause(action) else {
            return false;
        };
        let ctx = context();
        let (ty, attr) = (ctx.expand_key("Car"), ctx.expand_key(attribute));
        let types = vec![ty.clone()];
        let attrs = vec![attr.clone()];
        let ids = ["urn:ngsi-ld:Car:1"];
        let op = Operation {
            types: &types,
            attrs: &attrs,
            ids: &ids,
            ..Operation::new(clause)
        };
        match crate::decide::decide(Some(t), who, &op, Utc::now()) {
            Decision::Allow => true,
            Decision::Deny(_) | Decision::AllowIf(_) => false,
            Decision::Filter(f) => {
                let omitted = f.omit_by_type.get(&ty).is_some_and(|o| o.contains(&attr));
                let shown = f.rules.is_empty()
                    || f.rules.iter().any(|r| {
                        r.types.contains(&ty) && (r.every_attribute || r.attributes.contains(&attr))
                    });
                shown && !omitted
            }
        }
    }

    #[test]
    fn the_check_on_an_attribute_equals_the_real_decision_for_every_generated_policy(
    ) -> Result<(), String> {
        let clauses = [
            json!(["5.7.1"]),
            json!(["5.7.1", "5.6.4"]),
            json!(["5.7.1", "5.6.4", "5.6.5"]),
            json!(["5.6.5"]),
        ];
        let information = [
            None,
            Some(json!([{"entities": [{"type": "Car"}]}])),
            Some(json!([{"entities": [{"type": "Car"}], "propertyNames": ["a"]}])),
            Some(json!([{"entities": [{"type": "Car"}], "propertyNames": ["a", "b"]}])),
            Some(json!([{"entities": [{"type": "Bus"}], "propertyNames": ["a"]}])),
        ];
        let mut cases = 0_u32;
        for grant_ops in &clauses {
            for grant_info in &information {
                for prohibit in std::iter::once(None).chain(information.iter().map(Some)) {
                    for prohibit_ops in &clauses {
                        let mut permit = json!({"id": "p", "assignee": {"kind": "role", "id": "public"},
                                                "operations": grant_ops});
                        if let (Some(o), Some(info)) = (permit.as_object_mut(), grant_info) {
                            o.insert("information".into(), info.clone());
                        }
                        let mut policies = vec![permit];
                        if let Some(info) = prohibit {
                            let mut no = json!({"id": "n", "effect": "prohibit",
                                                "assignee": {"kind": "role", "id": "public"},
                                                "operations": prohibit_ops});
                            if let (Some(o), Some(info)) = (no.as_object_mut(), info) {
                                o.insert("information".into(), info.clone());
                            }
                            policies.push(no);
                        }
                        let t = compiled(&Value::Array(policies))?;
                        let public = caller(&[("x-jc-roles", "")]);
                        let grants = Grants::of(Some(&t), &public, Utc::now());
                        for (_, action) in ATTRIBUTE_ACTIONS {
                            for attribute in ["a", "b", "c"] {
                                assert_eq!(
                                    may(&grants, action, attribute),
                                    real(&t, &public, action, attribute),
                                    "{action} {attribute} {grant_ops} {grant_info:?} {prohibit:?} {prohibit_ops}"
                                );
                                cases = cases.saturating_add(1);
                            }
                        }
                        if prohibit.is_none() {
                            break;
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 4 * 5 * (1 + 5 * 4) * 9);
        Ok(())
    }
}
