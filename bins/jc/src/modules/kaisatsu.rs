//! kaisatsu (identity provider) inside `jc`: the same state the standalone `kaisatsu serve`
//! builds, on the in-memory store, with the KEK read from its file.

use crate::app::StartError;
use crate::config::KaisatsuSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use kaisatsu_core::{LocalAuthorizer, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_import::{ApplyOptions, apply_realm_files};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{PgStore, PgStoreConfig};
use std::sync::Arc;

/// Opens the configured `Store` for kaisatsu inside `jc` (K16):
/// - When `database_url_file` is set, reads the URL from the file, refuses any URL containing an
///   inline password, connects `PgStore` with `verify_boot_rls`, and spawns the change listener.
/// - When `database_url_file` is absent, logs a `WARN` once and falls back to `MemoryStore`.
pub async fn open_kaisatsu_store(
    cfg: &KaisatsuSection,
    kek: Kek,
) -> Result<Arc<dyn Store>, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    if let Some(url_file) = &cfg.database_url_file {
        let raw_url = std::fs::read_to_string(url_file).map_err(|e| {
            err(&format!(
                "cannot read database_url_file {}: {e}",
                url_file.display()
            ))
        })?;
        let trimmed = raw_url.trim().to_owned();
        if trimmed.is_empty() {
            return Err(err(&format!(
                "database_url_file {} is empty",
                url_file.display()
            )));
        }
        let pg_cfg = PgStoreConfig {
            url_without_password: trimmed,
            password_file: cfg.db_password_file.clone(),
            max_connections: cfg.db_max_connections.unwrap_or(5),
        };
        let pg = PgStore::connect(&pg_cfg, kek).await.map_err(|e| err(&e))?;
        pg.spawn_change_listener();
        Ok(Arc::new(pg))
    } else {
        tracing::warn!(
            "kaisatsu: database_url_file is not set; mounting kaisatsu on in-memory store (demo/test only)"
        );
        Ok(Arc::new(MemoryStore::new(kek)))
    }
}

pub async fn build(cfg: &KaisatsuSection, mount: Mount) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let kek_file = cfg.kek_file.as_deref().ok_or_else(|| err(&"no kek_file"))?;
    let kek = Kek::from_file(kek_file).map_err(|e| err(&e))?;
    let store = open_kaisatsu_store(cfg, kek.clone()).await?;
    if let Some(dir) = &cfg.import_dir {
        let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
        apply_realm_files(&store, &files, &ApplyOptions::default())
            .await
            .map_err(|e| err(&e))?;
    }
    let state = KaisatsuState::new(
        store,
        kek,
        OutboundGuard::default(),
        Arc::new(LocalAuthorizer),
    );
    Ok(router(state, mount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};

    #[tokio::test]
    async fn k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password()
     {
        let dir = tempfile::tempdir().unwrap();
        let kek_path = dir.path().join("kek");
        std::fs::write(&kek_path, "07".repeat(32)).unwrap();

        // Unknown field in [kaisatsu] is refused by deny_unknown_fields.
        let bad_toml = format!(
            "[kaisatsu]\nkek_file = {:?}\ndatabase_url_inline = \"postgres://x\"\n",
            kek_path
        );
        assert!(Config::parse_file(dir.path(), &bad_toml).is_err());

        // Valid database_url_file is parsed and merged.
        let url_file = dir.path().join("db_url");
        std::fs::write(
            &url_file,
            "postgresql://kaisatsu_app:leaked@localhost:5432/db\n",
        )
        .unwrap();
        let good_toml = format!(
            "role = \"kaisatsu\"\n[kaisatsu]\nkek_file = {:?}\ndatabase_url_file = {:?}\n",
            kek_path, url_file
        );
        let parsed = Config::parse_file(dir.path(), &good_toml).unwrap();
        let resolved = Config::merge(Cli::default(), parsed).unwrap();
        assert_eq!(
            resolved.kaisatsu.database_url_file.as_deref(),
            Some(url_file.as_path())
        );

        // Inline password inside database_url_file is refused before any network connection.
        let kek = Kek::from_file(&kek_path).unwrap();
        let err = open_kaisatsu_store(&resolved.kaisatsu, kek.clone())
            .await
            .err()
            .expect("inline password must be refused");
        assert!(err.to_string().contains("must not carry a password"));

        // Without database_url_file, falls back to MemoryStore.
        let mem_section = KaisatsuSection {
            kek_file: Some(kek_path),
            import_dir: None,
            database_url_file: None,
            db_password_file: None,
            db_max_connections: None,
        };
        let store = open_kaisatsu_store(&mem_section, kek).await.unwrap();
        assert!(store.list_realms().await.unwrap().is_empty());
    }
}
