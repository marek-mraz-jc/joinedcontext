//! Restore of an Organization's data backup into that same Organization (T-7171).
//!
//! The archive is opened with an age identity an administrator gives for one request: it lives
//! in a [`Zeroizing`] buffer for that request and is never stored, logged or audited. The
//! version must be one this Organization's own backup wrote (the run log, which the Portal kept,
//! not the archive's index, which anyone holding a key could have written; critique row 9).
//!
//! A restore is two people's act. The first administrator checks a version (the dry run: per
//! table, the rows it would create, update, leave as they are, and the rows it refuses) and
//! requests it; a second administrator of the same Organization approves it with the identity
//! again, and only then are rows written, in one transaction. The request is a row of
//! `organization_data_restores`; every step is in the backup's audit log.
//!
//! Rows are re-keyed to the Organization as it is now: a row is written only when its key (its
//! project, its Organization) is the Organization's today, and an existing row is overwritten
//! only when it is the Organization's too, so an id another Organization's row holds by now is
//! refused, never taken over. A restore creates and updates; it deletes nothing that was added
//! since. Tables whose rows decide access or record what happened are never restored
//! ([`NOT_RESTORED`]). After the rows, every approved erasure of the Organization's projects is
//! applied again to the Portal's copies, so a person erased since the backup stays erased.

use std::collections::BTreeMap;
use std::io::{BufRead, Read};

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use zeroize::Zeroizing;

use super::data::{DataIndex, Scratch, Table, FORMAT_VERSION, PORTAL_TABLES};
use super::{Target, DATA};
use crate::error::ApiError;
use crate::state::AppState;

/// Portal tables a restore never writes, with the reason the plan shows.
pub const NOT_RESTORED: &[(&str, &str)] = &[
    (
        "access_requests",
        "an access decision: restoring it could open access that was withdrawn since",
    ),
    (
        "person_invitations",
        "an invitation lets a person in: it is sent again, never restored",
    ),
    (
        "erasures",
        "the erasure log only grows; its erasures are applied again after the restore",
    ),
    (
        "domain_verifications",
        "a domain is proven again, never restored",
    ),
    ("organization_backup_log", "the audit only grows"),
    (
        "agent_runs",
        "a run's state belongs to the run that made it",
    ),
    (
        "agent_run_events",
        "a run's state belongs to the run that made it",
    ),
    ("app_builds", "a build is made again from its commit"),
    ("workspaces", "a workspace preview belongs to its branch"),
    ("sync_source_state", "the sync reads its source again"),
];

/// What a restore does to one table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreCounts {
    pub create: u64,
    pub update: u64,
    pub unchanged: u64,
    /// Rows whose key is not the Organization's today, or whose id another Organization's
    /// row holds: never written.
    pub refused: u64,
}

/// The dry run of one version, or what its restore did.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataPlan {
    pub organization: String,
    pub version_id: String,
    /// When the backup was taken, RFC 3339.
    pub taken_at: String,
    /// Per module, per table.
    pub modules: BTreeMap<String, BTreeMap<String, RestoreCounts>>,
    /// The tables of the archive a restore leaves alone, with why.
    pub not_restored: BTreeMap<String, String>,
    /// The modules the archive does not hold, with why.
    pub not_in_archive: BTreeMap<String, String>,
    /// Approved erasures applied again after the rows (zero on a dry run).
    pub erasures_replayed: u64,
}

impl DataPlan {
    /// Rows the restore writes.
    pub fn writes(&self) -> u64 {
        self.modules
            .values()
            .flat_map(|tables| tables.values())
            .map(|c| c.create + c.update)
            .sum()
    }
}

fn unreadable() -> ApiError {
    ApiError::Unprocessable {
        detail: "identity: this key does not open the archive; use the working or the recovery \
                 identity (AGE-SECRET-KEY-1…) whose public key is on this page"
            .into(),
        errors: vec!["identity: this key does not open the archive".into()],
    }
}

/// The identity an administrator pasted, parsed; the text stays in its zeroizing buffer.
pub fn identity_of(text: &Zeroizing<String>) -> Result<age::x25519::Identity, ApiError> {
    text.trim()
        .parse::<age::x25519::Identity>()
        .map_err(|_| ApiError::Unprocessable {
            detail: "identity: an age identity, one line starting with AGE-SECRET-KEY-1".into(),
            errors: vec![
                "identity: an age identity, one line starting with AGE-SECRET-KEY-1".into(),
            ],
        })
}

