//! The files of one open data portal (T-7417): upload, list, rename, replace and delete the
//! images and documents its pages, theme and layout use, under
//! `/api/v1/projects/org/portals/{name}/files`. The files of the Organization's default portal
//! are its Organization-wide files, served on every portal of that Organization.
//!
//! The Portal holds no handle on the file store: jc hands it a [`FileDesk`] at start, which keeps
//! each portal's files under its catalogue's Organization prefix. Every route reads the `Portal`
//! manifest first (one of another Organization, or none, is the same 404; a member without
//! propose on it is 403), and only then reads an upload's body.

use std::sync::{Arc, RwLock};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use jc_core::kinds::{PortalSpec, Verb};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::portal_sources::{portal_for, DeskError, DeskFuture};
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// Largest body a route reads: the store's own cap of one file (10 MiB) plus room to name it in
/// its refusal.
const MAX_BODY: usize = 10 * 1024 * 1024 + 1;

/// One stored file of a portal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalFile {
    /// Stable across a rename or a replace, e.g. `f-0193...`.
    pub id: String,
    /// e.g. `hero.png`.
    pub name: String,
    /// Read from the bytes, never the name: `image/png`, `image/jpeg`, `image/webp`,
    /// `image/svg+xml`, `application/pdf`, `text/csv` or the XLSX type.
    pub content_type: String,
    pub size: u64,
    pub sha256: String,
    /// The address on the portal's hosts, immutable for these bytes: `/files/{sha256}/{name}`.
    pub path: String,
    pub uploaded_by: String,
    /// RFC 3339.
    pub uploaded_at: String,
    /// The portals whose manifest names this file's address; a file in use is not deleted.
    #[serde(default)]
    pub used_by: Vec<String>,
}

/// The limits of the Organization's files and what it holds over all its portals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileLimits {
    pub files: usize,
    pub bytes: u64,
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_bytes: u64,
}

/// The files of a portal.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PortalFiles {
    pub items: Vec<PortalFile>,
    pub limits: FileLimits,
}

/// Why a file write was refused, as the store says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRefusal {
    /// A type, name or cap refusal, naming the field and the limit.
    Invalid(String),
    Desk(DeskError),
}

impl From<DeskError> for FileRefusal {
    fn from(e: DeskError) -> Self {
        Self::Desk(e)
    }
}

/// A boxed future of a [`FileDesk`] write.
pub type FileFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, FileRefusal>> + Send + 'a>>;

/// The store side of a portal's files.
pub trait FileDesk: Send + Sync {
    fn list<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, (Vec<PortalFile>, FileLimits)>;
    fn upload<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        name: &'a str,
        bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile>;
    fn rename<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        id: &'a str,
        name: &'a str,
    ) -> FileFuture<'a, PortalFile>;
    fn replace<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        id: &'a str,
        bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile>;
    fn delete<'a>(&'a self, portal: &'a PortalSpec, by: &'a str, id: &'a str)
        -> FileFuture<'a, ()>;
}

static DESK: RwLock<Option<Arc<dyn FileDesk>>> = RwLock::new(None);

/// Hands the file store's desk to the Portal from now on.
pub fn hand_file_desk_to(desk: Arc<dyn FileDesk>) {
    *DESK.write().unwrap_or_else(|e| e.into_inner()) = Some(desk);
}

fn desk() -> Result<Arc<dyn FileDesk>, ApiError> {
    DESK.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| {
            ApiError::Unavailable(
                "this installation stores no open data portal files; ask the operator to enable \
                 the open data catalogue"
                    .into(),
            )
        })
}

fn refusal(e: FileRefusal) -> ApiError {
    match e {
        FileRefusal::Invalid(detail) => ApiError::Unprocessable {
            errors: vec![detail.clone()],
            detail,
        },
        FileRefusal::Desk(DeskError::NotFound) => {
            ApiError::NotFound("no such file in this portal; reload the list".into())
        }
        FileRefusal::Desk(DeskError::Busy) => {
            ApiError::Conflict("another change of this file is running; try again".into())
        }
        FileRefusal::Desk(DeskError::Failed(detail)) => {
            tracing::warn!(error = %detail, "portal file call failed");
            ApiError::Internal("the file store did not answer; try again later".into())
        }
    }
}

