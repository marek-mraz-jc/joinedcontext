//! An Organization's data backup (T-7168): its database rows, module by module, in one zip
//! age-encrypted as a whole to the two recipients its administrators set.
//!
//! Each module hands the frame a [`Source`]: a read-only connection to its own database, the
//! keys that select the Organization's rows there (resolved once, at the start of the run, and
//! written into the archive's index), and the tables with the columns that may leave. The frame
//! reads every table of one database in one `REPEATABLE READ READ ONLY` transaction whose
//! snapshot (`pg_export_snapshot()`) the index records, so each database's part is one moment;
//! two databases are two moments, which the README says.
//!
//! A column is exported only when its table's allow-list names it: a column a later migration
//! adds stays out until someone decides it may leave. Secret material (token hashes, sealed
//! secrets, preview salts) is never on a list.
//!
//! The archive is built in a temporary file and encrypted file to file; nothing of it stays on
//! disk after the run. The index is inside the encrypted envelope, so the archive's claim of
//! whose it is is never trusted: a restore checks the run log, which the Portal wrote.

use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{projects_of, Target};
use crate::error::ApiError;
use crate::state::AppState;

/// The archive layout this code writes; a restore refuses any other.
pub const FORMAT_VERSION: u32 = 1;

/// One table a module exports: the rows `filter` selects (`$1` is the key list named by `keys`,
/// a `text[]`), with only the `columns` named here.
#[derive(Debug)]
pub struct Table {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    /// The name of the key list in the module's [`Source::keys`] bound as `$1`.
    pub keys: &'static str,
    pub filter: &'static str,
}

/// What a module's exporter hands the frame.
pub struct Source {
    pub pool: sqlx::PgPool,
    /// `schema.` prefix of the module's tables; empty for `public`.
    pub schema: &'static str,
    pub keys: BTreeMap<&'static str, Vec<String>>,
    pub tables: &'static [Table],
    /// The session settings the module's row-level security reads, one list per tenant of the
    /// Organization: every table is read once per tenant, `set_config(name, value, true)`
    /// inside the one snapshot. Empty: each table is read once with nothing set (the Portal's
    /// own database has no row-level security).
    pub scopes: Vec<Vec<(&'static str, String)>>,
}

/// One module's part of the index.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModuleIndex {
    pub exported: bool,
    /// Why the module is not in the archive; empty when it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The `pg_export_snapshot()` id the module's rows were read at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub snapshot: String,
    /// The keys that selected the Organization's rows, by name (critique row 3).
    #[serde(default)]
    pub keys: BTreeMap<String, Vec<String>>,
    /// Rows per table, each in `module/<module>/<table>.jsonl`.
    #[serde(default)]
    pub tables: BTreeMap<String, u64>,
}

/// `index.yaml` of a data archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataIndex {
    pub format_version: u32,
    pub organization: String,
    /// RFC 3339, the start of the run.
    pub taken_at: String,
    pub taken_by: String,
    pub modules: BTreeMap<String, ModuleIndex>,
}

/// The modules whose data is not in this archive yet, and why. Each slice that adds an
/// exporter takes its line out.
pub const NOT_EXPORTED_YET: &[(&str, &str)] = &[(
    "assistant",
    "jc_assistant has no Organization key yet; the assistant's runs are in portal/agent_runs",
)];

macro_rules! by_project {
    ($name:literal, [$($column:literal),+ $(,)?]) => {
        Table {
            name: $name,
            columns: &[$($column),+],
            keys: "projects",
            filter: "project = ANY($1)",
        }
    };
}

