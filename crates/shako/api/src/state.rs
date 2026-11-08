#![forbid(unsafe_code)]
//! Shared state (`ShakoState`), streaming body helpers, and request authentication/authorization
//! extraction for `shako-api`, `shako-registry`, `shako-mcp`, and `shako-web`.

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use jc_router::Mount;
use parking_lot::RwLock;
use serde_json::{Value, json};
use shako_core::{
    Action, Authorizer, InstanceId, InstanceRecord, MemoryBudget, MemorySecretStore, ObjectTarget,
    OperationRegistry, OrgId, OutboundGuard, ProblemDetails, RefPolicy, RepoRecord, RequestCtx,
    SecretStore, ShakoError, ShakoResult, ShakoSigningKey, StaticTokenBinding, Store, Subject,
    TokenVerifier,
};
use shako_git::RepoLocks;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Callback invoked in-process whenever an outbox event is dispatched (S14: Portal in-process bus).
pub type InProcessEventSubscriber = Arc<dyn Fn(&shako_core::OutboxEvent) + Send + Sync>;

/// Shared state of a `shako` forge instance (S1: private fields for store, signing key, repo root).
#[derive(Clone)]
pub struct ShakoState {
    pub default_instance_id: InstanceId,
    pub default_org_id: OrgId,
    repo_root: PathBuf,
    store: Arc<dyn Store>,
    pub secret_store: Arc<dyn SecretStore>,
    pub authorizer: Arc<dyn Authorizer>,
    pub token_verifier: TokenVerifier,
    pub outbound_guard: OutboundGuard,
    pub memory_budget: MemoryBudget,
    pub repo_locks: RepoLocks,
    pub registry: Arc<OperationRegistry>,
    pub owner_to_org: Arc<RwLock<BTreeMap<String, OrgId>>>,
    pub event_bus: broadcast::Sender<shako_core::OutboxEvent>,
    pub subscribers: Arc<RwLock<Vec<InProcessEventSubscriber>>>,
    pub docs_url: String,
}

impl ShakoState {
    #[must_use]
    pub fn new(
        default_instance_id: InstanceId,
        default_org_id: OrgId,
        repo_root: PathBuf,
        store: Arc<dyn Store>,
        authorizer: Arc<dyn Authorizer>,
        token_verifier: TokenVerifier,
        outbound_guard: OutboundGuard,
    ) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            default_instance_id,
            default_org_id,
            repo_root,
            store,
            secret_store: Arc::new(MemorySecretStore::new()),
            authorizer,
            token_verifier,
            outbound_guard,
            memory_budget: MemoryBudget::default(),
            repo_locks: RepoLocks::new(),
            registry: Arc::new(OperationRegistry::new()),
            owner_to_org: Arc::new(RwLock::new(BTreeMap::new())),
            event_bus: tx,
            subscribers: Arc::new(RwLock::new(Vec::new())),
            docs_url: "https://docs.joinedcontext.invalid/shako".into(),
        }
    }

    #[must_use]
    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    #[must_use]
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    #[must_use]
    pub fn signing_key(&self) -> &ShakoSigningKey {
        self.token_verifier.signing_key()
    }

    pub fn register_owner_org(&self, owner: impl Into<String>, org_id: OrgId) {
        let owner_s = owner.into();
        self.token_verifier
            .register_org_alias(owner_s.clone(), org_id);
        self.owner_to_org.write().insert(owner_s, org_id);
    }

    #[must_use]
    pub fn resolve_org_for_owner(&self, owner: &str, caller: &Subject) -> OrgId {
        if let Some(&oid) = self.owner_to_org.read().get(owner) {
            return oid;
        }
        caller
            .organizations
            .first()
            .copied()
            .unwrap_or(self.default_org_id)
    }

    pub fn subscribe_events(&self, sub: InProcessEventSubscriber) {
        self.subscribers.write().push(sub);
    }

    /// Loads the `InstanceRecord` for `(instance_id, org_id)`, returning default quotas if not yet stored.
    pub async fn instance_for(&self, instance_id: InstanceId, org_id: OrgId) -> InstanceRecord {
        self.store
            .get_instance(instance_id, org_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| InstanceRecord::default_for(instance_id, org_id, "localhost"))
    }

    /// Loads the `RefPolicy` (branch + tag protection rules) for a repository.
    pub async fn ref_policy_for(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: shako_core::RepoId,
    ) -> ShakoResult<RefPolicy> {
        let branch_rules = self
            .store
            .list_protection_rules(instance_id, org_id, repo_id)
            .await?
            .into_iter()
            .map(|r| r.rule)
            .collect();
        let tag_rules = self
            .store
            .list_tag_protections(instance_id, org_id, repo_id)
            .await?
            .into_iter()
            .map(|r| r.rule)
            .collect();
        Ok(RefPolicy {
            branch_rules,
            tag_rules,
        })
    }

    /// Convenience constructor for standalone demos/tests with static token bindings.
    pub fn for_standalone(
        repo_root: PathBuf,
        store: Arc<dyn Store>,
        authorizer: Arc<dyn Authorizer>,
        default_org_id: OrgId,
        static_tokens: Vec<StaticTokenBinding>,
    ) -> ShakoResult<Self> {
        let key = ShakoSigningKey::generate()?;
        let verifier = TokenVerifier::new(key).with_static_tokens(static_tokens);
        Ok(Self::new(
            InstanceId::from_u128(1),
            default_org_id,
            repo_root,
            store,
            authorizer,
            verifier,
            OutboundGuard::default(),
        ))
    }
}

