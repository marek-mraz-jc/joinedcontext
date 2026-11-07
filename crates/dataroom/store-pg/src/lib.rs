#![forbid(unsafe_code)]
//! PostgreSQL [`Store`] implementation using `sqlx`, schema `dataroom`, forced RLS, and
//! startup security verification (C1, C14, research-tenancy §2.4 & §5.3).

use chrono::Utc;
use dataroom_core::odata_defs::dataroom_service;
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    text_in_lang, ActivityRow, Catalogue, CatalogueId, CataloguePlugin, DataroomError, Dataset,
    Distribution, HarvestRun, HarvestSource, OrgId, ProfileRecord, Publisher, RecordState,
    RequestCtx, ResourceView, Result, ShaclValidationReport, StaticPage, Store,
};
use odata_table::{PgTable, Table};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use sqlx::PgPool;
use std::sync::Arc;

/// All tenant tables in the `dataroom` schema that must have `relrowsecurity` and
/// `relforcerowsecurity` enabled at boot (C1).
pub const REQUIRED_RLS_TABLES: [&str; 12] = [
    "cat_catalogue",
    "cat_publisher",
    "cat_dataset",
    "cat_distribution",
    "cat_view",
    "cat_harvest_source",
    "cat_harvest_run",
    "cat_activity",
    "cat_shacl_result",
    "cat_profile",
    "cat_page",
    "cat_plugin",
];

/// Embedded migration SQL (`0001_init.sql`).
pub const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");

/// PostgreSQL store backed by a `PgPool`.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connects to PostgreSQL and verifies that the role does not bypass RLS and that every
    /// `dataroom.cat_*` table has forced RLS enabled (C1). Refuses to boot otherwise.
    pub async fn connect_and_verify(pool: PgPool) -> Result<Self> {
        verify_rls_on_pool(&pool).await?;
        Ok(Self { pool })
    }

    /// Creates a `PgStore` without running the boot check (used immediately before running migrations).
    pub fn from_pool_unchecked(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs schema migrations under an advisory lock (`hashtext('jc-migrate-dataroom')`).
    pub async fn migrate(&self) -> Result<()> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DataroomError::Storage(format!("acquire connection: {e}")))?;
        sqlx::query("SELECT pg_advisory_lock(hashtext('jc-migrate-dataroom'))")
            .execute(&mut *conn)
            .await
            .map_err(|e| DataroomError::Storage(format!("advisory lock: {e}")))?;

        let res = sqlx::raw_sql(MIGRATION_0001_SQL).execute(&mut *conn).await;

        let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('jc-migrate-dataroom'))")
            .execute(&mut *conn)
            .await;

        res.map_err(|e| DataroomError::Storage(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Opens a transaction with `jc.org` and `jc.catalogue` set locally via `set_config(..., true)`.
    async fn begin_tenant(
        &self,
        org_id: OrgId,
        catalogue_id: Option<CatalogueId>,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DataroomError::Storage(format!("begin tx: {e}")))?;
        set_tenant_config(&mut tx, org_id, catalogue_id).await?;
        Ok(tx)
    }
}

/// Sets `jc.org` and optionally `jc.catalogue` on the current transaction.
pub async fn set_tenant_config(
    conn: &mut PgConnection,
    org_id: OrgId,
    catalogue_id: Option<CatalogueId>,
) -> Result<()> {
    sqlx::query("SELECT set_config('jc.org', $1, true)")
        .bind(org_id.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|e| DataroomError::Storage(format!("set jc.org: {e}")))?;
    if let Some(cid) = catalogue_id {
        sqlx::query("SELECT set_config('jc.catalogue', $1, true)")
            .bind(cid.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| DataroomError::Storage(format!("set jc.catalogue: {e}")))?;
    }
    Ok(())
}