/// The Portal's tables (jc_portal) that hold an Organization's data, by project of the
/// Organization or by the Organization itself. Left out on purpose: tables of a person rather
/// than a project (connectors, connector_calls, user_preferences, revocations,
/// person_deletions), the installation's (feedback, organization_ids), and the backup's own
/// targets (they hold a sealed secret). Left out by column: `agent_runs.ticket_hash`,
/// `app_builds.token_sha256`, `workspaces.preview_salt`, `domain_verifications.challenge`.
pub const PORTAL_TABLES: &[Table] = &[
    by_project!(
        "access_requests",
        [
            "id",
            "project",
            "pipeline",
            "endpoint_project",
            "endpoint",
            "space",
            "operations",
            "types",
            "until",
            "reason",
            "state",
            "requested_by",
            "requested_at",
            "decided_by",
            "decided_at",
            "decision_reason",
            "change",
            "person",
        ]
    ),
    by_project!(
        "activity",
        [
            "id",
            "time",
            "project",
            "space",
            "kind",
            "source",
            "summary",
            "severity",
            "correlation_id",
            "details"
        ]
    ),
    by_project!(
        "agent_runs",
        [
            "id",
            "project",
            "app_name",
            "endpoint_name",
            "endpoint_slug",
            "profile",
            "app_class",
            "visibility",
            "prompt",
            "prompt_digest",
            "data_needs",
            "allows_write",
            "branch",
            "path_prefix",
            "status",
            "workspace",
            "merge_request",
            "preview_url",
            "steps",
            "tokens_used",
            "created_by",
            "created_at",
            "started_at",
            "finished_at",
            "expires_at",
            "error",
            "files",
            "first_frame_ms",
            "first_version_ms",
            "kind",
            "unattended",
            "continues",
            "endpoints",
            "title",
            "starter",
            "origin",
        ]
    ),
    Table {
        name: "agent_run_events",
        columns: &["run_id", "seq", "kind", "payload", "created_at"],
        keys: "projects",
        filter: "run_id IN (SELECT id FROM agent_runs WHERE project = ANY($1))",
    },
    by_project!(
        "alert_incidents",
        [
            "id",
            "project",
            "pipeline",
            "event",
            "detail",
            "opened_at",
            "recovered_at"
        ]
    ),
    by_project!(
        "alert_notices",
        [
            "id",
            "recipient",
            "subscription",
            "project",
            "pipeline",
            "event",
            "change",
            "detail",
            "created_at",
            "read_at"
        ]
    ),
    by_project!(
        "alert_subscriptions",
        [
            "id",
            "subject",
            "project",
            "scope",
            "target",
            "events",
            "delivery",
            "muted_until",
            "digest_at",
            "created_at"
        ]
    ),
    by_project!(
        "app_builds",
        [
            "id",
            "project",
            "app",
            "commit",
            "class",
            "trigger",
            "requested_by",
            "state",
            "expires_at",
            "created_at",
            "started_at",
            "finished_at",
            "digest",
            "reason",
            "log",
        ]
    ),
    by_project!(
        "app_service_usage",
        ["project", "app", "service", "subject", "day", "used"]
    ),
    by_project!(
        "data_views",
        [
            "id",
            "project",
            "space",
            "entity_type",
            "kind",
            "mode",
            "title",
            "owner_subject",
            "owner_name",
            "config",
            "version",
            "created_at",
            "updated_at",
        ]
    ),
    by_project!(
        "drafts",
        [
            "project",
            "kind",
            "name",
            "manifest",
            "verdict",
            "touched_by",
            "touched_kind",
            "version",
            "updated_at",
            "workspace"
        ]
    ),
    by_project!("email_unsubscribes", ["project", "app", "subject", "at"]),
    by_project!(
        "endpoint_usage_daily",
        ["project", "endpoint", "surface", "day", "calls", "caller"]
    ),
    by_project!(
        "entity_comments",
        [
            "id",
            "project",
            "space",
            "urn",
            "author",
            "author_name",
            "body",
            "mentions",
            "created_at",
            "author_subject"
        ]
    ),
    by_project!(
        "entity_trash",
        [
            "id",
            "project",
            "space",
            "owner",
            "urn",
            "type",
            "entity",
            "deleted_at"
        ]
    ),
    by_project!(
        "erasures",
        [
            "id",
            "project",
            "space",
            "subject",
            "reason",
            "actor",
            "requested_at",
            "completed_at",
            "groups",
            "stores",
            "approval",
            "decided_by",
            "decided_at",
            "decline_reason",
        ]
    ),
    by_project!(
        "grafana_visualizations",
        [
            "id",
            "project",
            "title",
            "endpoint",
            "entity_type",
            "attribute",
            "kind",
            "created_at"
        ]
    ),
    by_project!(
        "health_check",
        [
            "project",
            "kind",
            "name",
            "check",
            "status",
            "since",
            "last_ok",
            "checked_at",
            "detail",
            "fix_hint",
            "evidence_url",
            "detail_say",
            "fix_say",
        ]
    ),
    by_project!(
        "health_check_history",
        ["project", "kind", "name", "check", "at", "status"]
    ),
    by_project!(
        "link_check",
        ["project", "pipeline", "checked_at", "result"]
    ),
    by_project!(
        "mqtt_capture",
        [
            "id",
            "project",
            "broker",
            "topic",
            "client_id",
            "received_at",
            "expires_at",
            "bytes",
            "payload"
        ]
    ),
    by_project!(
        "notifications",
        [
            "id",
            "recipient",
            "project",
            "space",
            "urn",
            "comment_id",
            "author",
            "author_name",
            "excerpt",
            "created_at",
            "read_at",
            "kind",
            "link",
            "event_key",
        ]
    ),
    by_project!("person_invitations", ["person_id", "expires_at", "project"]),
    by_project!(
        "pipeline_log",
        [
            "id",
            "project",
            "pipeline",
            "run",
            "at",
            "record_id",
            "step",
            "outcome",
            "message"
        ]
    ),
    by_project!(
        "pipeline_record_hashes",
        ["project", "pipeline", "record", "hash"]
    ),
    by_project!(
        "pipeline_rejected",
        ["id", "project", "pipeline", "at", "record", "rule", "path", "message", "step", "run"]
    ),
    by_project!(
        "pipeline_runs",
        [
            "project",
            "pipeline",
            "run",
            "first_at",
            "last_at",
            "sent",
            "rejected",
            "failed",
            "created",
            "updated",
            "unchanged",
            "stream_hash",
        ]
    ),
    by_project!(
        "pipeline_source_complete",
        ["project", "pipeline", "run", "at"]
    ),
    by_project!(
        "pipeline_source_holds",
        ["project", "pipeline", "run", "reason", "would", "ledger", "at"]
    ),
    by_project!(
        "pipeline_source_ledger",
        [
            "project",
            "pipeline",
            "entity_id",
            "first_seen",
            "last_seen",
            "last_run",
            "misses",
            "last_written",
            "marked_run",
        ]
    ),
    by_project!(
        "pipeline_source_removed",
        [
            "project",
            "space",
            "entity_id",
            "pipeline",
            "run",
            "at",
            "cause"
        ]
    ),
    by_project!(
        "pipeline_status",
        ["project", "pipeline", "status", "updated_at"]
    ),
    by_project!(
        "pipeline_templates",
        [
            "id",
            "seq",
            "project",
            "title",
            "spec",
            "created_by",
            "created_at"
        ]
    ),
    by_project!(
        "space_quality",
        ["project", "space", "report", "new_rules", "at"]
    ),
    Table {
        name: "sync_source_state",
        columns: &[
            "namespace",
            "name",
            "observed_revision",
            "last_run_at",
            "open_proposal",
            "open_revision",
            "merge_request",
            "last_error",
            "paused",
            "updated_at",
        ],
        keys: "projects",
        filter: "namespace = ANY($1)",
    },
    by_project!(
        "workspaces",
        [
            "name",
            "title",
            "project",
            "owner",
            "base_revision",
            "scope",
            "preview_state",
            "created_at",
            "expires_at",
            "preview_author",
        ]
    ),
    Table {
        name: "domain_verifications",
        columns: &[
            "organization",
            "domain",
            "state",
            "method",
            "checked_at",
            "reason",
        ],
        keys: "organization",
        filter: "organization = ANY($1)",
    },
    Table {
        name: "organization_backup_log",
        columns: &[
            "id",
            "organization",
            "kind",
            "action",
            "actor",
            "at",
            "ok",
            "bytes",
            "object_key",
            "version_id",
            "message",
        ],
        keys: "organization",
        filter: "organization = ANY($1)",
    },
];

