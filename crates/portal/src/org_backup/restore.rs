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
//!
//! The run log binds each version to the SHA-256 of the bytes the run uploaded, and a request
//! to the bytes its dry run read: a repointed target or another object under the same version id
//! is refused, so the second administrator approves the plan of the bytes the first one checked.
//! Each archive entry and the archive as a whole are read under a size cap ([`CAPS`]).

use std::collections::BTreeMap;
use std::io::{BufRead, Read};

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use zeroize::Zeroizing;

use super::data::{
    project_folder, DataIndex, Scratch, Table, FORMAT_FLAT, FORMAT_VERSION, PORTAL_TABLES,
};
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
        "project_ids",
        "the installation mints a project's id and never reuses it: a restored id could take \
         one minted since; one project's folder is restored into a new project instead",
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
        "project_copies",
        "a copy's lineage is written by the restore that made the copy",
    ),
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
    /// Rows whose key is not the Organization's today, whose id another Organization's row
    /// holds, or that would break a unique rule a row of today holds: never written.
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

/// Why a pasted identity is refused: a reason, then what to paste instead.
const IDENTITY_REFUSED: &str =
    "identity: not an age identity; paste the one line that starts with AGE-SECRET-KEY-1";

/// The identity an administrator pasted, parsed; the text stays in its zeroizing buffer.
pub fn identity_of(text: &Zeroizing<String>) -> Result<age::x25519::Identity, ApiError> {
    text.trim()
        .parse::<age::x25519::Identity>()
        .map_err(|_| ApiError::Unprocessable {
            detail: IDENTITY_REFUSED.into(),
            errors: vec![IDENTITY_REFUSED.into()],
        })
}

/// The run log's word on `version`: one of this Organization's own data runs wrote it, and the
/// SHA-256 of the bytes each such run uploaded. The archive's index says the same or the restore
/// is refused, but the log is what is trusted.
pub async fn taken_by(
    state: &AppState,
    organization: &str,
    version: &str,
) -> Result<Vec<String>, ApiError> {
    let ours: Vec<String> = state
        .org_backups
        .entries(organization, DATA, i64::MAX)
        .await?
        .into_iter()
        .filter(|e| e.action == "run" && e.ok && e.version_id == version)
        .map(|e| e.sha256)
        .collect();
    if !ours.is_empty() {
        Ok(ours)
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

/// Refuses `sealed` unless it is the bytes a run logged for the version (`logged`, from
/// [`taken_by`]); answers their SHA-256.
pub fn same_bytes(sealed: &[u8], logged: &[String]) -> Result<String, ApiError> {
    let sha256 = super::sha256_hex(sealed);
    if logged.iter().any(|l| !l.is_empty() && *l == sha256) {
        Ok(sha256)
    } else {
        Err(ApiError::Unprocessable {
            detail: "version: the object in the bucket is not the one this Organization's backup \
                     uploaded under this version (the target was changed, or the version predates \
                     checksums): run a backup now and restore that version"
                .into(),
            errors: vec!["version: not the bytes this Organization's backup uploaded".into()],
        })
    }
}

/// How much an archive may unpack to: per entry and in total. A deflate bomb is refused with a
/// reason, never unpacked into the Portal's memory.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub entry: u64,
    pub total: u64,
}

/// The caps of a restore: a table's JSON lines up to 256 MiB, the archive up to 1 GiB. The
/// sealed archive itself is at most 256 MiB.
pub const CAPS: Caps = Caps {
    entry: 256 << 20,
    total: 1 << 30,
};

/// The index of an archive is a few kilobytes.
const INDEX_CAP: u64 = 1 << 20;

fn too_large(what: &str, cap: u64) -> ApiError {
    ApiError::Unprocessable {
        detail: format!(
            "version: {what} unpacks to more than {} MiB, more than a backup of this Portal \
             writes, so it is not restored",
            cap >> 20
        ),
        errors: vec![format!("version: {what} is too large to restore")],
    }
}

/// Reads `reader` whole, refusing it once it passes `cap` bytes.
pub(super) fn read_capped(reader: impl Read, cap: u64, what: &str) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|_| not_an_archive())?;
    if out.len() as u64 > cap {
        return Err(too_large(what, cap));
    }
    Ok(out)
}

