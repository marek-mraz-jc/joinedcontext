//! The in-process policy decision point (ADR-N-003, gateway-firewall).

pub mod conditional;
pub mod evaluator;
pub mod geo;
pub mod grants;
pub mod id_match;
pub mod metadata;
pub mod projection;
pub mod reaper;
pub mod scope_folding;
pub mod temporal;
pub mod write_guard;

use crate::auth::subject::Subject;
use crate::resolver::Endpoint;
use evaluator::{Request, Verdict};
use jc_core::kinds::Operation;
use jc_core::system_members::is_entity_system_member;
use jc_time::TimestampTz;
use std::collections::{BTreeMap, BTreeSet};

/// What decides. Every request passes it before anything is forwarded (GW1, ADR-N-003).
///
/// A trait rather than a function so a test can assert what the enforcement point does
/// with a verdict without having to write the policy that produces it.
pub trait Pdp: Send + Sync + 'static {
    /// The verdict for one caller, one operation and one request on one endpoint.
    fn decide(
        &self,
        subject: &Subject,
        operation: Operation,
        request: &Request,
        endpoint: &Endpoint,
    ) -> Verdict;
}

/// The decision point the gateway runs: the endpoint's own policies, evaluated now.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyPdp;

impl Pdp for PolicyPdp {
    fn decide(
        &self,
        subject: &Subject,
        operation: Operation,
        request: &Request,
        endpoint: &Endpoint,
    ) -> Verdict {
        let started = std::time::Instant::now();
        let verdict = evaluator::evaluate(
            subject,
            operation,
            request,
            &endpoint.space,
            &endpoint.policies,
            now(),
        );
        // OPS-16: the one number a dashboard cannot get from the edge. APISIX counts requests
        // and statuses; only the decision point knows a request was answered because a policy
        // let it through rather than because nothing looked.
        crate::telemetry::decided(operation, verdict.is_deny());

        // EP-61: the endpoint's own publication narrowing, folded into the one decision
        // every representation reads, so no encoder can forget it and no new
        // representation has to remember it.
        let verdict = match verdict {
            Verdict::Rewrite(mut constraints) if !endpoint.hidden_attributes.is_empty() => {
                // Beside what a prohibition withholds, never in its place (GW4).
                constraints
                    .hidden
                    .extend(endpoint.hidden_attributes.iter().cloned());
                constraints.restricted = true;
                Verdict::Rewrite(constraints)
            }
            verdict => verdict,
        };

        // MP-02: the endpoint's projection, intersected into the same decision. Types and
        // attributes narrow like a grant's; the residual filter is conjoined like a REWRITE
        // constraint; nothing here can add what a policy did not give.
        let verdict = match (verdict, &endpoint.projection) {
            (Verdict::Rewrite(constraints), Some(projection)) => {
                project_constraints(*constraints, projection)
            }
            (verdict, _) => verdict,
        };

        // The filter is the last thing narrowed, because it is narrowed by what the two steps
        // above decided: a type whose attributes do not cover every name the request filters or
        // orders on leaves the query before the broker is asked (T-1862).
        let verdict = match verdict {
            Verdict::Rewrite(constraints) => {
                drop_types_that_may_not_be_filtered(*constraints, request)
            }
            verdict => verdict,
        };

        // T-3473: an answer's type is judged as an IRI, so the granted names are read the way
        // the space's own models define them, once, beside the core context's reading.
        let verdict = match verdict {
            Verdict::Rewrite(mut constraints) => {
                let contexts = || {
                    endpoint
                        .models
                        .iter()
                        .filter_map(|model| model.context.as_deref())
                };
                constraints.type_iris = projection::model_type_iris(&constraints.types, contexts());
                // Each grant by its own names, so one grant's IRI never admits an entity to another
                // (T-3530).
                for grant in &mut constraints.grants {
                    grant.type_iris = projection::model_type_iris(&grant.types, contexts());
                }
                Verdict::Rewrite(constraints)
            }
            verdict => verdict,
        };
        crate::telemetry::decision_time(started.elapsed().as_secs_f64());
        verdict
    }
}

