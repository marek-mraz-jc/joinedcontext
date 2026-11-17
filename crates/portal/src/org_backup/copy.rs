//! Restore of one project's folder of an Organization's data archive into a new project
//! (T-7208; ARCH/TEN-12, TEN-16).
//!
//! A format 2 archive keeps each project's Portal rows in `module/portal/project/<ProjectId>/`.
//! An administrator picks one of those folders and a new name: the restore mints a new
//! [`ProjectId`] whose `project_ids` row names the source as its `parent`, writes the folder's
//! rows under the new project's key with their `project` column set to the new name and new row
//! ids where a row is addressed by one, records the lineage in `project_copies`, and applies
//! every approved erasure of the source project again to the copy. The source project and its
//! rows are never written.
//!
//! What is refused: a folder of a project deleted since the backup (its id is retired, TEN-16),
//! a folder the person may not read (the same 404 as an id the archive does not hold, so an id
//! never tells whether it exists), and a new name another project, copy or Organization holds.
//! The copy inherits no grant and no manifest: a Policy reaches it only where it names it
//! (TEN-12), so the source's Endpoint Policy stays its ceiling.

use std::collections::{BTreeMap, HashMap};

use jc_types::{ProjectId, ProjectKey};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;

use super::data::{FORMAT_VERSION, PORTAL_TABLES};
use super::restore::{
    failed, folder_lines, internal, key_columns, not_an_archive, quoted, Opened, NOT_RESTORED,
};
use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::state::AppState;

/// One project folder of an archive the person may restore.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedProject {
    /// The project's id, the folder's name.
    pub id: String,
    /// The project's name today.
    pub name: String,
}

/// The folders of one archive: those the person may restore, and how many it leaves out.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedProjects {
    pub projects: Vec<ArchivedProject>,
    /// Names of the archive's projects deleted since the backup: never restored (TEN-16).
    pub deleted: Vec<String>,
}

/// What a copy writes, or wrote.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopyPlan {
    pub version_id: String,
    /// When the backup was taken, RFC 3339.
    pub taken_at: String,
    /// The source project's id and name.
    pub source: String,
    pub source_name: String,
    /// The new project's name.
    pub name: String,
    /// The new project's id; empty on a check, which mints none.
    pub project_id: String,
    /// Rows written per table, and rows a unique rule or a missing reference refused.
    pub tables: BTreeMap<String, CopyCounts>,
    /// The tables a copy leaves alone, with why.
    pub not_restored: BTreeMap<String, String>,
    /// Approved erasures of the source applied to the copy (on a check: that would be).
    pub erasures_replayed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopyCounts {
    pub create: u64,
    pub refused: u64,
}

impl CopyPlan {
    pub fn writes(&self) -> u64 {
        self.tables.values().map(|c| c.create).sum()
    }
}

/// A row's id column that names another table's row: rewritten to the copy's id of that row,
/// so the copy's rows point at each other and never at the source's.
const REFERENCES: &[(&str, &str, &str)] = &[
    ("alert_notices", "subscription", "alert_subscriptions"),
    ("notifications", "comment_id", "entity_comments"),
];

/// The tables a copy writes, each referenced table ahead of the tables pointing at it.
fn copied_tables() -> Vec<&'static super::data::Table> {
    let mut tables: Vec<_> = PORTAL_TABLES
        .iter()
        .filter(|t| t.keys == "projects" && !NOT_RESTORED.iter().any(|(n, _)| *n == t.name))
        .collect();
    tables.sort_by_key(|t| !REFERENCES.iter().any(|(_, _, to)| *to == t.name));
    tables
}

/// A new text id for a row of `table`, in the form its own code mints.
fn new_text_id(table: &str) -> String {
    match table {
        "grafana_visualizations" => crate::grafana::new_id(),
        _ => crate::ops::data_views::new_id(),
    }
}

fn not_found() -> ApiError {
    ApiError::NotFound(
        "project: this backup holds no project by that id that you may read; pick one of the list"
            .into(),
    )
}

fn invalid(field: &str, message: String) -> ApiError {
    ApiError::Unprocessable {
        detail: format!("{field}: {message}"),
        errors: vec![format!("{field}: {message}")],
    }
}