/// The Portal tables left out of the data backup on purpose, with the reason. Every table of
/// jc_portal is on [`PORTAL_TABLES`] or here; a test holds that, so a new table is decided on.
pub const PORTAL_LEFT_OUT: &[(&str, &str)] = &[
    (
        "connectors",
        "a person's own connector grants, not an Organization's",
    ),
    (
        "connector_calls",
        "a person's connector calls, not an Organization's",
    ),
    ("user_preferences", "a person's own settings"),
    (
        "revocations",
        "sign-in revocations of people, installation-wide",
    ),
    ("person_deletions", "the installation's people records"),
    ("feedback", "the installation's feedback inbox"),
    ("organization_ids", "the installation's id registry"),
    (
        "organization_hosts",
        "the installation's host registry: a host is proposed and approved again, never restored",
    ),
    (
        "operator_changes",
        "the installation operators' Changes: hosts and operators, not an Organization's",
    ),
    (
        "operator_audit",
        "the installation operators' append-only audit, not an Organization's",
    ),
    (
        "organization_backup_targets",
        "the backup's own target: it holds a sealed secret",
    ),
    ("_sqlx_migrations", "the database's migration history"),
    (
        "organization_data_restores",
        "the restore requests of this backup: a restore is decided again, never restored",
    ),
];

