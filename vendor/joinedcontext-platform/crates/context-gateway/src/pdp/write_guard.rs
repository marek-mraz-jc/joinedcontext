//! What a write payload has to satisfy before it reaches a broker (T-0153, GW16..GW19).
//!
//! Reads are narrowed; writes are not. A write either falls entirely inside one grant or
//! it is refused whole, because a partially applied write stores something nobody asked
//! for. Three things are checked against the payload itself, not against the query
//! parameters a client could simply omit: the entity's identifier, its `scope`, and the
//! coordinates of its own `location`.

use crate::pdp::evaluator::Constraints;
use crate::pdp::geo::{entity_point, granted_polygon};
use crate::pdp::projection::{model_type_iris, type_iri, ungranted};
use jc_core::{ProblemDetails, Urn};
use serde_json::Value;
use std::collections::BTreeSet;

/// Why a write was refused (GW16, GW17).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The payload carries no usable entity id, or one that is not an NGSI-LD URN of its type
    /// (PF-43).
    #[error("entity id is not a valid NGSI-LD URN: {0}")]
    MalformedId(String),
    /// The entity's type is outside every grant (GW11).
    #[error("entity type `{0}` is outside the grant")]
    TypeOutsideGrant(String),
    /// The entity's id matches no `id` or `idPattern` of the grant (R24, GW16).
    #[error("entity id `{0}` is outside the grant")]
    IdOutsideGrant(String),
    /// The payload touches an attribute no grant covers (GW17).
    #[error("attribute `{0}` is outside the grant")]
    AttributeOutsideGrant(String),
    /// The entity's `scope` is outside the granted scope tree (R29, R31).
    #[error("scope `{0}` is outside the granted scopes")]
    ScopeOutsideGrant(String),
    /// A whole entity names no `scope` while the grant covers scoped entities only (F-PLT2-009).
    #[error("the entity names no scope, and the grant covers only entities inside its scopes")]
    ScopeMissing,
    /// The entity's own location lies outside the granted area (GW16).
    #[error("the entity's location lies outside the granted area")]
    LocationOutsideGrant,
    /// The payload tries to carry access control inside the data (GW29).
    #[error("attribute `{0}` is access control and does not belong in an entity")]
    SmuggledPolicyAttribute(String),
}

impl From<Refusal> for ProblemDetails {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            // A malformed identifier is a bad request: the caller can see and fix what is
            // wrong with it (PF-43).
            Refusal::MalformedId(_) => ProblemDetails::urn_scheme()
                .with_field("id")
                .with_detail(refusal.to_string()),
            // Everything else is a policy decision, and the body never says which rule
            // decided it (GW6).
            _ => ProblemDetails::forbidden(),
        }
    }
}

/// Attribute names an entity may never carry: policy belongs in `Policy` entities, never
/// inside the data it guards (GW28, GW29).
const POLICY_VOCABULARY: &[&str] = &[
    "owner",
    "acl",
    "allowedRoles",
    "visibility",
    "permissions",
    "policy",
];

/// Checks one write payload against the constraint set (GW16, GW17, GW19), reading its type
/// under the NGSI-LD core context alone: [`check_in`] with no model.
pub fn check(entity: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    check_in(entity, constraints, &[])
}

/// Checks one write payload against the constraint set (GW16, GW17, GW19). `contexts` are the
/// Endpoint's models' own `@context`s, which with the core context read an expanded type.
///
/// The payload must be a single NGSI-LD entity; a batch is checked per entity by the
/// caller, so a mixed outcome can answer 207 (GW18).
pub fn check_in(
    entity: &Value,
    constraints: &Constraints,
    contexts: &[&Value],
) -> Result<(), Refusal> {
    check_no_smuggled_policy(entity)?;
    check_id(entity, contexts)?;
    check_type(entity, constraints)?;
    if let Some(raw) = entity_id(entity) {
        check_granted_id(raw, constraints)?;
    }
    // A whole entity without a scope would sit outside every scope the grant names, so a grant
    // on `/geo/SK/BB` could create entities no scoped read of it reaches (F-PLT2-009). A
    // fragment keeps the scope the entity already has, so only the whole entity must carry one.
    if constraints.granted_scopes.is_some() && entity.get("scope").is_none() {
        return Err(Refusal::ScopeMissing);
    }
    check_fragment(entity, constraints)
}

