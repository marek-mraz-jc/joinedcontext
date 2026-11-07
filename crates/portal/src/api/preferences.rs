//! `GET`/`PUT /api/v1/preferences`: how the signed-in person likes the UI (UI-09, UI-10).
//!
//! Scoped by the session's Keycloak `sub` (CC-40) and nothing else: there is no path parameter,
//! so no caller can name another person's row.

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::db;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The browser's saved dashboard state, per dashboard name. Opaque to the Portal, so it is capped
/// rather than understood: a saved layout is small, anything bigger is not a layout.
const MAX_LAYOUTS_BYTES: usize = 64 * 1024;
/// The pages a person opened last, newest first (UI-90).
const MAX_RECENT: usize = 10;
/// The pages a person starred (UI-90).
const MAX_FAVOURITES: usize = 50;

/// A page of the Portal a person went to or starred: where it is and what it was called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// A path inside the Portal, with its query: `/projects/helsinki/pipelines/air?tab=runs`.
    pub path: String,
    /// The page's title when it was opened, 1 to 200 characters.
    pub title: String,
}

impl Place {
    fn validate(&self) -> Result<(), String> {
        let path = &self.path;
        // Only an address of this Portal: never another origin (`//host`, `https:`), never a
        // script, never a control character a link could smuggle.
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('\\')
            || path.len() > 512
            || path.chars().any(char::is_control)
        {
            return Err(format!("place path '{path}' is not a path of this Portal"));
        }
        let title_len = self.title.chars().count();
        if title_len == 0 || title_len > 200 || self.title.chars().any(char::is_control) {
            return Err("a place's title is 1 to 200 characters of text".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Preferences {
    /// `light`, `dark` or `system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// ISO 639-1 language code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// The project the shell opens on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<String>,
    /// Free JSON per dashboard name, the browser's own saved state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub dashboard_layouts: BTreeMap<String, serde_json::Value>,
    /// Whether manifest forms show the fields a `UiSchema` marks `advanced` (CC-29, UI-02).
    ///
    /// Absent means off, so a person who has never chosen gets the view CC-29 asks for. It is a
    /// display preference: it changes what a form shows, never what a write may do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced_mode: Option<bool>,
    /// Whether the person put the first-run checklist away (T-3233). Absent shows it; the help
    /// menu clears it. The steps tick themselves from the project, never from this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_run_dismissed: Option<bool>,
    /// The last pages the person opened, newest first, at most 10 (UI-90).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<Place>,
    /// The pages the person starred, at most 50 (UI-90).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favourites: Vec<Place>,
}

impl Preferences {
    /// The few fields the Portal understands are checked; the rest is only bounded.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(theme) = &self.theme {
            if !matches!(theme.as_str(), "light" | "dark" | "system") {
                return Err(format!("theme '{theme}' is not light, dark or system"));
            }
        }
        if let Some(locale) = &self.locale {
            jc_core::names::validate_locale(locale).map_err(|e| e.to_string())?;
        }
        if let Some(project) = &self.default_project {
            if !is_dns1123(project) {
                return Err(format!("defaultProject '{project}' is not a DNS-1123 name"));
            }
        }
        for name in self.dashboard_layouts.keys() {
            if !is_dns1123(name) {
                return Err(format!(
                    "dashboardLayouts key '{name}' is not a dashboard name"
                ));
            }
        }
        if self.recent.len() > MAX_RECENT {
            return Err(format!(
                "recent holds {} places, the limit is {MAX_RECENT}",
                self.recent.len()
            ));
        }
        if self.favourites.len() > MAX_FAVOURITES {
            return Err(format!(
                "favourites holds {} places, the limit is {MAX_FAVOURITES}",
                self.favourites.len()
            ));
        }
        for place in self.recent.iter().chain(&self.favourites) {
            place.validate()?;
        }
        let layouts_len = serde_json::to_vec(&self.dashboard_layouts)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if layouts_len > MAX_LAYOUTS_BYTES {
            return Err(format!(
                "dashboardLayouts is {layouts_len} bytes, the limit is {MAX_LAYOUTS_BYTES}"
            ));
        }
        Ok(())
    }
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state
        .db
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("no preferences database is configured".into()))
}

fn db_error(err: sqlx::Error) -> ApiError {
    // The DSN, the SQL and the driver's wording stay in the log (TS-09).
    tracing::error!(error = %err, "preferences database call failed");
    ApiError::from_db(&err, "preferences")
}

/// The person's stored row, its old connectors moved out first.
async fn stored_row(
    state: &AppState,
    identity: &Identity,
) -> Result<Option<serde_json::Value>, ApiError> {
    let pool = pool(state)?;
    // Every page load asks this and the pool holds five connections: only a row with old
    // connectors pays a second read (T-6520).
    let stored = db::load_preferences(pool, &identity.subject)
        .await
        .map_err(db_error)?;
    if !stored
        .as_ref()
        .is_some_and(crate::personal_connectors::holds_old)
    {
        return Ok(stored);
    }
    crate::personal_connectors::move_one(state, identity).await?;
    db::load_preferences(pool, &identity.subject)
        .await
        .map_err(db_error)
}

#[utoipa::path(
    get,
    path = "/api/v1/preferences",
    summary = "Read Preferences",
    description = "The caller's own Portal preferences, empty before the first save. Nobody reads another person's.",
    tag = "preferences",
    responses(
        (status = 200, description = "The caller's preferences, empty before the first save", body = Preferences),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No preferences database configured", body = ProblemDetails)
    )
)]
pub async fn get_preferences(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Preferences>, ApiError> {
    let stored = stored_row(&state, &user.0.identity).await?;
    // A row written by an older Portal may hold a field this one no longer knows; it is dropped
    // rather than turned into a 500 on the caller's own settings.
    // The keys only the Portal keeps (old connectors) are hidden from the person's own view.
    let preferences = stored
        .map(crate::personal_connectors::without_held)
        .and_then(|value| serde_json::from_value::<Preferences>(value).ok())
        .unwrap_or_default();
    Ok(Json(preferences))
}

#[utoipa::path(
    put,
    path = "/api/v1/preferences",
    summary = "Save Preferences",
    description = "Stores the caller's own Portal preferences and answers what is now saved. A field the Portal understands is validated; the rest is kept as sent.",
    tag = "preferences",
    request_body(
        content = Preferences,
        example = json!({ "locale": "sk", "theme": "dark", "advancedMode": false, "defaultProject": "helsinki" })
    ),
    responses(
        (status = 200, description = "Stored; the body is what is now saved", body = Preferences),
        (status = 400, description = "A field the Portal understands is invalid", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Missing or mismatched CSRF token", body = ProblemDetails),
        (status = 503, description = "No preferences database configured", body = ProblemDetails)
    )
)]
pub async fn put_preferences(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Preferences>, ApiError> {
    // Parsed by hand, as every write handler here is: axum's `Json` extractor answers a bad body
    // with a plain-text 422, and the contract says problem+json 400 (API/01 sections 2 and 8).
    let preferences: Preferences = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("preferences body is invalid: {e}")))?;
    preferences.validate().map_err(ApiError::BadRequest)?;
    let pool = pool(&state)?;
    let subject = &user.0.identity.subject;
    // Old connectors still in the row move first, so this whole-document save cannot drop them.
    crate::personal_connectors::move_one(&state, &user.0.identity).await?;
    let mut value = serde_json::to_value(&preferences)
        .map_err(|e| ApiError::Internal(format!("preferences did not serialize: {e}")))?;
    // What the move left (a list it could not read, connectors it kept aside) is carried over.
    let stored = db::load_preferences(pool, subject)
        .await
        .map_err(db_error)?;
    if let Some(object) = value.as_object_mut() {
        object.extend(crate::personal_connectors::held(stored.as_ref()));
    }
    db::save_preferences(pool, subject, &value)
        .await
        .map_err(db_error)?;
    Ok(Json(preferences))
}

