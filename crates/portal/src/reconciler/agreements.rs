//! An accepted data-space agreement, compiled into the Policies the broker's engine enforces
//! (DS-10, DS-03, R26, T-3927).
//!
//! The connector writes the agreement as a `DataAgreement` manifest whose `constraints` already
//! hold the ODRL agreement's residual filters, losslessly (DS-10). What the participant gets is
//! the offered Endpoints' own Policies, assigned to its DID, held to the agreement's period and
//! narrowed by those filters, so an agreement can never reach past its offer (DS-03):
//!
//! | dimension | rule |
//! |---|---|
//! | assignee | only the offered Endpoints' public Policies are copied; a ServiceAccount's, role's or group's grant stays theirs |
//! | operations | the Policy's, cut to the offer's ODRL permission actions; an action outside CIM 009 and `odrl:read` is refused |
//! | types, attributes | the offer's, copied; the agreement names none of its own |
//! | `q`, `scopeQ` | conjunction of both: a filter the agreement did not repeat stays in force |
//! | `geoQ`, `temporalQ` | the offer's, or the agreement's where the offer names none; two different ones are refused |
//! | validity | the overlap of the agreement's period and the Policy's own |
//!
//! Deciding that one polygon or interval lies inside another is geometry; a wrong answer widens
//! a fence somebody drew on a map, so two different ones are refused rather than intersected.
//! The offer's prohibitions are copied to the DID unchanged: they only take access away. The
//! DID reaches the broker from the verified token alone (`x-jc-did`, T-3800); nothing here
//! reads it from a request.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use jc_core::kinds::{
    AgreementRole, AgreementState, DataAgreementSpec, DataOfferSpec, Operation, OperationGroup,
    OperationRef, PolicySpec, Principal, PrincipalKind, Validity,
};
use serde_json::Value;

/// Why an agreement compiles into nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// Not accepted (yet, or any more): nothing is granted.
    #[error("the agreement is {0}, and only an accepted or finalized one grants access (DS-09)")]
    NotAccepted(&'static str),
    /// The agreement names no offer, or one this project does not hold.
    #[error("the agreement's offer {0:?} is not a DataOffer of this project (DS-07)")]
    NoOffer(String),
    /// The offered Endpoints permit nothing, so there is nothing to narrow.
    #[error("the offer permits nothing; an agreement on it grants nothing (DS-03)")]
    GrantsNothing,
    /// The agreement reaches past what the offer grants (DS-03).
    #[error(
        "the agreement asks for {dimension} {asked:?}, which the offer does not grant (DS-03)"
    )]
    Widens {
        dimension: &'static str,
        asked: String,
    },
    /// The offer's ODRL names an action that is no CIM 009 operation (R8, R26).
    #[error("the offer's action {0:?} is not a CIM 009 operation or odrl:read (R8)")]
    UnknownAction(String),
    /// The agreement's period and the offer's do not overlap.
    #[error("the agreement's period and the offered Policy's do not overlap (DS-10)")]
    NoOverlap,
}

/// The Policies one agreement becomes, each under a name of its own in the offer's space.
///
/// `offered` is every Policy bound to an Endpoint of the offer, by manifest name, and `actions`
/// the offer's ODRL permission actions ([`offer_actions`]): together the ceiling. A
/// consumer-side agreement grants nothing here: its data is the other participant's.
pub fn compile(
    name: &str,
    agreement: &DataAgreementSpec,
    actions: &[Operation],
    offered: &[(String, PolicySpec)],
) -> Result<Vec<(String, PolicySpec)>, CompileError> {
    if agreement.role != AgreementRole::Provider {
        return Ok(Vec::new());
    }
    if !matches!(
        agreement.state,
        AgreementState::Accepted | AgreementState::Finalized
    ) {
        return Err(CompileError::NotAccepted(agreement.state.as_str()));
    }
    let did = Principal {
        kind: PrincipalKind::Did,
        id: agreement.remote_participant.as_str().to_owned(),
    };
    let wanted = &agreement.constraints;
    let mut compiled = Vec::new();
    for (policy_name, policy) in offered {
        let mut granted = policy.clone();
        granted.assignee = did.clone();
        if !policy.effect.is_prohibition() {
            if !is_public(&policy.assignee) {
                continue;
            }
            granted.operations = expand(&policy.operations)
                .filter(|op| actions.contains(op))
                .map(OperationRef::Single)
                .collect();
            if granted.operations.is_empty() {
                continue;
            }
            granted.q = conjunction("q", policy.q.as_deref(), wanted.q.as_deref())?;
            granted.scope_q = conjunction(
                "scopeQ",
                policy.scope_q.as_deref(),
                wanted.scope_q.as_deref(),
            )?;
            granted.geo_q = fence("geoQ", policy.geo_q.as_deref(), wanted.geo_q.as_deref())?;
            granted.temporal_q = fence(
                "temporalQ",
                policy.temporal_q.as_deref(),
                wanted.temporal_q.as_deref(),
            )?;
            granted.validity = Some(overlap(policy.validity.as_ref(), &agreement.validity)?);
        }
        compiled.push((format!("agreement-{name}-{policy_name}"), granted));
    }
    if !compiled.iter().any(|(_, p)| !p.effect.is_prohibition()) {
        return Err(CompileError::GrantsNothing);
    }
    Ok(compiled)
}

