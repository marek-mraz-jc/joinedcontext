#![forbid(unsafe_code)]
//! `shako-server`: Mountable routers (`router(state, mount)` and `registry_router(state, mount)`),
//! security headers + per-response CSP nonce middleware, declarative manifest `apply -f` / `check -f`,
//! and unified CLI dispatcher (Contract S1, S6, S8, S21).

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::Utc;
use jc_router::Mount;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use shako_api::ShakoState;
use shako_api::{api_router, authenticate_request, exec_odata_operation, odata_error_response};
use shako_core::{
    BranchProtectionRule, InstanceRecord, OrgId, ProtectionRuleRecord, PushMirrorRecord, RepoId,
    RepoRecord, RequestCtx, ShakoError, ShakoResult, WebhookFormat, WebhookId, WebhookRecord,
    validate_branch_name, validate_repo_or_owner_name,
};
use shako_git::{compact_repository, fsck_repository, init_bare_repo};
use shako_mcp::{call_mcp_tool, list_mcp_tools};
use shako_registry::{oci_v2_routes, stripped_v2_registry_routes};
use shako_web::web_router;

#[derive(Clone)]
struct DefaultMount(Mount);

/// Per-response CSP nonce and security headers middleware (THINK-CHECKLIST 3, S18, S21).
async fn security_and_mount_headers(
    Extension(default_mount): Extension<DefaultMount>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // Handlers read `Extension<Mount>`: `jc-router` inserts it; a standalone router gets the
    // mount it was built with, so links and redirects honour the configured prefix (S1).
    let mount = match req.extensions().get::<Mount>() {
        Some(m) => m.clone(),
        None => {
            req.extensions_mut().insert(default_mount.0.clone());
            default_mount.0
        }
    };
    let public_base = mount.public_base();
    let cookie_name = mount.cookie_name("shako_session");

    let mut nonce_bytes = [0u8; 16];
    rand::fill(&mut nonce_bytes);
    let nonce = hex::encode(nonce_bytes);
    let csp = format!(
        "default-src 'none'; style-src 'self' 'nonce-{nonce}'; img-src 'self'; frame-ancestors 'self'; object-src 'none'; base-uri 'self'; form-action 'self'"
    );

    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    if let Ok(hv) = HeaderValue::from_str(&csp) {
        headers.entry("content-security-policy").or_insert(hv);
    }
    headers
        .entry("x-content-type-options")
        .or_insert_with(|| HeaderValue::from_static("nosniff"));
    headers
        .entry("x-frame-options")
        .or_insert_with(|| HeaderValue::from_static("SAMEORIGIN"));
    headers
        .entry("referrer-policy")
        .or_insert_with(|| HeaderValue::from_static("strict-origin-when-cross-origin"));
    if let Ok(hv) = HeaderValue::from_str(&public_base) {
        headers.insert("x-shako-public-base", hv);
    }
    if let Ok(hv) = HeaderValue::from_str(&cookie_name) {
        headers.insert("x-shako-cookie-name", hv);
    }
    resp
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpCallReq {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn http_mcp_list(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let tools = list_mcp_tools(&state, &ctx);
    (StatusCode::OK, Json(json!({ "tools": tools }))).into_response()
}

async fn http_mcp_call(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(req): Json<McpCallReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    match call_mcp_tool(&state, &ctx, &mount, &req.name, req.arguments).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(e) => odata_error_response(&e),
    }
}

/// Builds the main `shako` router honouring `mount` (Contract S1).
pub fn router(state: ShakoState, mount: Mount) -> Router {
    let mcp_router = Router::new()
        .route("/mcp/tools", get(http_mcp_list))
        .route("/mcp/call", post(http_mcp_call));

    Router::new()
        .merge(api_router())
        .merge(oci_v2_routes())
        .merge(mcp_router)
        .merge(web_router())
        .with_state(state)
        .layer(middleware::from_fn(security_and_mount_headers))
        .layer(Extension(DefaultMount(mount)))
}

/// Builds the host-root `/v2` container registry router for `jc-router` (Contract S1).
pub fn registry_router(state: ShakoState, mount: Mount) -> Router {
    stripped_v2_registry_routes()
        .with_state(state)
        .layer(middleware::from_fn(security_and_mount_headers))
        .layer(Extension(DefaultMount(mount)))
}

// =========================================================================
// Declarative Manifests: `check -f` (offline) and `apply -f` (idempotent, S1)
// =========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ShakoManifestBundle {
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub instance: Option<ManifestInstanceSpec>,
    #[serde(default)]
    pub repositories: Vec<ManifestRepoSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestInstanceSpec {
    pub host: String,
    #[serde(default)]
    pub max_repos: Option<u64>,
    #[serde(default)]
    pub max_storage_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestRepoSpec {
    pub owner: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_main")]
    pub default_branch: String,
    #[serde(default)]
    pub protections: Vec<BranchProtectionRule>,
    #[serde(default)]
    pub mirrors: Vec<ManifestMirrorSpec>,
    #[serde(default)]
    pub webhooks: Vec<ManifestWebhookSpec>,
}

fn default_main() -> String {
    "main".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestMirrorSpec {
    pub remote_name: String,
    pub remote_address: String,
    pub credential_secret_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestWebhookSpec {
    pub url: String,
    pub secret_ref: String,
}

/// Offline validation of a `ShakoManifestBundle` (`shako check -f`, Contract S1).
pub fn check_manifest_bundle(
    bundle: &ShakoManifestBundle,
    state: &ShakoState,
    org_id: OrgId,
) -> ShakoResult<()> {
    if bundle.api_version != "joinedcontext.com/v1alpha1" || bundle.kind != "ForgeManifest" {
        return Err(ShakoError::Unprocessable(
            "manifest must have apiVersion 'joinedcontext.com/v1alpha1' and kind 'ForgeManifest'"
                .into(),
        ));
    }
    for r in &bundle.repositories {
        validate_repo_or_owner_name(&r.owner, "owner")?;
        validate_repo_or_owner_name(&r.name, "repository")?;
        validate_branch_name(&r.default_branch)?;
        for rule in &r.protections {
            shako_core::RefPolicy::validate_rule_for_repo(
                rule,
                state.authorizer.as_ref(),
                state.default_instance_id,
                org_id,
                &r.owner,
                &r.name,
            )?;
        }
        for m in &r.mirrors {
            state.outbound_guard.validate_url(&m.remote_address, None)?;
            shako_core::check_secret_ref("credentialSecretRef", &m.credential_secret_ref)?;
        }
        for w in &r.webhooks {
            state.outbound_guard.validate_url(&w.url, None)?;
            shako_core::check_secret_ref("secretRef", &w.secret_ref)?;
        }
    }
    Ok(())
}

/// Idempotently applies a `ShakoManifestBundle` (`shako apply -f`, Contract S1).
pub async fn apply_manifest_bundle(
    state: &ShakoState,
    org_id: OrgId,
    bundle: &ShakoManifestBundle,
) -> ShakoResult<Value> {
    check_manifest_bundle(bundle, state, org_id)?;
    let now = Utc::now();

    if let Some(inst) = &bundle.instance {
        let mut rec = InstanceRecord::default_for(state.default_instance_id, org_id, &inst.host);
        if let Some(mr) = inst.max_repos {
            rec.max_repos = mr;
        }
        if let Some(ms) = inst.max_storage_bytes {
            rec.max_storage_bytes = ms;
        }
        state.store().upsert_instance(rec).await?;
    }

    let mut applied_repos = 0usize;
    for r in &bundle.repositories {
        let repo = match state
            .store()
            .get_repo_by_name(state.default_instance_id, org_id, &r.owner, &r.name)
            .await?
        {
            Some(existing) => existing,
            None => {
                let repo_id = RepoId::new();
                let root = state.repo_root().to_path_buf();
                let inst_id = state.default_instance_id;
                let def_b = r.default_branch.clone();
                let desc = r.description.clone();
                tokio::task::spawn_blocking(move || {
                    init_bare_repo(&root, inst_id, repo_id, &def_b, true, &desc)
                })
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
                state
                    .store()
                    .create_repo(RepoRecord {
                        id: repo_id,
                        instance_id: state.default_instance_id,
                        org_id,
                        project_id: r.name.clone(),
                        owner: r.owner.clone(),
                        name: r.name.clone(),
                        description: r.description.clone(),
                        default_branch: r.default_branch.clone(),
                        private: true,
                        archived: false,
                        size_bytes: 0,
                        next_pr_number: 1,
                        created_at: now,
                        updated_at: now,
                    })
                    .await?
            }
        };
        state.register_owner_org(&r.owner, org_id);
        for rule in &r.protections {
            state
                .store()
                .upsert_protection_rule(ProtectionRuleRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: state.default_instance_id,
                    org_id,
                    repo_id: repo.id,
                    rule: rule.clone(),
                    created_at: now,
                    updated_at: now,
                })
                .await?;
        }
        for m in &r.mirrors {
            state
                .store()
                .upsert_push_mirror(PushMirrorRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: state.default_instance_id,
                    org_id,
                    repo_id: repo.id,
                    remote_name: m.remote_name.clone(),
                    remote_address: m.remote_address.clone(),
                    credential_secret_ref: m.credential_secret_ref.clone(),
                    interval: "8h0m0s".into(),
                    sync_on_commit: true,
                    last_sync: None,
                    last_error: String::new(),
                    created_at: now,
                })
                .await?;
        }
        let existing_hooks = state
            .store()
            .list_webhooks(state.default_instance_id, org_id, Some(repo.id))
            .await?;
        for w in &r.webhooks {
            if !existing_hooks.iter().any(|eh| eh.url == w.url) {
                state
                    .store()
                    .create_webhook(WebhookRecord {
                        id: WebhookId::new(),
                        instance_id: state.default_instance_id,
                        org_id,
                        repo_id: repo.id,
                        url: w.url.clone(),
                        events: vec!["push".into(), "pull_request".into()],
                        active: true,
                        secret_ref: w.secret_ref.clone(),
                        previous_secret_ref: None,
                        format: WebhookFormat::Shako,
                        created_at: now,
                    })
                    .await?;
            }
        }
        applied_repos += 1;
    }

    Ok(json!({ "status": "applied", "repositories": applied_repos }))
}

/// Runs `compact` over all repositories in `org_id` (Contract S1, S6).
pub async fn compact_all_repositories(state: &ShakoState, org_id: OrgId) -> ShakoResult<usize> {
    let repos = state
        .store()
        .list_repos(state.default_instance_id, org_id)
        .await?;
    let mut compacted = 0usize;
    for r in repos {
        let _guard = state.repo_locks.lock(r.id).await;
        let path = shako_core::repo_disk_path(state.repo_root(), r.instance_id, r.id)?;
        tokio::task::spawn_blocking(move || {
            compact_repository(&path, shako_git::DEFAULT_PRUNE_GRACE)
        })
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))??;
        compacted += 1;
    }
    Ok(compacted)
}

/// Runs `fsck` over all repositories in `org_id` (Contract S1, S6).
pub async fn fsck_all_repositories(state: &ShakoState, org_id: OrgId) -> ShakoResult<usize> {
    let repos = state
        .store()
        .list_repos(state.default_instance_id, org_id)
        .await?;
    let mut checked = 0usize;
    for r in repos {
        let path = shako_core::repo_disk_path(state.repo_root(), r.instance_id, r.id)?;
        tokio::task::spawn_blocking(move || fsck_repository(&path))
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
        checked += 1;
    }
    Ok(checked)
}

/// Executes a CLI operation through the unified `OperationRegistry` (Contract S8).
pub async fn execute_cli_operation(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    cli_command: &str,
    args: Value,
) -> ShakoResult<Value> {
    exec_odata_operation(state, ctx, mount, cli_command, None, args).await
}