/// The names of the portals of `spec`'s Organization whose manifest, or the Organization's,
/// names `/files/{sha256}/`.
fn used_by(state: &AppState, spec: &PortalSpec, sha256: &str) -> Vec<String> {
    let needle = format!("/files/{sha256}/");
    let page = state.mirror.list(
        ORG_NAMESPACE,
        "Portal",
        &crate::store::ListOptions::default(),
    );
    let mut out: Vec<String> = page
        .items
        .iter()
        .filter(|env| {
            env.spec.get("organizationRef").and_then(|v| v.as_str())
                == Some(spec.organization_ref.as_str())
                && env.spec.to_string().contains(&needle)
        })
        .map(|env| env.metadata.name.clone())
        .collect();
    let org_uses = state
        .mirror
        .get(ORG_NAMESPACE, "Organization", &spec.organization_ref)
        .is_some_and(|env| env.spec.to_string().contains(&needle));
    if org_uses {
        out.push(format!("Organization {}", spec.organization_ref));
    }
    out
}

fn person(user: &CurrentUser) -> String {
    let identity = &user.0.identity;
    identity
        .name
        .clone()
        .unwrap_or_else(|| identity.subject.clone())
}

async fn body_of(request: axum::extract::Request) -> Result<Vec<u8>, ApiError> {
    axum::body::to_bytes(request.into_body(), MAX_BODY)
        .await
        .map(|b| b.to_vec())
        .map_err(|_| {
            ApiError::TooLarge(
                "the file is larger than 10 MiB, the most one portal file may hold".into(),
            )
        })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadName {
    /// The file's display name, e.g. `hero.png`: one line without `/` or `\`, at most 120
    /// characters. Its extension decides nothing; the type is read from the bytes.
    pub name: String,
}

/// A new name for a file.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRename {
    /// e.g. `Titulná fotka.png`.
    pub name: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/org/portals/{name}/files",
    summary = "List Portal Files",
    description = "The portal's files by name, each with the portals that use it, and the Organization's file limits and usage over all its portals (T-7417).",
    tag = "system",
    params(("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`")),
    responses(
        (status = 200, description = "The files and the limits", body = PortalFiles),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization who may not read it", body = ProblemDetails),
        (status = 404, description = "No such portal in the caller's Organization", body = ProblemDetails),
        (status = 503, description = "This installation stores no portal files", body = ProblemDetails)
    )
)]
pub async fn list_portal_files(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<PortalFiles>, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Read)?;
    let (mut items, limits) = desk()?.list(&spec).await.map_err(|e| refusal(e.into()))?;
    for file in &mut items {
        file.used_by = used_by(&state, &spec, &file.sha256);
    }
    Ok(Json(PortalFiles { items, limits }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/org/portals/{name}/files",
    summary = "Upload Portal File",
    description = "Stores the body as a new file of the portal. The type is read from the bytes (PNG, JPEG, WebP, SVG, PDF, CSV or XLSX), never from the name; an SVG with a script, an event handler, an external reference or an entity is refused, and any SVG is served as an attachment. Refused past 10 MiB a file, or past the Organization's 500 files or 200 MiB. Writes an audit row (T-7417).",
    tag = "system",
    params(("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"), UploadName),
    request_body(content = String, content_type = "application/octet-stream", description = "The file's bytes", example = json!("linka;smer\n1;centrum\n")),
    responses(
        (status = 201, description = "The stored file", body = PortalFile),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal in the caller's Organization", body = ProblemDetails),
        (status = 413, description = "A body past 10 MiB", body = ProblemDetails),
        (status = 422, description = "A type, name or limit refusal, naming the limit", body = ProblemDetails),
        (status = 503, description = "This installation stores no portal files", body = ProblemDetails)
    )
)]
pub async fn upload_portal_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(file): Query<UploadName>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Propose)?;
    let desk = desk()?;
    // Read only once the caller may write here.
    let bytes = body_of(request).await?;
    let by = person(&user);
    let stored = desk
        .upload(&spec, &by, &file.name, bytes)
        .await
        .map_err(refusal)?;
    Ok((StatusCode::CREATED, Json(stored)).into_response())
}

