//! Every role of an App has a default group, `{app}-{role}`, committed with the role (AP-118,
//! ADR-N-031 §3.5).
//!
//! Any door that writes an App (propose, generation, import) asks `plan` what the same Change
//! must carry besides the App: the `Group` of each new role, annotated
//! `joinedcontext.com/app: {project}/{app}` and empty, ready to assign, and the `spec.access`
//! entry giving the role to it. A role taken away, or the App retired, takes its group with it,
//! with a warning when the group still has members. A default group holds nothing but its App
//! role: it is named in no `RoleBinding`, so it never grants a platform role.

use jc_core::kinds::AppLifecycle;
use jc_core::ObjectMeta;
use serde_json::{json, Value};

use super::reconciler::APP_LABEL;
use crate::auth::Identity;
use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::{ResourceEnvelope, API_VERSION};
use crate::state::AppState;

/// What the App's Change carries for its default groups.
#[derive(Debug, Default)]
pub struct DefaultGroups {
    /// New groups, empty, one per role that has none yet.
    pub written: Vec<ResourceEnvelope>,
    /// This App's groups whose role is gone, or all of them for a retired App.
    pub removed: Vec<ResourceEnvelope>,
    /// One sentence per removed group that still has members: they lose the role with it.
    pub warnings: Vec<String>,
}

/// The default group of `role` of App `app`.
pub fn group_name(app: &str, role: &str) -> String {
    format!("{app}-{role}")
}

