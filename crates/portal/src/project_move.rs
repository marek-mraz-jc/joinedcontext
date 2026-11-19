//! Moving a project to another Organization (T-7305): what the operators' `ProjectMove` Change
//! plans and applies. The project keeps its id; its registry row and every row keyed by
//! (old Organization, project id) take the target Organization's key in one transaction, under the
//! `project_move` policy of migration 0089. The Organization a request places the project in comes
//! from its Project manifest (`spec.organizationRef`, in Git), so the move applies only once the
//! manifest names the target: before that the registry would key the project under an
//! Organization the mirror does not place it in, and every keyed read would answer 404.
//! The rights follow the manifest: an Organization-scoped binding of the old Organization no
//! longer reaches the project, and the project's own bindings move with it.

use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::permissions::{organization_of, ORG_NAMESPACE};
use crate::store::{ListOptions, Mirror};

/// The rows of one keyed table that move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TableRows {
    pub table: String,
    pub rows: u64,
}

/// What a `ProjectMove` of `project` to `to` would do now, as the operators read it before they
/// approve.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    pub project: String,
    /// The Organization the registry files the project under now; `None` before it has one.
    pub from: Option<String>,
    pub to: String,
    /// The Organization the project's manifest names (or the installation's when it names none).
    pub manifest_organization: Option<String>,
    /// Rows keyed by the project under `from`, per table; only tables holding any are listed.
    pub tables: Vec<TableRows>,
    pub rows: u64,
    /// The project's own `RoleBinding`s, which move with it.
    pub bindings: Vec<String>,
    /// The project sits under `to` already: approving changes nothing.
    pub done: bool,
    /// Why the move cannot be applied now, each naming its next step; empty when it can.
    pub blockers: Vec<String>,
}

/// The registry's record of a live project: its id and the Organization's name and key.
struct Recorded {
    id: String,
    organization: Option<String>,
    key: Option<String>,
}

async fn recorded(pool: &PgPool, project: &str) -> Result<Option<Recorded>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.id::text, p.organization, o.key::text FROM project_ids p \
         LEFT JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, organization, key)| Recorded {
        id,
        organization,
        key,
    }))
}

/// The key of an active Organization of the registry.
async fn active_key(
    conn: impl sqlx::PgExecutor<'_>,
    organization: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT key::text FROM organizations WHERE name = $1 AND state = 'active'")
        .bind(organization)
        .fetch_optional(conn)
        .await
}

/// Every table of the schema keyed by (org_id, project_id), from the catalog, so a table a later
/// migration keys moves without a list to keep.
async fn keyed_tables(conn: impl sqlx::PgExecutor<'_>) -> Result<Vec<String>, sqlx::Error> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.table_name::text FROM information_schema.columns c \
         JOIN information_schema.tables t ON t.table_schema = c.table_schema \
              AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE' \
         WHERE c.table_schema = current_schema() AND c.column_name IN ('org_id', 'project_id') \
         GROUP BY c.table_name HAVING count(*) = 2 ORDER BY 1",
    )
    .fetch_all(conn)
    .await?;
    // Catalog names, quoted below all the same; anything but a plain name is left alone.
    Ok(tables
        .into_iter()
        .filter(|t| t.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .collect())
}

/// The rows each keyed table holds under (org, project), inside a transaction keyed with that pair.
async fn counts(
    tx: &mut sqlx::PgConnection,
    org: &str,
    project: &str,
) -> Result<Vec<TableRows>, sqlx::Error> {
    let mut found = Vec::new();
    for table in keyed_tables(&mut *tx).await? {
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM \"{table}\" WHERE org_id = $1::uuid AND project_id = $2::uuid"
        )))
        .bind(org)
        .bind(project)
        .fetch_one(&mut *tx)
        .await?;
        if rows > 0 {
            found.push(TableRows {
                table,
                rows: rows.unsigned_abs(),
            });
        }
    }
    Ok(found)
}

async fn keyed(
    pool: &PgPool,
    org: &str,
    project: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT set_config('statement_timeout', $3, true), \
         set_config('jc.org', $1, true), set_config('jc.project', $2, true)",
    )
    .bind(org)
    .bind(project)
    .bind(crate::db::BULK_STATEMENT_TIMEOUT)
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

/// The project's own `RoleBinding`s in the mirror, by name.
fn bindings(mirror: &Mirror, project: &str) -> Vec<String> {
    mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
        .into_iter()
        .filter(|b| b.spec["scope"]["project"].as_str() == Some(project))
        .map(|b| b.metadata.name)
        .collect()
}

