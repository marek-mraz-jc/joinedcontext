//! Validates application generation dataNeeds against endpoint projection and user grants (AP-44).

use crate::agents::endpoints::{self, RunEndpoint};
use crate::error::ApiError;
use crate::store::Mirror;

/// The distinct roles `dataNeeds[].roles` name, in first-seen order: what a published run declares
/// in its App's `spec.roles` (AP-91, AP-96).
pub fn declared_roles(data_needs: &[serde_json::Value]) -> Vec<String> {
    let mut roles: Vec<String> = Vec::new();
    for role in data_needs
        .iter()
        .filter_map(|need| need.get("roles").and_then(|r| r.as_array()))
        .flatten()
        .filter_map(|r| r.as_str())
    {
        if !roles.iter().any(|known| known == role) {
            roles.push(role.to_string());
        }
    }
    roles
}

/// `[a-z][a-z0-9-]{0,31}`, the App kind's role name (AP-90).
fn is_role_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every need checked against the endpoints it belongs to (AP-44): those of `run_endpoints` whose
/// context space is the need's, the primary otherwise.
pub fn validate_data_needs(
    mirror: &Mirror,
    project: &str,
    run_endpoints: &[RunEndpoint],
    visibility: &str,
    data_needs: &[serde_json::Value],
) -> Result<bool, ApiError> {
    if visibility == "public" {
        return Err(ApiError::BadRequest(
            "public visibility is refused for agent-generated applications (AP-42)".to_string(),
        ));
    }

    if data_needs.is_empty() {
        return Err(ApiError::BadRequest(
            "dataNeeds must not be empty (AP-44)".to_string(),
        ));
    }

    // The hidden attributes of each endpoint, aligned with `run_endpoints`.
    let mut hidden: Vec<Vec<String>> = Vec::new();
    for endpoint in run_endpoints {
        let envelope = mirror
            .get(project, "Endpoint", &endpoint.name)
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "endpoint '{}' not found in project '{project}'",
                    endpoint.name
                ))
            })?;
        hidden.push(
            envelope
                .spec
                .get("projection")
                .and_then(|p| p.get("hiddenAttributes"))
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        );
    }

    let mut violations = Vec::new();
    let mut allows_write = false;

    for (idx, need) in data_needs.iter().enumerate() {
        // The shape first: `spec.dataNeeds` of the App this run publishes is this value
        // verbatim, so a need the `App` kind cannot parse is a run that can never be published.
        // Failing here names the field while the person is still on the form (AP-44, CC-24).
        if let Err(error) = serde_json::from_value::<jc_core::kinds::DataNeed>(need.clone()) {
            violations.push(format!("dataNeeds[{idx}]: {error}"));
            continue;
        }

        let types = need
            .get("types")
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        if types.is_empty() {
            violations.push(format!(
                "dataNeeds[{idx}].types: at least one type required"
            ));
        }

        let attrs = need
            .get("attrs")
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        // An attribute is out of reach only when every endpoint of the need's space hides it:
        // an operations and a public endpoint of one space each publish their own part.
        let at = endpoints::of_need(run_endpoints, need);
        for attr in attrs {
            let hiding: Vec<&str> = at
                .iter()
                .filter(|&&i| {
                    hidden
                        .get(i)
                        .is_some_and(|names| names.iter().any(|h| h == attr))
                })
                .map(|&i| run_endpoints[i].name.as_str())
                .collect();
            if !hiding.is_empty() && hiding.len() == at.len() {
                violations.push(format!(
                    "attribute '{attr}' is hidden by endpoint '{}'",
                    hiding.join("' and '")
                ));
            }
        }

        // Every write jc-core counts, `appendAttrs`, `deleteAttrs` and the batches included (AP-98).
        allows_write |= crate::agents::held::writes(std::slice::from_ref(need));
    }

    // Publishing declares every role the needs name in `spec.roles` (AP-91, AP-96), so a name the
    // App kind would refuse fails here, while the person is still on the form.
    let roles = declared_roles(data_needs);
    for role in &roles {
        if !is_role_name(role) {
            violations.push(format!(
                "dataNeeds[].roles: '{role}' is not a role name ([a-z][a-z0-9-]{{0,31}}, AP-90)"
            ));
        }
    }
    if roles.len() > jc_core::kinds::app::MAX_APP_ROLES {
        violations.push(format!(
            "dataNeeds[].roles: {} distinct roles, an app declares at most {} (AP-90)",
            roles.len(),
            jc_core::kinds::app::MAX_APP_ROLES
        ));
    }

    if !violations.is_empty() {
        return Err(ApiError::Invalid {
            detail: "declared dataNeeds exceed what endpoint publishes (AP-44)".to_string(),
            errors: violations,
        });
    }

    Ok(allows_write)
}

#[cfg(test)]
mod tests {
    use super::{declared_roles, is_role_name};

    #[test]
    fn the_roles_needs_name_are_declared_once_in_first_seen_order() {
        let needs = [
            serde_json::json!({ "types": ["Alert"], "roles": ["steward"] }),
            serde_json::json!({ "types": ["Alert"] }),
            serde_json::json!({ "types": ["Alert"], "roles": ["editor", "steward"] }),
        ];
        assert_eq!(declared_roles(&needs), ["steward", "editor"]);
        assert!(declared_roles(&[]).is_empty());
    }

    #[test]
    fn a_role_name_is_the_app_kinds_pattern() {
        for name in ["steward", "a", "data-editor-2", &"a".repeat(32)] {
            assert!(is_role_name(name), "{name}");
        }
        for name in [
            "",
            "Steward",
            "2x",
            "-x",
            "a_b",
            "endpoint:x",
            &"a".repeat(33),
        ] {
            assert!(!is_role_name(name), "{name}");
        }
    }
}
