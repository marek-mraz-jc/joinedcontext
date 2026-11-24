#![forbid(unsafe_code)]
//! PostgreSQL [`Store`] implementation using `sqlx`, schema `dataroom`, forced RLS, and
//! startup security verification (C1, C14, research-tenancy §2.4 & §5.3).

use dataroom_core::odata_defs::{
    activity_to_row, catalogue_to_row, dataroom_service, dataset_to_row, distribution_to_row,
    harvest_run_to_row, harvest_source_to_row, publisher_to_row, shacl_report_to_row,
};
use dataroom_core::store::{changed_since_read, next_modified, BoxFuture};
use dataroom_core::{
    text_in_lang, ActivityRow, Catalogue, CatalogueDeclaration, CatalogueId, CataloguePlugin,
    DataroomError, Dataset, Distribution, HarvestRun, HarvestSource, OrgId, PickPage,
    ProfileRecord, Publisher, RecordState, RequestCtx, ResourceView, Result, SearchCandidate,
    SearchPage, SearchQuery, ShaclValidationReport, StaticPage, Store,
};
use dataroom_search::SearchLexemes;
use jc_time::TimestampTz;
use odata_table::{MemoryTable, Table};
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

        let res = match sqlx::raw_sql(MIGRATION_0001_SQL).execute(&mut *conn).await {
            Ok(_) => reindex_search(&mut conn).await.map(|_| ()),
            Err(e) => Err(DataroomError::Storage(format!("{e}"))),
        };

        let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('jc-migrate-dataroom'))")
            .execute(&mut *conn)
            .await;

        res.map_err(|e| DataroomError::Storage(format!("migration failed: {e}")))
    }

    /// Moves every row of `catalogue` from tenant `from` to tenant `to` in one transaction, as
    /// the schema owner (ODP-12: each JC Organization's catalogue gets the Organization's own
    /// `OrgId`). Forced row level security binds the owner too, so the transaction lifts it per
    /// table around its one `UPDATE` and forces it again before the commit; a failure rolls back
    /// both, rows and settings. Deletes nothing; returns the number of rows moved (0, with no
    /// table lock taken, when the catalogue row is not under `from`, so a second start moves
    /// nothing).
    pub async fn move_catalogue(
        &self,
        from: OrgId,
        to: OrgId,
        catalogue: CatalogueId,
    ) -> Result<u64> {
        let fail =
            |e: sqlx::Error| DataroomError::Storage(format!("move catalogue {catalogue}: {e}"));
        let mut tx = self.pool.begin().await.map_err(fail)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('jc-migrate-dataroom'))")
            .execute(&mut *tx)
            .await
            .map_err(fail)?;
        // Nothing to move on every start after the first: look under the tenant's own row level
        // security first, so a start takes no table lock (ALTER TABLE) for an empty move.
        set_tenant_config(&mut tx, from, Some(catalogue)).await?;
        let (pending,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM dataroom.cat_catalogue WHERE org_id = $1 AND id = $2)",
        )
        .bind(from.as_uuid())
        .bind(catalogue.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(fail)?;
        if !pending {
            tx.commit().await.map_err(fail)?;
            return Ok(0);
        }
        let mut moved = 0;
        for table in REQUIRED_RLS_TABLES {
            let column = if table == "cat_catalogue" {
                "id"
            } else {
                "catalogue_id"
            };
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "ALTER TABLE dataroom.{table} NO FORCE ROW LEVEL SECURITY"
            )))
            .execute(&mut *tx)
            .await
            .map_err(fail)?;
            moved += sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE dataroom.{table}
                 SET org_id = $1,
                     doc = CASE WHEN doc ? 'orgId'
                                THEN jsonb_set(doc, '{{orgId}}', to_jsonb($1::text))
                                ELSE doc END
                 WHERE org_id = $2 AND {column} = $3"
            )))
            .bind(to.as_uuid())
            .bind(from.as_uuid())
            .bind(catalogue.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(fail)?
            .rows_affected();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "ALTER TABLE dataroom.{table} FORCE ROW LEVEL SECURITY"
            )))
            .execute(&mut *tx)
            .await
            .map_err(fail)?;
        }
        tx.commit().await.map_err(fail)?;
        Ok(moved)
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

/// Maps a database error to the store contract: a unique violation is the caller's conflict
/// (with CKAN's field error for a taken dataset URL), not a storage failure.
fn db_err(e: sqlx::Error) -> DataroomError {
    if let Some(db) = e.as_database_error() {
        if db.code().as_deref() == Some("23505") {
            return match db.constraint() {
                Some("cat_dataset_org_id_catalogue_id_name_key") => {
                    DataroomError::field_error("name", "That URL is already in use.")
                }
                Some("cat_publisher_org_id_catalogue_id_name_key") => DataroomError::Conflict(
                    "publisher name already exists in this catalogue".into(),
                ),
                Some("cat_catalogue_host_key" | "uq_cat_catalogue_host_lower") => {
                    DataroomError::Conflict("host is already claimed by another catalogue".into())
                }
                Some("uq_cat_catalogue_id") => {
                    DataroomError::Conflict("catalogue id belongs to another organisation".into())
                }
                other => DataroomError::Conflict(format!(
                    "duplicate record ({})",
                    other.unwrap_or("unnamed constraint")
                )),
            };
        }
    }
    DataroomError::Storage(e.to_string())
}

