#![forbid(unsafe_code)]
//! `kaisatsu-server` (W9):
//! - `router(state, mount) -> axum::Router` callable both by the standalone `kaisatsu` binary
//!   and by the monorepo `jc` binary (`jc serve --role kaisatsu`).
//! - Signing keys and KEK are held in private fields with no public accessors so no sibling module
//!   in the same process can read key material (W9).
//! - CLI subcommands (`serve`, `migrate`, `apply`, `check`, `plan`, `diff`, `export`, `keys rotate`).

use axum::Router;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, admin_router, odata_router, scim_router};
use kaisatsu_core::{Authorizer, LocalAuthorizer, Result, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_login::login_router;
/// The account console's client and audience, for a host that provisions the console client.
pub use kaisatsu_login::account::{ACCOUNT_AUDIENCE, ACCOUNT_CONSOLE_CLIENT};
use kaisatsu_mcp::mcp_router;
use kaisatsu_oidc::{OidcState, OutboundGuard, oidc_router};
use kaisatsu_store_memory::MemoryStore;
use std::path::PathBuf;
use std::sync::Arc;

/// Server state with private key-encryption key and private store pool (W2, W9).
#[derive(Clone)]
pub struct KaisatsuState {
    store: Arc<dyn Store>,
    kek: Kek,
    guard: OutboundGuard,
    authorizer: Arc<dyn Authorizer>,
}

impl KaisatsuState {
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        kek: Kek,
        guard: OutboundGuard,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            store,
            kek,
            guard,
            authorizer,
        }
    }

    #[must_use]
    pub fn in_memory_for_tests() -> Self {
        let kek = Kek::generate_for_tests();
        let store = Arc::new(MemoryStore::new(kek.clone()));
        Self {
            store,
            kek,
            guard: OutboundGuard::default(),
            authorizer: Arc::new(LocalAuthorizer),
        }
    }

    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    pub fn guard(&self) -> &OutboundGuard {
        &self.guard
    }
}

/// Builds the complete `kaisatsu` Axum [`Router`] for `mount` (W9).
pub fn router(state: KaisatsuState, mount: Mount) -> Router {
    let oidc_state = OidcState {
        store: state.store.clone(),
        kek: state.kek.clone(),
        guard: state.guard.clone(),
        default_mount: mount,
    };
    let admin_state = AdminState::new(oidc_state.clone(), state.authorizer.clone());

    let protected_apis = Router::new()
        .merge(admin_router(admin_state.clone()))
        .merge(odata_router(admin_state.clone()))
        .merge(scim_router(admin_state.clone()))
        .merge(mcp_router(admin_state));

    Router::new()
        .merge(oidc_router(oidc_state.clone()))
        .merge(login_router(oidc_state))
        .merge(kaisatsu_admin::compat::with_dpop_req_info(protected_apis))
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub mount: Mount,
    pub kek_file: PathBuf,
    pub db_url: Option<String>,
    pub db_password_file: Option<PathBuf>,
    pub import_dir: Option<PathBuf>,
}

