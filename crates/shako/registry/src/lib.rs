#![forbid(unsafe_code)]
//! `shako-registry` (Contract S13, Rows 12, 13, 14, Threat T4, T13):
//! - OCI distribution subset at `/v2/` and `/*` (so `jc-router`'s `/v2` prefix-stripped mount
//!   and the forge mount both work):
//!   - `GET /v2/` ping (`Docker-Distribution-Api-Version: registry/2.0`)
//!   - `GET /v2/token` and `POST /v2/token` (ES256 short-lived registry token, `aud = "container_registry"`)
//!   - `HEAD` / `GET /v2/{owner}/{image}/blobs/{digest}`
//!   - `POST /v2/{owner}/{image}/blobs/uploads/` (monolithic with `?digest=`, chunked upload session,
//!     and cross-repo `?mount={digest}&from={from_owner}/{from_repo}` within the SAME owner only!)
//!   - `PATCH` / `PUT /v2/{owner}/{image}/blobs/uploads/{uuid}` (chunked upload completion)
//!   - `PUT` / `GET` / `HEAD /v2/{owner}/{image}/manifests/{reference}` (4 MiB manifest cap,
//!     `Docker-Content-Digest` header, automatic cosign simple-signing ECDSA P-256 signature stored
//!     as `sha256-<hex>.sig` tag, and OCI `subject` indexing)
//!   - `GET /v2/{owner}/{image}/tags/list` and `GET /v2/{owner}/{image}/referrers/{digest}`
//! - Generic package registry: `PUT` / `GET /api/packages/{owner}/generic/{pkg}/{ver}/{file}`
//!   (no overwrite -> `409 Conflict`, SHA-256 recorded).

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use chrono::Utc;
use jc_router::Mount;
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{Value, json};
use shako_api::{
    ShakoState, authenticate_request, emit_and_dispatch_event, gitea_error_response,
    read_body_bounded, stream_bytes_in_64k_chunks,
};
use shako_core::{
    Action, BlobRecord, ObjectTarget, PackageFileRecord, PackageId, PackageRecord,
    PackageVersionRecord, RequestCtx, ShakoError, ShakoResult, TokenVerifier,
    remaining_storage_allowance, sha256_hex, validate_package_segment, validate_repo_or_owner_name,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

pub const MAX_OCI_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PACKAGE_BLOB_BYTES: usize = 256 * 1024 * 1024;

/// Open chunked upload sessions: upload id -> `(owner, image, bytes received so far)`.
type ChunkedUploads = RwLock<BTreeMap<String, (String, String, Vec<u8>)>>;

static CHUNKED_UPLOADS: LazyLock<ChunkedUploads> = LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn oci_error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    realm: Option<&str>,
) -> Response {
    let body = json!({
        "errors": [{
            "code": code,
            "message": message,
        }]
    });
    if let Some(r) = realm {
        let www = format!("Bearer realm=\"{r}\",service=\"container_registry\"");
        return (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::HeaderName::from_static("docker-distribution-api-version"),
                    "registry/2.0",
                ),
                (header::WWW_AUTHENTICATE, www.as_str()),
            ],
            Json(body),
        )
            .into_response();
    }
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::HeaderName::from_static("docker-distribution-api-version"),
                "registry/2.0",
            ),
        ],
        Json(body),
    )
        .into_response()
}

fn authenticate_oci(
    state: &ShakoState,
    headers: &HeaderMap,
    owner: Option<&str>,
) -> ShakoResult<RequestCtx> {
    // Accept either a `/v2/token` bearer (`aud = "container_registry"`) or a `shako` token
    if let Ok(ctx) = authenticate_request(state, headers, owner, "container_registry") {
        return Ok(ctx);
    }
    authenticate_request(state, headers, owner, "shako")
}

fn authorize_package(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg_type: &str,
    image_or_pkg: &str,
    action: Action,
) -> ShakoResult<()> {
    // `owner` names a directory under the blob store: refuse anything but a plain name (T1);
    // a percent-encoded `../` arrives decoded in the path parameter.
    validate_repo_or_owner_name(owner, "owner")
        .map_err(|_| ShakoError::NotFound("package not found".into()))?;
    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    let mut check_ctx = ctx.clone();
    check_ctx.org_id = target_org;
    let target = ObjectTarget::Package {
        instance_id: ctx.instance_id,
        org_id: target_org,
        owner: owner.to_owned(),
        package_type: pkg_type.to_owned(),
        name: image_or_pkg.to_owned(),
    };
    state
        .authorizer
        .check(&check_ctx, action, &target)
        .into_result("package not found")
}

