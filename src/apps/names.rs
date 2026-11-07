//! An App name is one address for the whole organization (AP-14a, AP-14, AP-26).
//!
//! `/apps/{name}/` and the pod `app-{name}` carry no project, so a second project's App of a name
//! another project already declares would share the first one's route, and the edge would serve
//! the visibility of whichever sorts first. The name is refused on every write before a Change
//! exists, and names the other project only to a caller who may read Apps there (PF-59).

use crate::auth::Identity;
use crate::error::ApiError;
use crate::state::AppState;

/// Refuses an App named like another project's App (AP-14a), or one whose client `app-{name}`
/// the realm already holds without this platform having made it (AP-114). The same App written
/// again in its own project is an update, not a clash. Every refusal names both owners: the
/// other one only to a caller who may read Apps there (PF-59), and this project always.
///
/// The other derived names need no check of their own: the pod, Service and Endpoint
/// `app-{name}` follow the App name, and a repository `{project}_{name}` is one pair because
/// neither a project nor an App name may carry `_` (MF-02).
pub fn check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
) -> Result<(), ApiError> {
    if kind != "App" {
        return Ok(());
    }
    if let Some(owner) = state
        .mirror
        .find(|env| {
            env.kind == "App"
                && env.metadata.name == name
                && env.metadata.namespace.as_deref() != Some(project)
        })
        .and_then(|env| env.metadata.namespace)
    {
        let holder = if crate::permissions::for_request(state, identity, &owner).may_read("App") {
            format!("project {owner}")
        } else {
            "another project".to_owned()
        };
        return Err(ApiError::Denied(format!(
            "the App name '{name}' is taken by {holder}, so project {project} cannot declare it \
             too: /apps/{name}/ is one address for the whole organization (AP-14a); choose \
             another name"
        )));
    }
    let client = crate::reconciler::app_clients::client_id(name);
    let new = state.mirror.get(project, "App", name).is_none();
    if new && state.foreign_names.has_client(&client) {
        return Err(ApiError::Denied(format!(
            "the App '{name}' of project {project} would log in with the Keycloak client \
             '{client}', which the realm already holds and this platform did not create; it is \
             never taken over (AP-114). Choose another name, or ask a realm administrator to \
             rename or remove that client"
        )));
    }
    Ok(())
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

    /// `doprava` holds the public App `board`, plus whatever the test adds.
    fn world(extra: Vec<ResourceEnvelope>) -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(manifest(
            "App",
            "board",
            "doprava",
            json!({ "kind": "static", "visibility": "public" }),
        ));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    fn app_reader_in(project: &str) -> Vec<ResourceEnvelope> {
        vec![
            manifest(
                "Role",
                "app-reader",
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": ["App"], "verbs": ["read"] }] }),
            ),
            manifest(
                "RoleBinding",
                "jana-reads-apps",
                ORG_NAMESPACE,
                json!({
                    "subjects": [{ "user": "jana@hel.fi" }],
                    "role": "app-reader",
                    "scope": { "project": project },
                }),
            ),
        ]
    }

    fn refusal(state: &AppState, project: &str, kind: &str, name: &str) -> Option<String> {
        match check(state, &who("jana@hel.fi"), project, kind, name) {
            Ok(()) => None,
            Err(ApiError::Denied(message)) => Some(message),
            Err(other) => panic!("a clash is a refusal, not {other:?}"),
        }
    }

    #[test]
    fn another_projects_app_name_is_refused_without_naming_a_project_the_caller_cannot_read() {
        let state = world(Vec::new());
        let message = refusal(&state, "ovzdusie", "App", "board").expect("a clash");
        assert!(
            message.contains("'board' is taken by another project, so project ovzdusie"),
            "{message}"
        );
        assert!(message.contains("AP-14a"), "{message}");
        assert!(!message.contains("doprava"), "{message}");
    }

    #[test]
    fn a_caller_who_may_read_apps_there_is_told_which_project_holds_the_name() {
        let state = world(app_reader_in("doprava"));
        let message = refusal(&state, "ovzdusie", "App", "board").expect("a clash");
        assert!(
            message.contains("taken by project doprava, so project ovzdusie cannot declare it"),
            "{message}"
        );
    }

    #[test]
    fn the_same_app_again_another_name_or_another_kind_is_not_a_clash() {
        let state = world(Vec::new());
        assert_eq!(
            refusal(&state, "doprava", "App", "board"),
            None,
            "an update"
        );
        assert_eq!(refusal(&state, "ovzdusie", "App", "board-2"), None);
        assert_eq!(refusal(&state, "ovzdusie", "Dashboard", "board"), None);
        assert_eq!(refusal(&state, "ovzdusie", "App", ""), None);
    }

    /// AP-114: a new App whose client `app-{name}` the realm holds unmanaged is refused, naming
    /// the project and the client; an App already declared here is still updated, and a managed
    /// client is this platform's own.
    #[test]
    fn a_new_app_is_refused_the_client_somebody_else_made_in_the_realm() {
        let state = world(Vec::new());
        state
            .foreign_names
            .set_clients(std::collections::BTreeSet::from([
                "app-radar".to_owned(),
                "app-board".to_owned(),
            ]));
        let message = refusal(&state, "ovzdusie", "App", "radar").expect("a clash");
        assert!(
            message.contains("App 'radar' of project ovzdusie"),
            "{message}"
        );
        assert!(message.contains("client 'app-radar'"), "{message}");
        assert!(message.contains("AP-114"), "{message}");
        assert_eq!(
            refusal(&state, "doprava", "App", "board"),
            None,
            "an App already declared is updated; the reconciler reports its client"
        );
        assert_eq!(refusal(&state, "ovzdusie", "App", "radar-2"), None);
        assert_eq!(refusal(&state, "ovzdusie", "Dashboard", "radar"), None);
    }
}