/// What moving `project` to `to` does now. `None` when the registry holds no live project of
/// that name.
pub async fn plan(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
    to: &str,
) -> Result<Option<MovePlan>, sqlx::Error> {
    let Some(found) = recorded(pool, project).await? else {
        return Ok(None);
    };
    let manifest_organization = organization_of(mirror, project);
    let mut blockers = Vec::new();
    if active_key(pool, to).await?.is_none() {
        blockers.push(format!(
            "organization: '{to}' is no active Organization of the registry; an operator adds it on the Organizations page first"
        ));
    }
    if found.organization.is_none() {
        blockers.push(format!(
            "project: '{project}' has no Organization in the registry yet; it takes the one its manifest names at its first keyed write, no move needed"
        ));
    }
    if mirror.get(ORG_NAMESPACE, "Project", project).is_none() {
        blockers.push(format!(
            "project: the Portal's configuration holds no Project '{project}'"
        ));
    } else if manifest_organization.as_deref() != Some(to) {
        blockers.push(format!(
            "organization: Project '{project}' names Organization '{}' in its manifest; set its spec.organizationRef to '{to}' (a Project Change) and approve the move once it is synced",
            manifest_organization.as_deref().unwrap_or("none")
        ));
    }
    let branches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.parent = $1::uuid AND b.expires_at > now()",
    )
    .bind(&found.id)
    .fetch_one(pool)
    .await?;
    if branches > 0 {
        blockers.push(format!(
            "project: '{project}' has {branches} live branch(es); merge or close them, or wait until they expire"
        ));
    }
    let done = found.organization.as_deref() == Some(to);
    let tables = match (&found.key, done) {
        (Some(org), false) => {
            let mut tx = keyed(pool, org, &found.id).await?;
            let tables = counts(&mut tx, org, &found.id).await?;
            tx.rollback().await?;
            tables
        }
        _ => Vec::new(),
    };
    Ok(Some(MovePlan {
        project: project.to_owned(),
        from: found.organization,
        to: to.to_owned(),
        manifest_organization,
        rows: tables.iter().map(|t| t.rows).sum(),
        tables,
        bindings: bindings(mirror, project),
        done,
        blockers,
    }))
}

/// Why an apply did not move the project.
#[derive(Debug)]
pub enum MoveError {
    /// A rule of [`plan`] does not hold now; the message names the next step.
    Refused(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for MoveError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

/// Moves `project` to `to` inside `tx`, which the caller commits with its decision and audit row:
/// the registry row is locked, every keyed row of the project under its old Organization takes
/// the target's key, then the registry row does. A project under `to` already moves nothing
/// (an apply is idempotent). Answers the plan as it was applied, with the rows moved per table.
pub async fn apply(
    tx: &mut sqlx::PgConnection,
    mirror: &Mirror,
    project: &str,
    to: &str,
) -> Result<MovePlan, MoveError> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.id::text, p.organization, o.key::text FROM project_ids p \
         LEFT JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1 FOR UPDATE OF p",
    )
    .bind(project)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, Some(from), Some(from_key))) = row else {
        return Err(MoveError::Refused(format!(
            "project: '{project}' is no project of the registry with an Organization"
        )));
    };
    let to_key = active_key(&mut *tx, to).await?.ok_or_else(|| {
        MoveError::Refused(format!(
            "organization: '{to}' is no active Organization of the registry"
        ))
    })?;
    let manifest_organization = organization_of(mirror, project);
    if manifest_organization.as_deref() != Some(to) {
        return Err(MoveError::Refused(format!(
            "organization: Project '{project}' names Organization '{}' in its manifest; set its spec.organizationRef to '{to}' first",
            manifest_organization.as_deref().unwrap_or("none")
        )));
    }
    let branches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.parent = $1::uuid AND b.expires_at > now()",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await?;
    if branches > 0 {
        return Err(MoveError::Refused(format!(
            "project: '{project}' has {branches} live branch(es); merge or close them first"
        )));
    }
    let mut moved = Vec::new();
    if from != to {
        sqlx::query(
            "SELECT set_config('statement_timeout', $4, true), set_config('jc.org', $1, true), \
             set_config('jc.project', $2, true), set_config('jc.move_to', $3, true)",
        )
        .bind(&from_key)
        .bind(&id)
        .bind(&to_key)
        .bind(crate::db::BULK_STATEMENT_TIMEOUT)
        .execute(&mut *tx)
        .await?;
        for table in keyed_tables(&mut *tx).await? {
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE \"{table}\" SET org_id = $3::uuid \
                 WHERE org_id = $1::uuid AND project_id = $2::uuid"
            )))
            .bind(&from_key)
            .bind(&id)
            .bind(&to_key)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if rows > 0 {
                moved.push(TableRows { table, rows });
            }
        }
        sqlx::query("UPDATE project_ids SET organization = $2 WHERE id = $1::uuid")
            .bind(&id)
            .bind(to)
            .execute(&mut *tx)
            .await?;
        // The rest of the caller's transaction (its decision and audit row) is keyed by nothing.
        sqlx::query(
            "SELECT set_config('jc.org', '', true), set_config('jc.project', '', true), \
             set_config('jc.move_to', '', true)",
        )
        .execute(&mut *tx)
        .await?;
    }
    Ok(MovePlan {
        project: project.to_owned(),
        done: from == to,
        from: Some(from),
        to: to.to_owned(),
        manifest_organization,
        rows: moved.iter().map(|t| t.rows).sum(),
        tables: moved,
        bindings: bindings(mirror, project),
        blockers: Vec::new(),
    })
}