/// The Portal's own source: its database, keyed by the Organization's projects (explicit
/// `organizationRef`, the installation fallback only for the installation's Organization, as
/// [`projects_of`] decides) and by the Organization's name.
fn portal_source(state: &AppState, organization: &str) -> Result<Source, String> {
    let pool = state
        .db
        .clone()
        .ok_or_else(|| "this Portal runs without a database".to_owned())?;
    let mut keys = BTreeMap::new();
    keys.insert("projects", projects_of(&state.mirror, organization));
    keys.insert("organization", vec![organization.to_owned()]);
    Ok(Source {
        pool,
        schema: "",
        keys,
        tables: PORTAL_TABLES,
        scopes: Vec::new(),
    })
}

/// Every module's source for `organization`, or why it is not exported.
async fn sources(
    state: &AppState,
    organization: &str,
) -> Vec<(&'static str, Result<Source, String>)> {
    let mut all = vec![("portal", portal_source(state, organization))];
    all.extend(super::modules::sources(state, organization).await);
    all
}

fn archive_error(err: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("the data archive failed: {err}"))
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The SELECT that reads one table's allowed columns of the Organization's rows as JSON lines.
pub fn select_of(schema: &str, table: &Table) -> String {
    let columns = table
        .columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    let schema = if schema.is_empty() {
        String::new()
    } else {
        format!("{}.", quoted(schema))
    };
    format!(
        "SELECT row_to_json(t)::text FROM (SELECT {columns} FROM {schema}{} WHERE {}) t",
        quoted(table.name),
        table.filter
    )
}

type Zip = zip::ZipWriter<BufWriter<std::fs::File>>;

/// Reads one module's tables in one read-only snapshot into `module/<module>/<table>.jsonl`.
async fn export_module(
    zip: &mut Zip,
    module: &str,
    source: &Source,
) -> Result<ModuleIndex, ApiError> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let failed = |err: sqlx::Error| {
        ApiError::Unavailable(format!("the {module} database could not be read: {err}"))
    };
    let mut tx = source.pool.begin().await.map_err(failed)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
    let snapshot: String = sqlx::query_scalar("SELECT pg_export_snapshot()")
        .fetch_one(&mut *tx)
        .await
        .map_err(failed)?;
    let mut index = ModuleIndex {
        exported: true,
        snapshot,
        keys: source
            .keys
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
        ..ModuleIndex::default()
    };
    for table in source.tables {
        let keys = source.keys.get(table.keys).cloned().unwrap_or_default();
        zip.start_file(format!("module/{module}/{}.jsonl", table.name), options)
            .map_err(archive_error)?;
        let mut rows = 0u64;
        let unscoped = [Vec::new()];
        let scopes = if source.scopes.is_empty() {
            &unscoped[..]
        } else {
            &source.scopes[..]
        };
        for scope in scopes.iter().filter(|_| !keys.is_empty()) {
            for (setting, value) in scope {
                sqlx::query("SELECT set_config($1, $2, true)")
                    .bind(setting)
                    .bind(value)
                    .execute(&mut *tx)
                    .await
                    .map_err(failed)?;
            }
            let sql = select_of(source.schema, table);
            let mut stream = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
                .bind(&keys)
                .fetch(&mut *tx);
            while let Some(line) = stream.try_next().await.map_err(failed)? {
                zip.write_all(line.as_bytes()).map_err(archive_error)?;
                zip.write_all(b"\n").map_err(archive_error)?;
                rows += 1;
            }
        }
        index.tables.insert(table.name.to_owned(), rows);
    }
    tx.rollback().await.map_err(failed)?;
    Ok(index)
}

const README_LEFT_OUT: &str = "\
## Left out on purpose

- Other Organizations' rows and the installation's own tables.
- Secret material: sealed secrets, token hashes, preview salts, people's credentials, second
  factors and sessions. A table's columns are an allow-list: a column not named there stays out.
- Settings and code: the configuration backup holds those.
";