/// Whether a payload key names the attribute `term`: the term itself, or an expanded or
/// compact IRI ending in it (`https://example.org/secret`, `ex:secret`), the rule of
/// `quality::is_owned` (F-PLT2-011).
pub fn names_attribute(key: &str, term: &str) -> bool {
    key == term
        || key
            .strip_suffix(term)
            .is_some_and(|head| head.ends_with(['/', '#', ':']))
}

/// A write's identifier against what the grant selects (T-0806, GW11, R24): the type the
/// URN carries must be a granted type, and the URN must match an `id` or `idPattern` when
/// the grant names any. Checked without a body, so the path id of an addressed write and a
/// bare URN in a batch delete get the same answer as an entity payload.
pub fn check_granted_id(raw: &str, constraints: &Constraints) -> Result<(), Refusal> {
    let urn: Urn = raw
        .parse()
        .map_err(|_| Refusal::MalformedId(raw.to_owned()))?;
    if !constraints.types.is_empty() && !constraints.types.contains(urn.entity_type()) {
        return Err(Refusal::TypeOutsideGrant(urn.entity_type().to_owned()));
    }
    if !constraints.id_patterns.permits(raw) {
        return Err(Refusal::IdOutsideGrant(raw.to_owned()));
    }
    Ok(())
}

fn entity_id(entity: &Value) -> Option<&str> {
    entity
        .get("id")
        .or_else(|| entity.get("@id"))
        .and_then(Value::as_str)
}

/// Checks a payload that carries no identifier of its own: the attribute fragment of a
/// `PATCH .../attrs` (GW17, R29, GW16).
///
/// The identifier of such a write is in the path, and the enforcement point checks it with
/// [`check_identifier`] before calling this.
pub fn check_fragment(fragment: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    check_no_smuggled_policy(fragment)?;
    check_attributes(fragment, constraints)?;
    check_scope(fragment, constraints)?;
    check_location(fragment, constraints)
}

/// Checks one entity identifier (PF-43) under the NGSI-LD core context alone:
/// [`check_identifier_in`] with no model.
pub fn check_identifier(raw: &str, declared_type: Option<&str>) -> Result<(), Refusal> {
    check_identifier_in(raw, declared_type, &[])
}

/// Checks one entity identifier (PF-43): an NGSI-LD URN, and of the type the payload states,
/// by its short name or the IRI the core context or one of `contexts` expands the URN's type to.
///
/// Nothing else of the URN is read (ADR-N-041, PF-42): the space the write lands in is the one
/// the path or the Endpoint resolved, and the Policy of that space decides it, so a URN naming
/// another organization or space reaches nothing it names. `declared_type` is the `type` the
/// payload states, when it states one.
pub fn check_identifier_in(
    raw: &str,
    declared_type: Option<&str>,
    contexts: &[&Value],
) -> Result<(), Refusal> {
    let urn: Urn = raw
        .parse()
        .map_err(|_| Refusal::MalformedId(raw.to_owned()))?;
    if declared_type.is_some_and(|declared| !names_type(declared, urn.entity_type(), contexts)) {
        return Err(Refusal::MalformedId(raw.to_owned()));
    }
    Ok(())
}

/// Whether a payload's `declared` type names the type `name`: the name itself, or the IRI the
/// NGSI-LD core context or an Endpoint model's `@context` expands it to (F3). The request's own
/// `@context` never counts: the gateway refuses it before the broker reads it (T-3287), and a
/// remote context is never fetched.
fn names_type(declared: &str, name: &str, contexts: &[&Value]) -> bool {
    declared == name
        || type_iri(name).as_deref() == Some(declared)
        || model_type_iris(&BTreeSet::from([name.to_owned()]), contexts.iter().copied())
            .contains(declared)
}

