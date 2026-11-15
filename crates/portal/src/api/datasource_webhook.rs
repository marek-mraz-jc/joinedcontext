//! A webhook DataSource's addresses and its test event (PL-118, ADR-N-071).
//!
//! A webhook is the one source a sender pushes to: the runner's shared ingest listener answers
//! it at `{ingest_url}/{project}.{pipeline}/{dataSource}`, one address per pipeline reading the
//! source. The page lists them, and "Send test event" posts a body the person typed through the
//! very handler a sender reaches, signed here with the credential the last sync handed the
//! runner. The credential never leaves this process: the answer is the runner's status and the
//! first bytes of its body.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use jc_core::kinds::data_source::{env_var_of, DataSourceSpec, DataSourceType, WebhookConnection};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::Duration;
use utoipa::ToSchema;

use crate::api::datasource_test::saved_source;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::reconciler::streams::stream_id;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The organization every stream of an in-process runner belongs to (ekiben's default).
const RUNNER_ORG: &str = "default";
/// The largest test body: the source's own cap, and never more than this.
const TEST_BODY_MAX: usize = 64 * 1024;
/// The bytes of the runner's answer the page shows.
const ANSWER_BYTES: usize = 2048;
/// How long a test event waits for the pipeline to take it.
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Where one pipeline reading the source is posted to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookAddress {
    /// The reading pipeline.
    pub pipeline: String,
    /// The URL a sender posts to, or `None` when this installation publishes no ingest listener.
    pub url: Option<String>,
}

/// What a sender needs to post to a webhook DataSource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookInfo {
    /// `hmac` or `bearer`.
    pub auth: String,
    /// The header the sender sets: the signature header, or `Authorization`.
    pub header: String,
    /// The largest body accepted, in bytes.
    pub max_body_bytes: u64,
    /// One address per reading pipeline, by pipeline name; empty when no pipeline reads it.
    pub addresses: Vec<WebhookAddress>,
    /// Whether this installation publishes the ingest listener at all.
    pub published: bool,
}

/// A test event: the body posted to one reading pipeline's address.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({ "pipeline": "ttn-uplinks", "body": "{\"temperature\": 21.5}" }))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestEvent {
    /// A pipeline that reads this source, such as `ttn-uplinks`.
    pub pipeline: String,
    /// The body, as the provider would send it, such as `{"temperature": 21.5}`.
    pub body: String,
}

/// What the runner answered a test event.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestEventAnswer {
    /// The HTTP status the sender would have seen: 200 once the pipeline took the event.
    pub status: u16,
    /// The first 2048 bytes of the answer's body.
    pub answer: String,
}

/// The webhook block of a saved source, or 422 for a source of another type.
fn webhook_of(source: &DataSourceSpec) -> Result<&WebhookConnection, ApiError> {
    match (&source.source_type, &source.webhook) {
        (DataSourceType::Webhook, Some(webhook)) => Ok(webhook),
        _ => Err(ApiError::Unprocessable {
            detail: "this DataSource is not of type webhook, so nothing posts to it".into(),
            errors: vec!["type".into()],
        }),
    }
}

/// The address pipeline `pipeline` of `project` is posted to for source `name`.
fn address(ingest_url: &str, project: &str, pipeline: &str, name: &str) -> String {
    format!("{ingest_url}/{}/{name}", stream_id(project, pipeline))
}