fn readme(index: &DataIndex) -> String {
    let mut text = format!(
        "# Data backup of {}\n\nTaken {} by {}. Format version {}. `index.yaml` lists every \
         module, the keys that selected the Organization's rows there and the rows per table.\n\n\
         Each module's rows are `module/<module>/<table>.jsonl`, one JSON object per line. The \
         rows of one database were read in one snapshot, so each database's part is consistent \
         in itself; two databases were read one after the other, not in one transaction.\n\n\
         ## Modules\n\n",
        index.organization, index.taken_at, index.taken_by, index.format_version
    );
    for (module, part) in &index.modules {
        if part.exported {
            let rows: u64 = part.tables.values().sum();
            text.push_str(&format!(
                "- `{module}`: exported, {} tables, {rows} rows\n",
                part.tables.len()
            ));
        } else {
            text.push_str(&format!("- `{module}`: not exported: {}\n", part.reason));
        }
    }
    text.push('\n');
    text.push_str(README_LEFT_OUT);
    text
}

/// A file in the temporary directory that is removed when it goes out of scope.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn path(&self) -> &std::path::Path {
        &self.0
    }

    pub(super) fn new(suffix: &str) -> Result<(Self, std::fs::File), ApiError> {
        let mut nonce = [0u8; 12];
        argon2::password_hash::rand_core::RngCore::fill_bytes(
            &mut argon2::password_hash::rand_core::OsRng,
            &mut nonce,
        );
        let path = std::env::temp_dir().join(format!(
            "jc-org-backup-{}{suffix}",
            nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ));
        let mut open = std::fs::OpenOptions::new();
        open.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
        let file = open.open(&path).map_err(archive_error)?;
        Ok((Self(path), file))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The recipients a data target encrypts to, parsed; a field that is not one is named.
pub fn parse_recipients(recipients: &[String]) -> Result<Vec<age::x25519::Recipient>, String> {
    recipients
        .iter()
        .zip(["workingRecipient", "recoveryRecipient"])
        .map(|(text, field)| {
            text.trim().parse::<age::x25519::Recipient>().map_err(|_| {
                format!(
                    "{field}: an age public key starting with age1, e.g. \
                     age1ql3z7hjy54pw3hyw…"
                )
            })
        })
        .collect()
}

/// The fingerprint the page shows of a recipient: the first 16 hex digits of its SHA-256, in
/// groups of four, so two admins can compare it with their key at a glance.
pub fn fingerprint(recipient: &str) -> String {
    let digest = Sha256::digest(recipient.trim().as_bytes());
    digest[..8]
        .chunks(2)
        .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join(":")
}

/// Encrypts `plain` to `recipients` file to file.
fn encrypt(
    recipients: &[age::x25519::Recipient],
    plain: &std::path::Path,
    sealed: std::fs::File,
) -> Result<(), String> {
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|e| e.to_string())?;
    let mut out = encryptor
        .wrap_output(BufWriter::new(sealed))
        .map_err(|e| e.to_string())?;
    let mut input = std::fs::File::open(plain).map_err(|e| e.to_string())?;
    std::io::copy(&mut input, &mut out).map_err(|e| e.to_string())?;
    out.finish()
        .and_then(|mut w| w.flush())
        .map_err(|e| e.to_string())
}

/// A test encryption to every recipient, done when a target is saved: a key that cannot be
/// encrypted to is said then, not at the first scheduled run.
pub fn test_encrypt(recipients: &[age::x25519::Recipient]) -> Result<(), String> {
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|e| e.to_string())?;
    let mut out = encryptor
        .wrap_output(Vec::new())
        .map_err(|e| e.to_string())?;
    out.write_all(b"joinedcontext data backup test")
        .and_then(|()| out.finish().map(|_| ()))
        .map_err(|e| e.to_string())
}