/// The constraints of one decision, narrowed to a projection (MP-02).
///
/// A request type outside the projection is a refusal rather than an empty answer: an
/// empty `types` set means "no type filter" downstream, and the broker would be asked for
/// every type instead of none (the same rule the evaluator applies to grants, GW10).
fn project_constraints(
    mut constraints: evaluator::Constraints,
    projection: &jc_core::kinds::ModelProjectionSpec,
) -> Verdict {
    let classes: BTreeSet<String> = projection
        .classes
        .iter()
        .map(|class| class.name.clone())
        .collect();
    let types = evaluator::narrow(&constraints.types, &classes);
    if types.is_empty() {
        return Verdict::Deny;
    }
    // Per type, because a projection is a statement about a class: `User: [age]` and
    // `Vehicle: [weight]` joined into one set served a Vehicle's `age` to anyone who asked for
    // both types (T-1862, MP-02). The joined set stays as `attrs`, which is the one list CIM 009
    // lets the broker be told; the answer is stripped by the map.
    let by_type: BTreeMap<String, BTreeSet<String>> = types
        .iter()
        .map(|class| {
            let slots: BTreeSet<String> = projection.attributes_of(class).unwrap_or_default();
            (
                class.clone(),
                evaluator::narrow_to_identity(&constraints.attrs, &slots),
            )
        })
        .collect();
    let slots: BTreeSet<String> = types
        .iter()
        .filter_map(|class| projection.attributes_of(class))
        .flatten()
        .collect();
    constraints.attrs = evaluator::narrow_to_identity(&constraints.attrs, &slots);
    constraints.attrs_by_type = by_type;
    constraints.types = types;
    if let Some(filter) = &projection.filter {
        if let Some(q) = &filter.q {
            constraints.q = evaluator::conjoin(constraints.q.as_deref(), std::slice::from_ref(q));
        }
        // A filter the constraint set has no slot to conjoin with applies when the grants
        // set none; a grant's own geo or temporal clamp is narrower by construction (GW11).
        if constraints.geo_q.is_none() {
            constraints.geo_q = filter.geo_q.clone();
        }
        if constraints.temporal_q.is_none() {
            constraints.temporal_q = filter.temporal_q.clone();
        }
    }
    constraints.restricted = true;
    Verdict::Rewrite(Box::new(constraints))
}

/// The referenced names a grant could withhold: every one but the system members each entity and
/// attribute carries (DM-130), so filtering on one references nothing a grant could withhold
/// (CIM 009 4.5.1).
pub fn selecting(referenced: &BTreeSet<String>) -> BTreeSet<String> {
    referenced
        .iter()
        .filter(|name| !is_entity_system_member(name) && name.as_str() != "observedAt")
        .cloned()
        .collect()
}