/// A source folder's project as the registry holds it now.
enum Source {
    /// A live project of the Organization, by its name today.
    Live(String),
    /// Deleted since the backup: its id is retired.
    Deleted(String),
    /// No project of this Organization.
    Unknown,
}

async fn live_source(
    state: &AppState,
    pool: &sqlx::PgPool,
    organization: &str,
    source: &ProjectId,
) -> Result<Source, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM project_ids WHERE id = $1::uuid AND organization = $2 \
         AND parent IS NULL AND NOT reserved",
    )
    .bind(source.to_string())
    .bind(organization)
    .fetch_optional(pool)
    .await
    .map_err(failed)?;
    let Some((name,)) = row else {
        return Ok(Source::Unknown);
    };
    let live = super::projects_of(&state.mirror, organization).contains(&name)
        && crate::project_ids::recorded(pool, &name)
            .await
            .map_err(failed)?
            .is_some_and(|key| key.project == *source);
    Ok(if live {
        Source::Live(name)
    } else {
        Source::Deleted(name)
    })
}

/// The project folders of `opened` the person may restore into a new project: a live project of
/// the Organization they may read. Deleted projects are named apart; one they may not read is
/// left out.
pub async fn projects(
    state: &AppState,
    identity: &Identity,
    organization: &str,
    opened: &Opened,
) -> Result<ArchivedProjects, ApiError> {
    let pool = database(state)?;
    let mut out = ArchivedProjects {
        projects: Vec::new(),
        deleted: Vec::new(),
    };
    let Some(part) = opened.index.modules.get("portal").filter(|p| p.exported) else {
        return Ok(out);
    };
    for (id, archived_name) in &part.projects {
        let Ok(source) = ProjectId::parse_verified(id) else {
            continue;
        };
        match live_source(state, &pool, organization, &source).await? {
            Source::Live(name)
                if crate::permissions::for_request(state, identity, &name).may_read_project() =>
            {
                out.projects.push(ArchivedProject {
                    id: id.clone(),
                    name,
                });
            }
            Source::Live(_) | Source::Unknown => {}
            Source::Deleted(_) => out.deleted.push(archived_name.clone()),
        }
    }
    out.projects.sort_by(|a, b| a.name.cmp(&b.name));
    out.deleted.sort();
    Ok(out)
}

fn database(state: &AppState) -> Result<sqlx::PgPool, ApiError> {
    state.db.clone().ok_or_else(|| {
        ApiError::Conflict(
            "this Portal runs without a database: there is nothing to restore into".into(),
        )
    })
}

/// What one copy asks for.
pub struct CopyRequest<'a> {
    pub organization: &'a str,
    pub version: &'a str,
    /// The source folder's project id, as the person picked it.
    pub source: &'a str,
    /// The new project's name.
    pub name: &'a str,
    /// `false` checks only: every row is written in a transaction that is rolled back.
    pub apply: bool,
    pub by: &'a str,
}

/// Checks the new name: a DNS label no project, copy or Organization of the installation holds.
async fn free_name(state: &AppState, pool: &sqlx::PgPool, name: &str) -> Result<(), ApiError> {
    if !crate::resource::is_dns1123(name) || name.len() > 63 {
        return Err(invalid(
            "name",
            "use lower-case letters, digits and '-', at most 63, e.g. air-quality-restored".into(),
        ));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_ids WHERE name = $1) \
         OR EXISTS (SELECT 1 FROM organizations WHERE name = $1)",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(failed)?;
    let in_mirror = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", name)
        .is_some();
    if taken || in_mirror {
        return Err(invalid(
            "name",
            format!("{name} is taken by a project, a copy or an Organization; pick a new name"),
        ));
    }
    Ok(())
}