/// The Organization's data as a zip in a scratch file, with its index.
async fn build_archive(
    state: &AppState,
    organization: &str,
    actor: &str,
) -> Result<(Scratch, DataIndex), ApiError> {
    let taken_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (scratch, file) = Scratch::new(".zip")?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let mut modules = BTreeMap::new();
    for (module, source) in sources(state, organization).await {
        let part = match source {
            Ok(source) => export_module(&mut zip, module, &source).await?,
            Err(reason) => ModuleIndex {
                reason,
                ..ModuleIndex::default()
            },
        };
        modules.insert(module.to_owned(), part);
    }
    for (module, reason) in NOT_EXPORTED_YET {
        modules
            .entry((*module).to_owned())
            .or_insert_with(|| ModuleIndex {
                reason: (*reason).to_owned(),
                ..ModuleIndex::default()
            });
    }
    let index = DataIndex {
        format_version: FORMAT_VERSION,
        organization: organization.to_owned(),
        taken_at,
        taken_by: actor.to_owned(),
        modules,
    };
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("index.yaml", options)
        .map_err(archive_error)?;
    zip.write_all(
        serde_norway::to_string(&index)
            .map_err(archive_error)?
            .as_bytes(),
    )
    .map_err(archive_error)?;
    zip.start_file("README.md", options)
        .map_err(archive_error)?;
    zip.write_all(readme(&index).as_bytes())
        .map_err(archive_error)?;
    zip.finish()
        .map_err(archive_error)?
        .flush()
        .map_err(archive_error)?;
    Ok((scratch, index))
}

/// The run's archive, encrypted to the target's recipients, in a scratch file: the file, its
/// size and the SHA-256 the run log binds the version by. An archive past one PutObject goes up
/// in parts ([`super::s3::Bucket::put_file`]).
pub(super) async fn encrypted_archive(
    state: &AppState,
    target: &Target,
    actor: &str,
) -> Result<(Scratch, u64, String), ApiError> {
    let recipients = parse_recipients(&target.recipients)
        .ok()
        .filter(|r| r.len() == 2)
        .ok_or_else(|| {
            ApiError::Conflict(
                "the data backup has no working and recovery recipient: set both on \
                 Administration > Backups"
                    .into(),
            )
        })?;
    let (plain, _index) = build_archive(state, &target.organization, actor).await?;
    let (sealed, file) = Scratch::new(".zip.age")?;
    let plain_path = plain.0.clone();
    let sealed_path = sealed.0.clone();
    let (size, sha256) = tokio::task::spawn_blocking(move || -> Result<(u64, String), String> {
        encrypt(&recipients, &plain_path, file)?;
        let mut sealed = std::fs::File::open(&sealed_path).map_err(|e| e.to_string())?;
        let mut digest = Sha256::new();
        let size = std::io::copy(&mut sealed, &mut digest).map_err(|e| e.to_string())?;
        let sha256 = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok((size, sha256))
    })
    .await
    .map_err(archive_error)?
    .map_err(archive_error)?;
    Ok((sealed, size, sha256))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_select_names_only_the_allowed_columns_and_binds_the_keys() {
        let sql = select_of("", &PORTAL_TABLES[1]);
        assert_eq!(
            sql,
            "SELECT row_to_json(t)::text FROM (SELECT \"id\", \"time\", \"project\", \"space\", \
             \"kind\", \"source\", \"summary\", \"severity\", \"correlation_id\", \"details\" \
             FROM \"activity\" WHERE project = ANY($1)) t"
        );
        assert!(select_of("kaisatsu", &PORTAL_TABLES[1]).contains("FROM \"kaisatsu\".\"activity\""));
    }

    #[test]
    fn no_secret_column_is_on_an_allow_list() {
        // Counts and content digests whose names only look like secrets.
        const NOT_SECRET: &[(&str, &str)] = &[
            ("agent_runs", "tokens_used"),
            ("pipeline_record_hashes", "hash"),
            ("pipeline_runs", "stream_hash"),
        ];
        let mut flagged = Vec::new();
        for table in PORTAL_TABLES {
            for column in table.columns {
                let named = ["secret", "token", "salt", "challenge", "password", "hash"]
                    .iter()
                    .any(|secret| column.contains(secret));
                if named && !NOT_SECRET.contains(&(table.name, column)) {
                    flagged.push(format!("{}.{column}", table.name));
                }
            }
        }
        assert!(
            flagged.is_empty(),
            "looks like secret material: {flagged:?}"
        );
    }

    #[test]
    fn a_recipient_that_is_not_age_is_named() {
        let identity = age::x25519::Identity::generate();
        let good = identity.to_public().to_string();
        assert_eq!(
            parse_recipients(&[good.clone(), good.clone()]).map(|r| r.len()),
            Ok(2)
        );
        let refused = parse_recipients(&[good, "ssh-ed25519 AAAA".into()]).unwrap_err();
        assert!(refused.starts_with("recoveryRecipient:"), "{refused}");
        assert_eq!(fingerprint("age1abc").len(), 19);
    }
}
