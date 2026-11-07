//! What a new Context Space is called, and which `{space}` segment its ids carry (PF-76,
//! PF-84, PF-44).
//!
//! A space is named locally, in its project; the `{space}` segment of every URN it holds is
//! rendered as `{project}-{name}` unless `spec.urnSegment` pins it (PF-84), and that segment is
//! what is unique across the organization. One function renders it, one gate refuses a segment
//! another space already renders, and both name the other project only to a caller who may
//! read it (PF-59): to everyone else the segment is simply "taken".

use crate::auth::Identity;
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::Mirror;
use serde_json::Value;

/// What to call a new Context Space, and why (PF-76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The local name to use: `wanted`, because a name is only unique in its project.
    pub name: String,
    /// Whether the segment [`Proposal::name`] renders is free in the organization. `false`
    /// means another space pins that segment, and the caller has to choose another name.
    pub available: bool,
    /// Why the name is not available, in the caller's own terms. `None` when it is.
    pub reason: Option<String>,
}

/// The pin a space manifest carries, if any (PF-84).
fn pin_of(spec: &Value) -> Option<&str> {
    spec.get("urnSegment").and_then(Value::as_str)
}

/// The `{space}` segment of the Context Space `name` of `project` (PF-84).
///
/// The one function every Portal path that mints an id, names a tenant or writes a URN calls,
/// through `jc_core::kinds::urn_segment`, which the gateway and the reconciler call too. A space
/// the mirror does not hold renders without a pin.
pub fn segment(mirror: &Mirror, project: &str, name: &str) -> String {
    let space = mirror.get(project, "ContextSpace", name);
    jc_core::kinds::urn_segment(
        project,
        name,
        space.as_ref().and_then(|env| pin_of(&env.spec)),
    )
}

/// The broker the Context Space `name` of `project` lives on (PF-111): its `brokerRef`, else the
/// organization's default, else the implicit `main`. Empty when its `brokerRef` names no
/// `ContextBroker`: such a space is never served from another broker.
pub fn broker(mirror: &Mirror, project: &str, name: &str) -> String {
    let declared: Vec<(String, jc_core::kinds::ContextBrokerSpec)> = mirror
        .matching(|env| env.kind == "ContextBroker")
        .into_iter()
        .filter_map(|env| Some((env.metadata.name, serde_json::from_value(env.spec).ok()?)))
        .collect();
    let space = mirror.get(project, "ContextSpace", name);
    let broker_ref = space
        .as_ref()
        .and_then(|env| env.spec.get("brokerRef"))
        .and_then(Value::as_str);
    let named = declared.iter().map(|(name, spec)| (name.as_str(), spec));
    jc_core::kinds::broker_of(broker_ref, named)
        .map(str::to_owned)
        .unwrap_or_default()
}

/// The address of broker `name` (PF-111): `main` is `JC_PORTAL_BROKER_URL`, any other broker
/// `JC_PORTAL_BROKER_URL_TEMPLATE` with `{broker}` replaced. `None` for a broker this Portal
/// cannot reach, whose spaces are never sent to `main` instead.
pub fn broker_address(name: &str, main: Option<&str>, template: Option<&str>) -> Option<String> {
    match name {
        jc_core::kinds::IMPLICIT_BROKER => main.map(str::to_owned),
        "" => None,
        name => template.map(|template| template.replace("{broker}", name)),
    }
}

/// The `{space}` segment of a Context Space manifest in hand, a draft included.
pub fn segment_of(env: &ResourceEnvelope) -> String {
    let project = env.metadata.namespace.as_deref().unwrap_or_default();
    jc_core::kinds::urn_segment(project, &env.metadata.name, pin_of(&env.spec))
}

/// The project of another Context Space that renders `wanted`, if any does.
fn holder(state: &AppState, wanted: &str, project: &str, name: &str) -> Option<String> {
    state
        .mirror
        .find(|env| {
            env.kind == "ContextSpace"
                && !(env.metadata.namespace.as_deref() == Some(project)
                    && env.metadata.name == name)
                && segment_of(env) == wanted
        })
        .and_then(|env| env.metadata.namespace)
}

/// How a collision is described to this caller (PF-59, PF-76): the project that holds the name
/// when the caller may read Context Spaces there, and nothing but "taken" when they may not.
fn taken_by(state: &AppState, identity: &Identity, owner: &str) -> String {
    if crate::permissions::for_request(state, identity, owner).may_read("ContextSpace") {
        format!("taken by project {owner}")
    } else {
        "taken".to_owned()
    }
}