fn blob_disk_path(state: &ShakoState, owner: &str, sha256_hex_digest: &str) -> PathBuf {
    let hex_part = sha256_hex_digest
        .strip_prefix("sha256:")
        .unwrap_or(sha256_hex_digest);
    state
        .repo_root()
        .join(state.default_instance_id.to_string())
        .join("blobs")
        .join(owner)
        .join("sha256")
        .join(hex_part)
}

fn validate_sha256_digest(digest: &str) -> ShakoResult<String> {
    let Some(hex_part) = digest.strip_prefix("sha256:") else {
        return Err(ShakoError::BadRequest(format!(
            "digest '{digest}' must start with sha256:"
        )));
    };
    if hex_part.len() != 64 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ShakoError::BadRequest(format!(
            "invalid sha256 digest '{digest}'"
        )));
    }
    Ok(format!("sha256:{}", hex_part.to_ascii_lowercase()))
}

// =========================================================================
// `/v2/` ping and `/v2/token` (Row 13)
// =========================================================================

async fn oci_v2_ping(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Response {
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let token_realm = format!("{}://{}/v2/token", mount.scheme, mount.host);
    match authenticate_oci(&state, &headers, None) {
        Ok(_) => (
            StatusCode::OK,
            [(
                header::HeaderName::from_static("docker-distribution-api-version"),
                "registry/2.0",
            )],
            Json(json!({})),
        )
            .into_response(),
        Err(_) => oci_error_response(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "authentication required",
            Some(&token_realm),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciTokenQuery {
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub offline_token: Option<String>,
}

async fn oci_get_token(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Query(q): Query<OciTokenQuery>,
) -> Response {
    let auth_hdr = match headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h,
        None => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "missing credentials",
                None,
            );
        }
    };
    let Some(raw_token) = TokenVerifier::extract_raw_token(auth_hdr) else {
        return oci_error_response(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "invalid credentials",
            None,
        );
    };
    let subject = match state.token_verifier.verify(&raw_token, "shako") {
        Ok(s) => s,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "invalid token",
                None,
            );
        }
    };

    let scope = q.scope.unwrap_or_default();
    let mut granted_scope = String::new();
    let mut target_org = subject
        .organizations
        .first()
        .copied()
        .unwrap_or(state.default_org_id);

    // Validate requested scope `repository:{owner}/{image}:{actions}` against Authorizer (T4)
    if let Some(rest) = scope.strip_prefix("repository:")
        && let Some((repo_part, actions_part)) = rest.rsplit_once(':')
        && let Some((owner, image)) = repo_part.split_once('/')
    {
        target_org = state.resolve_org_for_owner(owner, &subject);
        let ctx = RequestCtx::new(state.default_instance_id, target_org, subject.clone());
        let mut allowed_actions = Vec::new();
        for act in actions_part.split(',') {
            match act.trim() {
                "pull" => {
                    if authorize_package(&state, &ctx, owner, "container", image, Action::Read)
                        .is_ok()
                    {
                        allowed_actions.push("pull");
                    }
                }
                "push"
                    if authorize_package(
                        &state,
                        &ctx,
                        owner,
                        "container",
                        image,
                        Action::Publish,
                    )
                    .is_ok() =>
                {
                    allowed_actions.push("push");
                }
                _ => {}
            }
        }
        if allowed_actions.is_empty() {
            return oci_error_response(
                StatusCode::FORBIDDEN,
                "DENIED",
                "requested access to the resource is denied",
                None,
            );
        }
        granted_scope = format!("repository:{owner}/{image}:{}", allowed_actions.join(","));
    }

    match state
        .signing_key()
        .mint_registry_token(&subject, target_org, &granted_scope, 900)
    {
        Ok(jwt) => (
            StatusCode::OK,
            Json(json!({
                "token": jwt,
                "access_token": jwt,
                "expires_in": 900,
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

/// `POST /v2/token` — containerd's OAuth token probe (`dep:components/gitea/apisix-routes.yaml:37-49`, T-2665).
/// Answers 404 JSON (never a 302 HTML redirect!) so containerd falls back to `GET /v2/token`.
async fn oci_post_token(State(state): State<ShakoState>) -> Response {
    gitea_error_response(&state, &ShakoError::NotFound("use GET /v2/token".into()))
}

// =========================================================================
// Row 14: OCI Blobs, Uploads, Manifests, Tags, Referrers, Cosign Signing
// =========================================================================

async fn oci_head_or_get_blob(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    method: axum::http::Method,
    Path((owner, image, digest)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Read).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let clean_digest = match validate_sha256_digest(&digest) {
        Ok(d) => d,
        Err(_) => {
            return oci_error_response(
                StatusCode::BAD_REQUEST,
                "DIGEST_INVALID",
                "invalid digest",
                None,
            );
        }
    };

    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let Some(blob_rec) = state
        .store()
        .get_blob(
            ctx.instance_id,
            target_org,
            &owner,
            Some(&image),
            &clean_digest,
        )
        .await
        .ok()
        .flatten()
    else {
        return oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "blob unknown", None);
    };

    let disk_path = blob_disk_path(&state, &owner, &clean_digest);
    if method == axum::http::Method::HEAD {
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::HeaderName::from_static("docker-content-digest"),
                    clean_digest.as_str(),
                ),
                (
                    header::CONTENT_LENGTH,
                    blob_rec.size_bytes.to_string().as_str(),
                ),
            ],
            Body::empty(),
        )
            .into_response();
    }

    let Ok(bytes) = tokio::fs::read(&disk_path).await else {
        return oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "blob unknown", None);
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::HeaderName::from_static("docker-content-digest"),
                clean_digest.as_str(),
            ),
        ],
        stream_bytes_in_64k_chunks(bytes),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadQuery {
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub mount: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
}

