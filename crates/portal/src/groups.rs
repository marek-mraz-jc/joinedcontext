//! A Group name is one Keycloak group for the whole organization (AP-115, ADR-N-030 §3.5,
//! ADR-N-031).
//!
//! A `Group` belongs to the organization, or to the App whose default group it is, which the
//! annotation `joinedcontext.com/app: {project}/{app}` says. A write that would hand a group of
//! one owner to another is refused, and so is a new group whose name the realm already holds
//! without this platform managing it: the reconciler never takes such a group over (PF-63).
//! Both are refused before a Change exists, naming both owners.

use jc_core::kinds::{Rule, Verb};
use jc_core::ObjectMeta;
use serde_json::Value;

use crate::apps::reconciler::APP_LABEL;
use crate::auth::Identity;
use crate::error::ApiError;
use crate::permissions::{Effective, Grant, ORG_NAMESPACE};
use crate::state::AppState;

/// Refuses a `Group` whose name another owner holds (AP-115). Writing a group again under the
/// owner it has, or without saying one (a person editing the members), is an update.
pub fn check(
    state: &AppState,
    identity: &Identity,
    kind: &str,
    meta: &ObjectMeta,
) -> Result<(), ApiError> {
    if kind != "Group" {
        return Ok(());
    }
    let name = &meta.name;
    let wanted = meta.annotations.get(APP_LABEL);
    let Some(held) = state.mirror.get(ORG_NAMESPACE, "Group", name) else {
        if state.foreign_names.has_group(name) {
            return Err(ApiError::Denied(format!(
                "the group '{name}' for {} already exists in the realm and this platform does \
                 not manage it; it is never taken over (AP-115). Choose another name, or ask a \
                 realm administrator to rename or remove that group",
                written(wanted)
            )));
        }
        if let Some(project) = project_named_by(state, name) {
            return Err(ApiError::Denied(format!(
                "the group '{name}' is project {project}'s, filled from its role bindings, and \
                 admits to its Endpoints; a Group manifest may not take the name (PF-87). \
                 Choose another name"
            )));
        }
        return Ok(());
    };
    let current = held.metadata.annotations.get(APP_LABEL);
    if wanted.is_none() || wanted == current {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "the group '{name}' belongs to {}, so it cannot become the group of {} (AP-115); \
         choose another name",
        owner(state, identity, current),
        written(wanted)
    )))
}

/// The project whose derived group `name` is, when a `Project` manifest holds that slug.
fn project_named_by<'a>(state: &AppState, name: &'a str) -> Option<&'a str> {
    let project = name
        .strip_suffix("-readers")
        .or_else(|| name.strip_suffix("-writers"))?;
    state
        .mirror
        .get(ORG_NAMESPACE, "Project", project)
        .map(|_| project)
}

/// The owner the write names, in words: the caller's own manifest, so nothing is disclosed.
fn written(app: Option<&String>) -> String {
    match app.map(|app| app.split_once('/')) {
        None => "the organization".to_owned(),
        Some(Some((project, name))) => format!("the App {name} of project {project}"),
        Some(None) => format!("the App {}", app.map_or("", String::as_str)),
    }
}

/// Who a group belongs to today, in words: an App's project only to a caller who may read Apps
/// there (PF-59).
fn owner(state: &AppState, identity: &Identity, app: Option<&String>) -> String {
    match app.and_then(|app| app.split_once('/')) {
        None if app.is_none() => "the organization".to_owned(),
        Some((project, _))
            if crate::permissions::for_request(state, identity, project).may_read("App") =>
        {
            written(app)
        }
        _ => "an App of another project".to_owned(),
    }
}

/// The App whose default group the organization's `Group` `name` is, as `(project, App)`, when
/// the group is annotated with it and that App exists (AP-118).
fn app_of(state: &AppState, name: &str) -> Option<(String, Value)> {
    let held = state.mirror.get(ORG_NAMESPACE, "Group", name)?;
    let (project, app) = held.metadata.annotations.get(APP_LABEL)?.split_once('/')?;
    let app = state.mirror.get(project, "App", app)?;
    Some((project.to_owned(), serde_json::to_value(app).ok()?))
}

/// Whether the caller may read the organization's `Group` `name` because it is the default group
/// of an App they may read in its project (AP-119): the App page shows who holds each role.
pub fn may_read_as_app_group(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
) -> bool {
    project == ORG_NAMESPACE
        && kind == "Group"
        && app_of(state, name).is_some_and(|(owner, app)| {
            crate::permissions::for_request(state, identity, &owner).may_read_manifest("App", &app)
        })
}

