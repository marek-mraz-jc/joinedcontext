//! Serving the apps a project builds on the platform (AP-12, AP-14, AP-17).

/// An App's mail to people of the organization (AP-168).
pub mod ai;
/// The database of the server WASM Apps: a schema, roles and migrations per App (AP-149).
pub mod apps_db;
/// A build pod per App and its own build cache (AP-130, AP-131).
pub mod build_pods;
/// A lane's `status.build`, checked against the forge and published (AP-101, AP-104).
pub mod built;
/// The reconciler that compiles an App into its runtime and its grants (T-0227).
pub mod converge;
/// The default group of every App role, committed with the role (AP-118).
pub mod default_groups;
pub mod email;
pub mod fetch;
pub mod files;
pub mod functions;
/// The names of the Organization's own domain an App also answers on (AP-172).
pub mod hostnames;
pub mod jobs;
pub mod kube;
/// An App name is unique in the organization (AP-14a).
pub mod names;
/// A project's own namespace for its pod-backed Apps (AP-116).
pub mod project_namespace;
pub mod reconciler;
/// A person's roles in an application (ADR-N-027).
pub mod roles;
/// The platform services an App calls: layers, quotas, refusals (ADR-N-045).
pub mod services;
pub mod static_host;
pub mod template_demos;
pub mod wasm_apps;

/// The warning an App still written with a shape name of the previous release gets (AP-124):
/// `static` and `fullstack` are read as `ui` and `ui-rust` for one release, so the manifest passes,
/// and the person is told what to write before the release that refuses them.
pub fn renamed_shape(manifest: &serde_json::Value) -> Option<crate::ops::verdict::Finding> {
    if manifest.get("kind").and_then(serde_json::Value::as_str) != Some("App") {
        return None;
    }
    let written = manifest.pointer("/spec/kind")?.as_str()?;
    let (old, new) = jc_core::kinds::AppClass::renamed(written)?;
    Some(crate::ops::verdict::Finding {
        level: crate::ops::verdict::Level::Warning,
        path: "spec.kind".to_owned(),
        message: format!(
            "`{old}` is read as `{new}` for this release only and refused by the next: write \
             `kind: {new}` (AP-124)"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::renamed_shape;
    use serde_json::json;

    #[test]
    fn an_app_written_with_an_old_shape_name_is_told_the_new_one() {
        for (old, new) in [("static", "ui"), ("fullstack", "ui-rust")] {
            let finding = renamed_shape(&json!({ "kind": "App", "spec": { "kind": old } }))
                .expect("a warning");
            assert_eq!(finding.level, crate::ops::verdict::Level::Warning);
            assert_eq!(finding.path, "spec.kind");
            assert!(
                finding.message.contains(&format!("write `kind: {new}`")),
                "{}",
                finding.message
            );
        }
    }

    #[test]
    fn a_new_name_another_kind_or_no_shape_gets_no_warning() {
        for manifest in [
            json!({ "kind": "App", "spec": { "kind": "ui" } }),
            json!({ "kind": "App", "spec": { "kind": "ui-rust" } }),
            json!({ "kind": "App", "spec": {} }),
            json!({ "kind": "App" }),
            json!({ "kind": "Dashboard", "spec": { "kind": "static" } }),
            json!({ "kind": "App", "spec": { "kind": 3 } }),
        ] {
            assert_eq!(renamed_shape(&manifest), None, "{manifest}");
        }
    }
}
