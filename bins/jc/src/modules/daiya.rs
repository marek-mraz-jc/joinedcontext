//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve` on its
//! in-memory store. When the Portal runs beside it, first-start setup holds a password for the
//! Portal's Grafana client, which daiya accepts as its server administrator (HTTP Basic,
//! `DaiyaState::with_server_admin`), the role Grafana's admin user holds for the Portal.

use crate::config::DaiyaSection;
use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use axum::Router;
use daiya_model::{DaiyaStore, OrgId};
use daiya_server::{DaiyaState, router};
use daiya_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

/// daiya's store: PostgreSQL when `[daiya.database]` is set (migrations first when it names a
/// migration role, then the boot check that refuses an app role bypassing RLS), else memory.
async fn store(cfg: &DaiyaSection) -> Result<Arc<dyn DaiyaStore>, String> {
    let Some(db) = &cfg.database else {
        return Ok(Arc::new(MemoryStore::new()));
    };
    if let Some(pool) = db.migrate_pool().await? {
        daiya_store_pg::migrate(&pool)
            .await
            .map_err(|e| e.to_string())?;
        pool.close().await;
    }
    let pg = daiya_store_pg::PgStore::new(db.app_pool().await?);
    pg.verify_rls_enforced().await.map_err(|e| e.to_string())?;
    Ok(Arc::new(pg))
}

pub(crate) async fn state(cfg: &DaiyaSection, setup: &Setup) -> Result<DaiyaState, String> {
    let org = OrgId::from_u128(1);
    let store = store(cfg).await?;
    let Some(password) = setup.value(Secret::DashboardsPassword) else {
        return Ok(DaiyaState::for_standalone(store, org));
    };
    DaiyaState::for_standalone(store, org)
        .with_server_admin(DASHBOARDS_USER, password)
        .map_err(|e| e.to_string())
}

pub async fn build(cfg: &DaiyaSection, setup: &Setup, mount: Mount) -> Result<Router, String> {
    Ok(router(state(cfg, setup).await?, mount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use daiya_model::{FolderRecord, InstallationId, InstanceId};

    fn folder(org: OrgId) -> FolderRecord {
        let now = chrono::Utc::now();
        FolderRecord {
            installation_id: InstallationId::from_u128(1),
            org_id: org,
            instance_id: InstanceId::from_u128(1),
            project_id: Some("e2e".into()),
            uid: "project-e2e".into(),
            parent_uid: None,
            title: "e2e".into(),
            version: 1,
            managed_by: None,
            created_by: "admin".into(),
            updated_by: "admin".into(),
            created_at: now,
            updated_at: now,
        }
    }

    /// A folder written before a restart reads back after it; a role that bypasses RLS is
    /// refused at start.
    #[tokio::test]
    async fn daiya_on_postgres_keeps_state_over_a_restart_and_refuses_a_bypassing_role() {
        let Some(db) = crate::pg::testdb::create("daiya").await else {
            return;
        };
        let org = OrgId::from_u128(1);
        let cfg = DaiyaSection {
            database: Some(db.database.clone()),
        };
        store(&cfg)
            .await
            .unwrap()
            .upsert_folder(folder(org))
            .await
            .unwrap();
        let again = store(&cfg).await.unwrap();
        let read = again.get_folder(org, "project-e2e").await.unwrap();
        assert_eq!(
            read.map(|f| f.title),
            Some("e2e".to_owned()),
            "state survives a restart"
        );
        let bypassing = DaiyaSection {
            database: Some(db.admin.clone()),
        };
        let refused = store(&bypassing)
            .await
            .err()
            .expect("a superuser app role is refused");
        assert!(refused.to_lowercase().contains("rls"), "{refused}");
        drop(db.dir);
    }
}