/// What the caller may propose in `project`: their own grants, and for an update of an App's
/// default group that keeps its annotation, the right to propose that `Group` when they may
/// propose the App itself (AP-119). A project's steward keeps the members of their Apps' groups
/// and no other group of the organization; the grant creates, moves and deletes no group.
pub fn for_proposal(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    update: bool,
    manifest: &Value,
) -> Effective {
    let mut effective = crate::permissions::for_request(state, identity, project);
    if project != ORG_NAMESPACE || effective.may(kind, Verb::Propose) {
        return effective;
    }
    if kind == "RoleBinding" {
        effective
            .grants
            .extend(project_binding(state, identity, manifest));
        return effective;
    }
    if !update || kind != "Group" {
        return effective;
    }
    let Some(name) = manifest.pointer("/metadata/name").and_then(Value::as_str) else {
        return effective;
    };
    let Some((owner, app)) = app_of(state, name) else {
        return effective;
    };
    let kept = manifest
        .pointer("/metadata/annotations")
        .and_then(|annotations| annotations.get(APP_LABEL))
        .and_then(Value::as_str);
    let Some(held) = state.mirror.get(ORG_NAMESPACE, "Group", name) else {
        return effective;
    };
    let annotated = held.metadata.annotations.get(APP_LABEL).cloned();
    if kept.is_none() || kept.map(str::to_owned) != annotated {
        return effective;
    }
    // The members are what the App's steward keeps; any other change is the organization's
    // (F-POR4-010).
    if !only_members_differ(&held, manifest) {
        return effective;
    }
    // The App's own check, constraints and the build lane's status-only rule included.
    if crate::permissions::for_request(state, identity, &owner)
        .check("App", Verb::Propose, Some(&app))
        .is_err()
    {
        return effective;
    }
    let app_name = app
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    effective.grants.push(Grant {
        role: format!("propose on App {app_name}"),
        binding: format!("the default group of App {app_name} (AP-119)"),
        scope: format!("project:{owner}"),
        space: None,
        organization: None,
        rule: Rule {
            kinds: vec!["Group".to_owned()],
            verbs: vec![Verb::Propose],
            constraints: Vec::new(),
        },
    });
    effective
}

/// A binding scoped to one project, or to a context space of it, is proposed by whoever may propose
/// `RoleBinding` in that project (S-27, PF-52): a project's administrator grants inside it. The
/// binding it replaces must lie in the same project, so nobody moves another project's or the
/// organization's binding into theirs. What the binding grants is still held to the proposer's
/// own rights there (`within_own_rights`), and approval stays the organization's.
fn project_binding(state: &AppState, identity: &Identity, manifest: &Value) -> Option<Grant> {
    let project = crate::permissions::binding_project(&state.mirror, manifest.get("spec")?)?;
    let name = manifest.pointer("/metadata/name").and_then(Value::as_str)?;
    if let Some(held) = state.mirror.get(ORG_NAMESPACE, "RoleBinding", name) {
        if crate::permissions::binding_project(&state.mirror, &held.spec).as_ref() != Some(&project)
        {
            return None;
        }
    }
    crate::permissions::for_request(state, identity, &project)
        .check("RoleBinding", Verb::Propose, Some(manifest))
        .ok()?;
    Some(Grant {
        role: "propose on RoleBinding".to_owned(),
        binding: format!("propose on RoleBinding in project {project} (PF-52)"),
        scope: format!("project:{project}"),
        space: None,
        organization: None,
        rule: Rule {
            kinds: vec!["RoleBinding".to_owned()],
            verbs: vec![Verb::Propose],
            constraints: Vec::new(),
        },
    })
}

/// What `identity` may do when removing `name` of `kind` from `project`: their permissions there,
/// and at the organization's namespace a binding scoped to one project, or to a context space of
/// it, also goes by whoever may delete `RoleBinding` in that project (S-32, PF-52): the revoke
/// beside `project_binding`'s grant. Approval stays the organization's.
pub fn for_removal(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
) -> Effective {
    let mut effective = crate::permissions::for_request(state, identity, project);
    if project == ORG_NAMESPACE && kind == "RoleBinding" && !effective.may(kind, Verb::Delete) {
        effective
            .grants
            .extend(project_unbinding(state, identity, name));
    }
    effective
}

