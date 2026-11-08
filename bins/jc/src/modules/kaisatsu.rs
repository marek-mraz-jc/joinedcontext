//! kaisatsu (identity provider) inside `jc`: the same state the standalone `kaisatsu serve`
//! builds, on PostgreSQL when `database_url_file` is set (else the in-memory store), with the
//! KEK read from its file.

use crate::app::StartError;
use crate::config::KaisatsuSection;
use crate::module::Module;
use crate::modules::BuildCtx;
use crate::setup;
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

/// Applies the realm files of `dir` (one realm, merged in file-name order). A directory without
/// realm files is nothing to apply, not an error: a first start mounts an empty import dir.
async fn apply_import_dir(store: &Arc<dyn Store>, dir: &std::path::Path) -> Result<(), StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
    if files.is_empty() {
        return Ok(());
    }
    apply_realm_files(store, &files, &ApplyOptions::default())
        .await
        .map(|_| ())
        .map_err(|e| err(&e))
}

/// Applies the realm the Portal signs in through (first-start setup), when the Portal runs beside
/// kaisatsu. Apply mode `no-delete`: a restart updates what changed and removes nothing an
/// operator added. Secrets reach the importer as `$(env:...)` values from memory.
async fn apply_portal_realm(store: &Arc<dyn Store>, ctx: BuildCtx<'_>) -> Result<(), StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let (Some(base), Some(_)) = (
        ctx.siblings.portal_base(),
        ctx.setup.value(setup::Secret::PortalClientSecret),
    ) else {
        return Ok(());
    };
    let realm = setup::portal_realm(&base);
    let options = ApplyOptions {
        env_vars: setup::realm_env(ctx.setup),
        ..ApplyOptions::default()
    };
    let report = apply_realm_files(store, &[("jc-portal.json".to_owned(), realm)], &options)
        .await
        .map_err(|e| err(&e))?;
    tracing::info!(
        realm = %report.realm,
        changed = report.has_changes(),
        "kaisatsu: the Portal's realm is applied"
    );
    Ok(())
}

pub async fn build(
    cfg: &KaisatsuSection,
    ctx: BuildCtx<'_>,
    mount: Mount,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let kek_file = cfg
        .kek_file
        .as_deref()
        .or_else(|| ctx.setup.path(setup::Secret::KaisatsuKek))
        .ok_or_else(|| err(&"no kek_file"))?;
    let kek = Kek::from_file(kek_file).map_err(|e| err(&e))?;
    let store = open_kaisatsu_store(cfg, kek.clone()).await?;
    if let Some(dir) = &cfg.import_dir {
        apply_import_dir(&store, dir).await?;
    }
    apply_portal_realm(&store, ctx).await?;
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

    /// An empty import dir (first start) applies nothing; a realm file in it is applied.
    #[tokio::test]
    async fn an_empty_import_dir_applies_nothing_and_a_realm_file_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let kek = Kek::from_raw_bytes(&[7u8; 32]).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek));
        apply_import_dir(&store, dir.path()).await.unwrap();
        assert!(store.list_realms().await.unwrap().is_empty());
        std::fs::write(
            dir.path().join("realm.json"),
            r#"{"realm": "rehearsal", "enabled": true}"#,
        )
        .unwrap();
        apply_import_dir(&store, dir.path()).await.unwrap();
        assert!(store.get_realm("rehearsal").await.unwrap().is_some());
    }

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

    /// K16 Postgres path (runs with `KAISATSU_TEST_DATABASE_URL`, a superuser URL): a superuser
    /// URL is refused by the boot RLS check; the request role on a migrated database opens a
    /// `PgStore` whose writes persist across connections.
    #[tokio::test]
    async fn k16_postgres_store_runs_the_boot_rls_check_and_persists() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        let Ok(url) = std::env::var("KAISATSU_TEST_DATABASE_URL") else {
            println!("SKIP k16 postgres path: KAISATSU_TEST_DATABASE_URL is unset");
            return;
        };
        let base = PgConnectOptions::from_str(&url).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let kek_path = dir.path().join("kek");
        std::fs::write(&kek_path, "07".repeat(32)).unwrap();
        let kek = Kek::from_file(&kek_path).unwrap();
        let host = format!("{}:{}", base.get_host(), base.get_port());
        let section = |user: &str, db: &str, pw: &str| {
            let url_file = dir.path().join(format!("url-{user}"));
            std::fs::write(&url_file, format!("postgres://{user}@{host}/{db}\n")).unwrap();
            let pw_file = dir.path().join(format!("pw-{user}"));
            std::fs::write(&pw_file, pw).unwrap();
            KaisatsuSection {
                kek_file: Some(kek_path.clone()),
                import_dir: None,
                database_url_file: Some(url_file),
                db_password_file: Some(pw_file),
                db_max_connections: Some(2),
            }
        };

        // 1. The superuser connects but the boot check refuses it.
        let su_pw = url
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('@'))
            .and_then(|(cred, _)| cred.split_once(':'))
            .map(|(_, pw)| pw.to_owned())
            .unwrap_or_default();
        let err = open_kaisatsu_store(
            &section(
                base.get_username(),
                base.get_database().unwrap_or("postgres"),
                &su_pw,
            ),
            kek.clone(),
        )
        .await
        .err()
        .expect("a superuser request role is refused");
        assert!(err.to_string().contains("boot check refused"), "{err}");

        // 2. kaisatsu_app on a database migrated by kaisatsu_migrate opens and persists.
        let app_pw = uuid_like();
        let mig_pw = uuid_like();
        for (role, pw) in [("kaisatsu_app", &app_pw), ("kaisatsu_migrate", &mig_pw)] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
            )))
            .execute(&admin)
            .await
            .unwrap();
        }
        let db = format!("kaisatsu_jc_{}", uuid_like());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {db} OWNER kaisatsu_migrate"
        )))
        .execute(&admin)
        .await
        .unwrap();
        let mig = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                base.clone()
                    .database(&db)
                    .username("kaisatsu_migrate")
                    .password(&mig_pw),
            )
            .await
            .unwrap();
        PgStore::run_migrations(&mig).await.unwrap();
        mig.close().await;
        let app = section("kaisatsu_app", &db, &app_pw);
        let store = open_kaisatsu_store(&app, kek.clone()).await.unwrap();
        store
            .upsert_realm(kaisatsu_core::RealmConfig {
                id: "k16".into(),
                realm: "k16".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        drop(store);
        let again = open_kaisatsu_store(&app, kek).await.unwrap();
        assert!(
            again.get_realm("k16").await.unwrap().is_some(),
            "persisted in PostgreSQL"
        );
        drop(again);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {db} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{n:x}{:x}", std::process::id())
    }
}
