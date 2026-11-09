//! The build listener: what a build pod may ask of the Portal, with its build's token and
//! nothing else (AP-130, the jc build lane).
//!
//! A listener of its own (`JC_PORTAL_BUILD_BIND`), so the build pods' NetworkPolicy lets them
//! reach this port of the Portal and no other: not the public routes, not the internal listener
//! (model keys, connectors), not the forge, the identity provider or the gateway jc serves on its
//! main port. Every route takes `Authorization: Bearer <token>`, the token of one build:
//!
//! - `GET /builds/{id}/source`: the source of that build's commit of that App's repository, a
//!   `.tar.gz`; the repository and the commit come from the build, never from the request.
//! - `PUT /builds/{id}/artifacts/{bundle|image|sbom}`: one artifact, capped while it streams.
//! - `PUT /builds/{id}/log`: the build's log, at most 1 MiB.
//! - `POST /builds/{id}/finish`: the build is over. Its uploads freeze before the Portal hashes
//!   anything; the Portal computes the digest itself, checks it as it checks any build (the
//!   commit is the head of the default branch, the sizes, the image's blobs), publishes the
//!   package and writes `status.build` as its own lane (AP-73, AP-101, AP-104).
//!
//! No token: 401. Another build's token, or no such build: 404. An expired token: 401. A build
//! that is no longer running: 409. A body over its cap: 413, before more of it is written.

use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::apps::build_pods::Settings;
use crate::apps::build_records::{self, Record};
use crate::apps::built::{self, Held, MAX_BUNDLE_BYTES, MAX_IMAGE_BYTES};
use crate::error::ApiError;
use crate::state::AppState;

/// The largest SBOM a build uploads.
const MAX_SBOM_BYTES: u64 = 16 * 1024 * 1024;

/// The largest source archive the Portal hands a build: the largest bundle an App may publish.
const MAX_SOURCE_BYTES: u64 = MAX_BUNDLE_BYTES;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/builds/{id}/source", get(source))
        .route("/builds/{id}/artifacts/{name}", put(artifact))
        .route("/builds/{id}/log", put(log))
        .route("/builds/{id}/finish", post(finish))
}

/// The build store and the lane's settings, or 503 on a Portal that builds nothing.
fn lane(state: &AppState) -> Result<(PgPool, Settings), ApiError> {
    match (state.db.clone(), state.config.build_pods.clone()) {
        (Some(db), Some(settings)) => Ok((db, settings)),
        _ => Err(ApiError::Unavailable(
            "this Portal builds no App: JC_PORTAL_BUILD_NAMESPACE or its database is not set"
                .into(),
        )),
    }
}

fn store(err: sqlx::Error) -> ApiError {
    ApiError::Internal(format!("the build store: {err}"))
}

/// The running build `id` whose token the request carries.
async fn admitted(
    state: &AppState,
    id: i64,
    headers: &HeaderMap,
) -> Result<(PgPool, Settings, Record), ApiError> {
    let (db, settings) = lane(state)?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    let not_found = || ApiError::NotFound(format!("build {id} not found"));
    let held = build_records::token_of(&db, id)
        .await
        .map_err(store)?
        .ok_or_else(not_found)?;
    if !build_records::same_hash(&held, &build_records::token_hash(token)) {
        return Err(not_found());
    }
    let build = build_records::get(&db, id)
        .await
        .map_err(store)?
        .ok_or_else(not_found)?;
    if build.state != "running" {
        return Err(ApiError::Conflict(format!(
            "build {id} is {}, and takes nothing more",
            build.state
        )));
    }
    if build.expires_at.is_none_or(|at| at <= chrono::Utc::now()) {
        return Err(ApiError::Unauthorized);
    }
    Ok((db, settings, build))
}

/// The folder a build's uploads are kept in until it ends.
fn folder(settings: &Settings, id: i64) -> PathBuf {
    settings.work_dir.join(id.to_string())
}

/// `GET /builds/{id}/source`: the build's commit of its App's repository as a `.tar.gz`.
async fn source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (_, _, build) = admitted(&state, id, &headers).await?;
    let shako = state
        .shako
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no forge is configured".into()))?;
    let repo = shako.for_application(crate::agents::repository::name(&build.project, &build.app));
    let bytes = repo
        .source_archive(&build.commit, MAX_SOURCE_BYTES)
        .await
        .map_err(|err| {
            ApiError::Unavailable(format!(
                "the forge did not hand out {} of {}/{}: {err}",
                build.commit, repo.owner, repo.repo
            ))
        })?;
    Ok(([(header::CONTENT_TYPE, "application/gzip")], bytes).into_response())
}

/// The cap of an artifact by name, or `None` for a name a build does not upload.
fn cap(name: &str) -> Option<u64> {
    match name {
        "bundle" => Some(MAX_BUNDLE_BYTES),
        "image" => Some(MAX_IMAGE_BYTES),
        "sbom" => Some(MAX_SBOM_BYTES),
        _ => None,
    }
}