/// The run log's word on `version`: one of this Organization's own data runs wrote it. The
/// archive's index says the same or the restore is refused, but the log is what is trusted.
pub async fn taken_by(state: &AppState, organization: &str, version: &str) -> Result<(), ApiError> {
    let ours = state
        .org_backups
        .entries(organization, DATA, i64::MAX)
        .await?
        .into_iter()
        .any(|e| e.action == "run" && e.ok && e.version_id == version);
    if ours {
        Ok(())
    } else {
        Err(ApiError::Unprocessable {
            detail: format!(
                "version: '{version}' was not written by this Organization's data backup, so it \
                 is not restored here"
            ),
            errors: vec!["version: not a backup of this Organization".into()],
        })
    }
}

/// An opened archive: the decrypted zip in a scratch file, and its index.
pub struct Opened {
    zip: zip::ZipArchive<std::fs::File>,
    pub index: DataIndex,
    _scratch: Scratch,
}

/// Decrypts `sealed` with `identity` into a scratch file and reads its index, which must be
/// this code's format and name `organization`.
pub async fn open(
    sealed: Vec<u8>,
    identity: age::x25519::Identity,
    organization: &str,
) -> Result<Opened, ApiError> {
    let (scratch, file) = Scratch::new(".zip")?;
    let path = scratch.path().to_path_buf();
    let opened = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let decryptor = age::Decryptor::new(sealed.as_slice()).map_err(|_| unreadable())?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| unreadable())?;
        let mut out = std::io::BufWriter::new(file);
        std::io::copy(&mut reader, &mut out).map_err(|_| unreadable())?;
        drop(out);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).map_err(internal)?)
            .map_err(|_| not_an_archive())?;
        let mut text = String::new();
        zip.by_name("index.yaml")
            .map_err(|_| not_an_archive())?
            .read_to_string(&mut text)
            .map_err(|_| not_an_archive())?;
        let index: DataIndex = serde_norway::from_str(&text).map_err(|_| not_an_archive())?;
        Ok((zip, index))
    })
    .await
    .map_err(internal)??;
    let (zip, index) = opened;
    if index.format_version != FORMAT_VERSION {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "version: archive format {} is not the format {FORMAT_VERSION} this Portal restores",
                index.format_version
            ),
            errors: vec!["version: another archive format".into()],
        });
    }
    if index.organization != organization {
        return Err(ApiError::Unprocessable {
            detail: "version: this archive is another Organization's and is never restored here"
                .into(),
            errors: vec!["version: another Organization's archive".into()],
        });
    }
    Ok(Opened {
        zip,
        index,
        _scratch: scratch,
    })
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("the restore failed: {err}"))
}

fn not_an_archive() -> ApiError {
    ApiError::Unprocessable {
        detail: "version: not a data backup archive".into(),
        errors: vec!["version: not a data backup archive".into()],
    }
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The primary key columns of a Portal table.
async fn key_columns(conn: &mut PgConnection, table: &str) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar(
        "SELECT a.attname::text FROM pg_index i \
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
         WHERE i.indrelid = to_regclass($1) AND i.indisprimary ORDER BY a.attnum",
    )
    .bind(format!("public.{table}"))
    .fetch_all(conn)
    .await
    .map_err(failed)
}

fn failed(err: sqlx::Error) -> ApiError {
    ApiError::Unavailable(format!("the Portal database could not be restored: {err}"))
}

/// The SQL that classifies one archived row ($2, jsonb) against the table, the Organization's
/// keys bound as $1.
fn classify_sql(table: &Table, keys: &[String]) -> String {
    let name = quoted(table.name);
    let columns = table
        .columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    let matches = keys
        .iter()
        .map(|k| format!("e.{0} IS NOT DISTINCT FROM i.{0}", quoted(k)))
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "WITH i AS (SELECT * FROM jsonb_populate_record(NULL::{name}, $2)), \
         e AS (SELECT e.* FROM {name} e, i WHERE {matches}) \
         SELECT CASE \
           WHEN NOT EXISTS (SELECT 1 FROM i WHERE {filter}) THEN 'refused' \
           WHEN NOT EXISTS (SELECT 1 FROM e) THEN 'create' \
           WHEN NOT EXISTS (SELECT 1 FROM e WHERE {filter}) THEN 'refused' \
           WHEN (SELECT to_jsonb(x) FROM (SELECT {columns} FROM e) x) \
              = (SELECT to_jsonb(y) FROM (SELECT {columns} FROM i) y) THEN 'unchanged' \
           ELSE 'update' END",
        filter = table.filter
    )
}