/// `GET /api/v1/projects/{project}/datasources/{name}/webhook` (PL-118).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/datasources/{name}/webhook",
    summary = "Webhook Addresses",
    description = "The URL each reading pipeline is posted to, the header a sender sets and the \
                   body cap. Never the credential.",
    tag = "datasources",
    params(
        ("project" = String, Path, description = "Project slug"),
        ("name" = String, Path, description = "DataSource name"),
    ),
    responses(
        (status = 200, description = "The addresses", body = WebhookInfo),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant reads this DataSource", body = ProblemDetails),
        (status = 404, description = "No such DataSource in this project", body = ProblemDetails),
        (status = 422, description = "The DataSource is not of type webhook", body = ProblemDetails),
    )
)]
pub async fn webhook_info(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<WebhookInfo>, ApiError> {
    let unknown = || ApiError::NotFound(format!("no DataSource {name} in project {project}"));
    if !(is_dns1123(&project) && is_dns1123(&name)) {
        return Err(unknown());
    }
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !effective.may_read_project() {
        return Err(unknown());
    }
    let envelope = state
        .mirror
        .get(&project, "DataSource", &name)
        .ok_or_else(unknown)?;
    let manifest =
        serde_json::to_value(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check("DataSource", Verb::Read, Some(&manifest))?;
    let source: DataSourceSpec = serde_json::from_value(envelope.spec.clone())
        .map_err(|e| ApiError::BadRequest(format!("the saved source does not parse: {e}")))?;
    let webhook = webhook_of(&source)?;
    let ingest_url = state.config.ingest_url.as_deref();
    let addresses = crate::health_log::readers_of(&state.mirror, &project, &name)
        .into_iter()
        .map(|pipeline| WebhookAddress {
            url: ingest_url.map(|base| address(base, &project, &pipeline, &name)),
            pipeline,
        })
        .collect();
    let (auth, header) = match &webhook.auth.hmac {
        Some(hmac) => ("hmac", hmac.header.clone()),
        None => ("bearer", "Authorization".to_owned()),
    };
    Ok(Json(WebhookInfo {
        auth: auth.into(),
        header,
        max_body_bytes: webhook.max_body_bytes(),
        addresses,
        published: ingest_url.is_some(),
    }))
}

/// The request a sender with `credential` would send: the hex SHA-256 HMAC of the body in the
/// provider's header with GitHub's `sha256=` prefix, or the bearer token.
fn signed(
    request: reqwest::RequestBuilder,
    webhook: &WebhookConnection,
    credential: &str,
    body: &str,
) -> Result<reqwest::RequestBuilder, ApiError> {
    match &webhook.auth.hmac {
        Some(hmac) => {
            let mut mac = Hmac::<Sha256>::new_from_slice(credential.as_bytes())
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            mac.update(body.as_bytes());
            let digest: String = mac
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            Ok(request.header(hmac.header.as_str(), format!("sha256={digest}")))
        }
        None => Ok(request.bearer_auth(credential)),
    }
}

/// `POST /api/v1/projects/{project}/datasources/{name}/test-event` (PL-118).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/datasources/{name}/test-event",
    summary = "Webhook Test Event",
    description = "Posts the body to one reading pipeline's webhook through the runner's ingest \
                   handler, signed with the source's credential, and answers the status a sender \
                   would see. The event runs through the pipeline like a real one.",
    tag = "datasources",
    params(
        ("project" = String, Path, description = "Project slug"),
        ("name" = String, Path, description = "DataSource name"),
    ),
    request_body = TestEvent,
    responses(
        (status = 200, description = "What the runner answered", body = TestEventAnswer),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes a DataSource in this project", body = ProblemDetails),
        (status = 404, description = "No such DataSource in this project", body = ProblemDetails),
        (status = 422, description = "The source is no webhook, `pipeline` reads no such source, or `body` is empty or over the cap", body = ProblemDetails),
        (status = 503, description = "The runner does not run in this process, or holds no credential for the source yet", body = ProblemDetails),
    )
)]
pub async fn test_event(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(event): Json<TestEvent>,
) -> Result<Json<TestEventAnswer>, ApiError> {
    let source = saved_source(&state, &user.0.identity, &project, &name)?;
    let webhook = webhook_of(&source)?;
    let readers = crate::health_log::readers_of(&state.mirror, &project, &name);
    if !(is_dns1123(&event.pipeline) && readers.contains(&event.pipeline)) {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "pipeline: no pipeline of project {project} reads DataSource {name} under that name; pick one of the list"
            ),
            errors: vec!["pipeline".into()],
        });
    }
    let cap = usize::try_from(webhook.max_body_bytes())
        .unwrap_or(usize::MAX)
        .min(TEST_BODY_MAX);
    if event.body.trim().is_empty() || event.body.len() > cap {
        return Err(ApiError::Unprocessable {
            detail: format!("body: 1 to {cap} bytes, such as {{\"temperature\": 21.5}}"),
            errors: vec!["body".into()],
        });
    }
    let unavailable = || {
        ApiError::Unavailable(
            "the pipeline runner does not run beside this Portal, so it cannot send a test event; post to the address with the provider's own tool".into(),
        )
    };
    let runner = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(unavailable)?;
    let host = crate::pipeline_secrets::host_runner().ok_or_else(unavailable)?;
    let reference = webhook
        .credential()
        .ok_or_else(|| ApiError::Internal("a validated webhook holds one credential".into()))?;
    let credential = host
        .credential(&env_var_of(&name, reference))
        .ok_or_else(|| {
            ApiError::Unavailable(format!(
                "the runner holds no value for secret '{}' yet; wait for the next sync after the pipeline is applied, and check the pipeline's status",
                reference.name
            ))
        })?;
    let url = format!(
        "{}/ingest/{RUNNER_ORG}/{}/{name}",
        runner.trim_end_matches('/'),
        stream_id(&project, &event.pipeline)
    );
    let client = reqwest::Client::builder()
        .timeout(TEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let request = signed(
        client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json"),
        webhook,
        &credential,
        &event.body,
    )?;
    drop(credential);
    let response = request
        .body(event.body)
        .send()
        .await
        .map_err(|e| ApiError::Unavailable(format!("the runner did not answer: {e}")))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let answer = match text.char_indices().nth(ANSWER_BYTES) {
        Some((at, _)) => text[..at].to_owned(),
        None => text,
    };
    Ok(Json(TestEventAnswer { status, answer }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/datasources/{name}/webhook",
            get(webhook_info),
        )
        .route(
            "/projects/{project}/datasources/{name}/test-event",
            post(test_event),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook(auth: serde_json::Value) -> WebhookConnection {
        serde_json::from_value(serde_json::json!({ "auth": auth })).expect("a webhook block")
    }

    #[test]
    fn an_address_is_the_pipelines_stream_and_the_source() {
        assert_eq!(
            address(
                "https://portal.example.org/ingest/default",
                "hel",
                "ttn",
                "uplinks"
            ),
            "https://portal.example.org/ingest/default/hel.ttn/uplinks"
        );
    }

    /// The signature a provider sends: GitHub's documented example, secret "It's a Secret to
    /// Everybody" over "Hello, World!".
    #[test]
    fn an_hmac_test_event_carries_the_providers_signature() {
        let hook = webhook(serde_json::json!({
            "hmac": { "header": "X-Hub-Signature-256", "secretRef": { "name": "gh", "key": "secret" } }
        }));
        let request = signed(
            reqwest::Client::new().post("http://127.0.0.1:9/x"),
            &hook,
            "It's a Secret to Everybody",
            "Hello, World!",
        )
        .expect("signed")
        .build()
        .expect("a request");
        assert_eq!(
            request.headers()["X-Hub-Signature-256"],
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        assert!(request.headers().get("authorization").is_none());
    }

    #[test]
    fn a_bearer_test_event_carries_the_token() {
        let hook = webhook(serde_json::json!({
            "bearer": { "tokenRef": { "name": "cs", "key": "token" } }
        }));
        let request = signed(
            reqwest::Client::new().post("http://127.0.0.1:9/x"),
            &hook,
            "tok-1",
            "{}",
        )
        .expect("signed")
        .build()
        .expect("a request");
        assert_eq!(request.headers()["authorization"], "Bearer tok-1");
    }

    #[test]
    fn a_source_of_another_type_has_no_webhook() {
        let source: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "http", "http": { "url": "https://feeds.example/aq.json" }
        }))
        .expect("a source");
        assert!(matches!(
            webhook_of(&source),
            Err(ApiError::Unprocessable { errors, .. }) if errors == ["type"]
        ));
    }
}
