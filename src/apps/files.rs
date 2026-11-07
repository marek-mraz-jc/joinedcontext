//! `/apps/{name}/api/services/files…`: the objects of an App that is not `wasm`, under its own
//! prefix `apps/<shard>/<id>/` of the apps bucket, as a `wasm` App keeps them through
//! `jc:app/blob` (AP-170, AP-145, API/06 §3, T-3584).
//!
//! The App names a key relative to its prefix and never sees the bucket or a credential: the
//! Portal checks the key, adds the prefix and signs with the store's root credential. Each object
//! is at most 25 MiB and the prefix holds at most the App's `filesMiB`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jc_core::kinds::AppService;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::apps_db::app_id;
use super::services::{self, Quota};
use crate::artifact_store::Client as Store;
use crate::error::ApiError;
use crate::state::AppState;

/// One stored PUT at a time per App, from its quota check to its write, so concurrent uploads
/// that each fit alone never pass `filesMiB` together (F-POR2-016).
static PUTS: std::sync::LazyLock<crate::keyed::Keyed> = std::sync::LazyLock::new(Default::default);

/// The largest object one call stores (AP-170).
pub const MAX_OBJECT_BYTES: usize = 25 * 1024 * 1024;
/// The body limit of the route: one byte over the object's, so that byte reaches the handler and
/// is answered with the problem rather than axum's bare 413.
pub const MAX_BODY_BYTES: usize = MAX_OBJECT_BYTES + 1;
/// The longest key below the App's prefix, as `jc:app/blob` has it.
const MAX_KEY_BYTES: usize = 512;
/// How long a presigned URL holds (AP-145).
const PRESIGN_SECONDS: u32 = 300;
const MIB: u64 = 1024 * 1024;

/// An App's key, checked as `jc:app/blob` checks it: relative, no empty, `.` or `..` segment, no
/// backslash or control character, at most 512 bytes. Nothing is rewritten.
pub fn checked(key: &str) -> Result<&str, String> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(format!("a key is 1 to {MAX_KEY_BYTES} bytes"));
    }
    if key.starts_with('/') {
        return Err("a key is relative to the application's own prefix".into());
    }
    if key.chars().any(|c| c.is_control() || c == '\\') {
        return Err("a key holds no control character or backslash".into());
    }
    if key
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("a key has no empty, `.` or `..` segment".into());
    }
    Ok(key)
}

/// A listing prefix: empty, a checked key, or one ending in `/`.
fn checked_prefix(prefix: &str) -> Result<&str, String> {
    if prefix.is_empty() {
        return Ok(prefix);
    }
    checked(prefix.strip_suffix('/').unwrap_or(prefix)).map(|_| prefix)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    path: String,
    size: u64,
    content_type: String,
    modified_at: String,
}

/// The App's place in the store, once the gate let the call through.
struct Files {
    store: Arc<Store>,
    bucket: String,
    prefix: String,
    project: String,
    app: String,
}

fn bad(detail: impl Into<String>) -> Response {
    ApiError::BadRequest(detail.into()).into_response()
}

fn unavailable(err: impl std::fmt::Display) -> Response {
    // The store's answer names the bucket and the key, not a credential; the key stays in it.
    tracing::warn!(error = %err, "an App's files did not reach the store");
    ApiError::Unavailable("the file store did not answer".into()).into_response()
}

async fn open(
    state: &AppState,
    (headers, uri): (&HeaderMap, &Uri),
    name: &str,
    method: &Method,
) -> Result<Files, Box<Response>> {
    let (project, _, _) =
        services::gate(state, headers, uri, name, (AppService::Files, method)).await?;
    let wasm = state.wasm_apps.as_ref().ok_or_else(|| {
        Box::new(
            ApiError::Unavailable("this installation stores no App files".into()).into_response(),
        )
    })?;
    // A `wasm` App's shard is its placement; every other App is on shard 0, which only names
    // the prefix, since the Portal writes with the root credential.
    let shard = state
        .mirror
        .get(&project, "App", name)
        .and_then(|envelope| envelope.status)
        .and_then(|status| status.shard)
        .unwrap_or(0);
    Ok(Files {
        store: Arc::clone(&wasm.store),
        bucket: wasm.bucket.clone(),
        prefix: format!("apps/{shard}/{}/", app_id(&project, name)),
        project,
        app: name.to_owned(),
    })
}