/// The SQL that writes one archived row ($2) whose key is the Organization's ($1); an existing
/// row is overwritten only when it is the Organization's too.
fn upsert_sql(table: &Table, keys: &[String]) -> String {
    let name = quoted(table.name);
    let columns = table
        .columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    let set = table
        .columns
        .iter()
        .filter(|c| !keys.iter().any(|k| k == *c))
        .map(|c| format!("{0} = EXCLUDED.{0}", quoted(c)))
        .collect::<Vec<_>>();
    let conflict = keys
        .iter()
        .map(|k| quoted(k))
        .collect::<Vec<_>>()
        .join(", ");
    let action = if set.is_empty() {
        "DO NOTHING".to_owned()
    } else {
        format!(
            "DO UPDATE SET {} WHERE EXISTS (SELECT 1 FROM (SELECT cur.*) s WHERE {})",
            set.join(", "),
            table.filter
        )
    };
    format!(
        "INSERT INTO {name} AS cur ({columns}) SELECT {columns} \
         FROM jsonb_populate_record(NULL::{name}, $2) i WHERE {filter} \
         ON CONFLICT ({conflict}) {action}",
        filter = table.filter
    )
}

/// Raises the id sequence of a table past the ids a restore wrote, so the next row gets a new
/// one. Never lowers it.
async fn raise_sequence(conn: &mut PgConnection, table: &str) -> Result<(), ApiError> {
    let sequence: Option<String> = sqlx::query_scalar("SELECT pg_get_serial_sequence($1, 'id')")
        .bind(format!("public.{table}"))
        .fetch_one(&mut *conn)
        .await
        .map_err(failed)?;
    let Some(sequence) = sequence else {
        return Ok(());
    };
    let sql = format!(
        "SELECT setval($1::regclass, GREATEST((SELECT COALESCE(max(id), 1) FROM {}), \
         (SELECT last_value FROM {})))",
        quoted(table),
        sequence
            .split('.')
            .map(quoted)
            .collect::<Vec<_>>()
            .join(".")
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(&sequence)
        .execute(&mut *conn)
        .await
        .map_err(failed)?;
    Ok(())
}

/// Plans (apply = false) or applies the Portal part of `opened` for `target`, in one
/// transaction; a failed apply writes nothing.
pub async fn run(
    state: &AppState,
    target: &Target,
    version: &str,
    opened: &mut Opened,
    apply: bool,
) -> Result<DataPlan, ApiError> {
    let pool = state.db.clone().ok_or_else(|| {
        ApiError::Conflict(
            "this Portal runs without a database: there is nothing to restore into".into(),
        )
    })?;
    let organization = &target.organization;
    // The keys of today: the Organization's projects now, not the archive's list.
    let mut keys: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    keys.insert("projects", super::projects_of(&state.mirror, organization));
    keys.insert("organization", vec![organization.clone()]);

    let mut plan = DataPlan {
        organization: organization.clone(),
        version_id: version.to_owned(),
        taken_at: opened.index.taken_at.clone(),
        modules: BTreeMap::new(),
        not_restored: NOT_RESTORED
            .iter()
            .map(|(t, why)| (format!("portal/{t}"), (*why).to_owned()))
            .collect(),
        not_in_archive: opened
            .index
            .modules
            .iter()
            .filter(|(_, part)| !part.exported)
            .map(|(m, part)| (m.clone(), part.reason.clone()))
            .collect(),
        erasures_replayed: 0,
    };
    if !opened
        .index
        .modules
        .get("portal")
        .is_some_and(|p| p.exported)
    {
        return Ok(plan);
    }

    let mut tx = pool.begin().await.map_err(failed)?;
    let mut portal = BTreeMap::new();
    for table in PORTAL_TABLES
        .iter()
        .filter(|t| !NOT_RESTORED.iter().any(|(n, _)| *n == t.name))
    {
        let wanted = keys.get(table.keys).cloned().unwrap_or_default();
        let pk = key_columns(&mut tx, table.name).await?;
        if pk.is_empty() {
            return Err(internal(format!("table {} has no primary key", table.name)));
        }
        let classify = classify_sql(table, &pk);
        let upsert = upsert_sql(table, &pk);
        let mut counts = RestoreCounts::default();
        for line in lines(&mut opened.zip, table.name)? {
            let row: serde_json::Value =
                serde_json::from_str(&line).map_err(|_| not_an_archive())?;
            let verdict: String = sqlx::query_scalar(sqlx::AssertSqlSafe(classify.clone()))
                .bind(&wanted)
                .bind(&row)
                .fetch_one(&mut *tx)
                .await
                .map_err(failed)?;
            match verdict.as_str() {
                "create" => counts.create += 1,
                "update" => counts.update += 1,
                "unchanged" => counts.unchanged += 1,
                _ => counts.refused += 1,
            }
            if apply && matches!(verdict.as_str(), "create" | "update") {
                sqlx::query(sqlx::AssertSqlSafe(upsert.clone()))
                    .bind(&wanted)
                    .bind(&row)
                    .execute(&mut *tx)
                    .await
                    .map_err(failed)?;
            }
        }
        if apply && counts.create > 0 {
            raise_sequence(&mut tx, table.name).await?;
        }
        portal.insert(table.name.to_owned(), counts);
    }
    plan.modules.insert("portal".to_owned(), portal);
    if apply {
        let projects = keys.get("projects").cloned().unwrap_or_default();
        for erasure in crate::erasure::approved_in(&mut tx, &projects)
            .await
            .map_err(failed)?
        {
            crate::erasure::erase_portal_copies(&mut tx, &erasure)
                .await
                .map_err(ApiError::Unavailable)?;
            plan.erasures_replayed += 1;
        }
        tx.commit().await.map_err(failed)?;
    } else {
        tx.rollback().await.map_err(failed)?;
    }
    Ok(plan)
}

/// The JSON lines of `module/portal/<table>.jsonl`; none when the archive has no such file.
fn lines(zip: &mut zip::ZipArchive<std::fs::File>, table: &str) -> Result<Vec<String>, ApiError> {
    let Ok(file) = zip.by_name(&format!("module/portal/{table}.jsonl")) else {
        return Ok(Vec::new());
    };
    std::io::BufReader::new(file)
        .lines()
        .filter(|l| l.as_ref().map_or(true, |l| !l.trim().is_empty()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| not_an_archive())
}

/// One restore request: the version, the dry run it was asked on, and where it stands.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreRequest {
    pub id: i64,
    pub version_id: String,
    pub requested_by: String,
    /// RFC 3339.
    pub requested_at: String,
    /// `pending`, `applied`, `declined` or `failed`.
    pub state: String,
    pub plan: DataPlan,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub message: String,
}

type RequestRow = (
    i64,
    String,
    String,
    time::OffsetDateTime,
    String,
    sqlx::types::Json<DataPlan>,
    Option<String>,
    Option<time::OffsetDateTime>,
    String,
);

fn request_of(row: RequestRow) -> DataRestoreRequest {
    DataRestoreRequest {
        id: row.0,
        version_id: row.1,
        requested_by: row.2,
        requested_at: super::rfc3339(row.3),
        state: row.4,
        plan: row.5 .0,
        decided_by: row.6,
        decided_at: row.7.map(super::rfc3339),
        message: row.8,
    }
}

const REQUEST_COLUMNS: &str = "id, version_id, requested_by, requested_at, state, plan, \
                               decided_by, decided_at, message";

fn pool_of(state: &AppState) -> Result<sqlx::PgPool, ApiError> {
    state.db.clone().ok_or_else(|| {
        ApiError::Conflict(
            "this Portal runs without a database: there is nothing to restore into".into(),
        )
    })
}

/// Records a request for a second administrator to approve.
pub async fn request(
    state: &AppState,
    organization: &str,
    actor: &str,
    plan: &DataPlan,
) -> Result<DataRestoreRequest, ApiError> {
    let row: RequestRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO organization_data_restores (organization, version_id, requested_by, plan) \
         VALUES ($1, $2, $3, $4) RETURNING {REQUEST_COLUMNS}"
    )))
    .bind(organization)
    .bind(&plan.version_id)
    .bind(actor)
    .bind(sqlx::types::Json(plan))
    .fetch_one(&pool_of(state)?)
    .await
    .map_err(failed)?;
    Ok(request_of(row))
}