async fn oci_post_blob_upload(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image)): Path<(String, String)>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if let Err(e) = validate_repo_or_owner_name(&owner, "owner") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_package_segment(&image, "image") {
        return gitea_error_response(&state, &e);
    }
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Publish).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);

    // 1. Cross-repo blob mount (`?mount={digest}&from={from_owner}/{from_repo}`):
    //    Contract S13 & Threat T13: mount allowed ONLY within the SAME `owner`!
    if let (Some(mount_digest), Some(from_repo)) = (&q.mount, &q.from) {
        let Ok(clean_digest) = validate_sha256_digest(mount_digest) else {
            return oci_error_response(
                StatusCode::BAD_REQUEST,
                "DIGEST_INVALID",
                "invalid digest",
                None,
            );
        };
        let from_owner = from_repo
            .split_once('/')
            .map_or(owner.as_str(), |(fo, _)| fo);
        if from_owner != owner {
            return oci_error_response(
                StatusCode::FORBIDDEN,
                "DENIED",
                "cross-owner blob mount is forbidden",
                None,
            );
        }
        if let Ok(Some(existing)) = state
            .store()
            .get_blob(ctx.instance_id, target_org, &owner, None, &clean_digest)
            .await
        {
            let mounted = BlobRecord {
                id: uuid::Uuid::now_v7(),
                instance_id: ctx.instance_id,
                org_id: target_org,
                owner: owner.clone(),
                repo_name: image.clone(),
                digest: clean_digest.clone(),
                size_bytes: existing.size_bytes,
                created_at: Utc::now(),
            };
            let _ = state.store().upsert_blob(mounted).await;
            return (
                StatusCode::CREATED,
                [
                    (
                        header::LOCATION,
                        format!("/v2/{owner}/{image}/blobs/{clean_digest}").as_str(),
                    ),
                    (
                        header::HeaderName::from_static("docker-content-digest"),
                        clean_digest.as_str(),
                    ),
                ],
                Body::empty(),
            )
                .into_response();
        }
    }

    // 2. Monolithic upload (`?digest=sha256:...`, Row 14 `gitea.rs:1569`)
    if let Some(raw_digest) = &q.digest {
        let Ok(clean_digest) = validate_sha256_digest(raw_digest) else {
            return oci_error_response(
                StatusCode::BAD_REQUEST,
                "DIGEST_INVALID",
                "invalid digest",
                None,
            );
        };
        let Ok(bytes) = read_body_bounded(body, MAX_PACKAGE_BLOB_BYTES).await else {
            return oci_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "SIZE_INVALID",
                "blob too large",
                None,
            );
        };
        return finish_blob_store(
            &state,
            ctx.instance_id,
            target_org,
            &owner,
            &image,
            &clean_digest,
            &bytes,
        )
        .await;
    }

    // 3. Start chunked upload session
    let upload_uuid = uuid::Uuid::now_v7().to_string();
    let initial_bytes = read_body_bounded(body, MAX_PACKAGE_BLOB_BYTES)
        .await
        .unwrap_or_default();
    let len = initial_bytes.len();
    CHUNKED_UPLOADS.write().insert(
        upload_uuid.clone(),
        (owner.clone(), image.clone(), initial_bytes),
    );
    let loc = format!("/v2/{owner}/{image}/blobs/uploads/{upload_uuid}");
    let range = if len > 0 {
        format!("0-{}", len - 1)
    } else {
        "0-0".into()
    };
    (
        StatusCode::ACCEPTED,
        [
            (header::LOCATION, loc.as_str()),
            (
                header::HeaderName::from_static("docker-upload-uuid"),
                upload_uuid.as_str(),
            ),
            (header::RANGE, range.as_str()),
        ],
        Body::empty(),
    )
        .into_response()
}