/// Takes out of the query every type that may not be filtered on what the request filters on
/// (T-1862; owner's rule of 2026-09-18, MP-02, R9).
///
/// A filter is a read. `q=age>30` over a type whose `age` this endpoint does not serve used to
/// reach the broker as written and the answer was stripped afterwards, so the rows that came back
/// still said which entities have an `age` over thirty — and bisecting `N` reads the value exactly.
/// The rule is therefore about which entities are *considered*, not about what the answer carries:
/// a type stays only if every referenced attribute is one it may serve.
///
/// Strict on purpose: a type is dropped even when the name it may not serve sits in one branch of
/// an `|`, because an `|` still lets the branch decide whether a row comes back. ponytail: strict
/// drop; splitting the query per type is the upgrade if people miss those rows.
///
/// No type left means the answer is genuinely nothing: `constraints.empty` is what the surfaces
/// already answer with `200 []` for a query and `404` for an addressed read, which is exactly what
/// an attribute that does not exist would give.
pub fn drop_types_that_may_not_be_filtered(
    mut constraints: evaluator::Constraints,
    request: &Request,
) -> Verdict {
    // Kept for the answer, where each entity is held to what its own grants show (T-3530).
    constraints.referenced = selecting(&request.referenced);
    let referenced: Vec<&String> = constraints.referenced.iter().collect();
    if referenced.is_empty() {
        return Verdict::Rewrite(Box::new(constraints));
    }

    // Hidden is a denial over every type (EP-61): filtering on a hidden name can never be served,
    // whether or not a projection says which type owns it.
    if referenced
        .iter()
        .any(|name| constraints.hidden.contains(name.as_str()))
    {
        constraints.empty = true;
        constraints.restricted = true;
        return Verdict::Rewrite(Box::new(constraints));
    }

    if constraints.attrs_by_type.is_empty() {
        // No projection: the grants' own whitelist is the whole of the narrowing, and it is that
        // whitelist a filter is judged against — never `attrs`, which is what the caller asked
        // for. An empty whitelist is a grant over the whole entity, so nothing is referenced that
        // is not served.
        if !constraints.served.is_empty()
            && referenced
                .iter()
                .any(|name| !constraints.served.contains(name.as_str()))
        {
            constraints.empty = true;
            constraints.restricted = true;
        }
        return Verdict::Rewrite(Box::new(constraints));
    }

    let kept: BTreeSet<String> = constraints
        .attrs_by_type
        .iter()
        .filter(|(_, slots)| referenced.iter().all(|name| slots.contains(name.as_str())))
        .map(|(class, _)| class.clone())
        .collect();
    if kept.len() < constraints.attrs_by_type.len() {
        constraints.restricted = true;
    }
    if kept.is_empty() {
        constraints.empty = true;
        return Verdict::Rewrite(Box::new(constraints));
    }
    constraints
        .attrs_by_type
        .retain(|class, _| kept.contains(class));
    constraints.types = kept;
    Verdict::Rewrite(Box::new(constraints))
}

