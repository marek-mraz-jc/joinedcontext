//! The platform services an App calls on its own host: which layer switches one off, the quota in
//! force, today's use against it, and the refusals the routes answer (AP-163…AP-165, ADR-N-045,
//! API/06 §2, §3).
//!
//! A service is on only when the organization, the project and the App all list it; each quota is
//! the lowest a layer sets, the catalog's default when none does. The check runs on every call, so
//! switching a layer off stops an App that is already published (AP-164).

use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use jc_core::kinds::org_settings::entry_at;
use jc_core::kinds::{AppService, AppSpec, DEFAULT_ORGANIZATION_SERVICES};
use serde::Serialize;
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::roles::AppPerson;
use crate::auth::session::EDGE_TOKEN_HEADER;
use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;
use crate::store::Mirror;

/// What every service route checks before its own work, in this order: the call comes from the
/// App's own host, the App is published and the caller may open it, a person is signed in (a
/// service acts in a person's name, so an anonymous visitor of a public App calls none), a write
/// carries the CSRF header when the edge's cookie token rode along (AP-84), and `service` is on
/// at every layer (AP-164). Answers the App's project, its spec and the person.
pub async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    name: &str,
    (service, method): (AppService, &Method),
) -> Result<(String, AppSpec, AppPerson), Box<Response>> {
    if let Some(refused) = super::static_host::off_its_origin(state, headers, name, uri) {
        return Err(Box::new(refused));
    }
    let not_found =
        || Box::new(ApiError::NotFound(format!("app '{name}' not found")).into_response());
    let (project, spec, _) =
        super::static_host::published_app(state, name).ok_or_else(not_found)?;
    let person = super::roles::person(state, headers, &spec, name).await;
    if !super::roles::may_open(state, &project, name, &spec, person.as_ref()) {
        return Err(not_found());
    }
    let person = person.ok_or_else(|| Box::new(ApiError::Unauthorized.into_response()))?;
    if !method.is_safe()
        && headers.contains_key(&EDGE_TOKEN_HEADER)
        && !crate::auth::csrf::double_submitted(headers)
    {
        return Err(Box::new(ApiError::Forbidden.into_response()));
    }
    if let Some(layer) = off_at(&state.mirror, &project, &spec, service) {
        return Err(Box::new(service_off(service, layer)));
    }
    Ok((project, spec, person))
}

/// The layer that switched a service off (AP-164).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Organization,
    Project,
    App,
}

/// A per-App quota (AP-165): its member name in each layer's limits and its catalog path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    EmailsPerDay,
    AiTokensPerDay,
    FilesMiB,
}

impl Quota {
    fn member(self) -> &'static str {
        match self {
            Self::EmailsPerDay => "emailsPerDay",
            Self::AiTokensPerDay => "aiTokensPerDay",
            Self::FilesMiB => "filesMiB",
        }
    }

    fn of_app(self, spec: &AppSpec) -> Option<u32> {
        let limits = spec.limits.as_ref()?;
        match self {
            Self::EmailsPerDay => limits.emails_per_day,
            Self::AiTokensPerDay => limits.ai_tokens_per_day,
            Self::FilesMiB => limits.files_mib,
        }
    }
}

/// The spec of the Organization that owns `project`: its switches and quotas hold for its own
/// Apps alone (ADR-N-058).
fn organization(mirror: &Mirror, project: &str) -> Option<Value> {
    crate::permissions::organization_manifest_of(mirror, project).map(|envelope| envelope.spec)
}

fn project(mirror: &Mirror, project: &str) -> Option<Value> {
    mirror
        .get(ORG_NAMESPACE, "Project", project)
        .map(|envelope| envelope.spec)
}

/// The service list at `pointer` of a layer's spec; `None` when the layer sets none or one this
/// Portal cannot read, which then takes the layer above.
fn services_at(spec: Option<&Value>, pointer: &str) -> Option<Vec<AppService>> {
    serde_json::from_value(spec?.pointer(pointer)?.clone()).ok()
}

/// The layer that switches `service` off for App `spec` of `project`, `None` when all three have
/// it on. `identity` and `data` are every App's (AP-161).
pub fn off_at(
    mirror: &Mirror,
    project_name: &str,
    spec: &AppSpec,
    service: AppService,
) -> Option<Layer> {
    if matches!(service, AppService::Identity | AppService::Data) {
        return None;
    }
    let organization = organization(mirror, project_name);
    let allowed = services_at(organization.as_ref(), "/policies/apps/services")
        .unwrap_or_else(|| DEFAULT_ORGANIZATION_SERVICES.to_vec());
    if !allowed.contains(&service) {
        return Some(Layer::Organization);
    }
    let project = project(mirror, project_name);
    if services_at(project.as_ref(), "/apps/services").is_some_and(|list| !list.contains(&service))
    {
        return Some(Layer::Project);
    }
    (!spec.services.contains(&service)).then_some(Layer::App)
}