/// GW16: "may edit only data in location X" is a statement about the entity's own
/// coordinates, so the grant's area is tested against the payload, not against a query
/// parameter the caller could leave out.
///
/// Every granted area, and the entity has to be inside one of them (T-0807). Not `geo_q`,
/// which is what the broker was given: with two grants that is the caller's own area, so a
/// caller could draw its own polygon around the point it wanted to write, and with one
/// grant and a narrower caller area it is the caller's too.
fn check_location(entity: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    if constraints.geo_areas.is_empty() {
        return Ok(());
    }
    // No location at all is not a location outside the area: an entity that says nothing
    // about where it is cannot be placed outside the grant.
    let Some(point) = entity_point(entity) else {
        return match entity.get("location") {
            // A location the parser cannot read must not pass an area check it never ran.
            Some(_) => Err(Refusal::LocationOutsideGrant),
            None => Ok(()),
        };
    };

    let areas: Vec<_> = constraints
        .geo_areas
        .iter()
        .map(|area| granted_polygon(area))
        .collect();
    // An area the parser cannot read is an area the write cannot be shown to be outside of
    // either, so one unreadable grant refuses the write rather than leaving the rest to
    // decide it.
    if areas.iter().any(Option::is_none) {
        return Err(Refusal::LocationOutsideGrant);
    }
    match areas.into_iter().flatten().any(|area| area.contains(point)) {
        true => Ok(()),
        false => Err(Refusal::LocationOutsideGrant),
    }
}

/// The id the payload carries must be an NGSI-LD URN of its type (PF-43).
fn check_id(entity: &Value, contexts: &[&Value]) -> Result<(), Refusal> {
    let raw = entity_id(entity).ok_or_else(|| Refusal::MalformedId(String::new()))?;
    check_identifier_in(raw, entity.get("type").and_then(Value::as_str), contexts)
}

fn check_type(entity: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    if constraints.types.is_empty() {
        return Ok(());
    }
    let declared = entity
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::TypeOutsideGrant(String::new()))?;

    // A granted name, or the IRI the core context or the space's models give one: the
    // decision point read `type_iris` from the Endpoint's own models, never the request (F3).
    let granted = constraints.types.contains(declared)
        || constraints.type_iris.contains(declared)
        || constraints
            .types
            .iter()
            .any(|name| type_iri(name).as_deref() == Some(declared));
    if granted {
        Ok(())
    } else {
        Err(Refusal::TypeOutsideGrant(declared.to_owned()))
    }
}

/// GW17: a write touching any attribute outside the grant, or one a prohibition withholds
/// (GW4), is denied whole, never trimmed.
fn check_attributes(entity: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    let withheld = entity
        .as_object()
        .and_then(|members| {
            members.keys().find(|name| {
                constraints
                    .hidden
                    .iter()
                    .any(|hidden| names_attribute(name, hidden))
            })
        })
        .map(String::as_str);
    match withheld.or_else(|| ungranted(entity, &constraints.attrs).first().copied()) {
        Some(name) => Err(Refusal::AttributeOutsideGrant(name.to_owned())),
        None => Ok(()),
    }
}

fn check_no_smuggled_policy(entity: &Value) -> Result<(), Refusal> {
    let Some(members) = entity.as_object() else {
        return Ok(());
    };
    match members
        .keys()
        .find(|name| POLICY_VOCABULARY.contains(&name.as_str()))
    {
        Some(name) => Err(Refusal::SmuggledPolicyAttribute(name.clone())),
        None => Ok(()),
    }
}

/// The entity's `scope` must sit inside the granted scope tree: a grant on `/geo/SK/BB`
/// covers `/geo/SK/BB/Sasova`, never `/geo/SK/ZA` and never `/geo/SK/BBB` (R13, R29, R30).
fn check_scope(entity: &Value, constraints: &Constraints) -> Result<(), Refusal> {
    let Some(granted) = constraints.granted_scopes.as_deref() else {
        return Ok(());
    };
    let scopes = match entity.get("scope") {
        None => return Ok(()),
        Some(Value::String(scope)) => vec![scope.as_str()],
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        Some(_) => return Err(Refusal::ScopeOutsideGrant(String::new())),
    };

    for scope in scopes {
        if !granted_scopes(granted).any(|prefix| covers(prefix, scope)) {
            return Err(Refusal::ScopeOutsideGrant(scope.to_owned()));
        }
    }
    Ok(())
}

/// The individual scope paths of a `scopeQ`, whose operators (`;`, `|`, `(`, `)`) separate
/// the paths themselves.
fn granted_scopes(scope_q: &str) -> impl Iterator<Item = &str> {
    scope_q
        .split(['(', ')', ';', '|', ','])
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
}