/// Writes `body` to `to`, at most `limit` bytes: 413 the moment it passes the cap, the partial
/// file removed.
async fn write_capped(body: Body, to: &FsPath, limit: u64) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;
    let io = |err: std::io::Error| ApiError::Internal(format!("the build's upload: {err}"));
    let mut file = tokio::fs::File::create(to).await.map_err(io)?;
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| ApiError::BadRequest(format!("the upload broke off: {err}")))?;
        written = written.saturating_add(chunk.len() as u64);
        if written > limit {
            drop(file);
            let _ = tokio::fs::remove_file(to).await;
            return Err(ApiError::TooLarge(format!(
                "the upload is larger than the {limit} bytes it may be"
            )));
        }
        file.write_all(&chunk).await.map_err(io)?;
    }
    file.flush().await.map_err(io)?;
    file.sync_all().await.map_err(io)
}

/// `PUT /builds/{id}/artifacts/{name}`: one artifact of the build, replacing an earlier upload of
/// the same name. It lands only while the build runs (see [`build_records::while_running`]).
async fn artifact(
    State(state): State<AppState>,
    Path((id, name)): Path<(i64, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let limit = cap(&name).ok_or_else(|| {
        ApiError::NotFound(format!(
            "a build uploads bundle, image or sbom, not '{name}'"
        ))
    })?;
    let (db, settings, _) = admitted(&state, id, &headers).await?;
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|length| length > limit) {
        return Err(ApiError::TooLarge(format!(
            "{name} is larger than the {limit} bytes it may be"
        )));
    }
    let dir = folder(&settings, id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| ApiError::Internal(format!("the build's folder: {err}")))?;
    let partial = dir.join(format!("{name}.part"));
    write_capped(body, &partial, limit).await?;
    let landed =
        build_records::while_running(&db, id, || std::fs::rename(&partial, dir.join(&name)))
            .await
            .map_err(store)?;
    if !landed {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(ApiError::Conflict(format!(
            "build {id} stopped taking uploads while {name} arrived"
        )));
    }
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// `PUT /builds/{id}/log`: the build's log, at most [`build_records::MAX_LOG_BYTES`].
async fn log(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let (db, _, _) = admitted(&state, id, &headers).await?;
    let bytes = axum::body::to_bytes(body, build_records::MAX_LOG_BYTES)
        .await
        .map_err(|_| {
            ApiError::TooLarge(format!(
                "a build's log is at most {} bytes",
                build_records::MAX_LOG_BYTES
            ))
        })?;
    build_records::set_log(&db, id, &String::from_utf8_lossy(&bytes))
        .await
        .map_err(store)?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// What a build pod says when it is done.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Report {
    /// Whether the build left its artifact and SBOM.
    pub succeeded: bool,
    /// The SDK version the App was built against (`build.json`).
    #[serde(default)]
    pub sdk_version: Option<String>,
    /// Why it failed, in the build's words.
    #[serde(default)]
    pub reason: Option<String>,
}

/// What the Portal made of a finished build.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `POST /builds/{id}/finish`: freezes the build and concludes it. The conclusion runs on a task
/// of its own, so a pod that goes away mid-answer never leaves a build half published.
async fn finish(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(report): Json<Report>,
) -> Result<Json<Outcome>, ApiError> {
    let (db, settings, build) = admitted(&state, id, &headers).await?;
    if !build_records::freeze(&db, id).await.map_err(store)? {
        return Err(ApiError::Conflict(format!(
            "build {id} is no longer running"
        )));
    }
    let concluded = tokio::spawn(conclude(state, db, settings, build, report));
    concluded
        .await
        .map(Json)
        .map_err(|err| ApiError::Internal(format!("the build's conclusion stopped: {err}")))
}

/// Ends a frozen build: published and written, or failed with the reason.
async fn conclude(
    state: AppState,
    db: PgPool,
    settings: Settings,
    build: Record,
    report: Report,
) -> Outcome {
    let dir = folder(&settings, build.id);
    let outcome = if report.succeeded {
        publish(
            &state,
            &dir,
            &build,
            report.sdk_version.as_deref().unwrap_or_default(),
        )
        .await
    } else {
        Err(report
            .reason
            .map(|reason| reason.chars().take(2000).collect())
            .unwrap_or_else(|| "the build failed; its log says why".to_owned()))
    };
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let ended =
        build_records::finish(&db, build.id, outcome.as_deref().map_err(String::as_str)).await;
    if let Err(err) = ended {
        tracing::warn!(build = build.id, error = %err, "the build's end was not recorded");
    }
    match outcome {
        Ok(digest) => {
            tracing::info!(build = build.id, app = %build.app, commit = %build.commit, %digest, "build published");
            Outcome {
                state: "succeeded",
                digest: Some(digest),
                reason: None,
            }
        }
        Err(reason) => {
            tracing::info!(build = build.id, app = %build.app, commit = %build.commit, %reason, "build failed");
            Outcome {
                state: "failed",
                digest: None,
                reason: Some(reason),
            }
        }
    }
}

/// The identity the Portal's own lane writes `status.build` as: no person, no token, no role.
fn lane_identity() -> crate::auth::session::Identity {
    crate::auth::session::Identity {
        subject: "jc-portal:build-lane".into(),
        username: "jc-build-lane".into(),
        email: None,
        name: Some("App build lane".into()),
        roles: Vec::new(),
        groups: Vec::new(),
        client: None,
        second_factor: false,
    }
}

/// Whether `text` could be an SDK version (what `lane.mjs` accepts in `build.json`).
fn is_sdk_version(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// The digest of what a build uploaded: the bundle's SHA-256, or the image layout's one manifest.
pub fn digest_of(artifact: &[u8], image: bool) -> Result<String, String> {
    if !image {
        return Ok(format!("sha256:{:x}", Sha256::digest(artifact)));
    }
    let mut archive = tar::Archive::new(artifact);
    for entry in archive
        .entries()
        .map_err(|e| format!("the image is not a tar: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("the image is not a tar: {e}"))?;
        let path = entry
            .path()
            .map(|p| p.to_string_lossy().trim_start_matches("./").to_owned());
        if path.as_deref().ok() != Some("index.json") {
            continue;
        }
        let mut text = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut text).map_err(|e| e.to_string())?;
        let index: Value =
            serde_json::from_slice(&text).map_err(|e| format!("index.json is not JSON: {e}"))?;
        return match index["manifests"].as_array().map(Vec::as_slice) {
            Some([one]) => one["digest"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "index.json names no digest".to_owned()),
            _ => Err("index.json names not exactly one manifest".to_owned()),
        };
    }
    Err("the image layout has no index.json".to_owned())
}

/// Hashes the frozen uploads, then checks, publishes and writes `status.build` through the
/// propose door as the Portal's lane, holding exactly these bytes. Returns the digest.
async fn publish(
    state: &AppState,
    dir: &FsPath,
    build: &Record,
    sdk: &str,
) -> Result<String, String> {
    if !is_sdk_version(sdk) {
        return Err(format!(
            "the build reported no SDK version it was built against ('{sdk}')"
        ));
    }
    let app = state
        .mirror
        .get(&build.project, "App", &build.app)
        .ok_or_else(|| {
            format!(
                "project {} holds no App '{}' on main any more",
                build.project, build.app
            )
        })?;
    let image = app
        .spec
        .get("kind")
        .and_then(Value::as_str)
        .map(jc_core::kinds::AppClass::parse)
        == Some(Ok(jc_core::kinds::AppClass::UiRust));
    let name = if image { "image" } else { "bundle" };
    let read = |file: &str| {
        std::fs::read(dir.join(file)).map_err(|_| format!("the build uploaded no {file}"))
    };
    let artifact = read(name)?;
    let sbom = read("sbom")?;
    let digest = digest_of(&artifact, image)?;
    let mut body = serde_json::to_value(&app).map_err(|e| e.to_string())?;
    body["status"] = json!({ "build": {
        "digest": digest,
        "commit": build.commit,
        "sdkVersion": sdk,
        "builtAt": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    }});
    let held = Held {
        project: build.project.clone(),
        app: build.app.clone(),
        commit: build.commit.clone(),
        digest: digest.clone(),
        artifact: std::sync::Arc::new(artifact),
        sbom: std::sync::Arc::new(sbom),
    };
    let proposed = built::with_held(
        held,
        crate::api::mutate::propose_with_identity(
            &lane_identity(),
            state,
            &build.project,
            "apps",
            Some(&build.app),
            crate::change::Operation::Update,
            false,
            body,
        ),
    )
    .await;
    match proposed {
        Ok(_) => Ok(digest),
        Err(refused) => Err(refused.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_three_artifacts_have_a_cap() {
        assert_eq!(cap("bundle"), Some(MAX_BUNDLE_BYTES));
        assert_eq!(cap("image"), Some(MAX_IMAGE_BYTES));
        assert_eq!(cap("sbom"), Some(MAX_SBOM_BYTES));
        assert_eq!(cap("../token"), None);
        assert_eq!(cap("report"), None);
    }

    #[test]
    fn an_sdk_version_is_what_build_json_may_hold() {
        assert!(is_sdk_version("0.4.1"));
        assert!(is_sdk_version("1.0.0-rc.1+abc"));
        assert!(!is_sdk_version(""));
        assert!(!is_sdk_version("0.4.1; rm -rf"));
        assert!(!is_sdk_version(&"9".repeat(65)));
    }

    #[test]
    fn the_digest_of_a_bundle_is_its_hash_and_of_an_image_its_one_manifest() {
        assert_eq!(
            digest_of(b"abc", false).expect("a digest"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let layout = |index: &str| {
            let mut tar = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_size(index.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "./index.json", index.as_bytes())
                .expect("append");
            tar.into_inner().expect("tar")
        };
        let one = layout(r#"{"manifests":[{"digest":"sha256:aa"}]}"#);
        assert_eq!(digest_of(&one, true).expect("a digest"), "sha256:aa");
        let two = layout(r#"{"manifests":[{"digest":"sha256:aa"},{"digest":"sha256:bb"}]}"#);
        assert!(digest_of(&two, true).is_err());
        assert!(digest_of(b"not a tar", true).is_err());
    }
}