/// `sealed` decrypted with `identity` into memory, refused once it passes `cap` bytes: the
/// configuration bundle, which a restore unpacks in memory (T-7268).
pub fn decrypt_capped(
    sealed: &[u8],
    identity: &age::x25519::Identity,
    cap: u64,
) -> Result<Vec<u8>, ApiError> {
    let decryptor = age::Decryptor::new(sealed).map_err(|_| unreadable())?;
    let reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|_| unreadable())?;
    read_capped(reader, cap, "the bundle")
}

/// An opened archive: the decrypted zip in a scratch file, its index, and what it may still
/// unpack to.
pub struct Opened {
    pub(super) zip: zip::ZipArchive<std::fs::File>,
    pub index: DataIndex,
    caps: Caps,
    left: u64,
    _scratch: Scratch,
}

/// Decrypts `sealed` with `identity` into a scratch file and reads its index, which must be
/// this code's format and name `organization`.
pub async fn open(
    sealed: Vec<u8>,
    identity: age::x25519::Identity,
    organization: &str,
    caps: Caps,
) -> Result<Opened, ApiError> {
    let (scratch, file) = Scratch::new(".zip")?;
    let path = scratch.path().to_path_buf();
    let opened = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let decryptor = age::Decryptor::new(sealed.as_slice()).map_err(|_| unreadable())?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| unreadable())?;
        // age adds no compression: the plaintext is about the size of the sealed object.
        let mut out = std::io::BufWriter::new(file);
        let limit = caps.total.saturating_add(1);
        let copied =
            std::io::copy(&mut (&mut reader).take(limit), &mut out).map_err(|_| unreadable())?;
        if copied >= limit {
            return Err(too_large("the archive", caps.total));
        }
        drop(out);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).map_err(internal)?)
            .map_err(|_| not_an_archive())?;
        let text = read_capped(
            zip.by_name("index.yaml").map_err(|_| not_an_archive())?,
            INDEX_CAP,
            "index.yaml",
        )?;
        let text = String::from_utf8(text).map_err(|_| not_an_archive())?;
        let index: DataIndex = serde_norway::from_str(&text).map_err(|_| not_an_archive())?;
        Ok((zip, index))
    })
    .await
    .map_err(internal)??;
    let (zip, index) = opened;
    if index.format_version != FORMAT_VERSION && index.format_version != FORMAT_FLAT {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "version: archive format {} is not a format this Portal restores ({FORMAT_FLAT} \
                 or {FORMAT_VERSION})",
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
        caps,
        left: caps.total,
        _scratch: scratch,
    })
}

pub(super) fn internal(err: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("the restore failed: {err}"))
}

pub(super) fn not_an_archive() -> ApiError {
    ApiError::Unprocessable {
        detail: "version: not a data backup archive".into(),
        errors: vec!["version: not a data backup archive".into()],
    }
}