/// The pages a person opened since the last report, newest first (UI-90).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecentPlaces {
    /// 1 to 10 places, newest first.
    pub places: Vec<Place>,
}

/// Puts `opened` (newest first) before `stored`, each path once, the newest ten.
fn merged_recent(opened: Vec<Place>, stored: Vec<Place>) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::with_capacity(MAX_RECENT);
    for place in opened.into_iter().chain(stored) {
        if out.len() == MAX_RECENT {
            break;
        }
        if !out.iter().any(|kept| kept.path == place.path) {
            out.push(place);
        }
    }
    out
}

#[utoipa::path(
    post,
    path = "/api/v1/preferences/recent",
    summary = "Add Recent Pages",
    description = "Puts the pages the caller opened, newest first, at the top of their recent pages, each once, the newest ten kept; every other preference stays as stored. The browser sends them when its tab is hidden, so a visit is not a write.",
    tag = "preferences",
    request_body(
        content = RecentPlaces,
        example = json!({ "places": [{ "path": "/projects/helsinki/pipelines/air-quality/edit", "title": "air-quality · helsinki" }] })
    ),
    responses(
        (status = 200, description = "Stored; the body is what is now saved", body = Preferences),
        (status = 400, description = "No place, more than ten, or a place that is not a page of this Portal", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Missing or mismatched CSRF token", body = ProblemDetails),
        (status = 503, description = "No preferences database configured", body = ProblemDetails)
    )
)]
pub async fn add_recent(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Preferences>, ApiError> {
    let opened: RecentPlaces = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("recent pages body is invalid: {e}")))?;
    if opened.places.is_empty() || opened.places.len() > MAX_RECENT {
        return Err(ApiError::BadRequest(format!(
            "send 1 to {MAX_RECENT} places, not {}",
            opened.places.len()
        )));
    }
    for place in &opened.places {
        place.validate().map_err(ApiError::BadRequest)?;
    }
    let pool = pool(&state)?;
    let subject = &user.0.identity.subject;
    let mut preferences = db::load_preferences(pool, subject)
        .await
        .map_err(db_error)?
        .and_then(|value| serde_json::from_value::<Preferences>(value).ok())
        .unwrap_or_default();
    preferences.recent = merged_recent(opened.places, std::mem::take(&mut preferences.recent));
    let value = serde_json::to_value(&preferences)
        .map_err(|e| ApiError::Internal(format!("preferences did not serialize: {e}")))?;
    db::save_preferences(pool, subject, &value)
        .await
        .map_err(db_error)?;
    Ok(Json(preferences))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/preferences", get(get_preferences).put(put_preferences))
        .route("/preferences/recent", axum::routing::post(add_recent))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(json: serde_json::Value) -> Preferences {
        serde_json::from_value(json).expect("valid shape")
    }

    #[test]
    fn accepts_the_documented_example() {
        let p = prefs(serde_json::json!({
            "theme": "system",
            "locale": "sk",
            "defaultProject": "ovzdusie",
            "dashboardLayouts": { "ovzdusie-prehlad": { "collapsedLegend": true } },
            "advancedMode": true,
            "firstRunDismissed": true
        }));
        assert_eq!(p.validate(), Ok(()));
        assert_eq!(p.advanced_mode, Some(true));
        assert_eq!(p.first_run_dismissed, Some(true));
        // Absent is off, and stays absent on the way back out (CC-29).
        let unset = prefs(serde_json::json!({}));
        assert_eq!(unset.advanced_mode, None);
        assert!(serde_json::to_value(&unset)
            .expect("serializes")
            .get("advancedMode")
            .is_none());
        assert!(
            Preferences::default().validate().is_ok(),
            "nothing set is fine"
        );
    }

    #[test]
    fn recent_pages_put_the_new_first_once_each_and_keep_ten() {
        let place = |n: usize| Place {
            path: format!("/p{n}"),
            title: format!("p{n}"),
        };
        let stored: Vec<Place> = (0..10).map(place).collect();
        let merged = merged_recent(vec![place(20), place(3)], stored);
        let paths: Vec<&str> = merged.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/p20", "/p3", "/p0", "/p1", "/p2", "/p4", "/p5", "/p6", "/p7", "/p8"]
        );
        assert!(merged_recent(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn keeps_recent_and_starred_pages_of_this_portal_only() {
        let place = |path: &str| serde_json::json!({ "path": path, "title": "Air quality" });
        let ok = prefs(serde_json::json!({
            "recent": [place("/projects/helsinki/pipelines/air-quality?tab=runs")],
            "favourites": [place("/catalogue")]
        }));
        assert_eq!(ok.validate(), Ok(()));
        for wrong in [
            "https://evil.example/",
            "//evil.example/x",
            "javascript:alert(1)",
            "/a\\b",
            "/a\nb",
            "",
        ] {
            let p = prefs(serde_json::json!({ "favourites": [place(wrong)] }));
            assert!(p.validate().is_err(), "{wrong:?} is refused");
        }
        let untitled = prefs(serde_json::json!({ "recent": [{ "path": "/x", "title": "" }] }));
        assert!(untitled.validate().is_err());
        let eleven = prefs(serde_json::json!({ "recent": vec![place("/x"); 11] }));
        assert!(eleven.validate().is_err(), "ten recent pages at most");
        let fifty_one = prefs(serde_json::json!({ "favourites": vec![place("/x"); 51] }));
        assert!(fifty_one.validate().is_err(), "fifty starred pages at most");
        assert!(
            prefs(serde_json::json!({ "favourites": vec![place("/x"); 50] }))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn refuses_what_it_understands_and_is_wrong() {
        assert!(prefs(serde_json::json!({ "theme": "sepia" }))
            .validate()
            .is_err());
        assert!(prefs(serde_json::json!({ "locale": "slovak" }))
            .validate()
            .is_err());
        assert!(prefs(serde_json::json!({ "defaultProject": "Ovzdusie" }))
            .validate()
            .is_err());
        assert!(
            prefs(serde_json::json!({ "dashboardLayouts": { "../x": {} } }))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn unknown_fields_are_rejected_at_the_door() {
        let result = serde_json::from_value::<Preferences>(serde_json::json!({ "colour": "red" }));
        assert!(
            result.is_err(),
            "deny_unknown_fields keeps the contract honest"
        );
    }

    #[test]
    fn the_old_connectors_list_is_no_preference_any_more() {
        // They live in the connector store since T-5970 (personal_connectors moved them).
        let old = serde_json::json!({ "connectors": [{ "name": "air", "select": {} }] });
        assert!(serde_json::from_value::<Preferences>(old).is_err());
    }

    #[test]
    fn layouts_are_bounded() {
        let big = "x".repeat(MAX_LAYOUTS_BYTES);
        let p = prefs(serde_json::json!({ "dashboardLayouts": { "d": big } }));
        assert!(
            p.validate().is_err(),
            "a layout larger than the cap is not a layout"
        );
    }
}