async fn oci_patch_blob_upload(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image, upload_uuid)): Path<(String, String, String)>,
    body: Body,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Publish).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let Ok(chunk) = read_body_bounded(body, MAX_PACKAGE_BLOB_BYTES).await else {
        return oci_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "SIZE_INVALID",
            "chunk too large",
            None,
        );
    };
    let mut map = CHUNKED_UPLOADS.write();
    let Some((u_owner, u_image, buf)) = map.get_mut(&upload_uuid) else {
        return oci_error_response(
            StatusCode::NOT_FOUND,
            "BLOB_UPLOAD_UNKNOWN",
            "upload unknown",
            None,
        );
    };
    if u_owner != &owner || u_image != &image {
        return oci_error_response(
            StatusCode::NOT_FOUND,
            "BLOB_UPLOAD_UNKNOWN",
            "upload unknown",
            None,
        );
    }
    buf.extend_from_slice(&chunk);
    let end = buf.len().saturating_sub(1);
    let loc = format!("/v2/{owner}/{image}/blobs/uploads/{upload_uuid}");
    let range = format!("0-{end}");
    (
        StatusCode::ACCEPTED,
        [
            (header::LOCATION, loc.as_str()),
            (
                header::HeaderName::from_static("docker-upload-uuid"),
                upload_uuid.as_str(),
            ),
            (header::RANGE, range.as_str()),
        ],
        Body::empty(),
    )
        .into_response()
}

async fn oci_put_blob_upload(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image, upload_uuid)): Path<(String, String, String)>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Publish).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let Some(raw_digest) = &q.digest else {
        return oci_error_response(
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
            "missing digest query param",
            None,
        );
    };
    let Ok(clean_digest) = validate_sha256_digest(raw_digest) else {
        return oci_error_response(
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
            "invalid digest",
            None,
        );
    };
    let Ok(final_chunk) = read_body_bounded(body, MAX_PACKAGE_BLOB_BYTES).await else {
        return oci_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "SIZE_INVALID",
            "blob too large",
            None,
        );
    };
    let Some((u_owner, u_image, mut buf)) = CHUNKED_UPLOADS.write().remove(&upload_uuid) else {
        return oci_error_response(
            StatusCode::NOT_FOUND,
            "BLOB_UPLOAD_UNKNOWN",
            "upload unknown",
            None,
        );
    };
    if u_owner != owner || u_image != image {
        return oci_error_response(
            StatusCode::NOT_FOUND,
            "BLOB_UPLOAD_UNKNOWN",
            "upload unknown",
            None,
        );
    }
    buf.extend_from_slice(&final_chunk);
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    finish_blob_store(
        &state,
        ctx.instance_id,
        target_org,
        &owner,
        &image,
        &clean_digest,
        &buf,
    )
    .await
}