#[utoipa::path(
    patch,
    path = "/api/v1/projects/org/portals/{name}/files/{id}",
    summary = "Rename Portal File",
    description = "Gives the file a new display name; its bytes and hash stay. Writes an audit row (T-7417).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("id" = String, Path, description = "The file's id, e.g. `f-0193...`")
    ),
    request_body(content = FileRename, example = json!({"name": "Titulná fotka.png"})),
    responses(
        (status = 200, description = "The renamed file", body = PortalFile),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal or file in the caller's Organization", body = ProblemDetails),
        (status = 422, description = "A name refusal", body = ProblemDetails)
    )
)]
pub async fn rename_portal_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<PortalFile>, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Propose)?;
    // `application/json` or `application/merge-patch+json`: the body is the one member.
    let rename: FileRename = serde_json::from_slice(&body).map_err(|e| {
        ApiError::BadRequest(format!("the body is {{\"name\": \"hero.png\"}}: {e}"))
    })?;
    let by = person(&user);
    let mut file = desk()?
        .rename(&spec, &by, &id, &rename.name)
        .await
        .map_err(refusal)?;
    file.used_by = used_by(&state, &spec, &file.sha256);
    Ok(Json(file))
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/org/portals/{name}/files/{id}",
    summary = "Replace Portal File",
    description = "Stores new bytes for the file under the same id and name: a new hash and so a new immutable address, which a page naming the old address no longer reaches. Same type and limit rules as an upload; writes an audit row (T-7417).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("id" = String, Path, description = "The file's id, e.g. `f-0193...`")
    ),
    request_body(content = String, content_type = "application/octet-stream", description = "The new bytes", example = json!("linka;smer\n1;centrum\n2;nemocnica\n")),
    responses(
        (status = 200, description = "The replaced file", body = PortalFile),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal or file in the caller's Organization", body = ProblemDetails),
        (status = 409, description = "The file is in use: its old address would break", body = ProblemDetails),
        (status = 413, description = "A body past 10 MiB", body = ProblemDetails),
        (status = 422, description = "A type or limit refusal, naming the limit", body = ProblemDetails)
    )
)]
pub async fn replace_portal_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Json<PortalFile>, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Propose)?;
    let desk = desk()?;
    in_use(&state, &spec, desk.as_ref(), &id, "replaced").await?;
    let bytes = body_of(request).await?;
    let by = person(&user);
    let file = desk
        .replace(&spec, &by, &id, bytes)
        .await
        .map_err(refusal)?;
    Ok(Json(file))
}

/// 409 naming where file `id` is used.
async fn in_use(
    state: &AppState,
    spec: &PortalSpec,
    desk: &dyn FileDesk,
    id: &str,
    what: &str,
) -> Result<(), ApiError> {
    let (files, _) = desk.list(spec).await.map_err(|e| refusal(e.into()))?;
    let file = files
        .iter()
        .find(|f| f.id == id)
        .ok_or_else(|| refusal(FileRefusal::Desk(DeskError::NotFound)))?;
    let users = used_by(state, spec, &file.sha256);
    if users.is_empty() {
        return Ok(());
    }
    Err(ApiError::Conflict(format!(
        "file {} is used by {}; take it out there first, then it can be {what}",
        file.name,
        users.join(", ")
    )))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/org/portals/{name}/files/{id}",
    summary = "Delete Portal File",
    description = "Deletes the file and its bytes; refused while a portal or the Organization names its address. Writes an audit row (T-7417).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("id" = String, Path, description = "The file's id, e.g. `f-0193...`")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal or file in the caller's Organization", body = ProblemDetails),
        (status = 409, description = "The file is in use, naming where", body = ProblemDetails)
    )
)]
pub async fn delete_portal_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Propose)?;
    let desk = desk()?;
    in_use(&state, &spec, desk.as_ref(), &id, "deleted").await?;
    let by = person(&user);
    desk.delete(&spec, &by, &id).await.map_err(refusal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/org/portals/{name}/files",
            get(list_portal_files)
                .post(upload_portal_file)
                .layer(DefaultBodyLimit::max(MAX_BODY)),
        )
        .route(
            "/projects/org/portals/{name}/files/{id}",
            put(replace_portal_file)
                .patch(rename_portal_file)
                .delete(delete_portal_file)
                .layer(DefaultBodyLimit::max(MAX_BODY)),
        )
}