/// The quota in force for App `spec` of `project`: the lowest any layer sets, the catalog's default
/// where the organization sets none (AP-165).
pub fn quota(mirror: &Mirror, project_name: &str, spec: &AppSpec, quota: Quota) -> u32 {
    let read = |spec: Option<&Value>, pointer: String| -> Option<u32> {
        spec?
            .pointer(&pointer)?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
    };
    let member = quota.member();
    let organization = read(
        organization(mirror, project_name).as_ref(),
        format!("/limits/apps/{member}"),
    )
    .or_else(|| entry_at(&format!("spec.limits.apps.{member}")).and_then(|entry| entry.default))
    .unwrap_or(0);
    let project = read(
        project(mirror, project_name).as_ref(),
        format!("/apps/limits/{member}"),
    );
    [Some(organization), project, quota.of_app(spec)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(0)
}

/// Adds `count` to today's use of one counter if it stays within `limit`, and answers whether it
/// did. A counter is an App's (`subject` empty) or what one person received from it. Atomic: two
/// calls at once never both pass the last unit.
// ponytail: past days' rows stay; a nightly `DELETE … WHERE day < current_date` when they matter.
pub async fn take(
    db: impl sqlx::PgExecutor<'_>,
    (project, app): (&str, &str),
    service: &str,
    subject: &str,
    count: u32,
    limit: u32,
) -> Result<bool, sqlx::Error> {
    if count > limit {
        return Ok(false);
    }
    let row = sqlx::query(
        "INSERT INTO app_service_usage (project, app, service, subject, day, used) \
         VALUES ($1, $2, $3, $4, (now() AT TIME ZONE 'utc')::date, $5) \
         ON CONFLICT (project, app, service, subject, day) \
         DO UPDATE SET used = app_service_usage.used + EXCLUDED.used \
         WHERE app_service_usage.used + EXCLUDED.used <= $6 \
         RETURNING used",
    )
    .bind(project)
    .bind(app)
    .bind(service)
    .bind(subject)
    .bind(i64::from(count))
    .bind(i64::from(limit))
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Moves today's counter by `delta`, never below `0`: a reservation [`take`] made, settled to
/// what was spent (T-3661). A counter of another day is left alone.
pub async fn settle(
    db: impl sqlx::PgExecutor<'_>,
    (project, app): (&str, &str),
    service: &str,
    subject: &str,
    delta: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app_service_usage SET used = GREATEST(used + $5, 0) \
         WHERE project = $1 AND app = $2 AND service = $3 AND subject = $4 \
           AND day = (now() AT TIME ZONE 'utc')::date",
    )
    .bind(project)
    .bind(app)
    .bind(service)
    .bind(subject)
    .bind(delta)
    .execute(db)
    .await?;
    Ok(())
}

/// Today's use of one counter, `0` before its first.
pub async fn used_today(
    db: impl sqlx::PgExecutor<'_>,
    (project, app): (&str, &str),
    service: &str,
    subject: &str,
) -> Result<u32, sqlx::Error> {
    let used: Option<i64> = sqlx::query_scalar(
        "SELECT used FROM app_service_usage \
         WHERE project = $1 AND app = $2 AND service = $3 AND subject = $4 \
           AND day = (now() AT TIME ZONE 'utc')::date",
    )
    .bind(project)
    .bind(app)
    .bind(service)
    .bind(subject)
    .fetch_optional(db)
    .await?;
    Ok(used.map_or(0, |used| u32::try_from(used).unwrap_or(u32::MAX)))
}

/// The next midnight UTC, when every daily quota starts again.
pub fn reset_at(now: OffsetDateTime) -> OffsetDateTime {
    now.date()
        .next_day()
        .unwrap_or(now.date())
        .midnight()
        .assume_utc()
}

/// An `application/problem+json` of `…/errors/{slug}` with `extra` members beside the four.
pub(super) fn problem(
    status: StatusCode,
    slug: &str,
    title: &str,
    detail: String,
    extra: Value,
) -> Response {
    let mut body = json!({
        "type": format!("https://joinedcontext.com/errors/{slug}"),
        "title": title,
        "status": status.as_u16(),
        "detail": detail,
    });
    if let (Some(hint), Some(body)) = (crate::error::hint_of(slug), body.as_object_mut()) {
        body.insert("hint".into(), Value::String(hint));
    }
    if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
        body.extend(extra);
    }
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response
}