pub(super) fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The primary key columns of a Portal table.
pub(super) async fn key_columns(
    conn: &mut PgConnection,
    table: &str,
) -> Result<Vec<String>, ApiError> {
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

pub(super) fn failed(err: sqlx::Error) -> ApiError {
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
/// one. Never lowers it. A table without an `id` column has none to raise.
async fn raise_sequence(conn: &mut PgConnection, table: &str) -> Result<(), ApiError> {
    // pg_get_serial_sequence raises on a column that does not exist: ask only where it does.
    let sequence: Option<Option<String>> = sqlx::query_scalar(
        "SELECT pg_get_serial_sequence($1, 'id') FROM pg_attribute \
         WHERE attrelid = to_regclass($1) AND attname = 'id' AND NOT attisdropped",
    )
    .bind(format!("public.{table}"))
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed)?;
    let Some(sequence) = sequence.flatten() else {
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

/// Writes one row in a savepoint: `false` when a unique rule of today's rows refuses it (an
/// index beside the primary key), which then counts as refused and leaves the rest standing.
async fn write_row(
    conn: &mut PgConnection,
    upsert: &str,
    wanted: &[String],
    row: &serde_json::Value,
) -> Result<bool, ApiError> {
    sqlx::query("SAVEPOINT restore_row")
        .execute(&mut *conn)
        .await
        .map_err(failed)?;
    match sqlx::query(sqlx::AssertSqlSafe(upsert.to_owned()))
        .bind(wanted)
        .bind(row)
        .execute(&mut *conn)
        .await
    {
        Ok(_) => {
            sqlx::query("RELEASE SAVEPOINT restore_row")
                .execute(&mut *conn)
                .await
                .map_err(failed)?;
            Ok(true)
        }
        // A row another key holds is not this transaction's to overwrite: the policy refuses it.
        Err(sqlx::Error::Database(e))
            if e.is_unique_violation() || e.code().as_deref() == Some("42501") =>
        {
            sqlx::query("ROLLBACK TO SAVEPOINT restore_row")
                .execute(&mut *conn)
                .await
                .map_err(failed)?;
            Ok(false)
        }
        Err(e) => Err(failed(e)),
    }
}

/// Whether the Portal table `table` is keyed per project (a `project_id` column, T-7196): its row
/// takes the key of the transaction that writes it, so each project's rows are written in that
/// project's key, whatever row-level security this connection's role is under.
async fn keyed_per_project(conn: &mut PgConnection, table: &str) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' \
         AND table_name = $1 AND column_name = 'project_id')",
    )
    .bind(table)
    .fetch_one(conn)
    .await
    .map_err(failed)
}

/// The archived rows of `table`, parsed. Per project (`per_project`): grouped by the project
/// whose key a row takes, each with that key when the project is one of the Organization's
/// (`wanted`) and has one today, `None` otherwise; else one group with no key.
async fn batches(
    state: &AppState,
    pool: &sqlx::PgPool,
    table: &Table,
    wanted: &[String],
    lines: Vec<String>,
    per_project: bool,
) -> Result<Vec<(Option<jc_types::ProjectKey>, Vec<serde_json::Value>)>, ApiError> {
    let mut rows = Vec::with_capacity(lines.len());
    for line in lines {
        rows.push(serde_json::from_str::<serde_json::Value>(&line).map_err(|_| not_an_archive())?);
    }
    if !per_project {
        return Ok(vec![(None, rows)]);
    }
    let column = crate::project_ids::key_column(table.name);
    let mut by: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for row in rows {
        let project = row
            .get(column)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        by.entry(project).or_default().push(row);
    }
    let mut out = Vec::with_capacity(by.len());
    for (project, rows) in by {
        let key = if wanted.contains(&project) {
            crate::project_ids::key_of(pool, &state.mirror, &project)
                .await
                .map_err(failed)?
        } else {
            None
        };
        out.push((key, rows));
    }
    Ok(out)
}

/// The approval a restore writes under: the pending request and the second administrator.
pub struct Approval<'a> {
    pub id: i64,
    pub by: &'a str,
    /// The approver's Keycloak subject; never the asker's.
    pub subject: &'a str,
}

/// The request was taken by another approval, declined, or asked by the approver.
pub fn no_request(id: i64) -> ApiError {
    ApiError::NotFound(format!(
        "restore request {id} is not pending in this Organization, or you asked for it"
    ))
}