fn project_unbinding(state: &AppState, identity: &Identity, name: &str) -> Option<Grant> {
    let held = state.mirror.get(ORG_NAMESPACE, "RoleBinding", name)?;
    let project = crate::permissions::binding_project(&state.mirror, &held.spec)?;
    let target = serde_json::to_value(&held).ok()?;
    crate::permissions::for_request(state, identity, &project)
        .check("RoleBinding", Verb::Delete, Some(&target))
        .ok()?;
    Some(Grant {
        role: "delete on RoleBinding".to_owned(),
        binding: format!("delete on RoleBinding in project {project} (PF-52)"),
        scope: format!("project:{project}"),
        space: None,
        organization: None,
        rule: Rule {
            kinds: vec!["RoleBinding".to_owned()],
            verbs: vec![Verb::Read, Verb::Delete],
            constraints: Vec::new(),
        },
    })
}

/// Whether `manifest` equals the stored Group except in `spec.members` (and `status`).
fn only_members_differ(held: &crate::resource::ResourceEnvelope, manifest: &Value) -> bool {
    let without_members = |value: Value| {
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            object.remove("status");
        }
        if let Some(spec) = value.get_mut("spec").and_then(Value::as_object_mut) {
            spec.remove("members");
        }
        value
    };
    let proposed = serde_json::from_value::<crate::resource::ResourceEnvelope>(manifest.clone())
        .and_then(serde_json::to_value);
    match (serde_json::to_value(held), proposed) {
        (Ok(held), Ok(proposed)) => without_members(held) == without_members(proposed),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resource::{ResourceEnvelope, API_VERSION};
    use serde_json::json;
    use std::collections::BTreeSet;

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

    fn meta(name: &str, app: Option<&str>) -> ObjectMeta {
        let mut meta = ObjectMeta::new(name, ORG_NAMESPACE);
        if let Some(app) = app {
            meta.annotations
                .insert(APP_LABEL.to_owned(), app.to_owned());
        }
        meta
    }

    fn manifest(kind: &str, meta: ObjectMeta, spec: serde_json::Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: meta,
            spec,
            status: None,
        }
    }

    /// The org holds `operators`; `board` of `doprava` holds `board-viewer`; the realm holds
    /// `admins` unmanaged. `reader_in` may read Apps in that project.
    fn world(reader_in: Option<&str>) -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        let group = |meta| manifest("Group", meta, json!({ "members": [] }));
        state.mirror.upsert(group(self::meta("operators", None)));
        state
            .mirror
            .upsert(group(self::meta("board-viewer", Some("doprava/board"))));
        if let Some(project) = reader_in {
            state.mirror.upsert(manifest(
                "Role",
                ObjectMeta::new("app-reader", ORG_NAMESPACE),
                json!({ "rules": [{ "kinds": ["App"], "verbs": ["read"] }] }),
            ));
            state.mirror.upsert(manifest(
                "RoleBinding",
                ObjectMeta::new("jana-reads-apps", ORG_NAMESPACE),
                json!({
                    "subjects": [{ "user": "jana@hel.fi" }],
                    "role": "app-reader",
                    "scope": { "project": project },
                }),
            ));
        }
        state
            .foreign_names
            .set_groups(BTreeSet::from(["admins".to_owned()]));
        state
    }

    fn refusal(state: &AppState, kind: &str, meta: &ObjectMeta) -> Option<String> {
        match check(state, &who("jana@hel.fi"), kind, meta) {
            Ok(()) => None,
            Err(ApiError::Denied(message)) => Some(message),
            Err(other) => panic!("a clash is a refusal, not {other:?}"),
        }
    }

    /// AP-115: an App's default group named like the organization's group, or like another
    /// App's, is refused naming both owners.
    #[test]
    fn a_group_of_another_owner_is_refused_naming_both() {
        let state = world(Some("doprava"));
        let message =
            refusal(&state, "Group", &meta("operators", Some("ovzdusie/air"))).expect("a clash");
        assert!(message.contains("belongs to the organization"), "{message}");
        assert!(
            message.contains("the App air of project ovzdusie"),
            "{message}"
        );
        let message = refusal(
            &state,
            "Group",
            &meta("board-viewer", Some("ovzdusie/board")),
        )
        .expect("a clash");
        assert!(
            message.contains("belongs to the App board of project doprava"),
            "{message}"
        );
        assert!(message.contains("AP-115"), "{message}");
    }

    /// PF-59: the other owner's project is named only to a caller who may read Apps there.
    #[test]
    fn a_project_the_caller_cannot_read_is_not_named() {
        let state = world(None);
        let message = refusal(
            &state,
            "Group",
            &meta("board-viewer", Some("ovzdusie/board")),
        )
        .expect("a clash");
        assert!(message.contains("an App of another project"), "{message}");
        assert!(!message.contains("doprava"), "{message}");
    }

    /// AP-115: a new group the realm holds unmanaged is refused; the bootstrap administrators'
    /// group is never taken over.
    #[test]
    fn a_new_group_somebody_else_made_in_the_realm_is_refused() {
        let state = world(None);
        let message = refusal(&state, "Group", &meta("admins", None)).expect("a clash");
        assert!(
            message.contains("'admins' for the organization"),
            "{message}"
        );
        assert!(message.contains("never taken over"), "{message}");
    }

    /// The same group written again under its owner, or by a person editing its members, is an
    /// update; another kind or a free name is no clash.
    #[test]
    fn the_same_owner_a_members_edit_and_a_free_name_pass() {
        let state = world(None);
        assert_eq!(
            refusal(
                &state,
                "Group",
                &meta("board-viewer", Some("doprava/board"))
            ),
            None
        );
        assert_eq!(refusal(&state, "Group", &meta("board-viewer", None)), None);
        assert_eq!(refusal(&state, "Group", &meta("operators", None)), None);
        assert_eq!(
            refusal(&state, "Group", &meta("air-viewer", Some("ovzdusie/air"))),
            None
        );
        assert_eq!(refusal(&state, "Role", &meta("admins", None)), None);
    }

    /// T-6461: `{project}-readers` admits to the project's Endpoints, so a Group manifest cannot
    /// take the name of a project that exists; another ending or an unknown project stay free.
    #[test]
    fn a_group_cannot_take_the_name_of_a_projects_derived_group() {
        let state = world(None);
        state.mirror.upsert(manifest(
            "Project",
            ObjectMeta::new("doprava", ORG_NAMESPACE),
            json!({}),
        ));
        for name in ["doprava-readers", "doprava-writers"] {
            let text = refusal(&state, "Group", &meta(name, None)).expect("refused");
            assert!(text.contains("project doprava's"), "{text}");
        }
        for name in ["doprava-auditors", "ovzdusie-readers", "-readers"] {
            assert_eq!(refusal(&state, "Group", &meta(name, None)), None, "{name}");
        }
    }

    /// `world` with the App `board` of `doprava` and jana bound in `project` to a role with
    /// `rule` (JSON of one rule).
    fn with_role(project: &str, rule: serde_json::Value) -> AppState {
        let state = world(None);
        state.mirror.upsert(manifest(
            "App",
            ObjectMeta::new("board", "doprava"),
            json!({ "class": "ui" }),
        ));
        state.mirror.upsert(manifest(
            "Role",
            ObjectMeta::new("app-role", ORG_NAMESPACE),
            json!({ "rules": [rule] }),
        ));
        state.mirror.upsert(manifest(
            "RoleBinding",
            ObjectMeta::new("jana-app-role", ORG_NAMESPACE),
            json!({
                "subjects": [{ "user": "jana@hel.fi" }],
                "role": "app-role",
                "scope": { "project": project },
            }),
        ));
        state
    }

    fn group_write(name: &str, app: Option<&str>) -> serde_json::Value {
        let mut manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "Group",
            "metadata": { "name": name, "namespace": ORG_NAMESPACE },
            "spec": { "members": [{ "user": "eva@hel.fi" }] },
        });
        if let Some(app) = app {
            manifest["metadata"]["annotations"] = json!({ APP_LABEL: app });
        }
        manifest
    }

    fn may_propose(state: &AppState, update: bool, manifest: &serde_json::Value) -> bool {
        for_proposal(
            state,
            &who("jana@hel.fi"),
            ORG_NAMESPACE,
            "Group",
            update,
            manifest,
        )
        .check("Group", Verb::Propose, Some(manifest))
        .is_ok()
    }

    /// AP-119: whoever may propose the App proposes its default group's members, keeping the
    /// annotation; nothing else of the organization.
    #[test]
    fn the_apps_steward_proposes_its_default_group_and_no_other() {
        let steward = with_role("doprava", json!({ "kinds": ["App"], "verbs": ["propose"] }));
        assert!(may_propose(
            &steward,
            true,
            &group_write("board-viewer", Some("doprava/board"))
        ));
        // An organization group, a group moved or unannotated, a new group: all refused.
        assert!(!may_propose(
            &steward,
            true,
            &group_write("operators", None)
        ));
        assert!(!may_propose(
            &steward,
            true,
            &group_write("operators", Some("doprava/board"))
        ));
        assert!(!may_propose(
            &steward,
            true,
            &group_write("board-viewer", None)
        ));
        assert!(!may_propose(
            &steward,
            true,
            &group_write("board-viewer", Some("doprava/other"))
        ));
        assert!(!may_propose(
            &steward,
            false,
            &group_write("board-viewer", Some("doprava/board"))
        ));
        assert!(!may_propose(
            &steward,
            true,
            &group_write("fresh-viewer", Some("doprava/board"))
        ));
        // Anything beside the members stays the organization's (F-POR4-010).
        let mut described = group_write("board-viewer", Some("doprava/board"));
        described["spec"]["description"] = json!("now also the operators' group");
        assert!(!may_propose(&steward, true, &described));
        // The grant reaches only the organization's namespace, where Groups live.
        assert!(!for_proposal(
            &steward,
            &who("jana@hel.fi"),
            "doprava",
            "Group",
            true,
            &group_write("board-viewer", Some("doprava/board"))
        )
        .may("Group", Verb::Propose));
    }

    /// The steward of another project, a reader of the App and the build lane (whose `propose`
    /// on App writes `status.build` only) propose none of its groups.
    #[test]
    fn nobody_without_propose_on_that_app_proposes_its_group() {
        let write = group_write("board-viewer", Some("doprava/board"));
        for state in [
            with_role(
                "ovzdusie",
                json!({ "kinds": ["App"], "verbs": ["propose"] }),
            ),
            with_role("doprava", json!({ "kinds": ["App"], "verbs": ["read"] })),
            with_role(
                "doprava",
                json!({
                    "kinds": ["App"],
                    "verbs": ["propose"],
                    "constraints": [{ "field": "status.build" }],
                }),
            ),
        ] {
            assert!(!may_propose(&state, true, &write));
        }
    }

    /// AP-119: whoever may read the App reads its default groups, and no other group.
    #[test]
    fn the_apps_reader_reads_its_default_group_and_no_other() {
        let reader = with_role("doprava", json!({ "kinds": ["App"], "verbs": ["read"] }));
        let jana = who("jana@hel.fi");
        assert!(may_read_as_app_group(
            &reader,
            &jana,
            ORG_NAMESPACE,
            "Group",
            "board-viewer"
        ));
        assert!(!may_read_as_app_group(
            &reader,
            &jana,
            ORG_NAMESPACE,
            "Group",
            "operators"
        ));
        assert!(!may_read_as_app_group(
            &reader,
            &jana,
            ORG_NAMESPACE,
            "Role",
            "board-viewer"
        ));
        assert!(!may_read_as_app_group(
            &reader,
            &jana,
            "doprava",
            "Group",
            "board-viewer"
        ));
        let elsewhere = with_role("ovzdusie", json!({ "kinds": ["App"], "verbs": ["read"] }));
        assert!(!may_read_as_app_group(
            &elsewhere,
            &jana,
            ORG_NAMESPACE,
            "Group",
            "board-viewer"
        ));
    }

    fn binding_write(name: &str, scope: serde_json::Value) -> serde_json::Value {
        json!({
            "apiVersion": API_VERSION,
            "kind": "RoleBinding",
            "metadata": { "name": name, "namespace": ORG_NAMESPACE },
            "spec": { "subjects": [{ "user": "eva@hel.fi" }], "role": "app-role", "scope": scope },
        })
    }

    fn may_bind(state: &AppState, manifest: &serde_json::Value) -> bool {
        for_proposal(
            state,
            &who("jana@hel.fi"),
            ORG_NAMESPACE,
            "RoleBinding",
            false,
            manifest,
        )
        .check("RoleBinding", Verb::Propose, Some(manifest))
        .is_ok()
    }

    /// S-27, PF-52: whoever may propose RoleBinding in a project proposes a binding scoped to it or
    /// to one of its spaces; another project, the organization, or rescoping a binding held
    /// elsewhere into the project is refused.
    #[test]
    fn a_project_admin_binds_inside_their_project_and_nowhere_else() {
        let admin = with_role(
            "doprava",
            json!({ "kinds": ["RoleBinding"], "verbs": ["propose"] }),
        );
        admin.mirror.upsert(manifest(
            "ContextSpace",
            ObjectMeta::new("parking", "doprava"),
            json!({}),
        ));
        admin.mirror.upsert(manifest(
            "ContextSpace",
            ObjectMeta::new("air", "ovzdusie"),
            json!({}),
        ));
        admin.mirror.upsert(manifest(
            "RoleBinding",
            ObjectMeta::new("ovzdusie-readers", ORG_NAMESPACE),
            json!({ "subjects": [{ "user": "eva@hel.fi" }], "role": "app-role",
                    "scope": { "project": "ovzdusie" } }),
        ));
        assert!(may_bind(
            &admin,
            &binding_write("eva-doprava", json!({ "project": "doprava" }))
        ));
        assert!(may_bind(
            &admin,
            &binding_write("eva-parking", json!({ "contextSpace": "parking" }))
        ));
        // Its own binding, kept in the project, is updated too.
        assert!(may_bind(
            &admin,
            &binding_write("jana-app-role", json!({ "contextSpace": "parking" }))
        ));
        for refused in [
            binding_write("eva-ovzdusie", json!({ "project": "ovzdusie" })),
            binding_write("eva-air", json!({ "contextSpace": "air" })),
            binding_write("eva-org", json!({ "organization": "hel" })),
            binding_write(
                "eva-both",
                json!({ "organization": "hel", "project": "doprava" }),
            ),
            binding_write("ovzdusie-readers", json!({ "project": "doprava" })),
        ] {
            assert!(!may_bind(&admin, &refused), "{refused}");
        }
        // A reader of the project proposes no binding in it.
        let reader = with_role(
            "doprava",
            json!({ "kinds": ["RoleBinding"], "verbs": ["read"] }),
        );
        assert!(!may_bind(
            &reader,
            &binding_write("eva-doprava", json!({ "project": "doprava" }))
        ));
    }

    /// S-32, PF-52: whoever may delete RoleBinding in a project removes a binding scoped to it or
    /// to one of its spaces; a binding of another project, of the organization or one that is not
    /// there grants nothing, and propose alone removes nothing.
    #[test]
    fn a_project_admin_unbinds_inside_their_project_and_nowhere_else() {
        let may_remove = |state: &AppState, name: &str| {
            for_removal(
                state,
                &who("jana@hel.fi"),
                ORG_NAMESPACE,
                "RoleBinding",
                name,
            )
            .may("RoleBinding", Verb::Delete)
        };
        let admin = with_role(
            "doprava",
            json!({ "kinds": ["RoleBinding"], "verbs": ["delete"] }),
        );
        admin.mirror.upsert(manifest(
            "ContextSpace",
            ObjectMeta::new("parking", "doprava"),
            json!({}),
        ));
        for (name, scope) in [
            ("eva-doprava", json!({ "project": "doprava" })),
            ("eva-parking", json!({ "contextSpace": "parking" })),
            ("eva-ovzdusie", json!({ "project": "ovzdusie" })),
            ("eva-org", json!({ "organization": "hel" })),
        ] {
            admin.mirror.upsert(manifest(
                "RoleBinding",
                ObjectMeta::new(name, ORG_NAMESPACE),
                json!({ "subjects": [{ "user": "eva@hel.fi" }], "role": "app-role", "scope": scope }),
            ));
        }
        assert!(may_remove(&admin, "eva-doprava"));
        assert!(may_remove(&admin, "eva-parking"));
        assert!(!may_remove(&admin, "eva-ovzdusie"));
        assert!(!may_remove(&admin, "eva-org"));
        assert!(!may_remove(&admin, "absent"));
        // Nobody without delete in the project removes it.
        let stranger = who("petra@hel.fi");
        let removal = for_removal(
            &admin,
            &stranger,
            ORG_NAMESPACE,
            "RoleBinding",
            "eva-doprava",
        );
        assert!(!removal.may("RoleBinding", Verb::Delete));
        let proposer = with_role(
            "doprava",
            json!({ "kinds": ["RoleBinding"], "verbs": ["propose"] }),
        );
        proposer.mirror.upsert(manifest(
            "RoleBinding",
            ObjectMeta::new("eva-doprava", ORG_NAMESPACE),
            json!({ "subjects": [{ "user": "eva@hel.fi" }], "role": "app-role",
                    "scope": { "project": "doprava" } }),
        ));
        assert!(!may_remove(&proposer, "eva-doprava"));
    }
}
