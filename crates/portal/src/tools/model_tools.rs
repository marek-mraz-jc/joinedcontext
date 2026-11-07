//! Live schema preview for the LinkML editor, compiled by Model Tools.
//!
//! Model Tools is a stateless, versioned image (`linkml`, `schema-automator`,
//! `pysmartdatamodels`, the `ngsi_ld_kind` post-processor) that holds no credentials, reads no
//! platform state and may fetch only from the Smart Data Models organisation (DM-10, DM-18).
//! The Portal calls it so the editor never fetches a third-party schema itself, and so the
//! container needs no ingress. CI compiles with the same image version as this preview (DM-19).

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;
use crate::tools::sdm_mirror::{is_catalogue_id, Mirrored};

/// How long a compilation may take. The editor asks on every pause in typing, so a slow answer
/// is worse than no answer: the view shows the source without a preview and asks again.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(10);

/// Largest LinkML source the Portal forwards. Model Tools is stateless and shared, so the
/// Portal caps the payload before it reaches it rather than after (DM-18). A schema this size
/// is already far past what an editor session produces.
pub const MAX_REQUEST_BYTES: usize = 512 * 1024;

/// Largest sample `POST /api/v1/tools/infer-schema` takes (DM-55). The route has its own limit
/// because a sample is a file and a source is a text box.
pub const MAX_SAMPLE_BYTES: usize = 10 * 1024 * 1024;

/// A LinkML source to compile.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct GenerateRequest {
    /// LinkML YAML, exactly as the editor holds it.
    pub source: String,
}

/// One model of the Smart Data Models catalogue index, as the wizard lists and searches it.
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema, PartialEq)]
pub struct CatalogueModel {
    /// The catalogue identifier, `dataModel.Environment/AirQualityObserved`.
    pub id: String,
    /// The model name, `AirQualityObserved`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Attribute names, so the wizard can search by attribute without fetching the model.
    #[serde(default)]
    pub attributes: Vec<String>,
    /// The upstream commit the model was converted from (DM-08), where the mirror answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// One subject of the catalogue: `dataModel.Environment` and the models under it.
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema, PartialEq)]
pub struct CatalogueSubject {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub models: Vec<CatalogueModel>,
}

/// A catalogue model the mirror could not convert, listed instead of offered half broken.
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema, PartialEq)]
pub struct CatalogueFailure {
    /// The catalogue identifier.
    pub id: String,
    /// Why it did not convert, in the converter's words.
    pub finding: String,
}

/// The catalogue index: the mirror's, or the one Model Tools caches and refreshes daily (DM-12).
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Catalogue {
    #[serde(default)]
    pub subjects: Vec<CatalogueSubject>,
    /// When the cache was last filled from the catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    /// The last refresh did not reach the catalogue, so this index is the older cached one.
    /// An index a browser cannot refresh is still an index it can work from (DM-12).
    #[serde(default)]
    pub stale: bool,
    /// The models the mirror could not convert, each with its finding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<CatalogueFailure>,
}

/// `?refresh=true`: ask Model Tools to fill its cache now instead of waiting for the daily run.
#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct CatalogueQuery {
    #[serde(default)]
    pub refresh: bool,
    /// Comma-separated catalogue subjects (`dataModel.Parking`) whose models' attribute names
    /// Model Tools fills before it answers, at most [`MAX_SUBJECTS`] (T-3840).
    #[serde(default)]
    pub subject: Option<String>,
}

/// The most subjects one catalogue request fills: each is one fetch upstream, once per replica.
pub const MAX_SUBJECTS: usize = 8;

/// The subjects a catalogue request names, each a `dataModel.<Name>` of the catalogue's own
/// spelling; anything else is the caller's mistake and never reaches Model Tools (DM-10).
pub(crate) fn subjects_of(raw: Option<&str>) -> Result<Vec<&str>, ApiError> {
    let subjects: Vec<&str> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if subjects.len() > MAX_SUBJECTS {
        return Err(ApiError::BadRequest(format!(
            "subject names at most {MAX_SUBJECTS} catalogue subjects"
        )));
    }
    for subject in &subjects {
        let name = subject.strip_prefix("dataModel.").unwrap_or_default();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ApiError::BadRequest(format!(
                "subject {subject:?} is not a catalogue subject such as 'dataModel.Parking'"
            )));
        }
    }
    Ok(subjects)
}