/// Plans (`approval` none) or applies the Portal part of `opened` for `target`, in one
/// transaction. A plan writes every row in that transaction too, so it counts what the apply
/// meets, and then rolls back. An apply takes the pending request first and marks it applied
/// in the same transaction as the rows: a failed or interrupted apply writes nothing and leaves
/// the request pending.
pub async fn run(
    state: &AppState,
    target: &Target,
    version: &str,
    opened: &mut Opened,
    approval: Option<&Approval<'_>>,
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
    // Decided (T-7171, worker-t7169): the Portal holds a read-only role on the module databases
    // (least privilege), so a module's rows come back by the module's own restore, not here.
    for (module, part) in &opened.index.modules {
        if part.exported && module != "portal" {
            plan.not_restored.insert(
                format!("{module}/*"),
                format!(
                    "the Portal only reads the {module} database; its rows are in \
                     module/{module}/ of the archive for the {module} operator to restore"
                ),
            );
        }
    }
    let exported = opened
        .index
        .modules
        .get("portal")
        .is_some_and(|p| p.exported);

    // Keyed on the Organization's reserved project (T-7191): its restore requests answer only so.
    let reserved = crate::project_ids::reserved(&pool, organization)
        .await
        .map_err(failed)?
        .ok_or_else(|| super::unregistered(organization))?;
    let mut tx = crate::db::bulk_tenant_tx(&pool, reserved)
        .await
        .map_err(failed)?;
    if let Some(approval) = approval {
        // Held until the commit: a second approval waits here, then finds it taken.
        let pending: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM organization_data_restores WHERE id = $1 AND organization = $2 \
             AND state = 'pending' AND requested_subject <> $3 FOR UPDATE",
        )
        .bind(approval.id)
        .bind(organization)
        .bind(approval.subject)
        .fetch_optional(&mut *tx)
        .await
        .map_err(failed)?;
        if pending.is_none() {
            return Err(no_request(approval.id));
        }
    }
    let mut portal = BTreeMap::new();
    for table in PORTAL_TABLES
        .iter()
        .filter(|t| exported && !NOT_RESTORED.iter().any(|(n, _)| *n == t.name))
    {
        let wanted = keys.get(table.keys).cloned().unwrap_or_default();
        let pk = key_columns(&mut tx, table.name).await?;
        if pk.is_empty() {
            return Err(internal(format!("table {} has no primary key", table.name)));
        }
        let classify = classify_sql(table, &pk);
        let upsert = upsert_sql(table, &pk);
        let mut counts = RestoreCounts::default();
        // A project's row of a keyed table is written under that project's key alone (T-7201);
        // a row of a project the Organization does not hold today, or with no key, is refused.
        let per_project =
            table.keys == "projects" && keyed_per_project(&mut tx, table.name).await?;
        for (key, rows) in batches(
            state,
            &pool,
            table,
            &wanted,
            lines(opened, table.name)?,
            per_project,
        )
        .await?
        {
            match key {
                Some(key) => crate::db::enter(&mut tx, key).await.map_err(failed)?,
                None if per_project => {
                    counts.refused += rows.len() as u64;
                    continue;
                }
                None => {}
            }
            for row in rows {
                let verdict: String = sqlx::query_scalar(sqlx::AssertSqlSafe(classify.clone()))
                    .bind(&wanted)
                    .bind(&row)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(failed)?;
                let written = matches!(verdict.as_str(), "create" | "update")
                    && write_row(&mut tx, &upsert, &wanted, &row).await?;
                match verdict.as_str() {
                    "create" if written => counts.create += 1,
                    "update" if written => counts.update += 1,
                    "unchanged" => counts.unchanged += 1,
                    _ => counts.refused += 1,
                }
            }
        }
        if per_project {
            crate::db::enter(&mut tx, reserved).await.map_err(failed)?;
        }
        if approval.is_some() && counts.create > 0 {
            raise_sequence(&mut tx, table.name).await?;
        }
        portal.insert(table.name.to_owned(), counts);
    }
    if exported {
        plan.modules.insert("portal".to_owned(), portal);
    }
    if let Some(approval) = approval {
        let projects = keys.get("projects").cloned().unwrap_or_default();
        // Each project's erasures and copies in its own key (T-7202), then the Organization's
        // reserved key again for the restore request's own row.
        for project in &projects {
            if let Some(key) = crate::project_ids::recorded(&pool, project)
                .await
                .map_err(failed)?
            {
                crate::db::enter(&mut tx, key).await.map_err(failed)?;
            }
            for erasure in crate::erasure::approved_in(&mut tx, std::slice::from_ref(project))
                .await
                .map_err(failed)?
            {
                crate::erasure::erase_portal_copies(&mut tx, &erasure)
                    .await
                    .map_err(ApiError::Unavailable)?;
                plan.erasures_replayed += 1;
            }
        }
        crate::db::enter(&mut tx, reserved).await.map_err(failed)?;
        sqlx::query(
            "UPDATE organization_data_restores SET state = 'applied', decided_by = $2, \
             decided_at = now(), plan = $3, message = $4 WHERE id = $1",
        )
        .bind(approval.id)
        .bind(approval.by)
        .bind(sqlx::types::Json(&plan))
        .bind(applied_message(&plan))
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
        tx.commit().await.map_err(failed)?;
    } else {
        tx.rollback().await.map_err(failed)?;
    }
    Ok(plan)
}