/// The Organization's newest restore requests.
pub async fn requests(
    state: &AppState,
    organization: &str,
) -> Result<Vec<DataRestoreRequest>, ApiError> {
    let Some(pool) = state.db.clone() else {
        return Ok(Vec::new());
    };
    let rows: Vec<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REQUEST_COLUMNS} FROM organization_data_restores WHERE organization = $1 \
         ORDER BY id DESC LIMIT 20"
    )))
    .bind(organization)
    .fetch_all(&pool)
    .await
    .map_err(failed)?;
    Ok(rows.into_iter().map(request_of).collect())
}

/// Takes a pending request of the Organization for `decider` in one statement, so two
/// approvals never both run it. With `others_only` the person who asked cannot take it (an
/// approval); without, they can (withdrawing their own). `None` when there is no such request.
pub async fn decide(
    state: &AppState,
    organization: &str,
    id: i64,
    decider: &str,
    outcome: &str,
    message: &str,
    others_only: bool,
) -> Result<Option<DataRestoreRequest>, ApiError> {
    let row: Option<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE organization_data_restores SET state = $4, decided_by = $3, decided_at = now(), \
         message = $5 WHERE id = $1 AND organization = $2 AND state = 'pending' \
         AND (NOT $6 OR requested_by <> $3) RETURNING {REQUEST_COLUMNS}"
    )))
    .bind(id)
    .bind(organization)
    .bind(decider)
    .bind(outcome)
    .bind(message)
    .bind(others_only)
    .fetch_optional(&pool_of(state)?)
    .await
    .map_err(failed)?;
    Ok(row.map(request_of))
}