async fn finish_blob_store(
    state: &ShakoState,
    instance_id: shako_core::InstanceId,
    org_id: shako_core::OrgId,
    owner: &str,
    image: &str,
    expected_digest: &str,
    bytes: &[u8],
) -> Response {
    // Verify digest while streaming/storing (Threat T13)
    let computed = format!("sha256:{}", sha256_hex(bytes));
    if computed != expected_digest {
        return oci_error_response(
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
            "blob bytes do not match declared digest",
            None,
        );
    }
    let disk_path = blob_disk_path(state, owner, &computed);
    if let Some(parent) = disk_path.parent()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        return oci_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "UNKNOWN",
            "storage error",
            None,
        );
    }
    if tokio::fs::write(&disk_path, bytes).await.is_err() {
        return oci_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "UNKNOWN",
            "storage write failed",
            None,
        );
    }

    let rec = BlobRecord {
        id: uuid::Uuid::now_v7(),
        instance_id,
        org_id,
        owner: owner.to_owned(),
        repo_name: image.to_owned(),
        digest: computed.clone(),
        size_bytes: bytes.len() as u64,
        created_at: Utc::now(),
    };
    let _ = state.store().upsert_blob(rec).await;
    let _ = state
        .store()
        .adjust_quota_usage(instance_id, org_id, 0, 0, bytes.len() as i64)
        .await;

    let loc = format!("/v2/{owner}/{image}/blobs/{computed}");
    (
        StatusCode::CREATED,
        [
            (header::LOCATION, loc.as_str()),
            (
                header::HeaderName::from_static("docker-content-digest"),
                computed.as_str(),
            ),
        ],
        Body::empty(),
    )
        .into_response()
}