/// Strips `.git` suffix from a URL path segment if present (S3: `.git` suffix optional).
#[must_use]
pub fn strip_dot_git(segment: &str) -> &str {
    segment.strip_suffix(".git").unwrap_or(segment)
}

/// Extracts and verifies the caller's [`RequestCtx`] from `headers`, requiring a valid token
/// for `expected_audience` (`"shako"` or `"container_registry"`).
/// Never trusts `x-*` headers for identity or organization membership (THINK-CHECKLIST 2).
pub fn authenticate_request(
    state: &ShakoState,
    headers: &HeaderMap,
    owner_hint: Option<&str>,
    expected_audience: &str,
) -> ShakoResult<RequestCtx> {
    let auth_hdr = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ShakoError::Unauthorized("missing Authorization header".into()))?;
    let raw_token = TokenVerifier::extract_raw_token(auth_hdr)
        .ok_or_else(|| ShakoError::Unauthorized("malformed Authorization header".into()))?;
    let subject = state.token_verifier.verify(&raw_token, expected_audience)?;

    let active_org = match owner_hint {
        Some(owner) => state.resolve_org_for_owner(owner, &subject),
        None => subject
            .organizations
            .first()
            .copied()
            .unwrap_or(state.default_org_id),
    };

    Ok(RequestCtx::new(
        state.default_instance_id,
        active_org,
        subject,
    ))
}

/// Resolves a repository `(owner, repo)` and checks `required_action` via `Authorizer`.
/// If the repository does not exist OR is in another organization OR the caller lacks `Read`,
/// returns `ShakoError::NotFound` (`404`, never `403`, with the same body, S12 & T6).
pub async fn resolve_and_authorize_repo(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    raw_repo: &str,
    required_action: Action,
) -> ShakoResult<(RepoRecord, PathBuf)> {
    let repo_name = strip_dot_git(raw_repo);
    shako_core::validate_repo_or_owner_name(owner, "owner")
        .map_err(|_| ShakoError::NotFound("repository not found".into()))?;
    shako_core::validate_repo_or_owner_name(repo_name, "repository")
        .map_err(|_| ShakoError::NotFound("repository not found".into()))?;

    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    let dummy_target = ObjectTarget::Repository {
        instance_id: ctx.instance_id,
        org_id: target_org,
        project_id: None,
        owner: owner.to_owned(),
        repo: repo_name.to_owned(),
        git_ref: None,
    };

    // Always run BOTH the store lookup and the authorizer check so missing and forbidden repos
    // execute in the same latency band (S12 timing band requirement).
    let maybe_repo = state
        .store()
        .get_repo_by_name(ctx.instance_id, target_org, owner, repo_name)
        .await?;
    let target = match &maybe_repo {
        Some(r) => ObjectTarget::Repository {
            instance_id: r.instance_id,
            org_id: r.org_id,
            project_id: (!r.project_id.is_empty()).then(|| r.project_id.clone()),
            owner: r.owner.clone(),
            repo: r.name.clone(),
            git_ref: None,
        },
        None => dummy_target,
    };
    let decision = state.authorizer.check(ctx, required_action, &target);
    let Some(repo) = maybe_repo else {
        return Err(ShakoError::NotFound("repository not found".into()));
    };
    decision.into_result("repository not found")?;

    if required_action.is_write() && repo.archived && required_action != Action::Admin {
        return Err(ShakoError::Forbidden(
            "repository is archived and read-only".into(),
        ));
    }

    let disk_path = shako_core::repo_disk_path(state.repo_root(), repo.instance_id, repo.id)?;
    Ok((repo, disk_path))
}