/// A model to import from the Smart Data Models catalogue.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ImportSdmRequest {
    /// Catalogue identifier, `dataModel.Environment/AirQualityObserved`. Never a URL: the
    /// allowlist that limits fetching to the Smart Data Models organisation lives in Model
    /// Tools (DM-10), and the Portal refuses anything a caller could steer.
    pub model: String,
}

/// What one Model Tools run rendered. Every artifact is optional: a version that does not
/// render one omits it, and a source that does not compile yet answers with `errors` filled in
/// and the artifacts absent. A half-written model is the normal state of an editor.
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifacts {
    /// The LinkML source itself, which an import produces and the editor then edits; its
    /// annotations carry `spec.source.repository`, `path` and `commit` (DM-07, DM-08).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linkml: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    /// The documentation page, one Markdown file for the whole model: the same page
    /// `jcctl model generate` commits beside the source, so the editor previews what a
    /// reviewer will approve rather than a second rendering of it (DM-02, DM-32, DM-43).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shacl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// `jc-types.ts`: the row types a generated application compiles against (SDK-10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript: Option<String>,
    /// `model.qb.ttl`, the RDF Data Cube structure, only for a model that declares a Data
    /// Structure Definition (DM-60).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qb: Option<String>,
    /// The model with its imports merged, in the LinkML subset the broker reads: what the
    /// reconciler writes to a Context Space's broker tenant (`PUT /q/schemas/{tenant}`, T-3703).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_schema: Option<serde_json::Value>,
    /// The generator version that produced these artifacts, so a preview and a committed
    /// artifact set can be compared (DM-19).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_version: Option<String>,
    /// Compilation messages. Non-empty with no artifacts means the source does not compile.
    #[serde(default)]
    pub errors: Vec<String>,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(COMPILE_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

/// The Model Tools base URL, or the reason the Portal cannot reach it.
fn model_tools_url(state: &AppState, route: &str) -> Result<String, ApiError> {
    let base = state
        .config
        .model_tools_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no model tools service is configured".into()))?;
    Ok(format!("{}/{route}", base.trim_end_matches('/')))
}

/// The URL names an internal service, so the reason is logged and the caller learns only that
/// the compiler is unreachable.
fn unavailable(route: &str, what: &str, err: &dyn std::fmt::Display) -> ApiError {
    tracing::warn!(route = %route, error = %err, "model tools {what}");
    ApiError::Unavailable("the model tools service did not answer".into())
}

/// The `jc-types.ts` Model Tools renders for a LinkML source (SDK-10), or why there is none.
pub async fn typescript(state: &AppState, source: &str) -> Result<String, String> {
    if source.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "the model is larger than {MAX_REQUEST_BYTES} bytes"
        ));
    }
    let Json(artifacts) = compile(
        state,
        "generate",
        &GenerateRequest {
            source: source.to_owned(),
        },
    )
    .await
    .map_err(|err| err.to_string())?;
    match artifacts.typescript {
        Some(types) if !types.trim().is_empty() => Ok(types),
        _ if !artifacts.errors.is_empty() => Err(artifacts.errors.join("; ")),
        _ => Err("model tools rendered no typescript".to_owned()),
    }
}

/// Posts one body to a Model Tools route and reads the artifacts back.
async fn compile(
    state: &AppState,
    route: &str,
    body: &impl Serialize,
) -> Result<Json<Artifacts>, ApiError> {
    let url = model_tools_url(state, route)?;
    let response = http()
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|err| unavailable(route, "unreachable", &err))?;
    if !response.status().is_success() {
        return Err(unavailable(route, "refused", &response.status()));
    }
    let artifacts = response
        .json::<Artifacts>()
        .await
        .map_err(|err| unavailable(route, "answered unreadably", &err))?;
    Ok(Json(artifacts))
}