/// The name a new Context Space should carry in `project` (PF-76, PF-84).
///
/// The local name as asked: two projects may each hold a space called `air`, because their
/// ids render `{project}-air`. Every door proposes through this function, so the Portal's form,
/// the assistant's drafts and `jc_space_complete` cannot disagree about a name.
pub fn propose_name(
    state: &AppState,
    identity: &Identity,
    project: &str,
    wanted: &str,
) -> Proposal {
    let rendered = jc_core::kinds::urn_segment(project, wanted, None);
    let reason = holder(state, &rendered, project, wanted)
        .map(|owner| format!("'{rendered}' is {}", taken_by(state, identity, &owner)));
    Proposal {
        name: wanted.to_owned(),
        available: reason.is_none(),
        reason,
    }
}

/// Refuses a Context Space whose `{space}` segment another space already renders (PF-44,
/// PF-76, PF-84).
///
/// Called on every write before a Change exists, so the route, an operation, the assistant, an
/// import and a dry run all answer the same refusal. The same space written again is an
/// update, not a clash.
pub fn check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "ContextSpace" {
        return Ok(());
    }
    let rendered = jc_core::kinds::urn_segment(project, name, pin_of(spec));
    let Some(owner) = holder(state, &rendered, project, name) else {
        return Ok(());
    };
    let reason = taken_by(state, identity, &owner);
    Err(ApiError::Denied(format!(
        "the entity ids of context space '{name}' would carry '{rendered}', which is {reason}: \
         an id segment is unique in the organization (PF-84); choose another name or \
         spec.urnSegment"
    )))
}