async fn oci_put_manifest(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image, reference)): Path<(String, String, String)>,
    body: Body,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if let Err(e) = validate_repo_or_owner_name(&owner, "owner") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_package_segment(&image, "image") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_package_segment(&reference, "reference") {
        return gitea_error_response(&state, &e);
    }
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Publish).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }

    // Cap manifest at 4 MiB (Contract S13, Threat T13)
    let Ok(manifest_bytes) = read_body_bounded(body, MAX_OCI_MANIFEST_BYTES).await else {
        return oci_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "MANIFEST_INVALID",
            "manifest exceeds 4 MiB cap",
            None,
        );
    };
    let Ok(manifest_json) = serde_json::from_slice::<Value>(&manifest_bytes) else {
        return oci_error_response(
            StatusCode::BAD_REQUEST,
            "MANIFEST_INVALID",
            "manifest is not valid JSON",
            None,
        );
    };

    let hex_digest = sha256_hex(&manifest_bytes);
    let computed_digest = format!("sha256:{hex_digest}");
    if reference.starts_with("sha256:") && reference != computed_digest {
        return oci_error_response(
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
            "manifest digest mismatch",
            None,
        );
    }

    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .or_else(|| manifest_json.get("mediaType").and_then(Value::as_str))
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_owned();

    let subject_digest = manifest_json
        .get("subject")
        .and_then(|s| s.get("digest"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let artifact_type = manifest_json
        .get("artifactType")
        .and_then(Value::as_str)
        .or_else(|| {
            manifest_json
                .get("config")
                .and_then(|c| c.get("mediaType"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned);

    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let now = Utc::now();
    let pkg = match state
        .store()
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            owner: owner.clone(),
            package_type: "container".into(),
            name: image.clone(),
            created_at: now,
        })
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let ver_rec = PackageVersionRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: ctx.instance_id,
        org_id: target_org,
        package_id: pkg.id,
        owner: owner.clone(),
        package_type: "container".into(),
        package_name: image.clone(),
        version: reference.clone(),
        digest: computed_digest.clone(),
        media_type,
        manifest_bytes: manifest_bytes.clone(),
        subject_digest,
        artifact_type,
        size_bytes: manifest_bytes.len() as u64,
        signed: true,
        created_at: now,
    };
    let _ = state.store().upsert_package_version(ver_rec).await;

    // Contract S13: automatically sign the image digest with the installation ECDSA P-256 key
    // in cosign simple-signing format and store it as tag `sha256-<hex>.sig`!
    if !reference.ends_with(".sig") {
        let _ = store_cosign_signature_tag(
            &state,
            ctx.instance_id,
            target_org,
            pkg.id,
            &owner,
            &image,
            &computed_digest,
        )
        .await;
    }

    let _ = emit_and_dispatch_event(
        &state,
        target_org,
        None,
        "package.publish",
        format!("oci:{owner}/{image}:{computed_digest}"),
        json!({"owner": owner, "image": image, "reference": reference, "digest": computed_digest}),
    )
    .await;

    let loc = format!("/v2/{owner}/{image}/manifests/{computed_digest}");
    (
        StatusCode::CREATED,
        [
            (header::LOCATION, loc.as_str()),
            (
                header::HeaderName::from_static("docker-content-digest"),
                computed_digest.as_str(),
            ),
        ],
        Body::empty(),
    )
        .into_response()
}

async fn store_cosign_signature_tag(
    state: &ShakoState,
    instance_id: shako_core::InstanceId,
    org_id: shako_core::OrgId,
    pkg_id: PackageId,
    owner: &str,
    image: &str,
    image_digest: &str,
) -> ShakoResult<()> {
    let hex_digest = image_digest.strip_prefix("sha256:").unwrap_or(image_digest);
    let simple_signing_payload = json!({
        "critical": {
            "identity": {
                "docker-reference": format!("{owner}/{image}")
            },
            "image": {
                "docker-manifest-digest": image_digest
            },
            "type": "cosign container image signature"
        },
        "optional": {
            "creator": "shako"
        }
    });
    let payload_bytes = serde_json::to_vec(&simple_signing_payload)
        .map_err(|e| ShakoError::Internal(e.to_string()))?;
    let sig_b64 = state.signing_key().sign_cosign_payload(&payload_bytes);
    let payload_digest = format!("sha256:{}", sha256_hex(&payload_bytes));

    // Store the simple-signing payload as a blob
    let disk_path = blob_disk_path(state, owner, &payload_digest);
    if let Some(p) = disk_path.parent() {
        let _ = tokio::fs::create_dir_all(p).await;
    }
    let _ = tokio::fs::write(&disk_path, &payload_bytes).await;
    let _ = state
        .store()
        .upsert_blob(BlobRecord {
            id: uuid::Uuid::now_v7(),
            instance_id,
            org_id,
            owner: owner.to_owned(),
            repo_name: image.to_owned(),
            digest: payload_digest.clone(),
            size_bytes: payload_bytes.len() as u64,
            created_at: Utc::now(),
        })
        .await;

    let sig_tag = format!("sha256-{hex_digest}.sig");
    let sig_manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "size": 2,
            "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        },
        "layers": [{
            "mediaType": "application/vnd.dev.cosign.simplesigning.v1+json",
            "size": payload_bytes.len(),
            "digest": payload_digest,
            "annotations": {
                "dev.cosignproject.cosign/signature": sig_b64
            }
        }]
    });
    let sig_manifest_bytes =
        serde_json::to_vec(&sig_manifest).map_err(|e| ShakoError::Internal(e.to_string()))?;
    let sig_manifest_digest = format!("sha256:{}", sha256_hex(&sig_manifest_bytes));

    state
        .store()
        .upsert_package_version(PackageVersionRecord {
            id: uuid::Uuid::now_v7(),
            instance_id,
            org_id,
            package_id: pkg_id,
            owner: owner.to_owned(),
            package_type: "container".into(),
            package_name: image.to_owned(),
            version: sig_tag,
            digest: sig_manifest_digest,
            media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            manifest_bytes: sig_manifest_bytes.clone(),
            subject_digest: Some(image_digest.to_owned()),
            artifact_type: Some("application/vnd.dev.cosign.simplesigning.v1+json".into()),
            size_bytes: sig_manifest_bytes.len() as u64,
            signed: true,
            created_at: Utc::now(),
        })
        .await?;
    Ok(())
}

async fn oci_get_or_head_manifest(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    method: axum::http::Method,
    Path((owner, image, reference)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Read).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let Some(ver) = state
        .store()
        .get_package_version(
            ctx.instance_id,
            target_org,
            &owner,
            "container",
            &image,
            &reference,
        )
        .await
        .ok()
        .flatten()
    else {
        return oci_error_response(
            StatusCode::NOT_FOUND,
            "MANIFEST_UNKNOWN",
            "manifest unknown",
            None,
        );
    };

    let body = if method == axum::http::Method::HEAD {
        Vec::new()
    } else {
        ver.manifest_bytes
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ver.media_type.as_str()),
            (
                header::HeaderName::from_static("docker-content-digest"),
                ver.digest.as_str(),
            ),
            (header::CONTENT_LENGTH, ver.size_bytes.to_string().as_str()),
        ],
        body,
    )
        .into_response()
}