/// Reads one realm file: `.yaml` / `.yml` as YAML, everything else as JSON (W1b). Errors name
/// the file and never echo its content (realm files hold secret references).
fn read_realm_file(path: &std::path::Path) -> Result<(String, serde_json::Value)> {
    let bad = |what: &str| {
        kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {what}", path.display()))
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("realm.json")
        .to_owned();
    let raw = std::fs::read_to_string(path).map_err(|e| bad(&e.to_string()))?;
    let is_yaml = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    let val: serde_json::Value = if is_yaml {
        serde_norway::from_str(&raw).map_err(|e| bad(&format!("invalid YAML: {e}")))?
    } else {
        serde_json::from_str(&raw).map_err(|e| bad(&format!("invalid JSON: {e}")))?
    };
    if !val.is_object() {
        return Err(bad("a realm file holds one realm object"));
    }
    Ok((name, val))
}

/// Loads a realm file, or every `.json` / `.yaml` / `.yml` file of a directory in file-name
/// order (keycloak-config-cli merge order, W1b). Refuses symlinks escaping the directory (K17).
pub fn load_realm_files_from_path(
    path: &std::path::Path,
) -> Result<Vec<(String, serde_json::Value)>> {
    let mut out = Vec::new();
    if path.is_file() {
        out.push(read_realm_file(path)?);
    } else if path.is_dir() {
        let canonical_dir = path.canonicalize().map_err(|e| {
            kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", path.display()))
        })?;
        let entries = std::fs::read_dir(path).map_err(|e| {
            kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", path.display()))
        })?;
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !matches!(ext.to_ascii_lowercase().as_str(), "json" | "yaml" | "yml") {
                continue;
            }
            let canonical_file = p.canonicalize().map_err(|e| {
                kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", p.display()))
            })?;
            if !canonical_file.starts_with(&canonical_dir) {
                return Err(kaisatsu_core::KaisatsuError::BadRequest(format!(
                    "{}: symlink escapes import directory {}",
                    p.display(),
                    path.display()
                )));
            }
            if canonical_file.is_file() {
                out.push(read_realm_file(&p)?);
            }
        }
    } else {
        return Err(kaisatsu_core::KaisatsuError::BadRequest(format!(
            "{}: no such realm file or directory",
            path.display()
        )));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Computes a deterministic SHA-256 digest over the sorted `(filename, raw_bytes)` entries of
/// `dir` (refusing symlinks out of `dir`), used by `--import-watch` (K17).
pub fn compute_import_dir_digest(dir: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let canonical_dir = dir
        .canonicalize()
        .map_err(|e| kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", dir.display())))?;
    let entries = std::fs::read_dir(dir)
        .map_err(|e| kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", dir.display())))?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !matches!(ext.to_ascii_lowercase().as_str(), "json" | "yaml" | "yml") {
            continue;
        }
        let canonical_file = p.canonicalize().map_err(|e| {
            kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", p.display()))
        })?;
        if !canonical_file.starts_with(&canonical_dir) {
            return Err(kaisatsu_core::KaisatsuError::BadRequest(format!(
                "{}: symlink escapes import directory {}",
                p.display(),
                dir.display()
            )));
        }
        if canonical_file.is_file() {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let bytes = std::fs::read(&canonical_file).map_err(|e| {
                kaisatsu_core::KaisatsuError::BadRequest(format!("{}: {e}", p.display()))
            })?;
            files.push((name, bytes));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (name, bytes) in files {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(hex)
}

/// Groups realm files by their `realm` field, keeping file-name order inside each group.
pub fn group_files_by_realm(
    files: Vec<(String, serde_json::Value)>,
) -> Vec<Vec<(String, serde_json::Value)>> {
    let mut groups: Vec<(String, Vec<(String, serde_json::Value)>)> = Vec::new();
    for (name, doc) in files {
        let realm = doc["realm"].as_str().unwrap_or_default().to_owned();
        match groups.iter_mut().find(|(r, _)| *r == realm) {
            Some((_, list)) => list.push((name, doc)),
            None => groups.push((realm, vec![(name, doc)])),
        }
    }
    groups.into_iter().map(|(_, list)| list).collect()
}

/// Runs one poll step of `--import-dir` watch (K17): if the directory's SHA-256 changed since
/// `last_digest`, builds a plan with `ApplyMode::NoDelete` (`--prune` never implied) and executes
/// `apply_plan` (refusing stale plans). On any error, logs a warning and preserves `last_digest`
/// and the last good realm state.
pub async fn poll_import_dir_once(
    store: &Arc<dyn Store>,
    dir: &std::path::Path,
    last_digest: &mut String,
) -> Result<bool> {
    let digest = compute_import_dir_digest(dir)?;
    if &digest == last_digest {
        return Ok(false);
    }
    let files = load_realm_files_from_path(dir)?;
    let opts = kaisatsu_import::ApplyOptions {
        mode: kaisatsu_import::ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        ..Default::default()
    };
    for realm_files in group_files_by_realm(files) {
        let plan = kaisatsu_import::create_plan(store, &realm_files, &opts).await?;
        kaisatsu_import::apply_plan(store, &plan, &opts).await?;
    }
    *last_digest = digest;
    Ok(true)
}
