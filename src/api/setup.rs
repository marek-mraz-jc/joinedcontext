//! What a new organization still lacks (T-2748, API/01 §25, PF-90, UI-82).
//!
//! One read for the setup page: each step is judged from what the Portal already holds (the
//! mirror, the domain verification, the realm's people) and each operator item from the branding
//! block and the deployment's own statements. Nothing here writes: every step links to the page
//! that proposes that change the normal way.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::branding::Branding;
use crate::config::SetupStatements;
use crate::domain_verification::State as Verification;
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

/// One step or operator item and whether it is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SetupItem {
    pub id: &'static str,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SetupState {
    pub complete: bool,
    pub steps: Vec<SetupItem>,
    pub operator: Vec<SetupItem>,
}

/// The themes Keycloak ships: a realm on one of them has not been given the platform's pages.
const STOCK_THEMES: &[&str] = &["keycloak", "keycloak.v2", "base"];

fn item(id: &'static str, done: bool) -> SetupItem {
    SetupItem { id, done }
}

fn has_text(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

/// The steps an Organization Administrator takes, judged from the mirror and `people`, whether
/// the realm holds a second person.
pub fn steps(state: &AppState, people: bool) -> Vec<SetupItem> {
    let organization: Option<ResourceEnvelope> = state
        .mirror
        .matching(|envelope| {
            envelope.kind == "Organization"
                && envelope.metadata.namespace.as_deref() == Some(ORG_NAMESPACE)
        })
        .into_iter()
        .next();
    let spec = organization.as_ref().map(|o| &o.spec);
    let named = spec.is_some_and(|spec| {
        has_text(spec.get("domain"))
            && spec
                .get("locales")
                .and_then(Value::as_array)
                .is_some_and(|locales| !locales.is_empty())
    });
    let verified = organization
        .as_ref()
        .and_then(|o| o.status.as_ref())
        .and_then(|status| status.domain_verification.as_ref())
        .is_some_and(|v| v.state == Verification::Verified);
    let policies = spec
        .and_then(|spec| spec.get("projects"))
        .and_then(Value::as_object)
        .is_some_and(|projects| !projects.is_empty());
    let project = !state
        .mirror
        .matching(|envelope| {
            envelope.kind == "ContextSpace"
                && envelope
                    .spec
                    .get("dataModelRef")
                    .is_some_and(|reference| has_text(reference.get("name")))
        })
        .is_empty();
    let publishers = !state
        .mirror
        .matching(|envelope| envelope.kind == "CkanInstance")
        .is_empty();
    vec![
        item("organization", named),
        item("domain", verified),
        item("people", people),
        item("project", project),
        item("publishers", publishers),
        item("policies", policies),
    ]
}

/// What the installation provides: the branding block and the deployment's statements.
pub fn operator(branding: &Branding, said: &SetupStatements) -> Vec<SetupItem> {
    let named = branding.instance_name.trim() != Branding::default().instance_name
        || !branding.logo.trim().is_empty();
    let themed = said
        .login_theme
        .as_deref()
        .is_some_and(|theme| !STOCK_THEMES.contains(&theme));
    vec![
        item("branding", named),
        item("loginTheme", themed),
        item("smtp", said.smtp),
        item("backups", said.backups),
    ]
}

/// Whether the realm holds a person besides the one asking. Without an admin client, or when the
/// realm does not answer, it is not done: the page says so rather than failing.
async fn a_second_person(state: &AppState) -> bool {
    let Some(people) = state.people.as_deref() else {
        return false;
    };
    let Ok(admin) = people.admin().await else {
        return false;
    };
    admin
        .list("", 0, 2)
        .await
        .is_ok_and(|users| users.len() >= 2)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/setup",
    summary = "Get Organization Setup",
    description = "What the organization still lacks: each setup step and the installation's own part, done or not. Needs `approve` on Organization at organization scope, as `org-admin` holds it.",
    tag = "organization",
    responses(
        (status = 200, description = "The steps and the operator's part", body = SetupState),
        (status = 403, description = "Not an administrator of the organization (PF-03)", body = ProblemDetails),
    )
)]
pub async fn get_setup(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<SetupState>, ApiError> {
    crate::permissions::require_organization_admin(
        &state,
        &user.0.identity,
        "the organization's setup",
    )?;
    let steps = steps(&state, a_second_person(&state).await);
    let operator = operator(
        &Branding::load(state.config.branding_file.as_deref()),
        &state.config.setup,
    );
    let complete = steps.iter().chain(&operator).all(|item| item.done);
    Ok(Json(SetupState {
        complete,
        steps,
        operator,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/setup", get(get_setup))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operator_part_is_done_only_where_it_was_said() {
        let stock = operator(&Branding::default(), &SetupStatements::default());
        assert!(stock.iter().all(|item| !item.done), "{stock:?}");

        let branding = Branding {
            instance_name: "Helsinki data".into(),
            ..Branding::default()
        };
        let said = SetupStatements {
            login_theme: Some("joinedcontext".into()),
            smtp: true,
            backups: false,
        };
        assert_eq!(
            operator(&branding, &said),
            vec![
                item("branding", true),
                item("loginTheme", true),
                item("smtp", true),
                item("backups", false),
            ]
        );
        let stock_theme = SetupStatements {
            login_theme: Some("keycloak.v2".into()),
            ..SetupStatements::default()
        };
        assert!(!operator(&branding, &stock_theme)[1].done);
    }
}