async fn oci_list_tags(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Read).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let versions = state
        .store()
        .list_package_versions(
            ctx.instance_id,
            target_org,
            Some(&owner),
            Some("container"),
            Some(&image),
        )
        .await
        .unwrap_or_default();
    let tags: Vec<String> = versions
        .into_iter()
        .map(|v| v.version)
        .filter(|v| !v.starts_with("sha256:"))
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "name": format!("{owner}/{image}"),
            "tags": tags,
        })),
    )
        .into_response()
}

async fn oci_list_referrers(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, image, digest)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_oci(&state, &headers, Some(&owner)) {
        Ok(c) => c,
        Err(_) => {
            return oci_error_response(
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "authentication required",
                None,
            );
        }
    };
    if authorize_package(&state, &ctx, &owner, "container", &image, Action::Read).is_err() {
        return oci_error_response(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "not found", None);
    }
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let versions = state
        .store()
        .list_package_versions(
            ctx.instance_id,
            target_org,
            Some(&owner),
            Some("container"),
            Some(&image),
        )
        .await
        .unwrap_or_default();
    let manifests: Vec<Value> = versions
        .into_iter()
        .filter(|v| v.subject_digest.as_deref() == Some(digest.as_str()))
        .map(|v| {
            json!({
                "mediaType": v.media_type,
                "digest": v.digest,
                "size": v.size_bytes,
                "artifactType": v.artifact_type,
            })
        })
        .collect();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.index.v1+json",
        )],
        Json(json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": manifests,
        })),
    )
        .into_response()
}

// =========================================================================
// Row 12: Generic Packages (`/api/packages/{owner}/generic/{pkg}/{ver}/{file}`)
// =========================================================================

async fn generic_put_file(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, pkg, ver, file)): Path<(String, String, String, String)>,
    body: Body,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    for (val, label) in [(&pkg, "package"), (&ver, "version"), (&file, "filename")] {
        if let Err(e) = validate_package_segment(val, label) {
            return gitea_error_response(&state, &e);
        }
    }
    if let Err(e) = authorize_package(&state, &ctx, &owner, "generic", &pkg, Action::Publish) {
        return gitea_error_response(&state, &e);
    }

    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    if let Ok(Some(_)) = state
        .store()
        .get_package_file(ctx.instance_id, target_org, &owner, &pkg, &ver, &file)
        .await
    {
        return gitea_error_response(
            &state,
            &ShakoError::Conflict("package file already exists".into()),
        );
    }

    let instance = state.instance_for(ctx.instance_id, target_org).await;
    let usage = match state
        .store()
        .get_quota_usage(ctx.instance_id, target_org)
        .await
    {
        Ok(u) => u,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let max_bytes = match remaining_storage_allowance(&instance, &usage) {
        Ok(m) => (m as usize).min(MAX_PACKAGE_BLOB_BYTES),
        Err(e) => return gitea_error_response(&state, &e),
    };

    let bytes = match read_body_bounded(body, max_bytes).await {
        Ok(b) => b,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let sha256 = sha256_hex(&bytes);
    let disk_path = blob_disk_path(&state, &owner, &sha256);
    if let Some(p) = disk_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(p).await
    {
        return gitea_error_response(&state, &ShakoError::Storage(e.to_string()));
    }
    if let Err(e) = tokio::fs::write(&disk_path, &bytes).await {
        return gitea_error_response(&state, &ShakoError::Storage(e.to_string()));
    }

    let now = Utc::now();
    let pkg_rec = match state
        .store()
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            owner: owner.clone(),
            package_type: "generic".into(),
            name: pkg.clone(),
            created_at: now,
        })
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let ver_rec = match state
        .store()
        .upsert_package_version(PackageVersionRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            package_id: pkg_rec.id,
            owner: owner.clone(),
            package_type: "generic".into(),
            package_name: pkg.clone(),
            version: ver.clone(),
            digest: format!("sha256:{sha256}"),
            media_type: "application/octet-stream".into(),
            manifest_bytes: Vec::new(),
            subject_digest: None,
            artifact_type: None,
            size_bytes: bytes.len() as u64,
            signed: false,
            created_at: now,
        })
        .await
    {
        Ok(v) => v,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let file_rec = PackageFileRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: ctx.instance_id,
        org_id: target_org,
        version_id: ver_rec.id,
        owner: owner.clone(),
        package_name: pkg,
        version: ver,
        filename: file,
        sha256,
        size_bytes: bytes.len() as u64,
        created_at: now,
    };

    match state.store().insert_package_file(file_rec).await {
        Ok(saved) => {
            let _ = state
                .store()
                .adjust_quota_usage(ctx.instance_id, target_org, 0, 0, bytes.len() as i64)
                .await;
            (StatusCode::CREATED, Json(saved)).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn generic_get_file(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, pkg, ver, file)): Path<(String, String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = authorize_package(&state, &ctx, &owner, "generic", &pkg, Action::Read) {
        return gitea_error_response(&state, &e);
    }
    let target_org = state.resolve_org_for_owner(&owner, &ctx.subject);
    let Some(file_rec) = state
        .store()
        .get_package_file(ctx.instance_id, target_org, &owner, &pkg, &ver, &file)
        .await
        .ok()
        .flatten()
    else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("package file not found".into()),
        );
    };

    let disk_path = blob_disk_path(&state, &owner, &file_rec.sha256);
    let Ok(bytes) = tokio::fs::read(&disk_path).await else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("package blob not found".into()),
        );
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        stream_bytes_in_64k_chunks(bytes),
    )
        .into_response()
}