fn content_type(headers: &reqwest::header::HeaderMap) -> String {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Listing {
    prefix: String,
}

/// `GET /api/services/files?prefix={p}`.
pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(name): Path<String>,
    Query(listing): Query<Listing>,
) -> Response {
    let files = match open(&state, (&headers, &uri), &name, &Method::GET).await {
        Ok(files) => files,
        Err(refused) => return *refused,
    };
    let prefix = match checked_prefix(&listing.prefix) {
        Ok(prefix) => prefix,
        Err(why) => return bad(why),
    };
    let objects = match files
        .store
        .list(&files.bucket, &format!("{}{prefix}", files.prefix))
        .await
    {
        Ok(objects) => objects,
        Err(err) => return unavailable(err),
    };
    let mut infos = Vec::with_capacity(objects.len());
    for object in objects {
        let Some(path) = object.key.strip_prefix(&files.prefix) else {
            continue;
        };
        // ponytail: one HEAD per object for its type, which a listing does not carry; keep the
        // type in the App's database when an App holds thousands of objects.
        let kind = match files
            .store
            .object(
                reqwest::Method::HEAD,
                &files.bucket,
                &object.key,
                Vec::new(),
                &[],
                "read an object's type",
            )
            .await
        {
            Ok((_, headers, _)) => content_type(&headers),
            Err(err) => return unavailable(err),
        };
        infos.push(FileInfo {
            path: path.to_owned(),
            size: object.size,
            content_type: kind,
            modified_at: object.modified,
        });
    }
    Json(infos).into_response()
}

/// `GET /api/services/files/{path}`: the object with the type it was stored with.
pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    let files = match open(&state, (&headers, &uri), &name, &Method::GET).await {
        Ok(files) => files,
        Err(refused) => return *refused,
    };
    let key = match checked(&path) {
        Ok(key) => format!("{}{key}", files.prefix),
        Err(why) => return bad(why),
    };
    match files
        .store
        .object(
            reqwest::Method::GET,
            &files.bucket,
            &key,
            Vec::new(),
            &[],
            "read an object",
        )
        .await
    {
        Ok((200, stored, body)) => {
            let mut response = (StatusCode::OK, body).into_response();
            if let Ok(value) = HeaderValue::from_str(&content_type(&stored)) {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            // An App's object is the App's data, never a page of the App's origin.
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            );
            response
        }
        Ok((404, _, _)) => ApiError::NotFound(format!("no file '{path}'")).into_response(),
        Ok((status, _, _)) => unavailable(format!("the store answered {status}")),
        Err(err) => unavailable(err),
    }
}

/// Whether `size` bytes under `key` fit: at most 25 MiB, and the App's `filesMiB` with the object
/// it replaces taken out of the total as this one enters it.
async fn room(
    state: &AppState,
    files: &Files,
    (name, key): (&str, &str),
    size: u64,
) -> Result<(), Box<Response>> {
    if size > MAX_OBJECT_BYTES as u64 {
        return Err(Box::new(services::file_too_large(MAX_OBJECT_BYTES)));
    }
    let Some((_, spec, _)) = super::static_host::published_app(state, name) else {
        return Err(Box::new(
            ApiError::NotFound(format!("app '{name}' not found")).into_response(),
        ));
    };
    let limit = u64::from(services::quota(
        &state.mirror,
        &files.project,
        &spec,
        Quota::FilesMiB,
    )) * MIB;
    let held = files
        .store
        .list(&files.bucket, &files.prefix)
        .await
        .map_err(|err| Box::new(unavailable(err)))?;
    let used: u64 = held
        .iter()
        .filter(|object| object.key != key)
        .map(|object| object.size)
        .sum();
    if used.saturating_add(size) > limit {
        return Err(Box::new(services::quota_used(
            AppService::Files,
            "filesMiB",
            None,
        )));
    }
    Ok(())
}