#[utoipa::path(
    post,
    path = "/api/v1/tools/generate",
    summary = "Generate Model Artifacts",
    description = "Compiles LinkML source into its artifacts (JSON Schema, JSON-LD context, SHACL and the rest) through the model tools service, or answers the messages of a source that does not compile. Writes nothing.",
    tag = "tools",
    request_body(
        content = GenerateRequest,
        example = json!({ "source": "id: https://hel.fi/models/air\nname: air\nclasses:\n  AirQualityObserved:\n    attributes:\n      pm10: { range: float }\n" })
    ),
    responses(
        (status = 200, description = "Compiled artifacts, or the messages of a source that does not compile", body = Artifacts),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 413, description = "Source larger than the payload limit", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn generate(
    _user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<GenerateRequest>,
) -> Result<Json<Artifacts>, ApiError> {
    compile(&state, "generate", &request).await
}

#[utoipa::path(
    post,
    path = "/api/v1/tools/import-sdm",
    summary = "Import A Smart Data Model",
    description = "Compiles one Smart Data Models catalogue model into LinkML and its artifacts. Writes nothing; proposing it as a DataModel is a separate change.",
    tag = "tools",
    request_body(content = ImportSdmRequest, example = json!({ "model": "AirQualityObserved" })),
    responses(
        (status = 200, description = "Compiled artifacts of the catalogue model", body = Artifacts),
        (status = 400, description = "Not a Smart Data Models catalogue identifier", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn import_sdm(
    _user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<ImportSdmRequest>,
) -> Result<Json<Artifacts>, ApiError> {
    if !is_catalogue_id(&request.model) {
        return Err(ApiError::BadRequest(
            "model must be a Smart Data Models catalogue identifier such as \
             'dataModel.Environment/AirQualityObserved', not a URL"
                .into(),
        ));
    }
    import_mirrored(&state, &request).await
}

async fn import_mirrored(
    state: &AppState,
    request: &ImportSdmRequest,
) -> Result<Json<Artifacts>, ApiError> {
    let mirrored = match &state.sdm_mirror {
        Some(mirror) => mirror.model(&request.model).await,
        None => Mirrored::Unknown,
    };
    match mirrored {
        Mirrored::Model(linkml) => Ok(Json(Artifacts {
            linkml: Some(linkml),
            ..Artifacts::default()
        })),
        Mirrored::Failed(finding) => Ok(Json(Artifacts {
            errors: vec![finding],
            ..Artifacts::default()
        })),
        Mirrored::Unknown => compile(state, "import-sdm", request).await,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/tools/sdm-catalog",
    summary = "List Smart Data Models",
    description = "The Smart Data Models catalogue index, from the cache when a refresh did not reach the catalogue.",
    tag = "tools",
    params(
        ("refresh" = Option<bool>, Query, description = "Refresh the cached index now instead of waiting for the daily run"),
        ("subject" = Option<String>, Query, description = "Comma-separated subjects, at most 8, whose model attribute names are filled first")
    ),
    responses(
        (status = 200, description = "The catalogue index, from the cache when a refresh did not reach the catalogue", body = Catalogue),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn sdm_catalog(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<CatalogueQuery>,
) -> Result<Json<Catalogue>, ApiError> {
    let subjects = subjects_of(query.subject.as_deref())?;
    if subjects.is_empty() {
        return fetch_catalogue(&state, query.refresh, None).await.map(Json);
    }
    // Model Tools fills one subject per request and keeps what it filled, so the last answer
    // carries them all; a refresh is asked once, on the first.
    let mut catalogue = Catalogue::default();
    for (at, subject) in subjects.iter().enumerate() {
        catalogue = fetch_catalogue(&state, query.refresh && at == 0, Some(subject)).await?;
    }
    Ok(Json(catalogue))
}

/// The catalogue index from Model Tools, which the wizard and the organization's model list
/// (DM-63) both read.
pub async fn fetch_catalogue(
    state: &AppState,
    refresh: bool,
    subject: Option<&str>,
) -> Result<Catalogue, ApiError> {
    // The mirror holds every model's attributes already, and a refresh is its nightly run.
    if let Some(mirror) = &state.sdm_mirror {
        return Ok(mirror.catalogue().clone());
    }
    let route = "catalog";
    let url = model_tools_url(state, route)?;
    let mut query = vec![("refresh", refresh.to_string())];
    if let Some(subject) = subject {
        query.push(("subject", subject.to_owned()));
    }
    let response = http()
        .get(&url)
        // The only things a caller may steer here: whether the cache is refilled first and which
        // listed subject is filled. The catalogue itself is named by Model Tools' own allowlist,
        // never by the request (DM-10).
        .query(&query)
        .send()
        .await
        .map_err(|err| unavailable(route, "unreachable", &err))?;
    if !response.status().is_success() {
        return Err(unavailable(route, "refused", &response.status()));
    }
    response
        .json::<Catalogue>()
        .await
        .map_err(|err| unavailable(route, "answered unreadably", &err))
}

/// `POST /api/v1/tools/infer-schema`: a draft model from a sample file (T-0599, DM-54, DM-55).
///
/// The body is `multipart/form-data` with the file under `file` and an optional `format`
/// (`csv`, `xlsx`, `json`, `pdf`; the extension decides otherwise). The bytes go to Model Tools'
/// `/infer-schema` base64-encoded in JSON, are parsed there in memory and written nowhere, and
/// the draft comes back as Model Tools wrote it: `linkml`, `operations`, `detectedTypes`,
/// `matches`, `untyped`, `rows` (API/01 §11).
#[utoipa::path(
    post,
    path = "/api/v1/tools/infer-schema",
    tag = "tools",
    request_body(
        content = Object,
        description = "`multipart/form-data`: the sample under `file`, and an optional `format` \
                       (`csv`, `xlsx`, `json`, `pdf`) when the file name does not say",
        content_type = "multipart/form-data",
        example = json!({ "file": "station_id,pm10\n01,18.2\n", "format": "csv" })
    ),
    responses(
        (status = 200, description = "The draft model as Model Tools wrote it: `linkml`, \
                                      `operations`, `detectedTypes`, `matches`, `untyped`, `rows`", body = Object),
        (status = 400, description = "No `file` field, an upload that does not parse, or a \
                                      sample past the byte limit, or a `format` that is not one of the four", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 413, description = "Sample larger than the body limit", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn infer_schema(
    _user: CurrentUser,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    let (mut name, mut content, mut format) = (None, None, None);
    let upload_error = |what: &str, err: &axum::extract::multipart::MultipartError| {
        if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::BadRequest(format!(
                "the sample is larger than the {MAX_SAMPLE_BYTES} byte limit"
            ))
        } else {
            ApiError::BadRequest(format!("{what}: {err}"))
        }
    };
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| upload_error("the upload did not parse", &err))?
    {
        let field_name = field.name().unwrap_or_default().to_owned();
        match field_name.as_str() {
            "file" => {
                name = field.file_name().map(str::to_owned);
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|err| upload_error("field 'file'", &err))?;
                content = Some(bytes);
            }
            "format" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|err| upload_error("field 'format'", &err))?;
                format = Some(String::from_utf8_lossy(&bytes).trim().to_owned());
            }
            _ => {}
        }
    }
    let content = content
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| ApiError::BadRequest("the upload carries no 'file'".into()))?;
    let answer =
        infer_schema_from_bytes(&state, name.as_deref(), &content, format.as_deref()).await?;
    Ok(Json(answer))
}

/// The sample formats Model Tools parses (API/01 §11, DM-17).
pub const SAMPLE_FORMATS: [&str; 4] = ["csv", "xlsx", "json", "pdf"];

/// Core inference function from sample bytes, shared by the multipart upload route and `jc_model_infer`.
pub async fn infer_schema_from_bytes(
    state: &AppState,
    name: Option<&str>,
    content: &[u8],
    format: Option<&str>,
) -> Result<Value, ApiError> {
    if content.len() > MAX_SAMPLE_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the sample is larger than the {MAX_SAMPLE_BYTES} byte limit"
        )));
    }
    let mut body = serde_json::json!({
        "name": name.unwrap_or("sample"),
        "content": base64::engine::general_purpose::STANDARD.encode(content),
    });
    if let Some(format) = format.map(str::trim).filter(|f| !f.is_empty()) {
        // The Portal is the trust boundary: Model Tools switches its parser on this value, so a
        // caller names one of the four or none, and none lets the file name decide (DM-17).
        if !SAMPLE_FORMATS.contains(&format) {
            return Err(ApiError::BadRequest(format!(
                "format '{format}' is not one of {}; leave it out to go by the file name",
                SAMPLE_FORMATS.join(", ")
            )));
        }
        body["format"] = Value::String(format.to_owned());
    }

    let route = "infer-schema";
    let url = model_tools_url(state, route)?;
    let response = http()
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|err| unavailable(route, "unreachable", &err))?;
    let status = response.status();
    let answer = response
        .json::<Value>()
        .await
        .map_err(|err| unavailable(route, "answered unreadably", &err))?;
    if status == reqwest::StatusCode::BAD_REQUEST {
        let reasons = answer["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|joined| !joined.is_empty())
            .unwrap_or_else(|| "the sample could not be read".into());
        return Err(ApiError::BadRequest(reasons));
    }
    if !status.is_success() || !answer.is_object() {
        return Err(unavailable(route, "refused", &status));
    }
    Ok(answer)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/sdm-catalog", get(sdm_catalog))
        .route("/tools/generate", post(generate))
        .route("/tools/import-sdm", post(import_sdm))
        // A sample is a file: this route carries its own limit, set closest to the handler so
        // it wins over the router's (DM-55).
        .route(
            "/tools/infer-schema",
            post(infer_schema).layer(DefaultBodyLimit::max(MAX_SAMPLE_BYTES + 64 * 1024)),
        )
        // Model Tools is stateless and shared: an oversized source is refused here, before it
        // is read into memory or forwarded (DM-18).
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_identifier_is_two_plain_segments() {
        assert!(is_catalogue_id("dataModel.Environment/AirQualityObserved"));
        assert!(is_catalogue_id("dataModel.Transportation/Vehicle"));
    }

    #[test]
    fn nothing_a_caller_could_steer_is_an_identifier() {
        for steered in [
            "https://example.org/evil.json",
            "http://169.254.169.254/latest/meta-data",
            "//example.org/x",
            "dataModel.Environment/../../etc/passwd",
            "../secrets/x",
            "dataModel.Environment/Air Quality",
            "dataModel.Environment",
            "",
            "/",
            "dataModel.Environment/",
        ] {
            assert!(
                !is_catalogue_id(steered),
                "{steered} must not pass as a catalogue identifier"
            );
        }
    }

    #[test]
    fn artifacts_of_an_older_tool_version_still_parse() {
        // Every field is optional on purpose: a Model Tools version that renders no OWL must
        // not turn a working preview into a 503.
        let artifacts: Artifacts = serde_json::from_str(r#"{"shacl":"@prefix sh: <> ."}"#)
            .expect("a partial artifact set parses");
        assert_eq!(artifacts.shacl.as_deref(), Some("@prefix sh: <> ."));
        assert!(artifacts.json_schema.is_none());
        assert!(artifacts.errors.is_empty(), "absent errors is not an error");
    }

    #[test]
    fn artifacts_serialize_in_the_documented_camel_case() {
        let artifacts = Artifacts {
            generator_version: Some("linkml-1.8.0".into()),
            ..Artifacts::default()
        };
        let json = serde_json::to_string(&artifacts).expect("serialize");
        assert!(
            json.contains("\"generatorVersion\":\"linkml-1.8.0\""),
            "{json}"
        );
        assert!(!json.contains("jsonSchema"), "absent artifacts are omitted");
    }

    /// DM-02, DM-43: the Portal reads exactly the members Model Tools renders. A field this
    /// struct does not name is an artifact the editor cannot show, and one it names alone is
    /// a member no answer ever carries — both are drift, and both are silent without this.
    #[test]
    fn the_artifact_set_names_exactly_the_members_model_tools_answers_with() {
        let artifacts = Artifacts {
            linkml: Some("id: https://x/air".into()),
            json_schema: Some(serde_json::json!({})),
            context: Some(serde_json::json!({})),
            docs: Some("# air".into()),
            shacl: Some("@prefix sh: <> .".into()),
            owl: Some("@prefix owl: <> .".into()),
            example: Some(serde_json::json!({})),
            typescript: Some("export interface Air { id: string }".into()),
            qb: Some("@prefix qb: <http://purl.org/linked-data/cube#> .".into()),
            broker_schema: Some(serde_json::json!({"id": "https://x/air", "name": "air"})),
            generator_version: Some("linkml-1.11.1".into()),
            errors: Vec::new(),
        };
        let json: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&serde_json::to_string(&artifacts).expect("serialize"))
                .expect("an object");

        let members: std::collections::BTreeSet<&str> = json.keys().map(String::as_str).collect();
        assert_eq!(
            members,
            [
                "linkml",
                "jsonSchema",
                "context",
                "docs",
                "shacl",
                "owl",
                "example",
                "typescript",
                "qb",
                "brokerSchema",
                "generatorVersion",
                "errors",
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<&str>>()
        );
    }
}