/// Boot check (C1): verifies that:
/// 1. The current database role is neither `rolsuper` nor `rolbypassrls`.
/// 2. Every table in schema `dataroom` has both `relrowsecurity = true` and
///    `relforcerowsecurity = true`, and every table in [`REQUIRED_RLS_TABLES`] exists.
pub async fn verify_rls_on_pool(pool: &PgPool) -> Result<()> {
    let (is_super, bypass_rls): (bool, bool) = sqlx::query_as(
        "SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| DataroomError::Storage(format!("failed to inspect current role: {e}")))?;

    if is_super || bypass_rls {
        return Err(DataroomError::Storage(
            "boot refused (C1): application database role has SUPERUSER or BYPASSRLS".into(),
        ));
    }

    // Every table in the schema, not only the known ones: a table added later without forced
    // RLS refuses the boot as well (A6).
    let tables: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relrowsecurity, c.relforcerowsecurity
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'dataroom' AND c.relkind IN ('r', 'p')
         ORDER BY c.relname",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| DataroomError::Storage(format!("failed to check RLS in schema dataroom: {e}")))?;

    for (table, rls, forced) in &tables {
        if !(*rls && *forced) {
            return Err(DataroomError::Storage(format!(
                "boot refused (C1): table dataroom.{table} has relrowsecurity={rls}, relforcerowsecurity={forced}"
            )));
        }
    }
    for table in REQUIRED_RLS_TABLES {
        if !tables.iter().any(|(name, _, _)| name == table) {
            return Err(DataroomError::Storage(format!(
                "boot refused (C1): table dataroom.{table} does not exist"
            )));
        }
    }
    Ok(())
}

impl Store for PgStore {
    fn resolve_host<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        Box::pin(async move {
            let clean = host.split(':').next().unwrap_or(host).trim();
            let row: Option<(Json<Catalogue>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_catalogue WHERE host COLLATE \"C\" = $1 LIMIT 1",
            )
            .bind(clean)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(c),)| c))
        })
    }

    fn list_catalogues<'a>(&'a self, org_id: OrgId) -> BoxFuture<'a, Result<Vec<Catalogue>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(org_id, None).await?;
            let rows: Vec<(Json<Catalogue>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_catalogue WHERE org_id = $1 ORDER BY name COLLATE \"C\"",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(c),)| c).collect())
        })
    }

    fn get_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(org_id, Some(catalogue_id)).await?;
            let row: Option<(Json<Catalogue>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_catalogue WHERE org_id = $1 AND id = $2",
            )
            .bind(org_id.as_uuid())
            .bind(catalogue_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(c),)| c))
        })
    }

    fn upsert_catalogue<'a>(&'a self, catalogue: Catalogue) -> BoxFuture<'a, Result<Catalogue>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(catalogue.org_id, Some(catalogue.id))
                .await?;
            let title = text_in_lang(&catalogue.title, Some(&catalogue.default_language))
                .unwrap_or(&catalogue.name)
                .to_owned();
            let desc = text_in_lang(&catalogue.description, Some(&catalogue.default_language))
                .map(str::to_owned);
            sqlx::query(
                "INSERT INTO dataroom.cat_catalogue
                 (org_id, id, name, host, title, description, default_language, main_profile, modified, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (org_id, id) DO UPDATE SET
                    name = EXCLUDED.name,
                    host = EXCLUDED.host,
                    title = EXCLUDED.title,
                    description = EXCLUDED.description,
                    default_language = EXCLUDED.default_language,
                    main_profile = EXCLUDED.main_profile,
                    modified = EXCLUDED.modified,
                    doc = EXCLUDED.doc",
            )
            .bind(catalogue.org_id.as_uuid())
            .bind(catalogue.id.as_uuid())
            .bind(&catalogue.name)
            .bind(&catalogue.host)
            .bind(title)
            .bind(desc)
            .bind(&catalogue.default_language)
            .bind(&catalogue.main_profile)
            .bind(catalogue.modified)
            .bind(Json(&catalogue))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(catalogue)
        })
    }

    fn list_publishers<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Publisher>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<Publisher>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_publisher
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(p),)| p).collect())
        })
    }

    fn get_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Publisher>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let mut target_key = id_or_name.to_owned();
            let mut resolved = None;
            for _ in 0..4 {
                let row: Option<(Json<Publisher>,)> = sqlx::query_as(
                    "SELECT doc FROM dataroom.cat_publisher
                     WHERE org_id = $1 AND catalogue_id = $2
                       AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3 OR uri COLLATE \"C\" = $3)
                     LIMIT 1",
                )
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(&target_key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
                let Some((Json(pub_rec),)) = row else {
                    break;
                };
                if let Some(ref merged) = pub_rec.merged_into {
                    target_key = merged.clone();
                    resolved = Some(pub_rec);
                } else {
                    resolved = Some(pub_rec);
                    break;
                }
            }
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(resolved)
        })
    }

    fn upsert_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut publisher: Publisher,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            publisher.org_id = ctx.org_id;
            publisher.catalogue_id = ctx.catalogue_id;
            if publisher.id.is_empty() {
                publisher.id = format!("pub-{}", publisher.name);
            }
            let title = text_in_lang(&publisher.title, None)
                .unwrap_or(&publisher.name)
                .to_owned();
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_publisher
                 (org_id, catalogue_id, id, name, title, uri, state, merged_into, modified, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    name = EXCLUDED.name,
                    title = EXCLUDED.title,
                    uri = EXCLUDED.uri,
                    state = EXCLUDED.state,
                    merged_into = EXCLUDED.merged_into,
                    modified = EXCLUDED.modified,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&publisher.id)
            .bind(&publisher.name)
            .bind(title)
            .bind(&publisher.uri)
            .bind(publisher.state.as_str())
            .bind(&publisher.merged_into)
            .bind(publisher.modified)
            .bind(Json(&publisher))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(publisher)
        })
    }

    fn merge_publishers<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id_or_name: &'a str,
        target_id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            let Some(mut source) = self.get_publisher(ctx, source_id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{source_id_or_name}' not found"
                )));
            };
            let Some(target) = self.get_publisher(ctx, target_id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{target_id_or_name}' not found"
                )));
            };
            let datasets = self.list_datasets(ctx).await?;
            for mut ds in datasets {
                if ds.publisher_id == source.id || ds.publisher_id == source.name {
                    ds.publisher_id = target.name.clone();
                    ds.modified = Utc::now();
                    self.upsert_dataset(ctx, ds).await?;
                }
            }
            source.merged_into = Some(target.name.clone());
            source.state = RecordState::Withdrawn;
            source.modified = Utc::now();
            self.upsert_publisher(ctx, source).await?;
            Ok(target)
        })
    }

    fn withdraw_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            let Some(mut pub_rec) = self.get_publisher(ctx, id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{id_or_name}' not found"
                )));
            };
            pub_rec.state = RecordState::Withdrawn;
            pub_rec.modified = Utc::now();
            self.upsert_publisher(ctx, pub_rec).await
        })
    }

    fn list_datasets<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Dataset>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(d),)| d).collect())
        })
    }

    fn get_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Dataset>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let row: Option<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3)
                 LIMIT 1",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(id_or_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(d),)| d))
        })
    }

    fn upsert_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut dataset: Dataset,
    ) -> BoxFuture<'a, Result<Dataset>> {
        Box::pin(async move {
            dataset.org_id = ctx.org_id;
            dataset.catalogue_id = ctx.catalogue_id;
            if dataset.id.is_empty() {
                dataset.id = format!("pkg-{}", dataset.name);
            }
            for (i, dist) in dataset.distributions.iter_mut().enumerate() {
                dist.dataset_id = dataset.id.clone();
                if dist.id.is_empty() {
                    dist.id = format!("res-{}-{i}", dataset.name);
                }
            }
            let title = text_in_lang(&dataset.title, None)
                .unwrap_or(&dataset.name)
                .to_owned();
            let notes = text_in_lang(&dataset.description, None).map(str::to_owned);
            let tags_joined = dataset.tags.join(" ");
            let text_sk = format!(
                "{} {} {}",
                dataset.title.get("sk").map(String::as_str).unwrap_or(""),
                dataset.description.get("sk").map(String::as_str).unwrap_or(""),
                tags_joined
            );
            let text_cs = format!(
                "{} {} {}",
                dataset.title.get("cs").map(String::as_str).unwrap_or(""),
                dataset.description.get("cs").map(String::as_str).unwrap_or(""),
                tags_joined
            );
            let text_en = format!(
                "{} {} {}",
                dataset.title.get("en").map(String::as_str).unwrap_or(""),
                dataset.description.get("en").map(String::as_str).unwrap_or(""),
                tags_joined
            );
            let text_fi = dataset.title.get("fi").cloned().unwrap_or_default();
            let text_ca = dataset.title.get("ca").cloned().unwrap_or_default();

            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_dataset
                 (org_id, catalogue_id, id, name, title, notes, publisher_id, visibility, state,
                  licence_id, frequency, endpoint_url, issued, modified,
                  search_sk, search_cs, search_en, search_fi, search_ca, doc)
                 VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                    to_tsvector('dataroom.dataroom_simple', $15),
                    to_tsvector('dataroom.dataroom_simple', $16),
                    to_tsvector('dataroom.dataroom_simple', $17),
                    to_tsvector('dataroom.dataroom_simple', $18),
                    to_tsvector('dataroom.dataroom_simple', $19),
                    $20
                 )
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    name = EXCLUDED.name,
                    title = EXCLUDED.title,
                    notes = EXCLUDED.notes,
                    publisher_id = EXCLUDED.publisher_id,
                    visibility = EXCLUDED.visibility,
                    state = EXCLUDED.state,
                    licence_id = EXCLUDED.licence_id,
                    frequency = EXCLUDED.frequency,
                    endpoint_url = EXCLUDED.endpoint_url,
                    issued = EXCLUDED.issued,
                    modified = EXCLUDED.modified,
                    search_sk = EXCLUDED.search_sk,
                    search_cs = EXCLUDED.search_cs,
                    search_en = EXCLUDED.search_en,
                    search_fi = EXCLUDED.search_fi,
                    search_ca = EXCLUDED.search_ca,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&dataset.id)
            .bind(&dataset.name)
            .bind(title)
            .bind(notes)
            .bind(&dataset.publisher_id)
            .bind(dataset.visibility.as_str())
            .bind(dataset.state.as_str())
            .bind(&dataset.licence_id)
            .bind(&dataset.frequency)
            .bind(&dataset.endpoint_url)
            .bind(dataset.issued)
            .bind(dataset.modified)
            .bind(text_sk)
            .bind(text_cs)
            .bind(text_en)
            .bind(text_fi)
            .bind(text_ca)
            .bind(Json(&dataset))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;

            sqlx::query(
                "DELETE FROM dataroom.cat_distribution
                 WHERE org_id = $1 AND catalogue_id = $2 AND dataset_id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&dataset.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;

            for dist in &dataset.distributions {
                let dname = text_in_lang(&dist.title, None)
                    .unwrap_or(&dist.access_url)
                    .to_owned();
                sqlx::query(
                    "INSERT INTO dataroom.cat_distribution
                     (org_id, catalogue_id, dataset_id, id, name, access_url, download_url, format, media_type, byte_size, doc)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                )
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(&dataset.id)
                .bind(&dist.id)
                .bind(dname)
                .bind(&dist.access_url)
                .bind(&dist.download_url)
                .bind(&dist.format)
                .bind(&dist.media_type)
                .bind(dist.byte_size.and_then(|b| i64::try_from(b).ok()))
                .bind(Json(dist))
                .execute(&mut *tx)
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            }

            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(dataset)
        })
    }

    fn withdraw_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Dataset>> {
        Box::pin(async move {
            let Some(mut ds) = self.get_dataset(ctx, id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "dataset '{id_or_name}' not found"
                )));
            };
            ds.state = RecordState::Withdrawn;
            ds.modified = Utc::now();
            self.upsert_dataset(ctx, ds).await
        })
    }

    fn purge_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let Some(ds) = self.get_dataset(ctx, id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "dataset '{id_or_name}' not found"
                )));
            };
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "DELETE FROM dataroom.cat_distribution
                 WHERE org_id = $1 AND catalogue_id = $2 AND dataset_id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ds.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            sqlx::query(
                "DELETE FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ds.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(())
        })
    }

    fn get_distribution<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<(Dataset, Distribution)>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let row: Option<(String, Json<Distribution>)> = sqlx::query_as(
                "SELECT dataset_id, doc FROM dataroom.cat_distribution
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(distribution_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            let Some((ds_id, Json(dist))) = row else {
                return Ok(None);
            };
            let Some(ds) = self.get_dataset(ctx, &ds_id).await? else {
                return Ok(None);
            };
            Ok(Some((ds, dist)))
        })
    }

    fn list_resource_views<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        resource_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ResourceView>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<ResourceView>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2 AND distribution_id COLLATE \"C\" = $3
                 ORDER BY view_order ASC, id COLLATE \"C\" ASC",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(resource_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(v),)| v).collect())
        })
    }

    fn get_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ResourceView>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let row: Option<(Json<ResourceView>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(view_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(v),)| v))
        })
    }

    fn upsert_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut view: ResourceView,
    ) -> BoxFuture<'a, Result<ResourceView>> {
        Box::pin(async move {
            if view.id.is_empty() {
                view.id = format!("view-{}", uuid::Uuid::now_v7());
            }
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_view
                 (org_id, catalogue_id, distribution_id, id, view_type, title, view_order, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    view_type = EXCLUDED.view_type,
                    title = EXCLUDED.title,
                    view_order = EXCLUDED.view_order,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&view.resource_id)
            .bind(&view.id)
            .bind(&view.view_type)
            .bind(&view.title)
            .bind(view.order)
            .bind(Json(&view))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(view)
        })
    }

    fn delete_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let res = sqlx::query(
                "DELETE FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(view_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            if res.rows_affected() == 0 {
                return Err(DataroomError::NotFound(format!(
                    "resource view '{view_id}' not found"
                )));
            }
            Ok(())
        })
    }

    fn list_harvest_sources<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<HarvestSource>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<HarvestSource>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_source
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(s),)| s).collect())
        })
    }

    fn get_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestSource>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let row: Option<(Json<HarvestSource>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_source
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3)
                 LIMIT 1",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(id_or_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(s),)| s))
        })
    }

    fn upsert_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut source: HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestSource>> {
        Box::pin(async move {
            source.org_id = ctx.org_id;
            source.catalogue_id = ctx.catalogue_id;
            if source.id.is_empty() {
                source.id = format!("hs-{}", source.name);
            }
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_harvest_source
                 (org_id, catalogue_id, id, name, kind, url, publisher_id, enabled, modified, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    name = EXCLUDED.name,
                    kind = EXCLUDED.kind,
                    url = EXCLUDED.url,
                    publisher_id = EXCLUDED.publisher_id,
                    enabled = EXCLUDED.enabled,
                    modified = EXCLUDED.modified,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&source.id)
            .bind(&source.name)
            .bind(source.kind.as_str())
            .bind(&source.url)
            .bind(&source.publisher_id)
            .bind(source.enabled)
            .bind(source.modified)
            .bind(Json(&source))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(source)
        })
    }

    fn list_harvest_runs<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<HarvestRun>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<HarvestRun>,)> = match source_id {
                Some(sid) => {
                    sqlx::query_as(
                        "SELECT doc FROM dataroom.cat_harvest_run
                         WHERE org_id = $1 AND catalogue_id = $2 AND source_id COLLATE \"C\" = $3
                         ORDER BY started_at DESC",
                    )
                    .bind(ctx.org_id.as_uuid())
                    .bind(ctx.catalogue_id.as_uuid())
                    .bind(sid)
                    .fetch_all(&mut *tx)
                    .await
                }
                None => {
                    sqlx::query_as(
                        "SELECT doc FROM dataroom.cat_harvest_run
                         WHERE org_id = $1 AND catalogue_id = $2
                         ORDER BY started_at DESC",
                    )
                    .bind(ctx.org_id.as_uuid())
                    .bind(ctx.catalogue_id.as_uuid())
                    .fetch_all(&mut *tx)
                    .await
                }
            }
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(r),)| r).collect())
        })
    }

    fn get_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestRun>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let row: Option<(Json<HarvestRun>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_run
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(row.map(|(Json(r),)| r))
        })
    }

    fn upsert_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut run: HarvestRun,
    ) -> BoxFuture<'a, Result<HarvestRun>> {
        Box::pin(async move {
            run.org_id = ctx.org_id;
            run.catalogue_id = ctx.catalogue_id;
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_harvest_run
                 (org_id, catalogue_id, id, source_id, state, started_at, finished_at,
                  created_count, updated_count, unchanged_count, withdrawn_count, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    state = EXCLUDED.state,
                    finished_at = EXCLUDED.finished_at,
                    created_count = EXCLUDED.created_count,
                    updated_count = EXCLUDED.updated_count,
                    unchanged_count = EXCLUDED.unchanged_count,
                    withdrawn_count = EXCLUDED.withdrawn_count,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&run.id)
            .bind(&run.source_id)
            .bind(run.state.as_str())
            .bind(run.started_at)
            .bind(run.finished_at)
            .bind(run.created_count as i64)
            .bind(run.updated_count as i64)
            .bind(run.unchanged_count as i64)
            .bind(run.withdrawn_count as i64)
            .bind(Json(&run))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(run)
        })
    }

    fn record_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut row: ActivityRow,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            row.org_id = ctx.org_id;
            row.catalogue_id = ctx.catalogue_id;
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_activity
                 (org_id, catalogue_id, id, at, actor, operation, object_type, object_id, summary, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&row.id)
            .bind(row.at)
            .bind(&row.actor)
            .bind(&row.operation)
            .bind(&row.object_type)
            .bind(&row.object_id)
            .bind(&row.summary)
            .bind(Json(&row))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(())
        })
    }

    fn list_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        object_id: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ActivityRow>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<ActivityRow>,)> = match object_id {
                Some(oid) => {
                    sqlx::query_as(
                        "SELECT doc FROM dataroom.cat_activity
                         WHERE org_id = $1 AND catalogue_id = $2 AND object_id COLLATE \"C\" = $3
                         ORDER BY at DESC LIMIT $4",
                    )
                    .bind(ctx.org_id.as_uuid())
                    .bind(ctx.catalogue_id.as_uuid())
                    .bind(oid)
                    .bind(limit as i64)
                    .fetch_all(&mut *tx)
                    .await
                }
                None => {
                    sqlx::query_as(
                        "SELECT doc FROM dataroom.cat_activity
                         WHERE org_id = $1 AND catalogue_id = $2
                         ORDER BY at DESC LIMIT $3",
                    )
                    .bind(ctx.org_id.as_uuid())
                    .bind(ctx.catalogue_id.as_uuid())
                    .bind(limit as i64)
                    .fetch_all(&mut *tx)
                    .await
                }
            }
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(a),)| a).collect())
        })
    }

    fn upsert_shacl_report<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut report: ShaclValidationReport,
    ) -> BoxFuture<'a, Result<ShaclValidationReport>> {
        Box::pin(async move {
            report.org_id = ctx.org_id;
            report.catalogue_id = ctx.catalogue_id;
            if report.id.is_empty() {
                report.id = format!(
                    "{}:{}:{}",
                    report.dataset_id, report.profile_id, report.profile_version
                );
            }
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_shacl_result
                 (org_id, catalogue_id, id, dataset_id, profile_id, profile_version,
                  conforms, violation_count, warning_count, checked_at, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (org_id, catalogue_id, id) DO UPDATE SET
                    conforms = EXCLUDED.conforms,
                    violation_count = EXCLUDED.violation_count,
                    warning_count = EXCLUDED.warning_count,
                    checked_at = EXCLUDED.checked_at,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&report.id)
            .bind(&report.dataset_id)
            .bind(&report.profile_id)
            .bind(&report.profile_version)
            .bind(report.conforms)
            .bind(report.violation_count as i64)
            .bind(report.warning_count as i64)
            .bind(report.checked_at)
            .bind(Json(&report))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(report)
        })
    }

    fn list_shacl_reports<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset_id: Option<&'a str>,
        profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<ShaclValidationReport>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<ShaclValidationReport>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_shacl_result
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND ($3::text IS NULL OR dataset_id COLLATE \"C\" = $3)
                   AND ($4::text IS NULL OR profile_id COLLATE \"C\" = $4)
                 ORDER BY id COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(dataset_id)
            .bind(profile_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(r),)| r).collect())
        })
    }

    fn list_profiles<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<ProfileRecord>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<ProfileRecord>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_profile
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY id COLLATE \"C\", version COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(p),)| p).collect())
        })
    }

    fn upsert_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
    ) -> BoxFuture<'a, Result<ProfileRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_profile (org_id, catalogue_id, id, version, doc)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (org_id, catalogue_id, id, version) DO UPDATE SET doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&profile.id)
            .bind(&profile.version)
            .bind(Json(&profile))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(profile)
        })
    }

    fn list_pages<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<StaticPage>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<StaticPage>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_page
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY slug COLLATE \"C\", lang COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(p),)| p).collect())
        })
    }

    fn upsert_page<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut page: StaticPage,
    ) -> BoxFuture<'a, Result<StaticPage>> {
        Box::pin(async move {
            page.org_id = ctx.org_id;
            page.catalogue_id = ctx.catalogue_id;
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_page (org_id, catalogue_id, slug, lang, modified, doc)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (org_id, catalogue_id, slug, lang) DO UPDATE SET
                    modified = EXCLUDED.modified,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&page.slug)
            .bind(&page.lang)
            .bind(page.modified)
            .bind(Json(&page))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(page)
        })
    }

    fn list_plugins<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<CataloguePlugin>>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            let rows: Vec<(Json<CataloguePlugin>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_plugin
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(rows.into_iter().map(|(Json(p),)| p).collect())
        })
    }

    fn upsert_plugin<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut plugin: CataloguePlugin,
    ) -> BoxFuture<'a, Result<CataloguePlugin>> {
        Box::pin(async move {
            plugin.org_id = ctx.org_id;
            plugin.catalogue_id = ctx.catalogue_id;
            let mut tx = self.begin_tenant(ctx.org_id, Some(ctx.catalogue_id)).await?;
            sqlx::query(
                "INSERT INTO dataroom.cat_plugin (org_id, catalogue_id, name, modified, doc)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (org_id, catalogue_id, name) DO UPDATE SET
                    modified = EXCLUDED.modified,
                    doc = EXCLUDED.doc",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&plugin.name)
            .bind(plugin.modified)
            .bind(Json(&plugin))
            .execute(&mut *tx)
            .await
            .map_err(|e| DataroomError::Storage(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| DataroomError::Storage(e.to_string()))?;
            Ok(plugin)
        })
    }

    fn odata_table<'a>(
        &'a self,
        _ctx: &'a RequestCtx,
        set_name: &'a str,
    ) -> BoxFuture<'a, Result<Arc<dyn Table>>> {
        let service = dataroom_service();
        let model = Arc::new(service.model());
        let Some(info) = service.info(model, set_name) else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "entity set '{set_name}' not found"
            )))));
        };
        let relation = match set_name {
            "Catalogues" => "dataroom.cat_catalogue",
            "Datasets" => "dataroom.cat_dataset",
            "Distributions" => "dataroom.cat_distribution",
            "Publishers" => "dataroom.cat_publisher",
            "HarvestSources" => "dataroom.cat_harvest_source",
            "HarvestRuns" => "dataroom.cat_harvest_run",
            "ActivityRows" => "dataroom.cat_activity",
            "ShaclResults" => "dataroom.cat_shacl_result",
            _ => {
                return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                    "entity set '{set_name}' not found"
                )))));
            }
        };
        let res = PgTable::new(info, self.pool.clone(), relation)
            .map(|t| Arc::new(t) as Arc<dyn Table>)
            .map_err(DataroomError::from);
        Box::pin(std::future::ready(res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::{
        Catalogue, CatalogueId, OrgId, Publisher, RecordState, RequestCtx, Store, Subject,
        ThemeTokens, Visibility,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    #[tokio::test]
    async fn a6_postgres_rls_tenant_isolation_and_boot_check() {
        let Some(db_url) = std::env::var("DATAROOM_TEST_DATABASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
        else {
            println!("SKIP a6_postgres_rls_tenant_isolation_and_boot_check: DATAROOM_TEST_DATABASE_URL is unset");
            return;
        };

        let admin_pool = PgPool::connect(&db_url).await.expect("connect test db");
        let mig_store = PgStore::from_pool_unchecked(admin_pool.clone());
        mig_store.migrate().await.expect("migrate dataroom schema");

        // A non-superuser, non-BYPASSRLS request role for the RLS isolation check and the boot
        // check (a superuser pool is refused for being one, which proves nothing about RLS).
        sqlx::query(
            "DO $$
            BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dataroom_app_test') THEN
                    CREATE ROLE dataroom_app_test NOLOGIN NOSUPERUSER NOBYPASSRLS;
                END IF;
            END $$;",
        )
        .execute(&admin_pool)
        .await
        .expect("create request role");
        sqlx::query("GRANT USAGE ON SCHEMA dataroom TO dataroom_app_test")
            .execute(&admin_pool)
            .await
            .expect("grant schema usage");
        sqlx::query(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO dataroom_app_test",
        )
        .execute(&admin_pool)
        .await
        .expect("grant table rights");

        let request_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET ROLE dataroom_app_test")
                        .execute(&mut *conn)
                        .await
                        .map(|_| ())
                })
            })
            .connect(&db_url)
            .await
            .expect("connect as request role");

        // The superuser pool is refused; the request role passes on the migrated schema.
        assert!(PgStore::connect_and_verify(admin_pool.clone()).await.is_err());
        PgStore::connect_and_verify(request_pool.clone())
            .await
            .expect("request role passes the boot check on the migrated schema");

        // Boot check refusal for a newly created table without forced RLS (C1, A6).
        sqlx::query("CREATE TABLE dataroom.zz_a6_no_forced_rls (org_id uuid NOT NULL)")
            .execute(&admin_pool)
            .await
            .expect("create table without forced RLS");
        let refused = PgStore::connect_and_verify(request_pool.clone()).await;
        sqlx::query("DROP TABLE dataroom.zz_a6_no_forced_rls")
            .execute(&admin_pool)
            .await
            .expect("drop test table");
        match refused {
            Err(DataroomError::Storage(msg)) => assert!(
                msg.contains("zz_a6_no_forced_rls"),
                "refusal names the table: {msg}"
            ),
            Err(other) => panic!("expected a storage refusal, got {other:?}"),
            Ok(_) => panic!("boot check must refuse a table without forced RLS"),
        }

        let store = PgStore::from_pool_unchecked(admin_pool.clone());
        let org_a = OrgId::from_uuid(Uuid::now_v7());
        let cat_a = CatalogueId::from_uuid(Uuid::now_v7());
        let org_b = OrgId::from_uuid(Uuid::now_v7());
        let cat_b = CatalogueId::from_uuid(Uuid::now_v7());

        for (org, cat, host) in [
            (org_a, cat_a, format!("a-{}.example.org", Uuid::now_v7().simple())),
            (org_b, cat_b, format!("b-{}.example.org", Uuid::now_v7().simple())),
        ] {
            store
                .upsert_catalogue(Catalogue {
                    org_id: org,
branding: None,
                    hidden_datasets: Default::default(),
                    id: cat,
                    name: "shared".into(),
                    host,
                    title: BTreeMap::from([("sk".into(), "Cat".into())]),
                    description: BTreeMap::new(),
                    homepage: None,
                    default_language: "sk".into(),
                    languages: vec!["sk".into()],
                    main_profile: "dcat-ap-3.0.1".into(),
                    active_profiles: Vec::new(),
                    default_licence: Some("cc-by".into()),
                    publisher_uri: None,
                    publisher_name: None,
                    contact_point: None,
                    theme: ThemeTokens::default(),
                    allow_cross_org_harvest: true,
                    allowed_outbound_hosts: Vec::new(),
                    applicable_legislation: Vec::new(),
                    modified: Utc::now(),
                })
                .await
                .expect("upsert catalogue");
        }

        let ctx_a = RequestCtx::new(
            org_a,
            cat_a,
            Subject::authenticated("alice", vec![org_a], Vec::new()),
        );
        let ctx_b = RequestCtx::new(
            org_b,
            cat_b,
            Subject::authenticated("bob", vec![org_b], Vec::new()),
        );

        store
            .upsert_publisher(
                &ctx_b,
                Publisher {
                    org_id: org_b,
badge_initials: None,
                    badge_colour: None,
                    declared_by_instance: None,
                    catalogue_id: cat_b,
                    id: "pub-b".into(),
                    name: "city-b".into(),
                    title: BTreeMap::from([("sk".into(), "City B".into())]),
                    description: BTreeMap::new(),
                    uri: "https://b.example.org/org".into(),
                    publisher_type: None,
                    identifier: None,
                    email: None,
                    homepage: None,
                    logo_url: None,
                    contact_point: None,
                    state: RecordState::Active,
                    merged_into: None,
                    editor_grants: Vec::new(),
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .expect("upsert pub b");

        // Verify as non-BYPASSRLS role `dataroom_app_test` that org A cannot see org B rows
        let mut tx = admin_pool.begin().await.expect("begin tx");
        sqlx::query("SET LOCAL ROLE dataroom_app_test")
            .execute(&mut *tx)
            .await
            .expect("set role dataroom_app_test");
        set_tenant_config(&mut tx, org_a, Some(cat_a))
            .await
            .expect("set tenant config A");

        let count_b_from_a: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dataroom.cat_publisher")
                .fetch_one(&mut *tx)
                .await
                .expect("count publishers as A");
        assert_eq!(count_b_from_a.0, 0, "RLS must hide org B rows from org A");
        tx.rollback().await.ok();

        assert!(store.get_publisher(&ctx_a, "city-b").await.unwrap().is_none());
        let _ = Visibility::Internal;
    }
}