/// What an applied restore says it did.
pub fn applied_message(plan: &DataPlan) -> String {
    format!(
        "{} row(s) written, {} erasure(s) applied again",
        plan.writes(),
        plan.erasures_replayed
    )
}

/// The JSON lines of `table` in the archive: `module/portal/<table>.jsonl` (format 1, or an
/// Organization outside the registry) and the table's file in every project folder (format 2).
fn lines(opened: &mut Opened, table: &str) -> Result<Vec<String>, ApiError> {
    let file = format!("/{table}.jsonl");
    let names: Vec<String> = opened
        .zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .filter(|n| {
            n.strip_prefix("module/portal/project/")
                .and_then(|rest| rest.strip_suffix(&file))
                .is_some_and(|id| !id.is_empty() && !id.contains('/'))
        })
        .collect();
    let mut out = file_lines(opened, &format!("module/portal/{table}.jsonl"))?;
    for name in names {
        out.extend(file_lines(opened, &name)?);
    }
    Ok(out)
}

/// The JSON lines of `name` in one project's folder (format 2).
pub(super) fn folder_lines(
    opened: &mut Opened,
    project: &jc_types::ProjectId,
    table: &str,
) -> Result<Vec<String>, ApiError> {
    file_lines(opened, &format!("{}{table}.jsonl", project_folder(project)))
}

/// The JSON lines of the archive file `name`, read under the entry cap and what is left of the
/// total; none when the archive has no such file.
fn file_lines(opened: &mut Opened, name: &str) -> Result<Vec<String>, ApiError> {
    let name = name.to_owned();
    let Ok(file) = opened.zip.by_name(&name) else {
        return Ok(Vec::new());
    };
    let cap = opened.caps.entry.min(opened.left);
    let what = if cap < opened.caps.entry {
        "the archive".to_owned()
    } else {
        name
    };
    let bytes = read_capped(file, cap, &what)?;
    opened.left -= bytes.len() as u64;
    bytes
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
    /// The asker's Keycloak subject: the approver must be another.
    #[serde(skip)]
    pub requested_subject: String,
    /// The SHA-256 of the sealed object the dry run read.
    #[serde(skip)]
    pub sha256: String,
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
    String,
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
        requested_subject: row.9,
        sha256: row.10,
    }
}

const REQUEST_COLUMNS: &str = "id, version_id, requested_by, requested_at, state, plan, \
                               decided_by, decided_at, message, requested_subject, sha256";

fn pool_of(state: &AppState) -> Result<sqlx::PgPool, ApiError> {
    state.db.clone().ok_or_else(|| {
        ApiError::Conflict(
            "this Portal runs without a database: there is nothing to restore into".into(),
        )
    })
}

