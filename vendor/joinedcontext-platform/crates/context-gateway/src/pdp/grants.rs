//! Each grant on its own (T-3530, EP-16): its types, id patterns, `q`, `scopeQ`, area and
//! attributes, as the evaluator builds the constraint set from them.
//!
//! The broker's view decides which entities and attributes a caller is served (ADR-N-050); the
//! gateway no longer judges an answer entity by entity (T-4755). What stays here is the attributes
//! a grant's own conditions read, which a write probe asks the broker for.

use crate::auth::subject::Subject;
use jc_core::kinds::{Operation, PolicySpec, RegistrationInfo, WriteCapability};
use jc_core::Urn;
use jc_time::TimestampTz;
use std::collections::BTreeSet;

/// One grant as an entity is judged against it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantView {
    /// The types it names; empty is every type.
    pub types: BTreeSet<String>,
    /// What the space's models make of those names, filled where the decision knows the models
    /// (T-3473): one grant's IRIs, never another's.
    pub type_iris: BTreeSet<String>,
    /// Its anchored id patterns; empty is every id.
    pub id_patterns: BTreeSet<String>,
    /// Its `q`, in the NGSI-LD query language.
    pub q: Option<String>,
    /// Its `scopeQ`.
    pub scope_q: Option<String>,
    /// Its area, as a `geoQ`.
    pub geo_q: Option<String>,
    /// The attributes it shows; empty is the whole entity (EP-16).
    pub attrs: BTreeSet<String>,
}

impl GrantView {
    /// The view of one policy: its own selectors, conditions and whitelist, and nobody else's.
    pub fn of(policy: &PolicySpec) -> Self {
        GrantView {
            types: super::evaluator::granted_types(&policy.information),
            type_iris: BTreeSet::new(),
            id_patterns: super::evaluator::id_patterns(policy),
            q: policy.q.clone(),
            scope_q: policy.scope_q.clone(),
            geo_q: policy.geo_q.clone(),
            attrs: super::evaluator::granted_attrs(&policy.information),
        }
    }
}

/// What of an entity a delete rule asks one grant for (S2-06).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Part<'a> {
    /// The entity by type and id: the delete the broker decides itself, on what it stores.
    Root,
    /// Every attribute, whatever the entity holds: a delete that reads nothing more.
    Whole,
    /// These attributes, all the entity holds.
    Holding(&'a [&'a str]),
    /// This one attribute.
    Attribute(&'a str),
}

/// Whether one selector of one grant alone gives `subject` the `operation` on `part` of `id`,
/// as the broker compiles a Policy (one rule per selector, ADR-N-050): the type, the id, the
/// attribute list and the attribute's capabilities are that one's, so no grant, entry or
/// selector lends another what it lacks (AUZ-13, GW16).
///
/// A grant with a condition on the stored entity gives nothing here, and neither does one with
/// a window to a write, which the broker admits under none.
pub(crate) fn one_grant_gives(
    subject: &Subject,
    policies: &[PolicySpec],
    operation: Operation,
    id: &str,
    part: Part<'_>,
    now: TimestampTz,
) -> bool {
    let Ok(urn) = id.parse::<Urn>() else {
        return false;
    };
    let kind = urn.entity_type();
    let applying = || {
        policies
            .iter()
            .filter(move |policy| super::evaluator::applies(policy, subject, operation, now))
    };
    // A prohibition refuses the delete of an entity holding an attribute it names, and of
    // every entity of its type when it names none.
    let withheld = applying().any(|policy| {
        policy.effect.is_prohibition()
            && match part {
                Part::Whole => prohibits(&policy.information, kind, |_| true),
                Part::Holding(held) => prohibits(&policy.information, kind, |named| {
                    held.iter().any(|name| same_attribute(name, named))
                }),
                Part::Root | Part::Attribute(_) => false,
            }
    });
    !withheld
        && applying().any(|policy| {
            !policy.effect.is_prohibition()
                && policy.q.is_none()
                && policy.scope_q.is_none()
                && policy.geo_q.is_none()
                && (!operation.is_write() || policy.temporal_q.is_none())
                && (policy.information.is_empty()
                    || policy
                        .information
                        .iter()
                        .any(|info| selects(info, kind, id) && covers(info, operation, part)))
        })
}