/// `PUT /api/services/files/{path}`: stores the body under the App's prefix, within 25 MiB and
/// the App's `filesMiB`.
pub(super) async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, path)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let files = match open(&state, (&headers, &uri), &name, &Method::PUT).await {
        Ok(files) => files,
        Err(refused) => return *refused,
    };
    let key = match checked(&path) {
        Ok(key) => format!("{}{key}", files.prefix),
        Err(why) => return bad(why),
    };
    let _turn = PUTS.lock(&files.prefix).await;
    if let Err(refused) = room(&state, &files, (&name, &key), body.len() as u64).await {
        return *refused;
    }
    let kind = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let size = body.len() as u64;
    match files
        .store
        .object(
            reqwest::Method::PUT,
            &files.bucket,
            &key,
            body.to_vec(),
            &[("content-type", kind.clone())],
            "store an object",
        )
        .await
    {
        Ok((200..=299, _, _)) => {}
        Ok((status, _, _)) => return unavailable(format!("the store answered {status}")),
        Err(err) => return unavailable(err),
    }
    tracing::info!(app = %files.app, project = %files.project, size, "app file stored");
    (
        StatusCode::CREATED,
        Json(FileInfo {
            path,
            size,
            content_type: kind,
            modified_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_default(),
        }),
    )
        .into_response()
}

/// `DELETE /api/services/files/{path}`; one already gone is deleted.
pub(super) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    let files = match open(&state, (&headers, &uri), &name, &Method::DELETE).await {
        Ok(files) => files,
        Err(refused) => return *refused,
    };
    let key = match checked(&path) {
        Ok(key) => format!("{}{key}", files.prefix),
        Err(why) => return bad(why),
    };
    match files.store.delete_object(&files.bucket, &key).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => unavailable(err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presign {
    method: String,
    /// The bytes a PUT will store: required for a PUT, refused on a GET.
    #[serde(default)]
    size: Option<u64>,
}

/// `POST /api/services/files/{path}:url`: a URL for one key and one method, for five minutes,
/// on the store's public origin (AP-145). A PUT names its `size`, held to 25 MiB and to what
/// `filesMiB` has left, and the URL signs that `Content-Length`, so the store refuses a body of
/// any other length (AP-170, T-3661). The quota counts the object once it is stored.
pub(super) async fn url(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, path)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let Some(path) = path.strip_suffix(":url").map(str::to_owned) else {
        return ApiError::NotFound(format!(
            "no action on '{path}'; a URL is asked at '{path}:url'"
        ))
        .into_response();
    };
    let files = match open(&state, (&headers, &uri), &name, &Method::POST).await {
        Ok(files) => files,
        Err(refused) => return *refused,
    };
    let key = match checked(&path) {
        Ok(key) => format!("{}{key}", files.prefix),
        Err(why) => return bad(why),
    };
    let (method, size) = match serde_json::from_slice::<Presign>(&body) {
        Ok(Presign { method, size: None }) if method == "GET" => (method, None),
        Ok(Presign {
            method,
            size: Some(size),
        }) if method == "PUT" => (method, Some(size)),
        Ok(Presign { method, .. }) if method == "GET" => return bad("a GET URL names no size"),
        Ok(Presign { method, .. }) if method == "PUT" => {
            return bad("a PUT URL names the size it stores, in bytes")
        }
        Ok(_) => return bad("method is GET or PUT"),
        Err(err) => {
            return bad(format!(
                "the body is not {{ \"method\", \"size\"? }}: {err}"
            ))
        }
    };
    if let Some(size) = size {
        if let Err(refused) = room(&state, &files, (&name, &key), size).await {
            return *refused;
        }
    }
    let Some(origin) = state.config.apps_store_origin.as_deref() else {
        return ApiError::Unavailable("this installation hands out no file URLs".into())
            .into_response();
    };
    let now = chrono::Utc::now();
    let url = files.store.presign(
        (&method, &files.bucket, &key),
        size,
        PRESIGN_SECONDS,
        origin,
        now,
    );
    let expires = now + chrono::Duration::seconds(i64::from(PRESIGN_SECONDS));
    Json(serde_json::json!({
        "url": url,
        "expiresAt": expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_stays_under_the_prefix_or_is_refused() {
        assert_eq!(checked("defects/1.jpg"), Ok("defects/1.jpg"));
        for key in [
            "",
            "/etc/passwd",
            "../other-app/x",
            "a/../../b",
            "a//b",
            "a/./b",
            "a\\b",
            "a\nb",
        ] {
            assert!(checked(key).is_err(), "{key:?}");
        }
        assert!(checked(&"k".repeat(MAX_KEY_BYTES + 1)).is_err());
        assert_eq!(checked_prefix(""), Ok(""));
        assert_eq!(checked_prefix("defects/"), Ok("defects/"));
        assert!(checked_prefix("../").is_err());
    }
}