/// Restores the folder `request.source` of `opened` into a new project named `request.name`.
pub async fn copy(
    state: &AppState,
    identity: &Identity,
    opened: &mut Opened,
    request: &CopyRequest<'_>,
) -> Result<CopyPlan, ApiError> {
    let pool = database(state)?;
    if opened.index.format_version < FORMAT_VERSION {
        return Err(invalid(
            "version",
            "this backup was taken before project folders (format 1): restore it whole, or run a \
             new backup and copy the project from that one"
                .into(),
        ));
    }
    let source = ProjectId::parse_verified(request.source).map_err(|_| not_found())?;
    let part = opened
        .index
        .modules
        .get("portal")
        .filter(|p| p.exported)
        .ok_or_else(not_found)?;
    if !part.projects.contains_key(&source.to_string()) {
        return Err(not_found());
    }
    let source_name = match live_source(state, &pool, request.organization, &source).await? {
        Source::Live(name) => name,
        Source::Unknown => return Err(not_found()),
        Source::Deleted(name) => {
            return Err(invalid(
                "project",
                format!(
                    "{name} was deleted since this backup and its id is retired: a deleted \
                     project's data is never restored (TEN-16)"
                ),
            ))
        }
    };
    if !crate::permissions::for_request(state, identity, &source_name).may_read_project() {
        return Err(not_found());
    }
    free_name(state, &pool, request.name).await?;
    let source_key = crate::project_ids::recorded(&pool, &source_name)
        .await
        .map_err(failed)?
        .ok_or_else(not_found)?;
    let new_key = ProjectKey::new(source_key.org, ProjectId::mint());

    let mut plan = CopyPlan {
        version_id: request.version.to_owned(),
        taken_at: opened.index.taken_at.clone(),
        source: source.to_string(),
        source_name: source_name.clone(),
        name: request.name.to_owned(),
        project_id: String::new(),
        tables: BTreeMap::new(),
        not_restored: NOT_RESTORED
            .iter()
            .map(|(t, why)| (format!("portal/{t}"), (*why).to_owned()))
            .collect(),
        erasures_replayed: 0,
    };

    // The source's erasures are read in its own key (T-7202) before the transaction enters the
    // copy's: row-level security shows each project its own rows only.
    let mut tx = crate::db::tenant_tx(&pool, source_key)
        .await
        .map_err(failed)?;
    let erasures = crate::erasure::approved_in(&mut tx, std::slice::from_ref(&source_name))
        .await
        .map_err(failed)?;
    sqlx::query(
        "INSERT INTO project_ids (id, name, parent, organization) VALUES ($1::uuid, $2, $3::uuid, $4)",
    )
    .bind(new_key.project.to_string())
    .bind(request.name)
    .bind(source.to_string())
    .bind(request.organization)
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => invalid(
            "name",
            format!("{} was taken a moment ago; pick a new name", request.name),
        ),
        e => failed(e),
    })?;
    crate::db::enter(&mut tx, new_key).await.map_err(failed)?;

    let mut ids: HashMap<&str, HashMap<String, String>> = HashMap::new();
    for table in copied_tables() {
        let lines = folder_lines(opened, &source, table.name)?;
        let counts = copy_table(&mut tx, table, request.name, lines, &mut ids).await?;
        plan.tables.insert(table.name.to_owned(), counts);
    }
    for mut erasure in erasures {
        erasure.project = request.name.to_owned();
        crate::erasure::erase_portal_copies(&mut tx, &erasure)
            .await
            .map_err(ApiError::Unavailable)?;
        plan.erasures_replayed += 1;
    }
    if !request.apply {
        tx.rollback().await.map_err(failed)?;
        return Ok(plan);
    }
    sqlx::query(
        "INSERT INTO project_copies (id, source, organization, version_id, taken_at, copied_by, rows) \
         VALUES ($1::uuid, $2::uuid, $3, $4, $5, $6, $7)",
    )
    .bind(new_key.project.to_string())
    .bind(source.to_string())
    .bind(request.organization)
    .bind(request.version)
    .bind(&plan.taken_at)
    .bind(request.by)
    .bind(plan.writes() as i64)
    .execute(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    plan.project_id = new_key.project.to_string();
    Ok(plan)
}