/// Whether a prohibition's `information` takes an attribute `named` from a `kind`, or all.
fn prohibits(information: &[RegistrationInfo], kind: &str, named: impl Fn(&str) -> bool) -> bool {
    information.is_empty()
        || information.iter().any(|info| {
            let mut names = info
                .property_names
                .iter()
                .chain(&info.relationship_names)
                .peekable();
            (info.entities.is_empty() || info.entities.iter().any(|e| e.entity_type == kind))
                && (names.peek().is_none() || names.any(|name| named(name)))
        })
}

/// Whether two spellings, term or IRI, may name one attribute. For what withholds, never for
/// what grants.
pub(crate) fn same_attribute(one: &str, other: &str) -> bool {
    super::write_guard::names_attribute(one, other)
        || super::write_guard::names_attribute(other, one)
}

/// Whether one selector of `info` names both the type and the id.
fn selects(info: &RegistrationInfo, kind: &str, id: &str) -> bool {
    info.entities.iter().any(|selector| {
        selector.entity_type == kind
            && match (&selector.id, &selector.id_pattern) {
                (Some(named), _) => named.to_string() == id,
                // anchored at both ends, as the broker anchors it
                (None, Some(pattern)) => antares_ql::regex::compile_anchored(pattern)
                    .is_ok_and(|compiled| compiled.is_match(id)),
                (None, None) => true,
            }
    })
}

/// Whether `info`'s own list and capabilities give `operation` on `part`.
fn covers(info: &RegistrationInfo, operation: Operation, part: Part<'_>) -> bool {
    let allows = |name: &str| {
        let (own, every) = (info.attributes.get(name), info.attributes.get("*"));
        let kind = match operation {
            Operation::DeleteEntity | Operation::DeleteAttrs => WriteCapability::Delete,
            Operation::UpdateAttrs => WriteCapability::Update,
            _ => {
                return own
                    .and_then(|caps| caps.read)
                    .or_else(|| every.and_then(|caps| caps.read))
                    .unwrap_or(true)
            }
        };
        own.and_then(|caps| caps.write.as_ref())
            .or_else(|| every.and_then(|caps| caps.write.as_ref()))
            .is_none_or(|kinds| kinds.contains(&kind))
    };
    let mut listed = info
        .property_names
        .iter()
        .chain(&info.relationship_names)
        .peekable();
    let every_attribute = listed.peek().is_none();
    match part {
        Part::Root => true,
        Part::Whole => {
            every_attribute && allows("*") && info.attributes.keys().all(|name| allows(name))
        }
        Part::Holding(held) => {
            let listed: BTreeSet<&str> = listed.map(String::as_str).collect();
            held.iter()
                .all(|name| (every_attribute || listed.contains(name)) && allows(name))
        }
        Part::Attribute(name) => {
            (every_attribute || listed.any(|listed| listed == name)) && allows(name)
        }
    }
}

/// The attributes the grants' own conditions read, which the broker has to answer with for the
/// conditions to be judged here: each `q`'s top-level names, and `location` under an area.
///
/// They are asked for beside the shown ones and stripped again by the projection unless a grant
/// the entity matches shows them.
pub fn condition_attributes(grants: &[GrantView]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for grant in grants {
        if let Some(node) = grant.q.as_deref().and_then(|q| antares_ql::parse_q(q).ok()) {
            names.extend(node.attribute_paths().into_iter().map(str::to_owned));
        }
        if grant.geo_q.is_some() {
            names.insert("location".to_owned());
        }
    }
    names
}

/// The read `read` split into one read per grant, each with that grant's own types, id patterns
/// and filter (F-PLT2-004, ADR 006). The union read pairs one grant's types with another's `q`:
/// with A {X, `a>1`} and B {Y, `b>1`}, an X entity with `b>1` passed. A grant whose types the
/// read does not ask is left out; an entity is readable when one of these reads finds it.
pub fn per_grant(read: &super::evaluator::Constraints) -> Vec<super::evaluator::Constraints> {
    read.grants
        .iter()
        .filter_map(|grant| {
            let types: BTreeSet<String> = if grant.types.is_empty() {
                read.types.clone()
            } else if read.types.is_empty() {
                grant.types.clone()
            } else {
                grant.types.intersection(&read.types).cloned().collect()
            };
            if types.is_empty() && !(grant.types.is_empty() && read.types.is_empty()) {
                return None;
            }
            let mut one = read.clone();
            one.types = types;
            one.id_patterns = grant.id_patterns.clone().into();
            one.q = super::evaluator::grant_filter(grant.q.as_deref(), grant.scope_q.as_deref())
                .map(|filter| format!("({filter})"));
            one.grants = vec![grant.clone()];
            Some(one)
        })
        .collect()
}