/// Builds the OCI `/v2/*` router for both `/v2/...` (inside the main router) and `/...`
/// (when mounted at `path_prefix = "/v2"` by `jc-router`, which strips `/v2`, Contract S1 & S13).
pub fn oci_v2_routes() -> Router<ShakoState> {
    Router::new()
        // Prefixed `/v2/*` routes
        .route("/v2", get(oci_v2_ping))
        .route("/v2/", get(oci_v2_ping))
        .route("/v2/token", get(oci_get_token).post(oci_post_token))
        .route(
            "/v2/{owner}/{image}/blobs/uploads/",
            post(oci_post_blob_upload),
        )
        .route(
            "/v2/{owner}/{image}/blobs/uploads",
            post(oci_post_blob_upload),
        )
        .route(
            "/v2/{owner}/{image}/blobs/uploads/{uuid}",
            patch(oci_patch_blob_upload).put(oci_put_blob_upload),
        )
        .route(
            "/v2/{owner}/{image}/blobs/{digest}",
            get(oci_head_or_get_blob).head(oci_head_or_get_blob),
        )
        .route(
            "/v2/{owner}/{image}/manifests/{reference}",
            put(oci_put_manifest)
                .get(oci_get_or_head_manifest)
                .head(oci_get_or_head_manifest),
        )
        .route("/v2/{owner}/{image}/tags/list", get(oci_list_tags))
        .route(
            "/v2/{owner}/{image}/referrers/{digest}",
            get(oci_list_referrers),
        )
        // Generic packages (Row 12)
        .route(
            "/api/packages/{owner}/generic/{pkg}/{ver}/{file}",
            put(generic_put_file).get(generic_get_file),
        )
}

/// Stripped-prefix `/v2` router mounted at `MountSpec { path_prefix: Some("/v2") }` in `bins/jc` (Contract S1).
pub fn stripped_v2_registry_routes() -> Router<ShakoState> {
    Router::new()
        .route("/", get(oci_v2_ping))
        .route("/token", get(oci_get_token).post(oci_post_token))
        .route(
            "/{owner}/{image}/blobs/uploads/",
            post(oci_post_blob_upload),
        )
        .route("/{owner}/{image}/blobs/uploads", post(oci_post_blob_upload))
        .route(
            "/{owner}/{image}/blobs/uploads/{uuid}",
            patch(oci_patch_blob_upload).put(oci_put_blob_upload),
        )
        .route(
            "/{owner}/{image}/blobs/{digest}",
            get(oci_head_or_get_blob).head(oci_head_or_get_blob),
        )
        .route(
            "/{owner}/{image}/manifests/{reference}",
            put(oci_put_manifest)
                .get(oci_get_or_head_manifest)
                .head(oci_get_or_head_manifest),
        )
        .route("/{owner}/{image}/tags/list", get(oci_list_tags))
        .route(
            "/{owner}/{image}/referrers/{digest}",
            get(oci_list_referrers),
        )
        .merge(oci_v2_routes())
}