/// The wall clock, read here in one place for policy validity windows and key expiry.
pub fn now() -> TimestampTz {
    TimestampTz::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn projection(yaml: &str) -> jc_core::kinds::ModelProjectionSpec {
        serde_norway::from_str(&format!(
            "contextSpaceRef: sensors\ndataModelRef: {{ name: sensors, version: 1.0.0 }}\n{yaml}"
        ))
        .expect("the projection parses")
    }

    fn by_type() -> evaluator::Constraints {
        evaluator::Constraints {
            types: set(&["Sensor", "Vehicle"]),
            attrs_by_type: [
                ("Sensor".to_owned(), set(&["id", "level", "name"])),
                ("Vehicle".to_owned(), set(&["id", "name", "weight"])),
            ]
            .into_iter()
            .collect(),
            ..evaluator::Constraints::default()
        }
    }

    fn filtering(names: &[&str]) -> Request {
        Request {
            referenced: set(names),
            ..Request::default()
        }
    }

    /// T-7767 (T-1862, MP-02): a filter keeps exactly the types that serve every name it
    /// reads; none left is an empty answer, and a name every type serves narrows nothing.
    #[test]
    fn a_filter_keeps_only_the_types_that_serve_every_name_it_reads() {
        let kept = drop_types_that_may_not_be_filtered(by_type(), &filtering(&["level"]));
        let kept = kept.constraints().expect("a rewrite");
        assert_eq!(kept.types, set(&["Sensor"]));
        assert_eq!(kept.attrs_by_type.keys().collect::<Vec<_>>(), ["Sensor"]);
        assert!(kept.restricted && !kept.empty);

        let none = drop_types_that_may_not_be_filtered(by_type(), &filtering(&["colour"]));
        let none = none.constraints().expect("a rewrite");
        assert!(none.empty && none.restricted);

        let both = drop_types_that_may_not_be_filtered(by_type(), &filtering(&["name"]));
        let both = both.constraints().expect("a rewrite");
        assert_eq!(both.types, set(&["Sensor", "Vehicle"]));
        assert!(!both.restricted && !both.empty);

        // System members and observedAt are no attribute a grant names.
        let system = drop_types_that_may_not_be_filtered(by_type(), &filtering(&["observedAt"]));
        assert!(!system.constraints().expect("a rewrite").restricted);
    }

    /// T-7767 (EP-61): filtering on a hidden name answers nothing for every type, and without
    /// a projection the grants' whitelist is what a filter is judged against.
    #[test]
    fn a_hidden_or_unserved_name_filters_to_nothing() {
        let hidden = evaluator::Constraints {
            hidden: set(&["owner"]),
            ..by_type()
        };
        let hidden = drop_types_that_may_not_be_filtered(hidden, &filtering(&["owner"]));
        assert!(hidden.constraints().expect("a rewrite").empty);

        let whitelisted = || evaluator::Constraints {
            served: set(&["level"]),
            ..evaluator::Constraints::default()
        };
        let outside = drop_types_that_may_not_be_filtered(whitelisted(), &filtering(&["colour"]));
        assert!(outside.constraints().expect("a rewrite").empty);
        let inside = drop_types_that_may_not_be_filtered(whitelisted(), &filtering(&["level"]));
        assert!(!inside.constraints().expect("a rewrite").empty);
        let whole = drop_types_that_may_not_be_filtered(
            evaluator::Constraints::default(),
            &filtering(&["colour"]),
        );
        assert!(
            !whole.constraints().expect("a rewrite").empty,
            "no whitelist is the whole entity"
        );
    }

    /// T-7767 (MP-02, GW10): a type the projection does not expose is a refusal, never every
    /// type; the projection's filter is conjoined with the grants' and its geo and temporal
    /// windows apply only where the grants set none.
    #[test]
    fn a_projection_refuses_an_unexposed_type_and_conjoins_its_filter() {
        let sensors = projection("classes: [{ name: Sensor, slots: [level] }]\n");
        let vehicles = evaluator::Constraints {
            types: set(&["Vehicle"]),
            ..evaluator::Constraints::default()
        };
        assert_eq!(project_constraints(vehicles, &sensors), Verdict::Deny);

        let filtered = projection(
            "classes: [{ name: Sensor, slots: [level] }]\nfilter:\n  q: level>2\n  \
             geoQ: georel=near;maxDistance==10&geometry=Point&coordinates=[1,1]\n  \
             temporalQ: timerel=after;timeAt=P1D\n",
        );
        let asked = evaluator::Constraints {
            types: set(&["Sensor"]),
            q: Some("name==\"a\"".to_owned()),
            ..evaluator::Constraints::default()
        };
        let verdict = project_constraints(asked.clone(), &filtered);
        let projected = verdict.constraints().expect("a rewrite");
        let q = projected.q.as_deref().unwrap_or_default();
        assert!(q.contains("level>2") && q.contains("name==\"a\""), "{q}");
        assert!(projected
            .geo_q
            .as_deref()
            .is_some_and(|g| g.contains("near")));
        assert_eq!(
            projected.temporal_q.as_deref(),
            Some("timerel=after;timeAt=P1D")
        );
        assert_eq!(
            projected.attrs_by_type["Sensor"],
            set(&["id", "level", "type"])
        );

        let clamped = evaluator::Constraints {
            geo_q: Some("granted".to_owned()),
            temporal_q: Some("granted".to_owned()),
            ..asked
        };
        let verdict = project_constraints(clamped, &filtered);
        let projected = verdict.constraints().expect("a rewrite");
        assert_eq!(projected.geo_q.as_deref(), Some("granted"));
        assert_eq!(projected.temporal_q.as_deref(), Some("granted"));
    }
}