#[cfg(test)]
mod per_grant_tests {
    use super::*;

    /// T-7799: a registration's `idPattern` that compiles only inside the `^(?:...)$` wrapper
    /// selects no id.
    #[test]
    fn a_selector_pattern_closing_the_wrapper_selects_nothing() {
        let info = |pattern: &str| RegistrationInfo {
            entities: vec![jc_core::kinds::EntitySelector {
                entity_type: "Car".to_owned(),
                id: None,
                id_pattern: Some(pattern.to_owned()),
            }],
            property_names: Vec::new(),
            relationship_names: Vec::new(),
            attributes: std::collections::BTreeMap::new(),
        };
        let escape = info("urn:x:a)|(.*");
        for id in ["urn:x:a", "urn:y:b", "x"] {
            assert!(!selects(&escape, "Car", id), "{id}");
        }
        let fine = info("urn:x:a|urn:x:b");
        assert!(selects(&fine, "Car", "urn:x:b") && !selects(&fine, "Car", "urn:x:bb"));
    }

    /// F-PLT2-004: each read carries one grant's types with that grant's filter alone.
    #[test]
    fn each_grant_reads_its_own_types_with_its_own_filter() {
        let grant = |ty: &str, q: &str| GrantView {
            types: BTreeSet::from([ty.to_owned()]),
            q: Some(q.to_owned()),
            ..GrantView::default()
        };
        let read = super::super::evaluator::Constraints {
            types: BTreeSet::from(["X".to_owned(), "Y".to_owned()]),
            q: Some("(((a>1))|((b>1)))".to_owned()),
            grants: vec![grant("X", "a>1"), grant("Y", "b>1")],
            ..Default::default()
        };
        let reads = per_grant(&read);
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].types, BTreeSet::from(["X".to_owned()]));
        assert_eq!(reads[0].q.as_deref(), Some("((a>1))"));
        assert_eq!(reads[1].types, BTreeSet::from(["Y".to_owned()]));
        assert_eq!(reads[1].q.as_deref(), Some("((b>1))"));
        let only_x = super::super::evaluator::Constraints {
            types: BTreeSet::from(["X".to_owned()]),
            ..read
        };
        let reads = per_grant(&only_x);
        assert_eq!(reads.len(), 1, "Y's grant reads nothing of an X target");
        assert_eq!(reads[0].q.as_deref(), Some("((a>1))"));
    }

    fn info(attributes: serde_json::Value, names: &[&str]) -> RegistrationInfo {
        serde_json::from_value(serde_json::json!({
            "entities": [{"type": "Car"}],
            "propertyNames": names,
            "attributes": attributes,
        }))
        .expect("a registration")
    }

    /// The capability an operation needs: delete for a delete, update for an update, the read
    /// flag (own name first, then `*`, then open) for every other operation.
    #[test]
    fn each_operation_asks_its_own_capability() {
        let caps = info(
            serde_json::json!({"speed": {"read": false, "write": ["update"]}, "*": {"read": true, "write": ["delete"]}}),
            &[],
        );
        assert!(covers(
            &caps,
            Operation::UpdateAttrs,
            Part::Attribute("speed")
        ));
        assert!(!covers(
            &caps,
            Operation::DeleteAttrs,
            Part::Attribute("speed")
        ));
        assert!(
            covers(&caps, Operation::DeleteAttrs, Part::Attribute("colour")),
            "`*` gives delete"
        );
        assert!(!covers(
            &caps,
            Operation::UpdateAttrs,
            Part::Attribute("colour")
        ));
        assert!(
            !covers(&caps, Operation::RetrieveEntity, Part::Attribute("speed")),
            "own read false"
        );
        assert!(
            covers(&caps, Operation::RetrieveEntity, Part::Attribute("colour")),
            "`*` read true"
        );
        let open = info(serde_json::json!({}), &[]);
        assert!(covers(
            &open,
            Operation::RetrieveEntity,
            Part::Attribute("any")
        ));
        assert!(covers(&open, Operation::DeleteEntity, Part::Whole));
        assert!(covers(&open, Operation::DeleteEntity, Part::Root));
        let named = info(
            serde_json::json!({"speed": {"write": ["delete"]}, "colour": {"write": ["update"]}}),
            &[],
        );
        assert!(
            !covers(&named, Operation::DeleteEntity, Part::Whole),
            "colour gives no delete"
        );
        let deletable = info(serde_json::json!({"speed": {"write": ["delete"]}}), &[]);
        assert!(covers(&deletable, Operation::DeleteEntity, Part::Whole));
    }

    /// A listed registration gives only what it lists; a whole delete needs an open list.
    #[test]
    fn a_listed_registration_gives_only_its_names() {
        let listed = info(serde_json::json!({}), &["speed"]);
        assert!(covers(
            &listed,
            Operation::DeleteAttrs,
            Part::Attribute("speed")
        ));
        assert!(!covers(
            &listed,
            Operation::DeleteAttrs,
            Part::Attribute("colour")
        ));
        assert!(covers(
            &listed,
            Operation::DeleteAttrs,
            Part::Holding(&["speed"])
        ));
        assert!(!covers(
            &listed,
            Operation::DeleteAttrs,
            Part::Holding(&["speed", "colour"])
        ));
        assert!(!covers(&listed, Operation::DeleteEntity, Part::Whole));
        assert!(
            covers(&listed, Operation::DeleteEntity, Part::Holding(&[])),
            "nothing held"
        );
    }

    /// An id that is no URN is given by no grant, whatever the policies say.
    #[test]
    fn an_id_that_is_no_urn_is_given_by_no_grant() {
        let subject = Subject::default();
        assert!(!one_grant_gives(
            &subject,
            &[],
            Operation::DeleteEntity,
            "not a urn",
            Part::Root,
            TimestampTz::now()
        ));
    }

    /// The attributes a grant's `q` reads and `location` under an area are asked for; a `q`
    /// that does not parse adds nothing.
    #[test]
    fn condition_attributes_name_what_the_conditions_read() {
        let grants = [
            GrantView {
                q: Some("speed>1;owner==\"a\"".to_owned()),
                ..GrantView::default()
            },
            GrantView {
                geo_q: Some("georel=near;maxDistance==10".to_owned()),
                ..GrantView::default()
            },
            GrantView {
                q: Some("((".to_owned()),
                ..GrantView::default()
            },
        ];
        let names = condition_attributes(&grants);
        assert_eq!(
            names,
            BTreeSet::from([
                "location".to_owned(),
                "owner".to_owned(),
                "speed".to_owned()
            ])
        );
        assert!(condition_attributes(&[]).is_empty());
        assert!(condition_attributes(&[GrantView::default()]).is_empty());
    }

    /// A grant of every type reads the read's types, a read of every type the grant's, and two
    /// open sides stay open; disjoint types read nothing.
    #[test]
    fn per_grant_types_follow_the_open_side() {
        let types = |names: &[&str]| {
            names
                .iter()
                .map(|n| (*n).to_owned())
                .collect::<BTreeSet<_>>()
        };
        let open_grant = GrantView::default();
        let x_grant = GrantView {
            types: types(&["X"]),
            ..GrantView::default()
        };
        let read = |read_types: &[&str], grant: &GrantView| super::super::evaluator::Constraints {
            types: types(read_types),
            grants: vec![grant.clone()],
            ..Default::default()
        };
        assert_eq!(
            per_grant(&read(&["Y"], &open_grant))[0].types,
            types(&["Y"])
        );
        assert_eq!(per_grant(&read(&[], &x_grant))[0].types, types(&["X"]));
        assert!(per_grant(&read(&[], &open_grant))[0].types.is_empty());
        assert!(per_grant(&read(&["Y"], &x_grant)).is_empty());
        assert!(
            per_grant(&super::super::evaluator::Constraints::default()).is_empty(),
            "no grant, no read"
        );
    }
}