/// Adds the access entry of each role's default group to `app` and says which groups the Change
/// writes and removes; a door that removes them takes them out of the access with [`release`]. A default group name another owner holds, or the realm holds unmanaged,
/// is refused naming both owners (AP-115); one longer than a name may be is refused naming it.
pub fn plan(
    state: &AppState,
    identity: &Identity,
    project: &str,
    app: &mut ResourceEnvelope,
) -> Result<DefaultGroups, ApiError> {
    let name = app.metadata.name.clone();
    let owner = format!("{project}/{name}");
    let retired =
        app.spec.get("lifecycle").and_then(Value::as_str) == Some(AppLifecycle::Retired.as_str());
    let roles: Vec<String> = if retired {
        Vec::new()
    } else {
        app.spec
            .get("roles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|role| role.get("name").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    };

    let mut out = DefaultGroups::default();
    let wanted: Vec<String> = roles.iter().map(|role| group_name(&name, role)).collect();
    for (role, group) in roles.iter().zip(&wanted) {
        jc_core::names::validate_dns1123_label(group).map_err(|_| {
            ApiError::BadRequest(format!(
                "the default group of role '{role}' would be named '{group}', longer than the 63 \
                 characters a name may have (AP-118); shorten the App or the role name"
            ))
        })?;
        let mut meta = ObjectMeta::new(group, ORG_NAMESPACE);
        meta.annotations.insert(APP_LABEL.to_owned(), owner.clone());
        crate::groups::check(state, identity, "Group", &meta)?;
        if state.mirror.get(ORG_NAMESPACE, "Group", group).is_none() {
            out.written.push(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: "Group".to_owned(),
                metadata: meta,
                spec: json!({ "members": [] }),
                status: None,
            });
        }
        give(&mut app.spec, role, group);
    }

    for held in state.mirror.matching(|env| {
        env.kind == "Group"
            && env.metadata.annotations.get(APP_LABEL) == Some(&owner)
            && !wanted.contains(&env.metadata.name)
    }) {
        let members = held
            .spec
            .get("members")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if members > 0 {
            out.warnings.push(format!(
                "the group '{}' is removed with its role and still has {members} member(s); \
                 they lose that role in {name}",
                held.metadata.name
            ));
        }
        out.removed.push(held);
    }
    Ok(out)
}

/// Gives `role` to `group` in `spec.access`, unless an entry already does.
fn give(spec: &mut Value, role: &str, group: &str) {
    let subject = json!({ "group": group });
    let Some(object) = spec.as_object_mut() else {
        return;
    };
    let access = object.entry("access").or_insert_with(|| json!([]));
    let Some(entries) = access.as_array_mut() else {
        return;
    };
    match entries
        .iter_mut()
        .find(|entry| entry.get("role").and_then(Value::as_str) == Some(role))
    {
        Some(entry) => {
            let subjects = entry
                .as_object_mut()
                .map(|entry| entry.entry("subjects").or_insert_with(|| json!([])));
            if let Some(Value::Array(subjects)) = subjects {
                if !subjects.contains(&subject) {
                    subjects.push(subject);
                }
            }
        }
        None => entries.push(json!({ "role": role, "subjects": [subject] })),
    }
}

/// Takes every `removed` group out of the App's `spec.access`, so the Change that removes them
/// names none of them; an entry left naming nobody goes with it.
pub fn release(app: &mut ResourceEnvelope, removed: &[ResourceEnvelope]) {
    let gone: Vec<Value> = removed
        .iter()
        .map(|group| json!({ "group": group.metadata.name }))
        .collect();
    let Some(entries) = app.spec.get_mut("access").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in entries.iter_mut() {
        if let Some(subjects) = entry.get_mut("subjects").and_then(Value::as_array_mut) {
            subjects.retain(|held| !gone.contains(held));
        }
    }
    entries.retain(|entry| {
        entry
            .get("subjects")
            .and_then(Value::as_array)
            .is_some_and(|subjects| !subjects.is_empty())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn who() -> Identity {
        Identity {
            client: None,
            subject: "f:1:jana".into(),
            username: "jana".into(),
            email: Some("jana@hel.fi".into()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        }
    }

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    fn group(name: &str, owner: Option<&str>, members: &[&str]) -> ResourceEnvelope {
        let mut group = envelope(
            "Group",
            name,
            ORG_NAMESPACE,
            json!({ "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>() }),
        );
        if let Some(owner) = owner {
            group
                .metadata
                .annotations
                .insert(APP_LABEL.to_owned(), owner.to_owned());
        }
        group
    }

    fn alerts(roles: &[&str], access: Value, lifecycle: &str) -> ResourceEnvelope {
        envelope(
            "App",
            "alerts",
            "helsinki",
            json!({
                "lifecycle": lifecycle,
                "roles": roles.iter().map(|r| json!({ "name": r })).collect::<Vec<_>>(),
                "access": access,
            }),
        )
    }

    fn names(envelopes: &[ResourceEnvelope]) -> Vec<&str> {
        envelopes.iter().map(|e| e.metadata.name.as_str()).collect()
    }

    /// AP-118: each new role gets an empty group annotated with the App, and the access entry
    /// giving it the role, beside whoever the author named already.
    #[test]
    fn each_new_role_gets_its_group_and_access_entry() {
        let state = AppState::new(Config::for_tests(), None);
        let mut app = alerts(
            &["viewer", "steward"],
            json!([{ "role": "steward", "subjects": [{ "user": "jana@hel.fi" }] }]),
            "published",
        );
        let planned = plan(&state, &who(), "helsinki", &mut app).expect("a plan");
        assert_eq!(names(&planned.written), ["alerts-viewer", "alerts-steward"]);
        assert!(planned.removed.is_empty() && planned.warnings.is_empty());
        let group = &planned.written[0];
        assert_eq!(group.metadata.annotations[APP_LABEL], "helsinki/alerts");
        assert_eq!(group.spec, json!({ "members": [] }));
        assert_eq!(
            app.spec["access"],
            json!([
                { "role": "steward", "subjects": [{ "user": "jana@hel.fi" }, { "group": "alerts-steward" }] },
                { "role": "viewer", "subjects": [{ "group": "alerts-viewer" }] },
            ])
        );
    }

    /// Writing the App again changes nothing: the groups exist, the entries are there, and the
    /// members somebody added stay.
    #[test]
    fn a_second_write_is_a_no_op() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(group(
            "alerts-viewer",
            Some("helsinki/alerts"),
            &["ada@hel.fi"],
        ));
        let access = json!([{ "role": "viewer", "subjects": [{ "group": "alerts-viewer" }] }]);
        let mut app = alerts(&["viewer"], access.clone(), "published");
        let planned = plan(&state, &who(), "helsinki", &mut app).expect("a plan");
        assert!(planned.written.is_empty() && planned.removed.is_empty());
        assert_eq!(app.spec["access"], access);
    }

    /// AP-118: a role taken away takes its group and the access entry left naming nobody, and a
    /// group with members warns; retiring the App removes every group of it.
    #[test]
    fn a_removed_role_or_a_retired_app_takes_its_groups_with_a_warning() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(group(
            "alerts-viewer",
            Some("helsinki/alerts"),
            &["ada@hel.fi", "bo@hel.fi"],
        ));
        state
            .mirror
            .upsert(group("alerts-steward", Some("helsinki/alerts"), &[]));
        let access = json!([
            { "role": "viewer", "subjects": [{ "group": "alerts-viewer" }] },
            { "role": "steward", "subjects": [{ "group": "alerts-steward" }] },
        ]);

        let mut app = alerts(&["steward"], access.clone(), "published");
        let planned = plan(&state, &who(), "helsinki", &mut app).expect("a plan");
        release(&mut app, &planned.removed);
        assert_eq!(names(&planned.removed), ["alerts-viewer"]);
        assert_eq!(planned.warnings.len(), 1, "{:?}", planned.warnings);
        assert!(
            planned.warnings[0].contains("still has 2 member(s)"),
            "{:?}",
            planned.warnings
        );
        assert_eq!(
            app.spec["access"],
            json!([{ "role": "steward", "subjects": [{ "group": "alerts-steward" }] }]),
            "the Change stays consistent: no entry names a group it removes"
        );

        let mut retired = alerts(&["viewer", "steward"], access, "retired");
        let planned = plan(&state, &who(), "helsinki", &mut retired).expect("a plan");
        release(&mut retired, &planned.removed);
        let mut removed = names(&planned.removed);
        removed.sort_unstable();
        assert_eq!(removed, ["alerts-steward", "alerts-viewer"]);
        assert!(planned.written.is_empty());
        assert_eq!(retired.spec["access"], json!([]));
    }

    /// AP-115: a default group named like the organization's own group, another App's, or an
    /// unmanaged realm group is refused naming both owners; no group is taken over.
    #[test]
    fn a_default_group_name_another_owner_holds_is_refused() {
        let state = AppState::new(Config::for_tests(), None);
        state
            .mirror
            .upsert(group("alerts-viewer", None, &["ada@hel.fi"]));
        let mut app = alerts(&["viewer"], json!([]), "published");
        let refused = plan(&state, &who(), "helsinki", &mut app).expect_err("a clash");
        let ApiError::Denied(message) = refused else {
            panic!("a clash is a refusal: {refused:?}");
        };
        assert!(message.contains("belongs to the organization"), "{message}");
        assert!(
            message.contains("the App alerts of project helsinki"),
            "{message}"
        );

        let state = AppState::new(Config::for_tests(), None);
        state
            .foreign_names
            .set_groups(std::collections::BTreeSet::from([
                "alerts-viewer".to_owned()
            ]));
        let mut app = alerts(&["viewer"], json!([]), "published");
        assert!(matches!(
            plan(&state, &who(), "helsinki", &mut app),
            Err(ApiError::Denied(_))
        ));
    }

    /// A name longer than a Group name may be is refused before anything is written.
    #[test]
    fn a_default_group_name_too_long_is_refused_naming_it() {
        let state = AppState::new(Config::for_tests(), None);
        let role = "r".repeat(60);
        let mut app = alerts(&[role.as_str()], json!([]), "published");
        let refused = plan(&state, &who(), "helsinki", &mut app).expect_err("too long");
        assert!(
            matches!(&refused, ApiError::BadRequest(m) if m.contains("63 characters")),
            "{refused:?}"
        );
    }
}