/// Only the space's owner sets or changes its retention, which deletes data (DM-85): the grant
/// that approves a Context Space there, beyond the one that proposes it. An unchanged
/// `spec.retention`, absent and empty alike, asks nothing more.
pub fn check_retention(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "ContextSpace" {
        return Ok(());
    }
    let rules = |spec: &Value| match spec.get("retention") {
        Some(Value::Array(rules)) => rules.clone(),
        _ => Vec::new(),
    };
    let held = state
        .mirror
        .get(project, "ContextSpace", name)
        .map(|env| rules(&env.spec))
        .unwrap_or_default();
    if held == rules(spec)
        || crate::permissions::for_request(state, identity, project).may_in(
            "ContextSpace",
            jc_core::kinds::Verb::Approve,
            Some(name),
        )
    {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "only an owner of context space '{name}', who may approve it, sets or changes its \
         retention: it deletes data (DM-85)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::{json, Value};

    fn who(email: &str) -> Identity {
        Identity {
            client: None,
            subject: format!("f:1:{email}"),
            username: email.split('@').next().unwrap_or(email).to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        }
    }

    fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    /// A world where `ovzdusie` already holds the space `mhd`, plus whatever the test adds.
    fn world(extra: Vec<ResourceEnvelope>) -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(manifest(
            "ContextSpace",
            "mhd",
            "ovzdusie",
            json!({ "isSandbox": false }),
        ));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    /// The binding that lets `jana@hel.fi` read Context Spaces in `project`.
    fn reader_in(project: &str) -> Vec<ResourceEnvelope> {
        vec![
            manifest(
                "Role",
                "space-reader",
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
            ),
            manifest(
                "RoleBinding",
                "jana-reads",
                ORG_NAMESPACE,
                json!({
                    "subjects": [{ "user": "jana@hel.fi" }],
                    "role": "space-reader",
                    "scope": { "project": project },
                }),
            ),
        ]
    }

    #[test]
    fn a_name_is_proposed_as_asked_and_renders_under_its_project() {
        let state = world(Vec::new());
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert_eq!(
            proposal.name, "mhd",
            "ovzdusie/mhd renders ovzdusie-mhd, so mhd is free here"
        );
        assert!(proposal.available);
        assert_eq!(proposal.reason, None);
        assert_eq!(segment(&state.mirror, "ovzdusie", "mhd"), "ovzdusie-mhd");
        assert_eq!(
            segment(&state.mirror, "doprava", "nothing-yet"),
            "doprava-nothing-yet"
        );
    }

    #[test]
    fn a_pin_wins_and_takes_its_segment_from_everyone_else() {
        let state = world(vec![manifest(
            "ContextSpace",
            "hub",
            "helsinki",
            json!({ "urnSegment": "doprava-mhd" }),
        )]);
        assert_eq!(segment(&state.mirror, "helsinki", "hub"), "doprava-mhd");
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert!(!proposal.available);
        assert_eq!(proposal.reason.as_deref(), Some("'doprava-mhd' is taken"));
    }

    #[test]
    fn a_taken_segment_names_the_holder_only_to_a_reader() {
        let pinned = manifest(
            "ContextSpace",
            "old",
            "ovzdusie",
            json!({ "urnSegment": "doprava-mhd" }),
        );
        let mut extra = reader_in("ovzdusie");
        extra.push(pinned.clone());
        let reader = world(extra);
        let said = format!(
            "{:?}",
            check(
                &reader,
                &who("jana@hel.fi"),
                "doprava",
                "ContextSpace",
                "mhd",
                &json!({})
            )
            .expect_err("the segment is pinned elsewhere")
        );
        assert!(said.contains("taken by project ovzdusie"), "{said}");
        assert!(said.contains("doprava-mhd"), "{said}");

        let stranger = world(vec![pinned]);
        let said = format!(
            "{:?}",
            check(
                &stranger,
                &who("jana@hel.fi"),
                "doprava",
                "ContextSpace",
                "mhd",
                &json!({})
            )
            .expect_err("taken")
        );
        assert!(said.contains("which is taken"), "{said}");
        assert!(!said.contains("ovzdusie"), "{said}");
    }

    #[test]
    fn the_gate_lets_the_same_space_and_other_kinds_through_and_checks_a_pin() {
        let state = world(reader_in("ovzdusie"));
        let jana = who("jana@hel.fi");
        // The same local name in two projects renders two segments.
        check(&state, &jana, "doprava", "ContextSpace", "mhd", &json!({}))
            .expect("doprava-mhd is free");
        // The same space written again is an update of that space.
        check(&state, &jana, "ovzdusie", "ContextSpace", "mhd", &json!({})).expect("its own space");
        // A pin onto another space's rendered segment is refused.
        check(
            &state,
            &jana,
            "doprava",
            "ContextSpace",
            "x",
            &json!({ "urnSegment": "ovzdusie-mhd" }),
        )
        .expect_err("ovzdusie-mhd is ovzdusie/mhd's");
        // Every other kind is named inside its project and is none of this gate's business.
        check(&state, &jana, "doprava", "Pipeline", "mhd", &json!({})).expect("not a space");
    }

    /// DM-85: a proposer who may not approve the space changes anything but its retention; an
    /// owner, bound to that space or the project, sets and removes it.
    #[test]
    fn only_an_owner_sets_or_changes_a_spaces_retention() {
        let role = |name: &str, verbs: Value| {
            manifest(
                "Role",
                name,
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": verbs }] }),
            )
        };
        let binding = |name: &str, user: &str, role: &str, scope: Value| {
            manifest(
                "RoleBinding",
                name,
                ORG_NAMESPACE,
                json!({ "subjects": [{ "user": user }], "role": role, "scope": scope }),
            )
        };
        let state = world(vec![
            role("proposer", json!(["read", "propose"])),
            role("owner", json!(["read", "propose", "approve"])),
            binding(
                "jana",
                "jana@hel.fi",
                "proposer",
                json!({ "project": "ovzdusie" }),
            ),
            binding(
                "eero",
                "eero@hel.fi",
                "owner",
                json!({ "contextSpace": "mhd" }),
            ),
            binding(
                "aino",
                "aino@hel.fi",
                "owner",
                json!({ "contextSpace": "other" }),
            ),
        ]);
        let ruled = json!({ "retention": [{ "entityType": "Vehicle", "history": "PT8H" }] });
        let ask = |user: &str, spec: &Value| {
            check_retention(&state, &who(user), "ovzdusie", "ContextSpace", "mhd", spec)
        };
        assert!(
            ask("jana@hel.fi", &json!({ "isSandbox": true })).is_ok(),
            "retention untouched"
        );
        assert!(
            ask("jana@hel.fi", &json!({ "retention": [] })).is_ok(),
            "empty is absent"
        );
        let said = format!("{:?}", ask("jana@hel.fi", &ruled).expect_err("a proposer"));
        assert!(said.contains("DM-85"), "{said}");
        assert!(
            ask("aino@hel.fi", &ruled).is_err(),
            "an owner of another space"
        );
        assert!(ask("eero@hel.fi", &ruled).is_ok(), "the space's owner");
        assert!(
            check_retention(
                &state,
                &who("jana@hel.fi"),
                "ovzdusie",
                "Endpoint",
                "mhd",
                &ruled
            )
            .is_ok(),
            "another kind"
        );

        state
            .mirror
            .upsert(manifest("ContextSpace", "mhd", "ovzdusie", ruled.clone()));
        assert!(ask("jana@hel.fi", &ruled).is_ok(), "unchanged");
        assert!(
            ask("jana@hel.fi", &json!({})).is_err(),
            "removing it is a change too"
        );
        assert!(ask("eero@hel.fi", &json!({})).is_ok());
    }
}
