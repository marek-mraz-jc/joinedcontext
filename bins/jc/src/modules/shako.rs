//! shako (Git forge) inside `jc`: mounts both the forge router (`/{slug}` or `git.{base_host}`)
//! and the host-root `/v2` container registry router (`shako_server::registry_router`).
//!
//! Records live in Postgres when `[shako.database]` is set (`shako_store_pg::PgStore`: boot RLS
//! check, the store log replayed for jc's one tenant before serving), else in memory. Git data
//! lives under `repo_root`, by default `<state_dir>/shako`, so it survives a restart too.

use crate::app::StartError;
use crate::config::ShakoSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use kaisatsu_crypto::Kek;
use shako_core::{
    Action, LocalAuthorizer, MemorySecretStore, OrgId, RoleBinding, SecretStore, ShakoSigningKey,
    StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{PortalBase, ShakoState, registry_router, router};
use shako_store_memory::MemoryStore;
use shako_store_pg::{PgSecretStore, PgStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Shako, e)
}

fn repo_root(configured: Option<&Path>, state_dir: &Path) -> Result<PathBuf, StartError> {
    let dir = match configured {
        Some(dir) => dir.to_path_buf(),
        None => state_dir.join("shako"),
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|e| err(format!("cannot create repo root {}: {e}", dir.display())))?;
    Ok(dir)
}

pub(crate) const INSTANCE: shako_core::InstanceId = shako_core::InstanceId::from_u128(1);
/// The one shako organization jc's forge runs under.
pub(crate) const ORG: OrgId = OrgId::from_u128(1);

/// The configured stores: Postgres (migrated first when a migration role is set, boot RLS
/// check, log replayed for `(INSTANCE, org_id)`), its hook and mirror secrets sealed in
/// `shako.secret` with the installation's KEK (T-7280); or memory for both without a database
/// section.
async fn open_store(
    cfg: &ShakoSection,
    org_id: OrgId,
    setup: &crate::setup::Setup,
) -> Result<(Arc<dyn shako_core::Store>, Arc<dyn SecretStore>), StartError> {
    let Some(db) = &cfg.database else {
        return Ok((
            Arc::new(MemoryStore::new()),
            Arc::new(MemorySecretStore::new()),
        ));
    };
    // Read before anything connects: a database without the key would keep secrets it cannot
    // open after the next start.
    let kek = setup
        .value(crate::setup::Secret::KaisatsuKek)
        .ok_or_else(|| {
            err(
                "shako seals its hook and mirror secrets with the installation's key-encryption \
                 key, and the state directory holds no kaisatsu-kek",
            )
        })?;
    let kek = Kek::from_raw_bytes(kek.as_bytes())
        .map_err(|_| err("the installation's kaisatsu-kek is not a 32-byte key"))?;
    if let Some(opts) = db.migrate_options().map_err(err)? {
        let pool = crate::pg::connect(opts, 1).await.map_err(err)?;
        PgStore::run_migrations(&pool).await.map_err(err)?;
        pool.close().await;
    }
    let store = PgStore::connect_with(db.app_options().map_err(err)?, db.max_connections())
        .await
        .map_err(err)?;
    let replayed = store.hydrate(INSTANCE, org_id).await.map_err(err)?;
    tracing::info!(replayed, "shako: records loaded from Postgres");
    let secrets = PgSecretStore::new(&store, INSTANCE, &kek).map_err(err)?;
    Ok((Arc::new(store), Arc::new(secrets)))
}

/// shako's signing key: the operator's or the one setup keeps in the state directory (generated
/// once), so a restart keeps every stored image signature and issued token verifiable. No key
/// held refuses the start, never a fresh key per boot.
fn signing_key(setup: &crate::setup::Setup) -> Result<ShakoSigningKey, StartError> {
    let text = setup
        .value(crate::setup::Secret::ShakoSigningKey)
        .ok_or_else(|| err("shako has no signing key (JCBIN_SHAKO_SIGNING_KEY_FILE)"))?;
    ShakoSigningKey::from_text(text.as_bytes()).map_err(|_| {
        err("shako's signing key is not a P-256 key (64 hex, raw 32 bytes or PKCS#8 PEM)")
    })
}

pub async fn build_state(
    cfg: &ShakoSection,
    setup: &crate::setup::Setup,
) -> Result<ShakoState, StartError> {
    let root = repo_root(cfg.repo_root.as_deref(), &setup.dir)?;
    let org_id = ORG;
    let (store, secrets) = open_store(cfg, org_id, setup).await?;
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
        org_id,
        principal: "admin".into(),
        actions: vec![
            Action::Admin,
            Action::Write,
            Action::Approve,
            Action::Publish,
        ],
        project_id: None,
        repo: None,
    }]));

    let key = signing_key(setup)?;

    let mut static_tokens = Vec::new();
    if let Some(token) = setup.value(crate::setup::Secret::ShakoToken) {
        static_tokens.push(StaticTokenBinding::new(
            token,
            Subject::authenticated("admin", "admin", vec![org_id], vec![]),
        ));
    }

    let verifier = TokenVerifier::new(key).with_static_tokens(static_tokens);
    let mut state = ShakoState::new(
        INSTANCE,
        org_id,
        root,
        store,
        authz,
        verifier,
        shako_core::OutboundGuard::default(),
    );
    state.secret_store = secrets;
    Ok(state)
}

/// `portal` is the Portal's public base: a browser asking shako's pages without a credential is
/// sent there (shako keeps no sessions).
pub fn build(state: ShakoState, mount: Mount, portal: &str) -> (Router, Router) {
    let main_router =
        router(state.clone(), mount.clone()).layer(axum::Extension(PortalBase(portal.into())));
    let v2_router = registry_router(state, mount);
    (main_router, v2_router)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn signing_key_is_kept_across_boots_and_missing_refuses_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        let boot = || crate::setup::Setup::prepare(tmp.path(), &[Module::Shako], &BTreeMap::new());
        let first = signing_key(&boot().unwrap()).unwrap();
        let second = signing_key(&boot().unwrap()).unwrap();
        assert_eq!(first.kid, second.kid, "same key after a restart");
        let payload = b"image digest";
        assert!(second.verify_cosign_payload(payload, &first.sign_cosign_payload(payload)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("shako-signing-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let without =
            crate::setup::Setup::prepare(tmp.path(), &[Module::Antares], &BTreeMap::new()).unwrap();
        let e = signing_key(&without).unwrap_err().to_string();
        assert!(e.contains("no signing key"), "{e}");
    }
}