/// What anyone may read: the only grant an offer exposes to a participant (DS-03).
fn is_public(assignee: &Principal) -> bool {
    assignee.kind == PrincipalKind::Role && assignee.id == "public"
}

/// The single operations of a list of operations and groups.
fn expand(refs: &[OperationRef]) -> impl Iterator<Item = Operation> + '_ {
    refs.iter()
        .flat_map(|r| match r {
            OperationRef::Single(op) => std::slice::from_ref(op),
            OperationRef::Group(group) => group.operations(),
        })
        .copied()
}

/// The operations the offer's ODRL `permission` actions name (DS-07, R26): CIM 009 operations
/// and groups, `ngsi-ld:` prefixed or not, and `odrl:read` as `retrieveOps`. Anything else is
/// refused: an action the mapping does not know cannot be held to.
pub fn offer_actions(policy: &Value) -> Result<Vec<Operation>, CompileError> {
    let mut names = Vec::new();
    for rule in policy
        .get("permission")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match rule.get("action") {
            Some(Value::Array(actions)) => names.extend(actions.iter().cloned()),
            Some(action) => names.push(action.clone()),
            None => {}
        }
    }
    let mut operations = Vec::new();
    for action in names {
        let name = match &action {
            Value::String(name) => name.as_str(),
            other => other.get("@id").and_then(Value::as_str).unwrap_or_default(),
        };
        let bare = name.strip_prefix("ngsi-ld:").unwrap_or(name);
        let parsed = match bare {
            "odrl:read" | "read" => Ok(OperationRef::Group(OperationGroup::RetrieveOps)),
            _ => serde_json::from_value::<OperationRef>(Value::String(bare.to_owned())),
        };
        let parsed = parsed.map_err(|_| CompileError::UnknownAction(action.to_string()))?;
        operations.extend(expand(std::slice::from_ref(&parsed)));
    }
    Ok(operations)
}

/// Both filters at once (NGSI-LD `;` is AND), the offer's alone when the agreement adds none.
/// The agreement's text comes from the other participant: one that closes the parenthesis it is
/// wrapped in (`b==2)|(c==3`) would turn the AND into an OR past the offer, so it is refused.
fn conjunction(
    dimension: &'static str,
    offer: Option<&str>,
    agreement: Option<&str>,
) -> Result<Option<String>, CompileError> {
    match (offer, agreement) {
        (Some(offer), Some(agreement)) if offer != agreement => {
            if !one_group(agreement) {
                return Err(CompileError::Widens {
                    dimension,
                    asked: agreement.to_owned(),
                });
            }
            Ok(Some(format!("({offer});({agreement})")))
        }
        (offer, agreement) => Ok(offer.or(agreement).map(str::to_owned)),
    }
}

/// Whether `text`'s parentheses balance outside its quoted strings, so wrapping it in one pair
/// keeps it one term.
fn one_group(text: &str) -> bool {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for c in text.chars() {
        match (quoted, escaped, c) {
            (true, true, _) => escaped = false,
            (true, false, '\\') => escaped = true,
            (_, _, '"') => quoted = !quoted,
            (false, _, '(') => depth += 1,
            (false, _, ')') => match depth.checked_sub(1) {
                Some(less) => depth = less,
                None => return false,
            },
            _ => {}
        }
    }
    depth == 0 && !quoted
}

