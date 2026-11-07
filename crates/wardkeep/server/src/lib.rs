#![forbid(unsafe_code)]
//! `wardkeep-server` (W9):
//! - `router(state, mount) -> axum::Router` callable both by the standalone `wardkeep` binary
//!   and by the monorepo `jc` binary (`jc serve --role wardkeep`).
//! - Signing keys and KEK are held in private fields with no public accessors so no sibling module
//!   in the same process can read key material (W9).
//! - CLI subcommands (`serve`, `migrate`, `apply`, `check`, `plan`, `diff`, `export`, `keys rotate`).

use axum::Router;
use jc_router::Mount;
use std::path::PathBuf;
use std::sync::Arc;
use wardkeep_admin::{admin_router, odata_router, scim_router, AdminState};
use wardkeep_core::{Authorizer, LocalAuthorizer, Result, Store};
use wardkeep_crypto::Kek;
use wardkeep_login::login_router;
use wardkeep_mcp::mcp_router;
use wardkeep_oidc::{oidc_router, OutboundGuard, OidcState};
use wardkeep_store_memory::MemoryStore;

/// Server state with private key-encryption key and private store pool (W2, W9).
#[derive(Clone)]
pub struct WardkeepState {
    store: Arc<dyn Store>,
    kek: Kek,
    guard: OutboundGuard,
    authorizer: Arc<dyn Authorizer>,
}

impl WardkeepState {
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

/// Builds the complete `wardkeep` Axum [`Router`] for `mount` (W9).
#[must_use]
pub fn router(state: WardkeepState, mount: Mount) -> Router {
    let oidc_state = OidcState {
        store: state.store.clone(),
        kek: state.kek.clone(),
        guard: state.guard.clone(),
        default_mount: mount,
    };
    let admin_state = AdminState::new(oidc_state.clone(), state.authorizer.clone());

    Router::new()
        .merge(oidc_router(oidc_state.clone()))
        .merge(login_router(oidc_state))
        .merge(admin_router(admin_state.clone()))
        .merge(odata_router(admin_state.clone()))
        .merge(scim_router(admin_state.clone()))
        .merge(mcp_router(admin_state))
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

pub fn load_realm_files_from_path(path: & std::path::Path) -> Result<Vec<(String, serde_json::Value)>> {
    let mut out = Vec::new();
    if path.is_file() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("realm.json")
            .to_owned();
        let raw = std::fs::read_to_string(path)
            .map_err(|e| wardkeep_core::WardkeepError::BadRequest(e.to_string()))?;
        let val: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| wardkeep_core::WardkeepError::BadRequest(e.to_string()))?;
        out.push((name, val));
    } else if path.is_dir() {
        let entries = std::fs::read_dir(path)
            .map_err(|e| wardkeep_core::WardkeepError::BadRequest(e.to_string()))?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "json" | "yaml" | "yml") {
                        let name = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("realm.json")
                            .to_owned();
                        let raw = std::fs::read_to_string(&p)
                            .map_err(|e| wardkeep_core::WardkeepError::BadRequest(e.to_string()))?;
                        let val: serde_json::Value = serde_json::from_str(&raw)
                            .map_err(|e| wardkeep_core::WardkeepError::BadRequest(e.to_string()))?;
                        out.push((name, val));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}