/// Streams an axum request `Body` into a `Vec<u8>`, refusing immediately once `limit_bytes` is exceeded.
/// Never calls unbounded `to_bytes` (00-task.md section 2, Threat T5).
pub async fn read_body_bounded(body: Body, limit_bytes: usize) -> ShakoResult<Vec<u8>> {
    let mut stream = body.into_data_stream();
    let mut out = Vec::new();
    while let Some(chunk_res) = stream.next().await {
        let chunk: Bytes =
            chunk_res.map_err(|e| ShakoError::BadRequest(format!("body read error: {e}")))?;
        if out.len().saturating_add(chunk.len()) > limit_bytes {
            return Err(ShakoError::QuotaExceeded {
                resource: "request_body_bytes".into(),
                limit: limit_bytes as u64,
            });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Streams a `Vec<u8>` produced on a blocking task to an axum `Body` in 64 KiB chunks through
/// a bounded `tokio::sync::mpsc::channel(4)` (00-task.md section 2, S22).
pub fn stream_bytes_in_64k_chunks(bytes: Vec<u8>) -> Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tokio::spawn(async move {
        for chunk in bytes.chunks(64 * 1024) {
            if tx.send(Ok(Bytes::copy_from_slice(chunk))).await.is_err() {
                break;
            }
        }
    });
    Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// Converts a [`ShakoError`] into a Gitea-compatible JSON error response `{"message": ..., "url": ...}` (S7).
#[must_use]
pub fn gitea_error_response(state: &ShakoState, err: &ShakoError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = err.to_gitea_error(&state.docs_url);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::RETRY_AFTER, "5"),
            ],
            axum::Json(body),
        )
            .into_response();
    }
    if status == StatusCode::UNAUTHORIZED {
        return (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::WWW_AUTHENTICATE, "Basic realm=\"shako\""),
            ],
            axum::Json(body),
        )
            .into_response();
    }
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        axum::Json(body),
    )
        .into_response()
}

/// Converts a [`ShakoError`] into an RFC 9457 `ProblemDetails` response (S8).
#[must_use]
pub fn problem_error_response(err: &ShakoError, instance: Option<String>) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let problem: ProblemDetails = err.to_problem(instance);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return (
            status,
            [
                (header::CONTENT_TYPE, "application/problem+json"),
                (header::RETRY_AFTER, "5"),
            ],
            axum::Json(problem),
        )
            .into_response();
    }
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        axum::Json(problem),
    )
        .into_response()
}

/// Converts a [`ShakoError`] into an OData 4.01 error JSON response (S9).
#[must_use]
pub fn odata_error_response(err: &ShakoError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = err.to_odata_error();
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json; odata.metadata=minimal"),
            (header::HeaderName::from_static("odata-version"), "4.01"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        axum::Json(body),
    )
        .into_response()
}

/// Formats a Gitea repository JSON representation honoring `mount` for `clone_url` and `html_url` (S1, S7).
#[must_use]
pub fn repo_to_gitea_json(repo: &RepoRecord, mount: &Mount) -> Value {
    let base = mount.public_base();
    let html_url = format!("{base}/{}/{}", repo.owner, repo.name);
    let clone_url = format!("{html_url}.git");
    json!({
        "id": repo.numeric_id(),
        "uuid": repo.id,
        "owner": {
            "id": 1,
            "login": repo.owner,
            "full_name": repo.owner,
        },
        "name": repo.name,
        "full_name": format!("{}/{}", repo.owner, repo.name),
        "description": repo.description,
        "private": repo.private,
        "archived": repo.archived,
        "default_branch": repo.default_branch,
        "size": repo.size_bytes / 1024,
        "html_url": html_url,
        "clone_url": clone_url,
        "created_at": repo.created_at.to_rfc3339(),
        "updated_at": repo.updated_at.to_rfc3339(),
    })
}