/// Writes one table's archived rows into the copy: the key column set to `name`, a row
/// addressed by an `id` given a new one (a sequence's next, or a minted text id) and recorded in
/// `ids`, a reference to a copied row rewritten to its copy. A row a unique rule or a missing
/// reference refuses is counted and left out.
async fn copy_table(
    tx: &mut PgConnection,
    table: &'static super::data::Table,
    name: &str,
    lines: Vec<String>,
    ids: &mut HashMap<&'static str, HashMap<String, String>>,
) -> Result<CopyCounts, ApiError> {
    let mut counts = CopyCounts::default();
    if lines.is_empty() {
        return Ok(counts);
    }
    let pk = key_columns(&mut *tx, table.name).await?;
    let addressed = pk == ["id"];
    let serial = addressed && has_sequence(&mut *tx, table.name).await?;
    let columns: Vec<&str> = table
        .columns
        .iter()
        .copied()
        .filter(|c| !(serial && *c == "id"))
        .collect();
    let list = columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {t} ({list}) SELECT {list} FROM jsonb_populate_record(NULL::{t}, $1) {returning}",
        t = quoted(table.name),
        returning = if addressed { "RETURNING id::text" } else { "" }
    );
    let key = crate::project_ids::key_column(table.name);
    for line in lines {
        let mut row: serde_json::Value =
            serde_json::from_str(&line).map_err(|_| not_an_archive())?;
        let Some(object) = row.as_object_mut() else {
            return Err(not_an_archive());
        };
        object.insert(key.to_owned(), serde_json::Value::String(name.to_owned()));
        let old_id = object.get("id").map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        if addressed && !serial {
            object.insert("id".into(), new_text_id(table.name).into());
        }
        for (_, column, to) in REFERENCES.iter().filter(|(t, _, _)| *t == table.name) {
            let old = object.get(*column).map(|v| v.to_string());
            let new = old.and_then(|o| ids.get(to).and_then(|m| m.get(&o)).cloned());
            // A reference to a row the copy does not hold stays unmatched and the foreign key
            // refuses the row: a copy never points at the source's rows.
            object.insert(
                (*column).to_owned(),
                match new {
                    Some(id) => serde_json::Value::String(id),
                    None => serde_json::Value::Null,
                },
            );
        }
        match write(&mut *tx, &sql, &row, addressed).await? {
            Some(new_id) => {
                counts.create += 1;
                if let (Some(old), Some(new)) = (old_id, new_id) {
                    ids.entry(table.name).or_default().insert(old, new);
                }
            }
            None => counts.refused += 1,
        }
    }
    Ok(counts)
}

/// Whether `table`'s `id` takes its value from a sequence.
async fn has_sequence(conn: &mut PgConnection, table: &str) -> Result<bool, ApiError> {
    let sequence: Option<Option<String>> = sqlx::query_scalar(
        "SELECT pg_get_serial_sequence($1, 'id') FROM pg_attribute \
         WHERE attrelid = to_regclass($1) AND attname = 'id' AND NOT attisdropped",
    )
    .bind(format!("public.{}", table))
    .fetch_optional(conn)
    .await
    .map_err(failed)?;
    Ok(sequence.flatten().is_some())
}

/// Writes one row in a savepoint: `Some(new id)` when written (the id `None` for a table not
/// addressed by one), `None` when a unique rule, a foreign key or a NOT NULL refuses it.
async fn write(
    conn: &mut PgConnection,
    sql: &str,
    row: &serde_json::Value,
    addressed: bool,
) -> Result<Option<Option<String>>, ApiError> {
    sqlx::query("SAVEPOINT copy_row")
        .execute(&mut *conn)
        .await
        .map_err(failed)?;
    let query = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_owned())).bind(row);
    let written = if addressed {
        query.fetch_one(&mut *conn).await.map(Some)
    } else {
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(row)
            .execute(&mut *conn)
            .await
            .map(|_| None)
    };
    match written {
        Ok(id) => {
            sqlx::query("RELEASE SAVEPOINT copy_row")
                .execute(&mut *conn)
                .await
                .map_err(failed)?;
            Ok(Some(id))
        }
        Err(sqlx::Error::Database(e))
            if e.is_unique_violation()
                || e.is_foreign_key_violation()
                || matches!(e.code().as_deref(), Some("23502" | "23514" | "42501")) =>
        {
            sqlx::query("ROLLBACK TO SAVEPOINT copy_row")
                .execute(&mut *conn)
                .await
                .map_err(failed)?;
            Ok(None)
        }
        Err(e) => Err(internal(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_writes_each_referenced_table_before_the_tables_pointing_at_it() {
        let order: Vec<&str> = copied_tables().iter().map(|t| t.name).collect();
        for (from, _, to) in REFERENCES {
            let at = |n: &str| order.iter().position(|t| *t == n).expect(n);
            assert!(at(to) < at(from), "{to} before {from}: {order:?}");
        }
        for (skipped, _) in NOT_RESTORED {
            assert!(!order.contains(skipped), "{skipped} is never copied");
        }
    }
}
