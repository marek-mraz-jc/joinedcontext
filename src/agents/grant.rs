//! `grant_role` (AG-77, PF-52): the conversation gives people or a group a role of the organization
//! on a scope. The Portal names the RoleBinding and checks it like every other proposal, which
//! holds it to what the person holds there; the Access page's grant form opens with it and the
//! person proposes. Nothing is proposed here.

use jc_core::kinds::{BindingValidity, Subject};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::change::call_of;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::API_VERSION;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRole {
    #[serde(default)]
    pub subjects: Vec<Subject>,
    #[serde(default)]
    pub role: String,
    /// One of `organization`, `project` or `contextSpace`, as in the manifest.
    #[serde(default)]
    pub scope: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity: Option<BindingValidity>,
}

/// The `grant_role` call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<GrantRole, String>> {
    call_of(answer, "grant_role")
}

/// A DNS-1123 label of at most 63 characters from free text.
fn label(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_ascii_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && (out.is_empty() || out.ends_with('-'))) {
            out.push(c);
        }
    }
    out.truncate(63);
    out.trim_end_matches('-').to_owned()
}

/// The RoleBinding the call asks for, named after its first subject, its role and its scope. An
/// organization scope carries the organization's name, the first label of its domain.
pub fn manifest(grant: &GrantRole, org_domain: &str) -> Result<Value, String> {
    let Some(first) = grant.subjects.first() else {
        return Err("name at least one subject, a user (their Keycloak user id) or a group".into());
    };
    // A person's id names the binding by its first block, as the grant form does (F-UI1-009).
    let who = first
        .user
        .as_deref()
        .map(|user| user.split(['@', '-']).next().unwrap_or(user))
        .or(first.group.as_deref())
        .unwrap_or_default();
    let role = grant.role.trim();
    if role.is_empty() {
        return Err("name the role to grant".into());
    }
    let organization = org_domain.split('.').next().unwrap_or(org_domain);
    let (scope, place) = match (
        grant.scope.get("organization"),
        grant.scope.get("project").and_then(Value::as_str),
        grant.scope.get("contextSpace").and_then(Value::as_str),
    ) {
        (Some(_), None, None) => (json!({ "organization": organization }), organization),
        (None, Some(project), None) => (json!({ "project": project }), project),
        (None, None, Some(space)) => (json!({ "contextSpace": space }), space),
        _ => {
            return Err(
                "the scope is exactly one of {\"organization\": true}, {\"project\": \"<name>\"} or {\"contextSpace\": \"<name>\"}".into(),
            )
        }
    };
    let mut spec = json!({ "subjects": grant.subjects, "role": role, "scope": scope });
    if let Some(validity) = &grant.validity {
        spec["validity"] = json!(validity);
    }
    Ok(json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": label(&format!("{who}-{role}-{place}")), "namespace": ORG_NAMESPACE },
        "spec": spec,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JANA: &str = "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14";

    fn grant(scope: Value) -> GrantRole {
        GrantRole {
            subjects: vec![Subject {
                user: Some(JANA.into()),
                group: None,
                ..Subject::default()
            }],
            role: "steward".into(),
            scope,
            validity: None,
        }
    }

    #[test]
    fn the_binding_is_named_after_who_what_and_where() {
        let binding =
            manifest(&grant(json!({ "project": "helsinki" })), "hel.fi").expect("binding");
        assert_eq!(binding["metadata"]["name"], "3f2a9c1e-steward-helsinki");
        assert_eq!(binding["metadata"]["namespace"], "org");
        assert_eq!(binding["spec"]["scope"], json!({ "project": "helsinki" }));
        assert_eq!(binding["spec"]["subjects"], json!([{ "user": JANA }]));

        let organization =
            manifest(&grant(json!({ "organization": true })), "hel.fi").expect("binding");
        assert_eq!(
            organization["spec"]["scope"],
            json!({ "organization": "hel" })
        );
        assert_eq!(organization["metadata"]["name"], "3f2a9c1e-steward-hel");
    }

    #[test]
    fn a_call_without_a_subject_a_role_or_one_scope_is_answered_with_what_is_missing() {
        let mut nobody = grant(json!({ "project": "helsinki" }));
        nobody.subjects.clear();
        assert!(manifest(&nobody, "hel.fi").is_err_and(|e| e.contains("subject")));
        let mut no_role = grant(json!({ "project": "helsinki" }));
        no_role.role = " ".into();
        assert!(manifest(&no_role, "hel.fi").is_err_and(|e| e.contains("role")));
        let two = grant(json!({ "project": "helsinki", "contextSpace": "bikes" }));
        assert!(manifest(&two, "hel.fi").is_err_and(|e| e.contains("exactly one")));
    }

    #[test]
    fn the_call_is_read_from_its_fence() {
        let answer = "Granting it.\n\n```json\n{\"tool\":\"grant_role\",\"subjects\":[{\"user\":\"3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14\"}],\"role\":\"steward\",\"scope\":{\"project\":\"helsinki\"}}\n```\n";
        let call = tool_call(answer).expect("a call").expect("parses");
        assert_eq!(call.role, "steward");
        assert!(tool_call("```json\n{\"tool\":\"change_resource\"}\n```").is_none());
    }
}
