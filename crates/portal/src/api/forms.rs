//! `GET /api/v1/forms`: the `kind: UiSchema` manifests that arrange the Portal's forms
//! (T-0452, UI-01, UI-02, CC-31).
//!
//! One request for the whole of `portal/forms/`, not one per kind: a form dialog opens on a
//! click, the directory is a few kilobytes, and a per-kind fetch would put a round trip in front
//! of every dialog. The manifests are organization-scoped and already in the mirror like every
//! other kind (T-0451), so this is a filter over the store rather than a second way of reading
//! the configuration repository.
//!
//! `GET /api/v1/kinds/{kind}/schema` beside it: the kind's draft-07 schema as jc-core exports it
//! (MF-09), which the YAML view reads for the field descriptions it shows on hover (T-5052).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::api::blueprints::ORG_NAMESPACE;
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::API_VERSION;
use crate::state::AppState;
use crate::store::ListOptions;

/// The kind whose manifests arrange a form.
const UI_SCHEMA_KIND: &str = "UiSchema";

#[utoipa::path(
    get,
    path = "/api/v1/forms",
    summary = "List Forms",
    description = "Every UiSchema manifest the Portal arranges its forms by. Any signed-in caller may read them; they hold layout, never data.",
    tag = "forms",
    responses(
        (status = 200, description = "Every UiSchema manifest in portal/forms/", body = ResourceList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_forms(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ResourceList>, ApiError> {
    // Unpaged on purpose: an organisation arranges tens of kinds, not thousands, and a form
    // arrangement the caller did not receive is a dialog rendered wrong with no way to know it.
    let page = state
        .mirror
        .list(ORG_NAMESPACE, UI_SCHEMA_KIND, &ListOptions::default());

    Ok(Json(ResourceList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        metadata: ListMeta {
            continue_token: None,
            remaining_item_count: None,
        },
        items: page.items,
        next_cursor: None,
        total_count: None,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/kinds/{kind}/schema",
    summary = "Get Kind Schema",
    description = "The draft-07 JSON Schema of a manifest kind, as jc-core exports it (MF-09). Any signed-in caller may read it; it describes fields, never data. An unknown kind is 404.",
    tag = "forms",
    params(("kind" = String, Path, description = "The manifest kind, e.g. `Policy`")),
    responses(
        (status = 200, description = "The kind's JSON Schema", body = Object),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such kind", body = ProblemDetails)
    )
)]
pub async fn kind_schema(
    _user: CurrentUser,
    Path(kind): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    kind_schema_of(&kind).map(Json)
}

fn kind_schema_of(kind: &str) -> Result<serde_json::Value, ApiError> {
    jc_core::registry::schema_of(kind)
        .ok_or_else(|| ApiError::NotFound(format!("the platform has no kind named `{kind}`")))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/forms", get(list_forms))
        .route("/kinds/{kind}/schema", get(kind_schema))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_answers_its_schema_and_an_unknown_one_is_404() {
        let schema = kind_schema_of("Policy").expect("Policy");
        assert!(schema["properties"]["spec"].is_object(), "{schema}");
        assert!(matches!(kind_schema_of("Nope"), Err(ApiError::NotFound(_))));
    }
}