/// Records a request for a second administrator to approve, bound to the bytes its dry run
/// read (`sha256`).
pub async fn request(
    state: &AppState,
    organization: &str,
    actor: &crate::auth::session::Identity,
    plan: &DataPlan,
    sha256: &str,
) -> Result<DataRestoreRequest, ApiError> {
    let mut tx = keyed_of(state, organization).await?;
    let row: RequestRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO organization_data_restores (organization, version_id, requested_by, plan, \
         requested_subject, sha256) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {REQUEST_COLUMNS}"
    )))
    .bind(organization)
    .bind(&plan.version_id)
    .bind(&actor.username)
    .bind(sqlx::types::Json(plan))
    .bind(&actor.subject)
    .bind(sha256)
    .fetch_one(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
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
    let Some(mut tx) = super::keyed(&pool, organization).await? else {
        return Ok(Vec::new());
    };
    let rows: Vec<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REQUEST_COLUMNS} FROM organization_data_restores WHERE organization = $1 \
         ORDER BY id DESC LIMIT 20"
    )))
    .bind(organization)
    .fetch_all(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(rows.into_iter().map(request_of).collect())
}

/// Ends a pending request of the Organization without writing rows (declined, withdrawn or
/// failed), in one statement. `None` when there is no such request.
pub async fn decide(
    state: &AppState,
    organization: &str,
    id: i64,
    decider: &str,
    outcome: &str,
    message: &str,
) -> Result<Option<DataRestoreRequest>, ApiError> {
    let mut tx = keyed_of(state, organization).await?;
    let row: Option<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE organization_data_restores SET state = $4, decided_by = $3, decided_at = now(), \
         message = $5 WHERE id = $1 AND organization = $2 AND state = 'pending' \
         RETURNING {REQUEST_COLUMNS}"
    )))
    .bind(id)
    .bind(organization)
    .bind(decider)
    .bind(outcome)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(row.map(request_of))
}

/// One pending request of the Organization, whoever asked.
pub async fn pending(
    state: &AppState,
    organization: &str,
    id: i64,
) -> Result<Option<DataRestoreRequest>, ApiError> {
    let mut tx = keyed_of(state, organization).await?;
    let row: Option<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REQUEST_COLUMNS} FROM organization_data_restores \
         WHERE id = $1 AND organization = $2 AND state = 'pending'"
    )))
    .bind(id)
    .bind(organization)
    .fetch_optional(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(row.map(request_of))
}

/// Whether the Organization has a restore request waiting: its data target and recipients
/// stay as they are until it is decided.
pub async fn any_pending(state: &AppState, organization: &str) -> Result<bool, ApiError> {
    let Some(pool) = state.db.clone() else {
        return Ok(false);
    };
    let Some(mut tx) = super::keyed(&pool, organization).await? else {
        return Ok(false);
    };
    let pending = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM organization_data_restores \
         WHERE organization = $1 AND state = 'pending')",
    )
    .bind(organization)
    .fetch_one(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(pending)
}

/// A transaction keyed on the Organization's reserved project, or why there is none.
async fn keyed_of(
    state: &AppState,
    organization: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
    super::keyed(&pool_of(state)?, organization)
        .await?
        .ok_or_else(|| super::unregistered(organization))
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
    fn an_entry_that_unpacks_past_its_cap_is_refused_with_a_reason() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "module/portal/activity.jsonl",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("entry");
        zip.write_all(&vec![b'{'; 64 << 10]).expect("write");
        let mut zip = zip::ZipArchive::new(zip.finish().expect("zip")).expect("archive");
        let packed = zip.by_index_raw(0).expect("entry").compressed_size();
        assert!(packed < 4 << 10, "deflate packs it to {packed} bytes");
        let entry = zip.by_index(0).expect("entry");
        let Err(refused) = read_capped(entry, 16 << 10, "module/portal/activity.jsonl") else {
            panic!("64 KiB past a 16 KiB cap");
        };
        assert!(
            refused
                .to_string()
                .contains("activity.jsonl unpacks to more than"),
            "{refused}"
        );
        let entry = zip.by_index(0).expect("entry");
        assert_eq!(
            read_capped(entry, 64 << 10, "x").expect("at the cap").len(),
            64 << 10
        );
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