/// The publisher with this id, name or IRI in the caller's tenant, without following merges;
/// with `lock`, its row stays locked for the rest of the caller's transaction.
async fn find_publisher(
    conn: &mut PgConnection,
    ctx: &RequestCtx,
    id_or_name: &str,
    lock: bool,
) -> Result<Option<Publisher>> {
    macro_rules! find {
        () => {
            "SELECT doc FROM dataroom.cat_publisher
         WHERE org_id = $1 AND catalogue_id = $2
           AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3 OR uri COLLATE \"C\" = $3)
         ORDER BY (id COLLATE \"C\" = $3) DESC, (name COLLATE \"C\" = $3) DESC
         LIMIT 1"
        };
    }
    let sql = if lock {
        concat!(find!(), " FOR UPDATE")
    } else {
        find!()
    };
    let row: Option<(Json<Publisher>,)> = sqlx::query_as(sql)
        .bind(ctx.org_id.as_uuid())
        .bind(ctx.catalogue_id.as_uuid())
        .bind(id_or_name)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?;
    Ok(row.map(|(Json(p),)| p))
}

/// Writes a publisher row inside the caller's tenant transaction.
async fn write_publisher(
    conn: &mut PgConnection,
    ctx: &RequestCtx,
    publisher: &Publisher,
) -> Result<()> {
    let title = text_in_lang(&publisher.title, None)
        .unwrap_or(&publisher.name)
        .to_owned();
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
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Fills the ids a new dataset and its distributions get, in the request's tenant.
fn normalise_dataset(ctx: &RequestCtx, mut dataset: Dataset) -> Dataset {
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
    dataset
}

/// Refuses a write into a deleted catalogue, so a harvest run still going when its catalogue
/// was deleted cannot write rows back. `FOR SHARE` waits for a delete in flight to commit and
/// then reads its tombstone. A catalogue with no row at all is the caller's business.
async fn refuse_deleted(conn: &mut PgConnection, ctx: &RequestCtx) -> Result<()> {
    let deleted: Option<(bool,)> = sqlx::query_as(
        "SELECT doc->>'deleted' IS NOT NULL FROM dataroom.cat_catalogue
         WHERE org_id = $1 AND id = $2 FOR SHARE",
    )
    .bind(ctx.org_id.as_uuid())
    .bind(ctx.catalogue_id.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    if let Some((true,)) = deleted {
        return Err(DataroomError::Gone(format!(
            "catalogue {} was deleted",
            ctx.catalogue_id
        )));
    }
    Ok(())
}

/// Writes a catalogue row inside the caller's tenant transaction.
async fn write_catalogue(conn: &mut PgConnection, catalogue: &Catalogue) -> Result<()> {
    let title = text_in_lang(&catalogue.title, Some(&catalogue.default_language))
        .unwrap_or(&catalogue.name)
        .to_owned();
    let desc =
        text_in_lang(&catalogue.description, Some(&catalogue.default_language)).map(str::to_owned);
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
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Appends an activity row inside the caller's tenant transaction, so it commits with the
/// change it records (AUD-02).
async fn insert_activity(
    conn: &mut PgConnection,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    mut row: ActivityRow,
) -> Result<()> {
    row.org_id = org_id;
    row.catalogue_id = catalogue_id;
    sqlx::query(
        "INSERT INTO dataroom.cat_activity
         (org_id, catalogue_id, id, at, actor, operation, object_type, object_id, summary, doc)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(org_id.as_uuid())
    .bind(catalogue_id.as_uuid())
    .bind(&row.id)
    .bind(row.at)
    .bind(&row.actor)
    .bind(&row.operation)
    .bind(&row.object_type)
    .bind(&row.object_id)
    .bind(&row.summary)
    .bind(Json(&row))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// [`insert_activity`] in the request's tenant, then the commit: the row lands with the change
/// the transaction holds or neither does (AUD-02). A row with no `object_id` names `object_id`.
async fn commit_with_activity(
    mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
    ctx: &RequestCtx,
    mut row: ActivityRow,
    object_id: &str,
) -> Result<()> {
    if row.object_id.is_empty() {
        object_id.clone_into(&mut row.object_id);
    }
    insert_activity(&mut tx, ctx.org_id, ctx.catalogue_id, row).await?;
    tx.commit().await.map_err(db_err)
}

/// Writes a resource view row inside the caller's tenant transaction.
async fn write_view(conn: &mut PgConnection, ctx: &RequestCtx, view: &ResourceView) -> Result<()> {
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
    .bind(Json(view))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Writes a harvest source row inside the caller's tenant transaction.
async fn write_source(
    conn: &mut PgConnection,
    ctx: &RequestCtx,
    source: &HarvestSource,
) -> Result<()> {
    refuse_deleted(conn, ctx).await?;
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
    .bind(Json(source))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Writes a plugin row inside the caller's tenant transaction.
async fn write_plugin(
    conn: &mut PgConnection,
    ctx: &RequestCtx,
    plugin: &CataloguePlugin,
) -> Result<()> {
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
    .bind(Json(plugin))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// The stored dataset `id_or_name` names, locked for the rest of the caller's transaction.
async fn dataset_for_update(
    conn: &mut PgConnection,
    ctx: &RequestCtx,
    id_or_name: &str,
) -> Result<Option<Dataset>> {
    let row: Option<(Json<Dataset>,)> = sqlx::query_as(
        "SELECT doc FROM dataroom.cat_dataset
         WHERE org_id = $1 AND catalogue_id = $2
           AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3)
         ORDER BY (id COLLATE \"C\" = $3) DESC
         LIMIT 1
         FOR UPDATE",
    )
    .bind(ctx.org_id.as_uuid())
    .bind(ctx.catalogue_id.as_uuid())
    .bind(id_or_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(row.map(|(Json(d),)| d))
}

/// Writes a dataset row and replaces its distribution rows inside the caller's tenant
/// transaction.
/// The version of the search columns this build writes (`cat_dataset.search_v`); a row below it
/// was written before them and is matched in Rust until [`PgStore::migrate`] reindexes it.
const SEARCH_VERSION: i16 = 1;

/// The `cat_dataset` search columns of a dataset, as text arrays for `array_to_tsvector`.
struct SearchColumns {
    name: Vec<String>,
    title: Vec<String>,
    keywords: Vec<String>,
    description: Vec<String>,
}

impl SearchColumns {
    fn of(dataset: &Dataset) -> Self {
        let lex = SearchLexemes::of(dataset);
        Self {
            name: lex.name_suffixes(),
            title: lex.title,
            keywords: lex.keywords,
            description: lex.description,
        }
    }
}

/// A `tsquery` matching a word that starts with any of `candidates` (`'a':* | 'b':*`); `None`
/// for no candidate, which matches nothing.
fn prefix_tsquery(candidates: &[String]) -> Option<String> {
    let alternatives: Vec<String> = candidates
        .iter()
        .map(|c| format!("'{}':*", c.replace('\\', "\\\\").replace('\'', "''")))
        .collect();
    (!alternatives.is_empty()).then(|| format!("({})", alternatives.join(" | ")))
}

/// A search candidate's record as the search reads it: no description, version notes,
/// provenance or SHACL shapes, and distributions holding only id and format (S11-38).
const SLIM_DOC: &str = "jsonb_set(
        jsonb_set(doc - 'versionNotes' - 'provenance' - 'customShaclShapes',
                  '{description}', '{}'::jsonb),
        '{distributions}',
        COALESCE((SELECT jsonb_agg(jsonb_build_object(
                             'id', d->'id', 'datasetId', d->'datasetId', 'title', '{}'::jsonb,
                             'accessUrl', '', 'format', d->'format') ORDER BY n)
                  FROM jsonb_array_elements(doc->'distributions') WITH ORDINALITY AS e(d, n)),
                 '[]'::jsonb))";

async fn read_publishers(conn: &mut PgConnection, ctx: &RequestCtx) -> Result<Vec<Publisher>> {
    let rows: Vec<(Json<Publisher>,)> = sqlx::query_as(
        "SELECT doc FROM dataroom.cat_publisher
         WHERE org_id = $1 AND catalogue_id = $2
         ORDER BY name COLLATE \"C\"",
    )
    .bind(ctx.org_id.as_uuid())
    .bind(ctx.catalogue_id.as_uuid())
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|(Json(p),)| p).collect())
}

/// Reindexes every row whose search columns are older than [`SEARCH_VERSION`], 200 at a time,
/// with row level security lifted for the table as in [`PgStore::move_catalogue`] (the rows of
/// every tenant); `conn` holds the migration lock.
async fn reindex_search(conn: &mut PgConnection) -> Result<u64> {
    let fail = |e: sqlx::Error| DataroomError::Storage(format!("search reindex: {e}"));
    let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(fail)?;
    sqlx::raw_sql("ALTER TABLE dataroom.cat_dataset NO FORCE ROW LEVEL SECURITY")
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    let mut done = 0;
    loop {
        let rows: Vec<(uuid::Uuid, uuid::Uuid, String, Json<Dataset>)> = sqlx::query_as(
            "SELECT org_id, catalogue_id, id, doc FROM dataroom.cat_dataset
             WHERE search_v < $1 LIMIT 200",
        )
        .bind(SEARCH_VERSION)
        .fetch_all(&mut *tx)
        .await
        .map_err(fail)?;
        if rows.is_empty() {
            break;
        }
        for (org, cat, id, Json(ds)) in rows {
            let lex = SearchColumns::of(&ds);
            sqlx::query(
                "UPDATE dataroom.cat_dataset SET search_v = $4,
                     search_name = array_to_tsvector($5), search_title = array_to_tsvector($6),
                     search_kw = array_to_tsvector($7), search_desc = array_to_tsvector($8)
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(org)
            .bind(cat)
            .bind(&id)
            .bind(SEARCH_VERSION)
            .bind(&lex.name)
            .bind(&lex.title)
            .bind(&lex.keywords)
            .bind(&lex.description)
            .execute(&mut *tx)
            .await
            .map_err(fail)?;
            done += 1;
        }
    }
    sqlx::raw_sql("ALTER TABLE dataroom.cat_dataset FORCE ROW LEVEL SECURITY")
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    tx.commit().await.map_err(fail)?;
    Ok(done)
}

async fn write_dataset(conn: &mut PgConnection, ctx: &RequestCtx, dataset: &Dataset) -> Result<()> {
    refuse_deleted(conn, ctx).await?;
    let title = text_in_lang(&dataset.title, None)
        .unwrap_or(&dataset.name)
        .to_owned();
    let notes = text_in_lang(&dataset.description, None).map(str::to_owned);
    let lex = SearchColumns::of(dataset);
    sqlx::query(
        "INSERT INTO dataroom.cat_dataset
             (org_id, catalogue_id, id, name, title, notes, publisher_id, visibility, state,
              licence_id, frequency, endpoint_url, issued, modified,
              search_v, search_name, search_title, search_kw, search_desc, doc)
             VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, array_to_tsvector($16), array_to_tsvector($17),
                array_to_tsvector($18), array_to_tsvector($19), $20
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
                search_v = EXCLUDED.search_v,
                search_name = EXCLUDED.search_name,
                search_title = EXCLUDED.search_title,
                search_kw = EXCLUDED.search_kw,
                search_desc = EXCLUDED.search_desc,
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
    .bind(SEARCH_VERSION)
    .bind(&lex.name)
    .bind(&lex.title)
    .bind(&lex.keywords)
    .bind(&lex.description)
    .bind(Json(&dataset))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;

    sqlx::query(
        "DELETE FROM dataroom.cat_distribution
             WHERE org_id = $1 AND catalogue_id = $2 AND dataset_id COLLATE \"C\" = $3",
    )
    .bind(ctx.org_id.as_uuid())
    .bind(ctx.catalogue_id.as_uuid())
    .bind(&dataset.id)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;

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
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }

    Ok(())
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
    let (is_super, bypass_rls): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
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
            let clean = host
                .split(':')
                .next()
                .unwrap_or(host)
                .trim()
                .to_ascii_lowercase();
            if clean.is_empty() {
                return Ok(None);
            }
            // No tenant is known yet: the host lookup policy lets this transaction read the
            // one catalogue row on the host it names, nothing else (forced RLS, C1).
            let mut tx = self.pool.begin().await.map_err(db_err)?;
            sqlx::query("SELECT set_config('jc.host', $1, true)")
                .bind(&clean)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            let row: Option<(Json<Catalogue>,)> =
                sqlx::query_as("SELECT doc FROM dataroom.cat_catalogue WHERE lower(host) = $1")
                    .bind(&clean)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(row.map(|(Json(c),)| c))
        })
    }

    fn delete_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let mut tx = self.begin_tenant(org_id, Some(catalogue_id)).await?;
            let marked = sqlx::query(
                "UPDATE dataroom.cat_catalogue
                 SET doc = jsonb_set(doc, '{deleted}', to_jsonb(now())), modified = now()
                 WHERE org_id = $1 AND id = $2",
            )
            .bind(org_id.as_uuid())
            .bind(catalogue_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
            if marked == 0 {
                tx.rollback().await.map_err(db_err)?;
                return Ok(false);
            }
            // Children first, in reverse migration order; the catalogue row stays (tombstone).
            for table in REQUIRED_RLS_TABLES
                .iter()
                .rev()
                .filter(|t| **t != "cat_catalogue")
            {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM dataroom.{table} WHERE org_id = $1 AND catalogue_id = $2"
                )))
                .bind(org_id.as_uuid())
                .bind(catalogue_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            tx.commit().await.map_err(db_err)?;
            Ok(true)
        })
    }

    fn upsert_catalogue<'a>(&'a self, catalogue: Catalogue) -> BoxFuture<'a, Result<Catalogue>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(catalogue.org_id, Some(catalogue.id))
                .await?;
            write_catalogue(&mut tx, &catalogue).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(catalogue)
        })
    }

    fn save_catalogue<'a>(
        &'a self,
        mut catalogue: Catalogue,
        expected: Option<TimestampTz>,
        mut activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Catalogue>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(catalogue.org_id, Some(catalogue.id))
                .await?;
            if let Some(expected) = expected {
                let stored: Option<(Json<Catalogue>,)> = sqlx::query_as(
                    "SELECT doc FROM dataroom.cat_catalogue WHERE org_id = $1 AND id = $2
                     FOR UPDATE",
                )
                .bind(catalogue.org_id.as_uuid())
                .bind(catalogue.id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
                if stored.is_none_or(|(Json(c),)| c.modified != expected) {
                    return Err(changed_since_read("catalogue", &catalogue.name));
                }
            }
            catalogue.modified = next_modified(catalogue.modified, expected);
            write_catalogue(&mut tx, &catalogue).await?;
            if activity.object_id.is_empty() {
                activity.object_id = catalogue.id.to_string();
            }
            insert_activity(&mut tx, catalogue.org_id, catalogue.id, activity).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(catalogue)
        })
    }

    fn declare_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        id: CatalogueId,
        declared: &'a CatalogueDeclaration,
    ) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        Box::pin(async move {
            // The members alone are merged into the row as it is when the write lands: another
            // replica's or an admin's write of the other members survives it (T-7520 C12). Not
            // `@>`: a name in two languages contains the one that dropped a language.
            let modified = TimestampTz::now();
            let mut tx = self.begin_tenant(org_id, Some(id)).await?;
            sqlx::query(
                "UPDATE dataroom.cat_catalogue
                 SET doc = doc || $3 || jsonb_build_object('modified', $5::jsonb), modified = $4
                 WHERE org_id = $1 AND id = $2 AND jsonb_strip_nulls(doc || $3) <> jsonb_strip_nulls(doc)",
            )
            .bind(org_id.as_uuid())
            .bind(id.as_uuid())
            .bind(Json(declared))
            .bind(modified)
            .bind(Json(modified))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            self.get_catalogue(org_id, id).await
        })
    }

    fn list_publishers<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Publisher>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let publishers = read_publishers(&mut tx, ctx).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(publishers)
        })
    }

    fn get_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Publisher>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let mut resolved = find_publisher(&mut tx, ctx, id_or_name, false).await?;
            // Follow merge redirects (up to 4 hops, C16).
            for _ in 0..4 {
                let Some(next) = resolved.as_ref().and_then(|p| p.merged_into.clone()) else {
                    break;
                };
                match find_publisher(&mut tx, ctx, &next, false).await? {
                    Some(found) => resolved = Some(found),
                    None => break,
                }
            }
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_publisher(&mut tx, ctx, &publisher).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(publisher)
        })
    }

    fn save_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut publisher: Publisher,
        expected: Option<TimestampTz>,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            publisher.org_id = ctx.org_id;
            publisher.catalogue_id = ctx.catalogue_id;
            if publisher.id.is_empty() {
                publisher.id = format!("pub-{}", publisher.name);
            }
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            if let Some(expected) = expected {
                // The doc's version: the column keeps microseconds, a read returns the doc.
                let stored: Option<(Json<Publisher>,)> = sqlx::query_as(
                    "SELECT doc FROM dataroom.cat_publisher
                     WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3
                     FOR UPDATE",
                )
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(&publisher.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
                if stored.is_none_or(|(Json(p),)| p.modified != expected) {
                    return Err(changed_since_read("publisher", &publisher.name));
                }
            }
            publisher.modified = next_modified(publisher.modified, expected);
            write_publisher(&mut tx, ctx, &publisher).await?;
            commit_with_activity(tx, ctx, activity, &publisher.id).await?;
            Ok(publisher)
        })
    }

    fn merge_publishers<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id_or_name: &'a str,
        target_id_or_name: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            // The source by its own record (an already merged source does not redirect to the
            // publisher it was merged into); the target as published, following redirects.
            let Some(mut source) = find_publisher(&mut tx, ctx, source_id_or_name, true).await?
            else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{source_id_or_name}' not found"
                )));
            };
            let mut target = find_publisher(&mut tx, ctx, target_id_or_name, true).await?;
            for _ in 0..4 {
                match target.as_ref().and_then(|t| t.merged_into.clone()) {
                    Some(next) => match find_publisher(&mut tx, ctx, &next, true).await? {
                        Some(found) => target = Some(found),
                        None => break,
                    },
                    None => break,
                }
            }
            let Some(target) = target else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{target_id_or_name}' not found"
                )));
            };
            if source.id == target.id {
                return Err(DataroomError::field_error(
                    "target",
                    "cannot merge a publisher into itself",
                ));
            }
            let rows: Vec<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND (publisher_id COLLATE \"C\" = $3 OR publisher_id COLLATE \"C\" = $4)
                 FOR UPDATE",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&source.id)
            .bind(&source.name)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            let now = TimestampTz::now();
            for (Json(mut ds),) in rows {
                ds.publisher_id = target.name.clone();
                ds.modified = now;
                write_dataset(&mut tx, ctx, &ds).await?;
            }
            source.merged_into = Some(target.name.clone());
            source.state = RecordState::Withdrawn;
            source.modified = now;
            write_publisher(&mut tx, ctx, &source).await?;
            commit_with_activity(tx, ctx, activity, &target.id).await?;
            Ok(target)
        })
    }

    fn withdraw_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>> {
        Box::pin(async move {
            // The record named, never the one a merge points it to (as MemoryStore), read and
            // written under its row lock (S11-25).
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let Some(mut pub_rec) = find_publisher(&mut tx, ctx, id_or_name, true).await? else {
                return Err(DataroomError::NotFound(format!(
                    "publisher '{id_or_name}' not found"
                )));
            };
            pub_rec.state = RecordState::Withdrawn;
            pub_rec.modified = next_modified(TimestampTz::now(), Some(pub_rec.modified));
            write_publisher(&mut tx, ctx, &pub_rec).await?;
            commit_with_activity(tx, ctx, activity, &pub_rec.id).await?;
            Ok(pub_rec)
        })
    }

    fn list_datasets<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Dataset>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(d),)| d).collect())
        })
    }

    fn search_page<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        query: &'a SearchQuery,
        pick: &'a mut PickPage<'_>,
    ) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            // Per term, the query that matches it; a row is kept when every term matches. A
            // term with no candidate matches nothing.
            let per_term: Option<Vec<String>> =
                query.terms.iter().map(|c| prefix_tsquery(c)).collect();
            let per_term = per_term.unwrap_or_default();
            let all = per_term.join(" & ");
            // The indexed rows the text matches (GIN), and the rows not indexed yet, which the
            // caller matches in Rust (their search columns are empty).
            let text_filter = match (query.terms.is_empty(), per_term.is_empty()) {
                (true, _) => "",
                (false, true) => "AND search_v < {SEARCH_VERSION}",
                (false, false) => "AND (search_all @@ $6::tsquery OR search_v < {SEARCH_VERSION})",
            }
            .replace("{SEARCH_VERSION}", &SEARCH_VERSION.to_string());
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let publishers = read_publishers(&mut tx, ctx).await?;
            // ponytail: O(matches) slim rows reach Rust, which authorizes (Authorizer) and counts
            // facets; facet counts in SQL would need the authorization rules in SQL.
            let rows: Vec<(Json<Dataset>, Option<i64>)> =
                sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "WITH q AS (SELECT array_agg(t::tsquery) AS terms FROM unnest($5::text[]) t)
                     SELECT CASE WHEN search_v >= {SEARCH_VERSION} THEN {SLIM_DOC} ELSE doc END,
                            CASE WHEN search_v >= {SEARCH_VERSION} THEN
                                (SELECT COALESCE(sum(
                                     CASE WHEN search_name @@ t THEN 8 ELSE 0 END
                                     + CASE WHEN search_title @@ t THEN 4 ELSE 0 END
                                     + CASE WHEN search_kw @@ t THEN 2 ELSE 0 END
                                     + CASE WHEN search_desc @@ t THEN 1 ELSE 0 END), 0)
                                 FROM unnest(q.terms) t)
                            END
                     FROM dataroom.cat_dataset, q
                     WHERE org_id = $1 AND catalogue_id = $2
                       AND (state = 'active' OR (state = 'draft' AND $3)
                            OR (state = 'withdrawn' AND $4))
                       AND (NOT $7 OR visibility = 'public')
                       AND $6::text IS NOT NULL -- types $6 where no filter reads it
                       {text_filter}
                     ORDER BY name COLLATE \"C\""
                )))
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(query.include_drafts)
                .bind(query.include_withdrawn)
                .bind(&per_term)
                .bind(all)
                .bind(query.public_only)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_err)?;
            let candidates = rows
                .into_iter()
                .map(|(Json(dataset), rank)| SearchCandidate {
                    dataset,
                    rank: rank.map(|r| usize::try_from(r).unwrap_or(usize::MAX)),
                })
                .collect();
            let ids = pick(&publishers, candidates)?;
            let found: Vec<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = ANY($3)",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            let mut by_id: std::collections::HashMap<String, Dataset> = found
                .into_iter()
                .map(|(Json(d),)| (d.id.clone(), d))
                .collect();
            let datasets = ids.iter().filter_map(|id| by_id.remove(id)).collect();
            Ok(SearchPage {
                publishers,
                datasets,
            })
        })
    }

    fn list_datasets_of_type<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset_type: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Dataset>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2 AND doc->>'datasetType' = $3
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(dataset_type)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(d),)| d).collect())
        })
    }

    fn get_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Dataset>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let row: Option<(Json<Dataset>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3)
                 ORDER BY (id COLLATE \"C\" = $3) DESC
                 LIMIT 1",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(id_or_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(row.map(|(Json(d),)| d))
        })
    }

    fn upsert_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset: Dataset,
    ) -> BoxFuture<'a, Result<Dataset>> {
        Box::pin(async move {
            let dataset = normalise_dataset(ctx, dataset);
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_dataset(&mut tx, ctx, &dataset).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(dataset)
        })
    }

    fn save_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset: Dataset,
        expected: Option<TimestampTz>,
        mut activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Dataset>> {
        Box::pin(async move {
            let mut dataset = normalise_dataset(ctx, dataset);
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            if let Some(expected) = expected {
                let stored = dataset_for_update(&mut tx, ctx, &dataset.id).await?;
                if stored.is_none_or(|d| d.modified != expected) {
                    return Err(changed_since_read("dataset", &dataset.name));
                }
            }
            dataset.modified = next_modified(dataset.modified, expected);
            write_dataset(&mut tx, ctx, &dataset).await?;
            if activity.object_id.is_empty() {
                activity.object_id.clone_from(&dataset.id);
            }
            insert_activity(&mut tx, ctx.org_id, ctx.catalogue_id, activity).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(dataset)
        })
    }

    fn withdraw_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Dataset>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let Some(mut ds) = dataset_for_update(&mut tx, ctx, id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "dataset '{id_or_name}' not found"
                )));
            };
            ds.state = RecordState::Withdrawn;
            ds.modified = next_modified(TimestampTz::now(), Some(ds.modified));
            write_dataset(&mut tx, ctx, &ds).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(ds)
        })
    }

    fn purge_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let Some(ds) = dataset_for_update(&mut tx, ctx, id_or_name).await? else {
                return Err(DataroomError::NotFound(format!(
                    "dataset '{id_or_name}' not found"
                )));
            };
            // Tables in the migration's order, so a concurrent migration cannot deadlock.
            sqlx::query(
                "DELETE FROM dataroom.cat_dataset
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ds.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            sqlx::query(
                "DELETE FROM dataroom.cat_distribution
                 WHERE org_id = $1 AND catalogue_id = $2 AND dataset_id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ds.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            let resource_ids: Vec<String> = ds.distributions.iter().map(|d| d.id.clone()).collect();
            sqlx::query(
                "DELETE FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND distribution_id COLLATE \"C\" = ANY($3)",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&resource_ids)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            // A purge leaves no copy of the record's values in its SHACL findings either.
            sqlx::query(
                "DELETE FROM dataroom.cat_shacl_result
                 WHERE org_id = $1 AND catalogue_id = $2 AND dataset_id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&ds.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            commit_with_activity(tx, ctx, activity, &ds.id).await?;
            Ok(())
        })
    }

    fn get_distribution<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<(Dataset, Distribution)>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let row: Option<(String, Json<Distribution>)> = sqlx::query_as(
                "SELECT dataset_id, doc FROM dataroom.cat_distribution
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(distribution_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(v),)| v).collect())
        })
    }

    fn list_views_of<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_ids: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<ResourceView>>> {
        Box::pin(async move {
            if distribution_ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<ResourceView>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND distribution_id COLLATE \"C\" = ANY($3)
                 ORDER BY array_position($3, distribution_id::text), view_order ASC,
                          id COLLATE \"C\" ASC",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(distribution_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(v),)| v).collect())
        })
    }

    fn get_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ResourceView>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let row: Option<(Json<ResourceView>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(view_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_view(&mut tx, ctx, &view).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(view)
        })
    }

    fn save_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut view: ResourceView,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<ResourceView>> {
        Box::pin(async move {
            if view.id.is_empty() {
                view.id = format!("view-{}", uuid::Uuid::now_v7());
            }
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_view(&mut tx, ctx, &view).await?;
            commit_with_activity(tx, ctx, activity, &view.id).await?;
            Ok(view)
        })
    }

    fn delete_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let res = sqlx::query(
                "DELETE FROM dataroom.cat_view
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(view_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            if res.rows_affected() == 0 {
                return Err(DataroomError::NotFound(format!(
                    "resource view '{view_id}' not found"
                )));
            }
            commit_with_activity(tx, ctx, activity, view_id).await?;
            Ok(())
        })
    }

    fn list_harvest_sources<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<HarvestSource>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<HarvestSource>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_source
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(s),)| s).collect())
        })
    }

    fn get_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestSource>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let row: Option<(Json<HarvestSource>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_source
                 WHERE org_id = $1 AND catalogue_id = $2
                   AND (id COLLATE \"C\" = $3 OR name COLLATE \"C\" = $3)
                 ORDER BY (id COLLATE \"C\" = $3) DESC
                 LIMIT 1",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(id_or_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(row.map(|(Json(s),)| s))
        })
    }

    fn delete_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let gone: Option<(String,)> = sqlx::query_as(
                "DELETE FROM dataroom.cat_harvest_source
                 WHERE org_id = $1 AND catalogue_id = $2 AND (id = $3 OR name = $3)
                 RETURNING id",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(id_or_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            if let Some((id,)) = &gone {
                sqlx::query(
                    "DELETE FROM dataroom.cat_harvest_run
                     WHERE org_id = $1 AND catalogue_id = $2 AND source_id = $3",
                )
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            tx.commit().await.map_err(db_err)?;
            Ok(gone.is_some())
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_source(&mut tx, ctx, &source).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(source)
        })
    }

    fn save_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut source: HarvestSource,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<HarvestSource>> {
        Box::pin(async move {
            source.org_id = ctx.org_id;
            source.catalogue_id = ctx.catalogue_id;
            if source.id.is_empty() {
                source.id = format!("hs-{}", source.name);
            }
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_source(&mut tx, ctx, &source).await?;
            commit_with_activity(tx, ctx, activity, &source.id).await?;
            Ok(source)
        })
    }

    fn list_harvest_runs<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<HarvestRun>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(r),)| r).collect())
        })
    }

    fn get_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestRun>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let row: Option<(Json<HarvestRun>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_run
                 WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            refuse_deleted(&mut tx, ctx).await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(run)
        })
    }

    fn claim_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut run: HarvestRun,
        stale_before: TimestampTz,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            run.org_id = ctx.org_id;
            run.catalogue_id = ctx.catalogue_id;
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            refuse_deleted(&mut tx, ctx).await?;
            let stale: Vec<(Json<HarvestRun>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_harvest_run
                 WHERE org_id = $1 AND catalogue_id = $2 AND source_id COLLATE \"C\" = $3
                   AND state = 'running' AND started_at < $4
                 FOR UPDATE",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&run.source_id)
            .bind(stale_before)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            for (Json(mut old),) in stale {
                dataroom_core::abandon_run(&mut old);
                sqlx::query(
                    "UPDATE dataroom.cat_harvest_run SET state = $4, finished_at = $5, doc = $6
                     WHERE org_id = $1 AND catalogue_id = $2 AND id COLLATE \"C\" = $3",
                )
                .bind(ctx.org_id.as_uuid())
                .bind(ctx.catalogue_id.as_uuid())
                .bind(&old.id)
                .bind(old.state.as_str())
                .bind(old.finished_at)
                .bind(Json(&old))
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            // The partial unique index `cat_harvest_run_one_running` is the lock: a second
            // Running row of the source waits for the first transaction, then inserts nothing.
            let inserted = sqlx::query(
                "INSERT INTO dataroom.cat_harvest_run
                 (org_id, catalogue_id, id, source_id, state, started_at, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (org_id, catalogue_id, source_id) WHERE state = 'running'
                 DO NOTHING",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&run.id)
            .bind(&run.source_id)
            .bind(run.state.as_str())
            .bind(run.started_at)
            .bind(Json(&run))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
            tx.commit().await.map_err(db_err)?;
            Ok(inserted == 1)
        })
    }

    fn record_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        row: ActivityRow,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            insert_activity(&mut tx, ctx.org_id, ctx.catalogue_id, row).await?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
                    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
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
                    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
                    .fetch_all(&mut *tx)
                    .await
                }
            }
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(r),)| r).collect())
        })
    }

    fn list_profiles<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<ProfileRecord>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<ProfileRecord>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_profile
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY id COLLATE \"C\", version COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(rows.into_iter().map(|(Json(p),)| p).collect())
        })
    }

    fn upsert_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
    ) -> BoxFuture<'a, Result<ProfileRecord>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(profile)
        })
    }

    fn save_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<ProfileRecord>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let inserted = sqlx::query(
                "INSERT INTO dataroom.cat_profile (org_id, catalogue_id, id, version, doc)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (org_id, catalogue_id, id, version) DO NOTHING",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .bind(&profile.id)
            .bind(&profile.version)
            .bind(Json(&profile))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            if inserted.rows_affected() == 0 {
                return Err(DataroomError::Conflict(format!(
                    "profile '{}' version '{}' already exists; upload a new version",
                    profile.id, profile.version
                )));
            }
            let object_id = format!("{}:{}", profile.id, profile.version);
            commit_with_activity(tx, ctx, activity, &object_id).await?;
            Ok(profile)
        })
    }

    fn list_pages<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<StaticPage>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<StaticPage>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_page
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY slug COLLATE \"C\", lang COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
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
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(page)
        })
    }

    fn list_plugins<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<CataloguePlugin>>> {
        Box::pin(async move {
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            let rows: Vec<(Json<CataloguePlugin>,)> = sqlx::query_as(
                "SELECT doc FROM dataroom.cat_plugin
                 WHERE org_id = $1 AND catalogue_id = $2
                 ORDER BY name COLLATE \"C\"",
            )
            .bind(ctx.org_id.as_uuid())
            .bind(ctx.catalogue_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
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
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_plugin(&mut tx, ctx, &plugin).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(plugin)
        })
    }

    fn save_plugin<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut plugin: CataloguePlugin,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<CataloguePlugin>> {
        Box::pin(async move {
            plugin.org_id = ctx.org_id;
            plugin.catalogue_id = ctx.catalogue_id;
            let mut tx = self
                .begin_tenant(ctx.org_id, Some(ctx.catalogue_id))
                .await?;
            write_plugin(&mut tx, ctx, &plugin).await?;
            commit_with_activity(tx, ctx, activity, &plugin.name).await?;
            Ok(plugin)
        })
    }

    fn odata_table<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        set_name: &'a str,
    ) -> BoxFuture<'a, Result<Arc<dyn Table>>> {
        Box::pin(async move {
            let service = dataroom_service();
            let model = Arc::new(service.model());
            let Some(info) = service.info(model, set_name) else {
                return Err(DataroomError::NotFound(format!(
                    "entity set '{set_name}' not found"
                )));
            };
            // Every row comes through a tenant transaction, so forced RLS binds the request's
            // organization and catalogue (C1, C14). `PgTable` runs its statement on the bare
            // pool, where no tenant is set and RLS hides every row; the rows are therefore read
            // here and answered by the reference evaluator, as in the memory store.
            if !dataroom_core::odata_defs::set_readable(ctx, set_name) {
                let table = MemoryTable::new(info, Vec::new()).map_err(DataroomError::from)?;
                return Ok(Arc::new(table) as Arc<dyn Table>);
            }
            let lang = self
                .get_catalogue(ctx.org_id, ctx.catalogue_id)
                .await?
                .map(|c| c.default_language);
            let lang = lang.as_deref();
            let rows: Vec<serde_json::Value> = match set_name {
                "Catalogues" => self
                    .list_catalogues(ctx.org_id)
                    .await?
                    .iter()
                    .filter(|c| c.deleted.is_none())
                    .map(catalogue_to_row)
                    .collect(),
                "Datasets" => self
                    .list_datasets(ctx)
                    .await?
                    .iter()
                    .map(|d| dataset_to_row(d, lang))
                    .collect(),
                "Distributions" => self
                    .list_datasets(ctx)
                    .await?
                    .iter()
                    .flat_map(|ds| {
                        ds.distributions.iter().map(move |dist| {
                            distribution_to_row(ds.org_id, ds.catalogue_id, dist, lang)
                        })
                    })
                    .collect(),
                "Publishers" => self
                    .list_publishers(ctx)
                    .await?
                    .iter()
                    .map(|p| publisher_to_row(p, lang))
                    .collect(),
                "HarvestSources" => self
                    .list_harvest_sources(ctx)
                    .await?
                    .iter()
                    .map(harvest_source_to_row)
                    .collect(),
                "HarvestRuns" => self
                    .list_harvest_runs(ctx, None)
                    .await?
                    .iter()
                    .map(harvest_run_to_row)
                    .collect(),
                "ActivityRows" => self
                    .list_activity(ctx, None, usize::MAX)
                    .await?
                    .iter()
                    .map(activity_to_row)
                    .collect(),
                "ShaclResults" => self
                    .list_shacl_reports(ctx, None, None)
                    .await?
                    .iter()
                    .map(shacl_report_to_row)
                    .collect(),
                _ => {
                    return Err(DataroomError::NotFound(format!(
                        "entity set '{set_name}' has no Postgres source"
                    )))
                }
            };
            let table = MemoryTable::new(info, rows).map_err(DataroomError::from)?;
            Ok(Arc::new(table) as Arc<dyn Table>)
        })
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
    #[allow(clippy::disallowed_methods)] // The test database URL; jc never runs this.
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
        assert!(PgStore::connect_and_verify(admin_pool.clone())
            .await
            .is_err());
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
        let refused = refused.err();
        assert!(
            matches!(&refused, Some(DataroomError::Storage(msg)) if msg.contains("zz_a6_no_forced_rls")),
            "the boot check refuses and names the table: {refused:?}"
        );

        let store = PgStore::from_pool_unchecked(admin_pool.clone());
        let org_a = OrgId::from_uuid(Uuid::now_v7());
        let cat_a = CatalogueId::from_uuid(Uuid::now_v7());
        let org_b = OrgId::from_uuid(Uuid::now_v7());
        let cat_b = CatalogueId::from_uuid(Uuid::now_v7());

        for (org, cat, host) in [
            (
                org_a,
                cat_a,
                format!("a-{}.example.org", Uuid::now_v7().simple()),
            ),
            (
                org_b,
                cat_b,
                format!("b-{}.example.org", Uuid::now_v7().simple()),
            ),
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
                    modified: TimestampTz::now(),
                    deleted: None,
                    public_base: None,
                    portal_base: None,
                    left_hosts: Vec::new(),
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
                    created: TimestampTz::now(),
                    modified: TimestampTz::now(),
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

        let count_b_from_a: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dataroom.cat_publisher")
            .fetch_one(&mut *tx)
            .await
            .expect("count publishers as A");
        assert_eq!(count_b_from_a.0, 0, "RLS must hide org B rows from org A");
        tx.rollback().await.ok();

        assert!(store
            .get_publisher(&ctx_a, "city-b")
            .await
            .unwrap()
            .is_none());
        let _ = Visibility::Internal;
    }
}