/// A fence the agreement may add where the offer has none, and never move.
fn fence(
    dimension: &'static str,
    offer: Option<&str>,
    agreement: Option<&str>,
) -> Result<Option<String>, CompileError> {
    match (offer, agreement) {
        (Some(offer), Some(agreement)) if offer != agreement => Err(CompileError::Widens {
            dimension,
            asked: agreement.to_owned(),
        }),
        (offer, agreement) => Ok(offer.or(agreement).map(str::to_owned)),
    }
}

/// The later start and the earlier end of the two periods.
fn overlap(policy: Option<&Validity>, agreement: &Validity) -> Result<Validity, CompileError> {
    let later = |a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>| a.max(b);
    let earlier = |a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>| match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let validity = Validity {
        from: later(policy.and_then(|own| own.from), agreement.from),
        to: earlier(policy.and_then(|own| own.to), agreement.to),
    };
    match (validity.from, validity.to) {
        (Some(from), Some(to)) if from >= to => Err(CompileError::NoOverlap),
        _ => Ok(validity),
    }
}

/// Named Policies by (project, space).
pub type Granted = Vec<((String, String), Vec<(String, PolicySpec)>)>;

/// Every provider agreement of `project` with the Policies its offer's Endpoints are bound to,
/// by the offer's space; an agreement that does not compile is reported and grants nothing.
pub fn compile_all(
    agreements: &[(String, String, DataAgreementSpec)],
    offers: &BTreeMap<(String, String), DataOfferSpec>,
    bound: impl Fn(&str, &str) -> Vec<(String, PolicySpec)>,
) -> Granted {
    let mut out = Vec::new();
    for (project, name, agreement) in agreements {
        if agreement.role != AgreementRole::Provider {
            continue;
        }
        let offer_name = agreement
            .offer_ref
            .as_ref()
            .map(|offer| offer.name().to_owned())
            .unwrap_or_default();
        let compiled = match offers.get(&(project.clone(), offer_name.clone())) {
            None => Err(CompileError::NoOffer(offer_name)),
            Some(offer) => {
                let mut offered: Vec<(String, PolicySpec)> = Vec::new();
                for endpoint in &offer.endpoint_refs {
                    for policy in bound(project, endpoint.name()) {
                        if !offered.iter().any(|(n, _)| *n == policy.0) {
                            offered.push(policy);
                        }
                    }
                }
                offer_actions(&offer.policy)
                    .and_then(|actions| compile(name, agreement, &actions, &offered))
                    .map(|policies| (offer.context_space_ref.clone(), policies))
            }
        };
        match compiled {
            Ok((space, policies)) if !policies.is_empty() => {
                out.push(((project.clone(), space), policies));
            }
            Ok(_) => {}
            Err(why) => tracing::warn!(
                project = %project,
                agreement = %name,
                error = %why,
                "an agreement grants nothing"
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DID: &str = "did:web:partner.example";

    /// `compile` under an offer whose ODRL permits `odrl:read`.
    fn compile_read(
        name: &str,
        agreement: &DataAgreementSpec,
        offered: &[(String, PolicySpec)],
    ) -> Result<Vec<(String, PolicySpec)>, CompileError> {
        let read = offer_actions(&json!({"permission": [{"action": "odrl:read"}]})).unwrap();
        compile(name, agreement, &read, offered)
    }

    fn offered() -> Vec<(String, PolicySpec)> {
        let permit: PolicySpec = serde_json::from_value(json!({
            "contextSpaceRef": "air", "assigner": "did:web:hel.fi",
            "assignee": {"kind": "role", "id": "public"},
            "operations": ["queryEntity"], "q": "pm10<500",
            "information": [{"entities": [{"type": "AirQualityObserved"}]}]
        }))
        .expect("a permit");
        let prohibit: PolicySpec = serde_json::from_value(json!({
            "contextSpaceRef": "air", "assigner": "did:web:hel.fi", "effect": "prohibition",
            "assignee": {"kind": "role", "id": "public"},
            "operations": ["queryEntity"],
            "information": [{"entities": [{"type": "Person"}]}]
        }))
        .expect("a prohibition");
        vec![("open".into(), permit), ("no-people".into(), prohibit)]
    }

    fn agreement(state: &str, constraints: serde_json::Value) -> DataAgreementSpec {
        serde_json::from_value(json!({
            "role": "provider", "offerRef": {"kind": "DataOffer", "name": "air-offer"},
            "remoteParticipant": DID, "agreementId": "urn:uuid:1", "state": state,
            "validity": {"from": "2026-10-01T00:00:00Z", "to": "2026-12-31T00:00:00Z"},
            "constraints": constraints
        }))
        .expect("an agreement")
    }

    #[test]
    fn an_agreement_filter_that_closes_its_parenthesis_is_refused() {
        for q in ["district==\"x\")|(pm10>0", ")|(", "a==1)", "(a==1"] {
            let refused = compile_read(
                "partner",
                &agreement("accepted", json!({ "q": q })),
                &offered(),
            );
            assert!(
                matches!(&refused, Err(CompileError::Widens { dimension: "q", asked }) if asked == q),
                "{q}: {refused:?}"
            );
        }
        // A parenthesis inside a quoted value is text, not grouping.
        let kept = compile_read(
            "partner",
            &agreement("accepted", json!({ "q": "name==\"a)|(b\";(x==1|y==2)" })),
            &offered(),
        )
        .expect("compiles");
        assert_eq!(
            kept[0].1.q.as_deref(),
            Some("(pm10<500);(name==\"a)|(b\";(x==1|y==2))")
        );
    }

    #[test]
    fn an_accepted_agreement_grants_the_offer_to_its_did_narrowed_and_in_its_period() {
        let compiled = compile_read(
            "partner",
            &agreement(
                "accepted",
                json!({"q": "district==\"Kallio\"", "geoQ": "georel=within"}),
            ),
            &offered(),
        )
        .expect("compiles");
        assert_eq!(compiled.len(), 2);
        let (name, permit) = &compiled[0];
        assert_eq!(name, "agreement-partner-open");
        assert_eq!(
            (permit.assignee.kind, permit.assignee.id.as_str()),
            (PrincipalKind::Did, DID)
        );
        assert_eq!(
            permit.q.as_deref(),
            Some("(pm10<500);(district==\"Kallio\")")
        );
        assert_eq!(permit.geo_q.as_deref(), Some("georel=within"));
        assert_eq!(permit.operations, offered()[0].1.operations);
        assert_eq!(permit.information, offered()[0].1.information);
        let validity = permit.validity.clone().expect("a period");
        assert_eq!(
            validity.to.map(|t| t.to_rfc3339()),
            Some("2026-12-31T00:00:00+00:00".into())
        );
        let (_, prohibition) = &compiled[1];
        assert!(prohibition.effect.is_prohibition());
        assert_eq!(prohibition.assignee.id, DID);
    }

    #[test]
    fn nothing_is_granted_before_acceptance_or_after_termination() {
        for state in ["requested", "offered", "terminated"] {
            assert!(matches!(
                compile_read("p", &agreement(state, json!({})), &offered()),
                Err(CompileError::NotAccepted(_))
            ));
        }
    }

    #[test]
    fn an_agreement_wider_than_the_offer_is_refused_per_dimension() {
        let mut fenced = offered();
        fenced[0].1.geo_q = Some("georel=within;geometry=Polygon".into());
        fenced[0].1.temporal_q = Some("timerel=after;timeAt=2026-01-01T00:00:00Z".into());
        for (dimension, constraints) in [
            ("geoQ", json!({"geoQ": "georel=near;maxDistance==1000"})),
            (
                "temporalQ",
                json!({"temporalQ": "timerel=after;timeAt=2020-01-01T00:00:00Z"}),
            ),
        ] {
            match compile_read("p", &agreement("finalized", constraints), &fenced) {
                Err(CompileError::Widens { dimension: d, .. }) => assert_eq!(d, dimension),
                other => panic!("{dimension}: {other:?}"),
            }
        }
        // The same fence repeated is not a widening.
        let same = json!({"geoQ": "georel=within;geometry=Polygon"});
        assert!(compile_read("p", &agreement("finalized", same), &fenced).is_ok());
    }

    #[test]
    fn an_offer_that_permits_nothing_grants_nothing() {
        let only_prohibition = vec![offered().remove(1)];
        assert_eq!(
            compile_read("p", &agreement("accepted", json!({})), &only_prohibition),
            Err(CompileError::GrantsNothing)
        );
        assert_eq!(
            compile_read("p", &agreement("accepted", json!({})), &[]),
            Err(CompileError::GrantsNothing)
        );
    }

    #[test]
    fn the_period_is_the_overlap_and_none_is_refused() {
        let mut ended = offered();
        ended[0].1.validity =
            Some(serde_json::from_value(json!({"to": "2026-09-01T00:00:00Z"})).unwrap());
        assert_eq!(
            compile_read("p", &agreement("accepted", json!({})), &ended),
            Err(CompileError::NoOverlap)
        );
    }

    #[test]
    fn a_consumer_agreement_and_a_missing_offer_grant_nothing() {
        let mut consumer = agreement("accepted", json!({}));
        consumer.role = AgreementRole::Consumer;
        assert_eq!(compile_read("p", &consumer, &offered()), Ok(Vec::new()));
        let none = compile_all(
            &[(
                "helsinki".into(),
                "p".into(),
                agreement("accepted", json!({})),
            )],
            &BTreeMap::new(),
            |_, _| offered(),
        );
        assert!(none.is_empty());
    }

    /// The integrator's case: an offered Endpoint carrying a public read Policy and a
    /// ServiceAccount's write Policy gives the DID the read alone, under the offer's actions.
    #[test]
    fn only_the_public_grant_is_offered_and_only_within_the_offers_actions() {
        let mut both = offered();
        let pipeline: PolicySpec = serde_json::from_value(json!({
            "contextSpaceRef": "air", "assigner": "did:web:hel.fi",
            "assignee": {"kind": "serviceAccount", "id": "air-pipeline"},
            "operations": ["updateOps", "createEntity", "queryEntity"],
            "information": [{"entities": [{"type": "AirQualityObserved"}]}]
        }))
        .expect("a write Policy");
        both.push(("pipeline-writes".into(), pipeline));
        both[0].1.operations = vec![
            OperationRef::Group(OperationGroup::RetrieveOps),
            OperationRef::Single(Operation::CreateEntity),
        ];
        let compiled =
            compile_read("p", &agreement("accepted", json!({})), &both).expect("compiles");
        let names: Vec<&str> = compiled.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["agreement-p-open", "agreement-p-no-people"]);
        let (_, permit) = &compiled[0];
        assert!(!permit.operations.is_empty());
        assert!(
            permit.operations.iter().all(|op| !op.is_write()),
            "{:?}",
            permit.operations
        );

        // An offer that permits queryEntity alone narrows the group to it.
        let query = offer_actions(&json!({"permission": [{"action": ["ngsi-ld:queryEntity"]}]}))
            .expect("a CIM 009 action");
        let narrowed = compile("p", &agreement("accepted", json!({})), &query, &both).unwrap();
        assert_eq!(
            narrowed[0].1.operations,
            [OperationRef::Single(Operation::QueryEntity)]
        );
        // An offer whose actions meet none of the public grant's operations grants nothing.
        let write = offer_actions(&json!({"permission": [{"action": "updateOps"}]})).unwrap();
        let mut read_only = offered();
        read_only.truncate(1);
        assert_eq!(
            compile("p", &agreement("accepted", json!({})), &write, &read_only),
            Err(CompileError::GrantsNothing)
        );
    }

    #[test]
    fn an_offer_action_outside_cim_009_is_refused() {
        for action in [json!("odrl:use"), json!("ngsi-ld:launchMissiles"), json!(7)] {
            assert!(
                matches!(
                    offer_actions(&json!({"permission": [{"action": action.clone()}]})),
                    Err(CompileError::UnknownAction(_))
                ),
                "{action}"
            );
        }
        assert_eq!(
            offer_actions(&json!({"permission": [{"action": {"@id": "ngsi-ld:retrieveEntity"}}]})),
            Ok(vec![Operation::RetrieveEntity])
        );
    }
}
