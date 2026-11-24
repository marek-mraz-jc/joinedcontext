//! Fixtures the operations' own tests share: a state, the people who call, the bindings that let
//! them, and the manifests a mirror holds. Test code only.

use serde_json::{json, Value};

use super::{Caller, Via};
use crate::auth::session::Identity;
use crate::config::Config;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use crate::state::AppState;

/// A Portal with no database, forge, broker or assistant.
pub(crate) fn state() -> AppState {
    AppState::new(Config::for_tests(), None)
}

/// A person named `username`, in `groups`, with no realm role.
pub(crate) fn person(username: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{username}"),
        username: username.into(),
        email: None,
        name: None,
        roles: vec![],
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        second_factor: false,
    }
}

/// A person through `via`.
pub(crate) fn caller(identity: Identity, via: Via) -> Caller {
    Caller::new(identity, via)
}

/// The installation's bootstrap administrator, at the Portal's own form.
pub(crate) fn admin(state: &AppState) -> Caller {
    let mut identity = person("admin", &[]);
    identity.roles = vec![state.config.bootstrap_admins.clone()];
    caller(identity, Via::Session)
}

/// A manifest as the mirror holds it.
pub(crate) fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// Group `group` may `verbs` the `kinds` of `project`, through a Role and a RoleBinding named
/// after the group.
pub(crate) fn grant(state: &AppState, group: &str, project: &str, kinds: &[&str], verbs: &[&str]) {
    state.mirror.upsert(envelope(
        "Role",
        group,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        group,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "group": group }], "role": group, "scope": { "project": project } }),
    ));
}