/// One pending request of the Organization, whoever asked.
pub async fn pending(
    state: &AppState,
    organization: &str,
    id: i64,
) -> Result<Option<DataRestoreRequest>, ApiError> {
    let row: Option<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REQUEST_COLUMNS} FROM organization_data_restores \
         WHERE id = $1 AND organization = $2 AND state = 'pending'"
    )))
    .bind(id)
    .bind(organization)
    .fetch_optional(&pool_of(state)?)
    .await
    .map_err(failed)?;
    Ok(row.map(request_of))
}

/// Writes how an approved request ended.
pub async fn finish(
    state: &AppState,
    id: i64,
    outcome: &str,
    plan: &DataPlan,
    message: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE organization_data_restores SET state = $2, plan = $3, message = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(outcome)
    .bind(sqlx::types::Json(plan))
    .bind(message)
    .execute(&pool_of(state)?)
    .await
    .map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restored_row_is_written_only_under_the_organizations_key() {
        let table = &PORTAL_TABLES[1];
        let keys = vec!["id".to_owned()];
        let upsert = upsert_sql(table, &keys);
        assert!(upsert.contains("i WHERE project = ANY($1) ON CONFLICT (\"id\") DO UPDATE SET"));
        assert!(
            upsert
                .ends_with("WHERE EXISTS (SELECT 1 FROM (SELECT cur.*) s WHERE project = ANY($1))"),
            "{upsert}"
        );
        let classify = classify_sql(table, &keys);
        assert!(classify.contains("e.\"id\" IS NOT DISTINCT FROM i.\"id\""));
        assert_eq!(classify.matches("project = ANY($1)").count(), 2);
    }

    #[test]
    fn every_not_restored_table_is_one_the_backup_exports() {
        for (name, _) in NOT_RESTORED {
            assert!(PORTAL_TABLES.iter().any(|t| t.name == *name), "{name}");
        }
    }

    #[test]
    fn an_identity_that_is_not_age_is_named() {
        let Err(refused) = identity_of(&Zeroizing::new("age1notasecret".into())) else {
            panic!("a public key is no identity");
        };
        assert!(refused.to_string().contains("identity"), "{refused}");
        let id = age::x25519::Identity::generate();
        let text =
            Zeroizing::new(age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_owned());
        assert!(identity_of(&text).is_ok());
    }
}