fn name(service: AppService) -> String {
    serde_json::to_value(service)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `403 …/service-off`: the service and the layer that switched it off (AP-164).
pub fn service_off(service: AppService, layer: Layer) -> Response {
    let service = name(service);
    let field = match layer {
        Layer::Organization => "the organization's spec.policies.apps.services",
        Layer::Project => "the project's spec.apps.services",
        Layer::App => "the App's spec.services",
    };
    problem(
        StatusCode::FORBIDDEN,
        "service-off",
        "Service Off",
        format!("{service} is not in {field}"),
        json!({ "service": service, "layer": layer }),
    )
}

/// `429 …/quota`: the quota used up and, for a daily one, when it resets with `Retry-After`
/// (AP-165). A quota of what an App holds (`filesMiB`) resets when it deletes, so it names no time.
pub fn quota_used(service: AppService, quota: &str, now: Option<OffsetDateTime>) -> Response {
    let Some(now) = now else {
        return problem(
            StatusCode::TOO_MANY_REQUESTS,
            "quota",
            "Quota Used",
            format!("{quota} is used up"),
            json!({ "service": name(service), "quota": quota }),
        );
    };
    let reset = reset_at(now);
    let reset_text = reset
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut response = problem(
        StatusCode::TOO_MANY_REQUESTS,
        "quota",
        "Quota Used",
        format!("{quota} is used up until {reset_text}"),
        json!({ "service": name(service), "quota": quota, "resetAt": reset_text }),
    );
    let wait = (reset - now).whole_seconds().max(1);
    if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// `413 …/file-too-large`: an object over the size one call may store (AP-170).
pub fn file_too_large(limit: usize) -> Response {
    problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        "file-too-large",
        "File Too Large",
        format!("an object is at most {limit} bytes"),
        json!({ "limit": limit }),
    )
}

/// `403 …/recipient-refused`: a recipient who is not a person of the organization (AP-168).
pub fn recipient_refused(to: &str) -> Response {
    problem(
        StatusCode::FORBIDDEN,
        "recipient-refused",
        "Recipient Refused",
        format!("'{to}' is no person of the organization with a verified address"),
        json!({ "to": to }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        }
    }

    /// The organization allows `services` and caps emails at 3; project `roads` lists `email`
    /// and raises its cap to 10.
    fn layers(services: Value) -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            "bb",
            json!({ "domain": "bb.sk", "locales": ["sk"], "defaultLocale": "sk",
                    "policies": { "apps": { "services": services } },
                    "limits": { "apps": { "emailsPerDay": 3 } } }),
        ));
        mirror.upsert(envelope(
            "Project",
            "roads",
            json!({ "organizationRef": { "kind": "Organization", "name": "bb" },
                    "apps": { "services": ["email"], "limits": { "emailsPerDay": 10 } } }),
        ));
        mirror
    }

    fn app(limits: Value) -> AppSpec {
        serde_json::from_value(json!({
            "kind": "static", "source": { "path": "." }, "build": {}, "visibility": "project",
            "dataNeeds": [], "services": ["email"], "limits": limits,
        }))
        .expect("an App spec")
    }

    #[test]
    fn the_organization_off_wins_over_a_project_and_an_app_that_list_it() {
        let spec = app(json!({}));
        let off = layers(json!(["identity", "data"]));
        assert_eq!(
            off_at(&off, "roads", &spec, AppService::Email),
            Some(Layer::Organization)
        );
        let on = layers(json!(["identity", "data", "email"]));
        assert_eq!(off_at(&on, "roads", &spec, AppService::Email), None);
    }

    /// F-R2POR3-011: another Organization's switches and quotas never hold for this one's App.
    #[test]
    fn only_the_apps_own_organization_sets_its_services_and_quotas() {
        let spec = app(json!({}));
        let mirror = layers(json!(["identity", "data", "email"]));
        mirror.upsert(envelope(
            "Organization",
            "aa",
            json!({ "domain": "aa.sk", "locales": ["sk"], "defaultLocale": "sk",
                    "policies": { "apps": { "services": ["identity", "data"] } },
                    "limits": { "apps": { "emailsPerDay": 1 } } }),
        ));
        assert_eq!(off_at(&mirror, "roads", &spec, AppService::Email), None);
        assert_eq!(quota(&mirror, "roads", &spec, Quota::EmailsPerDay), 3);
    }

    #[test]
    fn a_quota_above_the_layer_over_it_is_clamped_to_that_layer() {
        let mirror = layers(json!(["email"]));
        let quota = |limits| quota(&mirror, "roads", &app(limits), Quota::EmailsPerDay);
        assert_eq!(
            quota(json!({})),
            3,
            "the project's 10 is clamped to the organization's 3"
        );
        assert_eq!(quota(json!({ "emailsPerDay": 50 })), 3);
        assert_eq!(
            quota(json!({ "emailsPerDay": 2 })),
            2,
            "the App may go lower"
        );
    }

    #[test]
    fn the_quota_resets_at_the_next_midnight_utc() {
        let now = time::macros::datetime!(2026-10-10 21:30 UTC);
        assert_eq!(reset_at(now), time::macros::datetime!(2026-10-11 0:00 UTC));
        let response = quota_used(AppService::Email, "emailsPerDay", Some(now));
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "9000");
    }
}