/// Whether a granted scope path covers a scope, as a path prefix rather than a string
/// prefix: `/geo/SK/BB` covers `/geo/SK/BB/Sasova` but not `/geo/SK/BBB`.
fn covers(granted: &str, scope: &str) -> bool {
    let granted = granted.trim_end_matches("/#").trim_end_matches('/');
    scope == granted
        || scope
            .strip_prefix(granted)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "urn:ngsi-ld:AirQualityObserved:station-01";
    const MODEL_IRI: &str = "https://smartdatamodels.org/dataModel.Environment/AirQualityObserved";
    const CORE_IRI: &str = "https://uri.etsi.org/ngsi-ld/default-context/AirQualityObserved";

    /// The space's model, as the Endpoint carries it.
    fn model() -> Value {
        json!({"@context": {"AirQualityObserved": MODEL_IRI}})
    }

    /// A grant of the short name, with the model's reading the decision point adds to it.
    fn grant() -> Constraints {
        let types: BTreeSet<String> = ["AirQualityObserved".to_owned()].into();
        let model = model();
        Constraints {
            type_iris: crate::pdp::projection::model_type_iris(&types, [&model].into_iter()),
            types,
            ..Constraints::default()
        }
    }

    fn entity(id: &str, kind: &str) -> Value {
        json!({"id": id, "type": kind})
    }

    /// F3: a type written as the IRI the server's core context or the Endpoint's model gives the
    /// granted short name passes the URN check and the grant, as the short name does.
    #[test]
    fn an_expanded_type_equal_to_a_granted_type_passes_both_checks() {
        let model = model();
        for kind in ["AirQualityObserved", MODEL_IRI, CORE_IRI] {
            check_in(&entity(ID, kind), &grant(), &[&model])
                .unwrap_or_else(|refusal| panic!("{kind}: {refusal}"));
        }
        // The core reading needs no model.
        check(&entity(ID, CORE_IRI), &grant()).expect("the core context's IRI");
        check_identifier(ID, Some(CORE_IRI)).expect("the core context's IRI");
    }

    /// F3: a type other than the URN's is refused whether it is written short or expanded, and
    /// a type outside the grant stays outside it in either form.
    #[test]
    fn a_mismatched_type_is_refused_short_and_expanded() {
        let model = model();
        for kind in [
            "WeatherObserved",
            "https://smartdatamodels.org/dataModel.Weather/WeatherObserved",
            "https://uri.etsi.org/ngsi-ld/default-context/WeatherObserved",
            "https://smartdatamodels.org/dataModel.Environment/AirQualityObservedX",
        ] {
            assert!(
                matches!(
                    check_in(&entity(ID, kind), &grant(), &[&model]),
                    Err(Refusal::MalformedId(_))
                ),
                "{kind}"
            );
        }
        let other = "urn:ngsi-ld:WeatherObserved:station-01";
        for kind in [
            "WeatherObserved",
            "https://uri.etsi.org/ngsi-ld/default-context/WeatherObserved",
        ] {
            assert!(
                matches!(
                    check_in(&entity(other, kind), &grant(), &[&model]),
                    Err(Refusal::TypeOutsideGrant(_))
                ),
                "{kind}"
            );
        }
    }

    /// F3: only the server's and the Endpoint's own contexts expand a name. A request that maps
    /// the granted short name to another IRI gains nothing: that IRI is neither the URN's type
    /// nor a granted one, and the request's context itself never reaches the broker.
    #[test]
    fn a_request_context_never_widens_the_grant() {
        let model = model();
        let mut write = entity("urn:ngsi-ld:AirQualityObserved:x", "https://x/B");
        write["@context"] = json!({"AirQualityObserved": "https://x/B"});
        assert!(matches!(
            check_in(&write, &grant(), &[&model]),
            Err(Refusal::MalformedId(_))
        ));
        let mut write = entity("urn:ngsi-ld:B:x", "https://x/B");
        write["@context"] = json!({"AirQualityObserved": "https://x/B"});
        assert!(check_in(&write, &grant(), &[&model]).is_err());
        assert!(matches!(
            check_type(&write, &grant()),
            Err(Refusal::TypeOutsideGrant(_))
        ));
        let body = serde_json::to_vec(&write).expect("json");
        assert!(crate::context_guard::check_body(&body).is_err());
    }
}
