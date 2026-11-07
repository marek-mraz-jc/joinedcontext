//! The Portal's own PostgreSQL: the things that are neither configuration nor context data.
//! Preferences are what a person likes about the UI (UI-09); a builder run and its event stream
//! are a conversation, which no manifest can hold (AG-43, AG-45). Everything else lives
//! in Git or in the broker.

use crate::agents::run::{AgentRun, AgentRunEvent};
// The four agent-run reads interpolate a `const` column list into their statement and nothing
// else: no caller value ever reaches the SQL text, every value is a bound parameter. That is
// what the wrapper asserts.
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::AssertSqlSafe;
use std::time::Duration;
use time::OffsetDateTime;

/// Connects and brings the schema up to date. Migrations are embedded, so a fresh database
/// needs no step outside the binary; running them twice is a no-op.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = pool_options().connect(url).await?;
    let mut migrator = sqlx::migrate!("./migrations");
    let newest = migrator.iter().map(|m| m.version).max().unwrap_or(0);
    let newer = newer_migrations(&pool, newest).await?;
    if tolerate_newer(newest, &newer).map_err(|e| sqlx::Error::Migrate(Box::new(e)))? {
        tracing::warn!(
            "the database holds migrations {newer:?} of a newer release, none marked breaking: \
             serving it as a rollback; roll forward again before another schema change"
        );
        migrator.set_ignore_missing(true);
    }
    migrator.run(&pool).await?;
    Ok(pool)
}

/// Idle connections close after 60 s (T-6201): the run queue and sweeps touch the pool more
/// often than sqlx's 10 min default, so all five stayed open all day, ~19 MiB of Postgres each.
fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(5)
        .min_connections(0)
        .idle_timeout(Duration::from_secs(60))
        .acquire_timeout(Duration::from_secs(5))
}

/// Version and description of each applied migration above `newest`.
async fn newer_migrations(pool: &PgPool, newest: i64) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE version > $1 ORDER BY version",
    )
    .bind(newest)
    .fetch_all(pool)
    .await
    .or_else(|e| match &e {
        // a database never migrated has no history table yet
        sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01") => Ok(Vec::new()),
        _ => Err(e),
    })
}

/// What the boot does with the migrations a newer release applied (`newer`: version and
/// description of each applied version above `newest`, this binary's own newest). A rollback
/// starts the previous image on the schema the release it rolls back already migrated, and
/// sqlx refuses every applied version it does not ship, so without this the previous image
/// never boots (T-5105, docs Deployment/16-release-runbook.md §5). A migration whose file name
/// starts with `breaking_` is one the previous release cannot serve: the way back is the
/// backup taken before the release, so the boot stops there. An unknown version at or below
/// `newest` is still refused by sqlx. `Ok(true)`: tolerate the newer, compatible migrations.
fn tolerate_newer(
    newest: i64,
    newer: &[(i64, String)],
) -> Result<bool, sqlx::migrate::MigrateError> {
    match newer.iter().find(|(_, d)| d.starts_with("breaking ")) {
        Some((v, _)) => Err(sqlx::migrate::MigrateError::VersionTooNew(*v, newest)),
        None => Ok(!newer.is_empty()),
    }
}

/// The stored document of one subject, `None` before the first save.
pub async fn load_preferences(
    pool: &PgPool,
    subject: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT preferences FROM user_preferences WHERE subject = $1",
    )
    .bind(subject)
    .fetch_optional(pool)
    .await
}

/// Replaces the subject's document whole (UI-10: immediate, no Git involved).
pub async fn save_preferences(
    pool: &PgPool,
    subject: &str,
    preferences: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_preferences (subject, preferences, updated_at) VALUES ($1, $2, now()) \
         ON CONFLICT (subject) DO UPDATE SET preferences = EXCLUDED.preferences, updated_at = now()",
    )
    .bind(subject)
    .bind(preferences)
    .execute(pool)
    .await
    .map(|_| ())
}

/// What one `SyncSource` run left behind (MF-30).
///
/// The row is the loop's whole memory: which revision of the source this repository carries,
/// when the last run happened, the proposal a run opened and nobody has answered yet, and
/// whether an operator switched syncing off. Nothing about the source itself is here — the
/// manifest is in Git and the credential is in the secret store.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct SyncStateRow {
    pub namespace: String,
    pub name: String,
    pub observed_revision: Option<String>,
    pub last_run_at: Option<i64>,
    /// The branch of the open proposal, which is `jcctl::sync::proposal_name`.
    pub open_proposal: Option<String>,
    /// The source revision that proposal carries; the branch holds only its first characters.
    pub open_revision: Option<String>,
    /// Where a reviewer answers it.
    pub merge_request: Option<String>,
    /// Why the last run did not finish, cleared by the next run that does.
    pub last_error: Option<String>,
    pub paused: bool,
}

/// The state of one source, `None` before its first run.
pub async fn load_sync_state(
    pool: &PgPool,
    namespace: &str,
    name: &str,
) -> Result<Option<SyncStateRow>, sqlx::Error> {
    sqlx::query_as::<_, SyncStateRow>(
        "SELECT namespace, name, observed_revision, last_run_at, open_proposal, open_revision, \
         merge_request, last_error, paused FROM sync_source_state \
         WHERE namespace = $1 AND name = $2",
    )
    .bind(namespace)
    .bind(name)
    .fetch_optional(pool)
    .await
}

/// Replaces the row whole: a run's state is one value, and writing half of it is how a
/// restarted Portal would re-propose a revision it had already proposed.
pub async fn save_sync_state(pool: &PgPool, row: &SyncStateRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sync_source_state (namespace, name, observed_revision, last_run_at, \
         open_proposal, open_revision, merge_request, last_error, paused, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
         ON CONFLICT (namespace, name) DO UPDATE SET \
         observed_revision = EXCLUDED.observed_revision, last_run_at = EXCLUDED.last_run_at, \
         open_proposal = EXCLUDED.open_proposal, open_revision = EXCLUDED.open_revision, \
         merge_request = EXCLUDED.merge_request, last_error = EXCLUDED.last_error, \
         paused = EXCLUDED.paused, updated_at = now()",
    )
    .bind(&row.namespace)
    .bind(&row.name)
    .bind(&row.observed_revision)
    .bind(row.last_run_at)
    .bind(&row.open_proposal)
    .bind(&row.open_revision)
    .bind(&row.merge_request)
    .bind(&row.last_error)
    .bind(row.paused)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The columns of `agent_runs` in the order [`AgentRun`](crate::agents::run::AgentRun) declares
/// them, with the four timestamps already rendered as the RFC 3339 strings the API answers.
///
/// Postgres does that rendering because `time` is compiled here with `formatting` and no parser:
/// reading a `timestamptz` into the struct would need one. `to_char` of a NULL column is NULL, so
/// the three optional timestamps stay optional.
const AGENT_RUN_COLUMNS: &str = "id, project, app_name, title, endpoint_name, endpoint_slug, endpoints, profile, \
     kind, unattended, continues, \
     app_class, visibility, prompt, prompt_digest, data_needs, allows_write, branch, path_prefix, \
     status, ticket_hash, workspace, merge_request, preview_url, first_frame_ms, first_version_ms, files, steps, tokens_used, created_by, origin, starter, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at, \
     to_char(started_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS started_at, \
     to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS finished_at, \
     to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS expires_at, \
     error";

/// The same treatment for one event of the stream.
const AGENT_EVENT_COLUMNS: &str = "run_id, seq, kind, payload, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at";

/// Records `run` only when its application has no live run, decided under a transaction-scoped
/// advisory lock on (project, application): two concurrent admissions of one App's run never
/// both pass the check (F-POR1-018). `Ok(false)` when another live run holds the App.
pub async fn insert_agent_run_alone(pool: &PgPool, run: &AgentRun) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || '/' || $2, 0))")
        .bind(&run.project)
        .bind(&run.app_name)
        .execute(&mut *tx)
        .await?;
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agent_runs WHERE project = $1 AND app_name = $2 \
         AND status NOT IN ('published', 'failed', 'cancelled', 'expired'))",
    )
    .bind(&run.project)
    .bind(&run.app_name)
    .fetch_one(&mut *tx)
    .await?;
    if live {
        return Ok(false);
    }
    insert_agent_run(&mut *tx, run).await?;
    tx.commit().await?;
    Ok(true)
}

/// Records a run at the moment it is admitted, ticket hash included (AG-43).
///
/// The two timestamps arrive as the RFC 3339 text the caller minted, and the cast is written into
/// the statement so the parameter stays a string on this side.
pub async fn insert_agent_run(
    pool: impl sqlx::PgExecutor<'_>,
    run: &AgentRun,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, \
         kind, unattended, continues, \
         app_class, visibility, prompt, prompt_digest, data_needs, allows_write, branch, \
         path_prefix, status, ticket_hash, steps, tokens_used, created_by, created_at, expires_at, \
         endpoints, starter, origin) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
         $19, $20, $21, $22, $23::text::timestamptz, $24::text::timestamptz, $25, $26, $27)",
    )
    .bind(&run.id)
    .bind(&run.project)
    .bind(&run.app_name)
    .bind(&run.endpoint_name)
    .bind(&run.endpoint_slug)
    .bind(&run.profile)
    .bind(&run.kind)
    .bind(run.unattended)
    .bind(&run.continues)
    .bind(&run.app_class)
    .bind(&run.visibility)
    .bind(&run.prompt)
    .bind(&run.prompt_digest)
    .bind(&run.data_needs)
    .bind(run.allows_write)
    .bind(&run.branch)
    .bind(&run.path_prefix)
    .bind(&run.status)
    .bind(&run.ticket_hash)
    .bind(run.steps)
    .bind(run.tokens_used)
    .bind(&run.created_by)
    .bind(&run.created_at)
    .bind(&run.expires_at)
    .bind(&run.endpoints)
    .bind(&run.starter)
    .bind(&run.origin)
    .execute(pool)
    .await
    .map(|_| ())
}

/// One run by id, `None` when there is no such row.
pub async fn load_agent_run(pool: &PgPool, id: &str) -> Result<Option<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Lists runs for a project matching filter criteria, newest first.
pub async fn list_agent_runs_filtered(
    pool: &PgPool,
    project: &str,
    filter: &crate::agents::store::RunFilter,
    limit: i64,
) -> Result<Vec<AgentRun>, sqlx::Error> {
    let mut sql = format!("SELECT {AGENT_RUN_COLUMNS} FROM agent_runs WHERE project = $1");
    let mut param_idx = 2;
    if filter.app.is_some() {
        sql.push_str(&format!(" AND app_name = ${param_idx}"));
        param_idx += 1;
    }
    if filter.kind.is_some() {
        sql.push_str(&format!(" AND kind = ${param_idx}"));
        param_idx += 1;
    }
    if filter.status.is_some() {
        sql.push_str(&format!(" AND status = ${param_idx}"));
        param_idx += 1;
    }
    if filter.created_by.is_some() {
        sql.push_str(&format!(" AND created_by = ${param_idx}"));
        param_idx += 1;
    }
    if filter.origin.is_some() {
        sql.push_str(&format!(" AND origin = ${param_idx}"));
        param_idx += 1;
    }
    // Ties by id, as the memory store sorts them, so a page is the same between two calls.
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id ASC LIMIT ${param_idx}"
    ));

    let mut query = sqlx::query_as::<_, AgentRun>(AssertSqlSafe(sql)).bind(project);
    if let Some(app) = &filter.app {
        query = query.bind(app);
    }
    if let Some(kind) = &filter.kind {
        query = query.bind(kind);
    }
    if let Some(status) = &filter.status {
        query = query.bind(status);
    }
    if let Some(created_by) = &filter.created_by {
        query = query.bind(created_by);
    }
    if let Some(origin) = &filter.origin {
        query = query.bind(origin);
    }
    query = query.bind(limit);

    query.fetch_all(pool).await
}

/// Loads the active (non-terminal) run for an application in a project, if any.
pub async fn load_live_agent_run_for_app(
    pool: &PgPool,
    project: &str,
    app_name: &str,
) -> Result<Option<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs \
         WHERE project = $1 AND app_name = $2 \
         AND status NOT IN ('published', 'failed', 'cancelled', 'expired') \
         ORDER BY created_at DESC LIMIT 1"
    )))
    .bind(project)
    .bind(app_name)
    .fetch_optional(pool)
    .await
}

/// Lists non-terminal runs whose expiration time is before the given timestamp.
pub async fn list_expired_agent_runs(
    pool: &PgPool,
    now_rfc3339: &str,
) -> Result<Vec<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs \
         WHERE status NOT IN ('published', 'failed', 'cancelled', 'expired') \
         AND expires_at < $1::text::timestamptz \
         ORDER BY expires_at ASC"
    )))
    .bind(now_rfc3339)
    .fetch_all(pool)
    .await
}

/// Every run in `status`, in every project, oldest first.
pub async fn list_agent_runs_in_status(
    pool: &PgPool,
    status: &str,
) -> Result<Vec<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs WHERE status = $1 ORDER BY created_at ASC"
    )))
    .bind(status)
    .fetch_all(pool)
    .await
}

/// Moves a run to another state. `started_at` is stamped by the first state after `queued` and
/// `finished_at` by the state the run stops in, so neither is ever moved twice.
pub async fn update_agent_run_status(
    pool: &PgPool,
    id: &str,
    from: &str,
    status: &str,
    error: Option<&str>,
    finished: bool,
) -> Result<bool, sqlx::Error> {
    // Only from the status the caller judged the transition on: two racing transitions, one
    // row changed (F-POR1-019).
    sqlx::query(
        "UPDATE agent_runs SET status = $2, error = COALESCE($3, error), \
         started_at = CASE WHEN started_at IS NULL AND $2 <> 'queued' THEN now() ELSE started_at END, \
         finished_at = CASE WHEN $4 AND finished_at IS NULL THEN now() ELSE finished_at END \
         WHERE id = $1 AND status = $5",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .bind(finished)
    .bind(from)
    .execute(pool)
    .await
    .map(|done| done.rows_affected() == 1)
}

/// Moves a run's lease: a run waiting for its change's approval is given the approval's lease
/// (T-2772).
pub async fn update_agent_run_expiry(
    pool: &PgPool,
    id: &str,
    expires_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET expires_at = $2::text::timestamptz WHERE id = $1")
        .bind(id)
        .bind(expires_at)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Forgets a run's ticket hash, which is what a cancellation does to its workspace's credential:
/// every later proxy call verifies against a hash no ticket can produce (AG-46).
pub async fn clear_agent_run_ticket(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET ticket_hash = '' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Adds one step's token use to the run's budget counter (AG-44).
pub async fn add_agent_run_usage(
    pool: &PgPool,
    id: &str,
    tokens: i64,
    steps: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET tokens_used = tokens_used + $2, steps = steps + $3 WHERE id = $1",
    )
    .bind(id)
    .bind(tokens)
    .bind(steps)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Records where the built application can be looked at (AP-46) and, the first time, how long
/// it took to get there (AP-57).
pub async fn set_agent_run_preview_url(
    pool: &PgPool,
    id: &str,
    url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET preview_url = $2, first_frame_ms = COALESCE(first_frame_ms, \
         (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint) WHERE id = $1",
    )
    .bind(id)
    .bind(url)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Records when the first generated version of an application was served.
pub async fn record_agent_run_first_version(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET first_version_ms = COALESCE(first_version_ms, \
         (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint) WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Keeps the files a run wrote (AP-56).
pub async fn set_agent_run_files(
    pool: &PgPool,
    id: &str,
    files: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET files = $2 WHERE id = $1")
        .bind(id)
        .bind(files)
        .execute(pool)
        .await
        .map(|_| ())
}

/// The application's name for people (AP-44).
pub async fn set_agent_run_title(pool: &PgPool, id: &str, title: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET title = $2 WHERE id = $1")
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .map(|_| ())
}

/// The endpoints a conversation reads from now on, the primary first (AG-75).
pub async fn set_agent_run_endpoints(
    pool: &PgPool,
    id: &str,
    endpoints: &serde_json::Value,
    primary_name: &str,
    primary_slug: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET endpoints = $2, endpoint_name = $3, endpoint_slug = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(endpoints)
    .bind(primary_name)
    .bind(primary_slug)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Records the merge request a publish opened (AP-55).
pub async fn set_agent_run_merge_request(
    pool: &PgPool,
    id: &str,
    number: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET merge_request = $2 WHERE id = $1")
        .bind(id)
        .bind(number)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Appends one event and answers with the sequence number it got (AG-45).
///
/// The number is allocated inside the statement, so a reader that asked for everything after
/// `seq` never sees a gap fill in behind it. Two writers can still pick the same number — the
/// Portal's own status events and the proxy's relay are separate callers — and the primary key
/// is what catches that; the retry is the whole handling, because the loser only needs the next
/// number.
pub async fn append_agent_run_event(
    pool: &PgPool,
    run_id: &str,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<AgentRunEvent, sqlx::Error> {
    let statement = format!(
        "INSERT INTO agent_run_events (run_id, seq, kind, payload) \
         SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3 FROM agent_run_events WHERE run_id = $1 \
         RETURNING {AGENT_EVENT_COLUMNS}"
    );
    let mut last = None;
    for _ in 0..3 {
        match sqlx::query_as::<_, AgentRunEvent>(AssertSqlSafe(statement.clone()))
            .bind(run_id)
            .bind(kind)
            .bind(payload)
            .fetch_one(pool)
            .await
        {
            Ok(event) => return Ok(event),
            Err(err) if is_unique_violation(&err) => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or(sqlx::Error::RowNotFound))
}

/// Everything a stream missed, oldest first (`Last-Event-ID`, AG-45).
pub async fn load_agent_run_events(
    pool: &PgPool,
    run_id: &str,
    after_seq: i64,
) -> Result<Vec<AgentRunEvent>, sqlx::Error> {
    sqlx::query_as::<_, AgentRunEvent>(AssertSqlSafe(format!(
        "SELECT {AGENT_EVENT_COLUMNS} FROM agent_run_events WHERE run_id = $1 AND seq > $2 \
         ORDER BY seq"
    )))
    .bind(run_id)
    .bind(after_seq)
    .fetch_all(pool)
    .await
}

/// The events of several runs of the given kinds, by run and in order: what the agents' activity
/// reads in one query (T-4851).
pub async fn load_agent_run_events_of(
    pool: &PgPool,
    run_ids: &[String],
    kinds: &[&str],
) -> Result<Vec<AgentRunEvent>, sqlx::Error> {
    sqlx::query_as::<_, AgentRunEvent>(AssertSqlSafe(format!(
        "SELECT {AGENT_EVENT_COLUMNS} FROM agent_run_events WHERE run_id = ANY($1) \
         AND kind = ANY($2) ORDER BY run_id, seq"
    )))
    .bind(run_ids)
    .bind(kinds)
    .fetch_all(pool)
    .await
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// A person whose Keycloak user is deleted once the Change that removes them from every `Group`
/// and `RoleBinding` is merged (PF-93, T-2683).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PersonDeletion {
    pub person_id: String,
    pub pull_request: i64,
    pub change_name: String,
}

/// Records a pending removal; `false` when one is already pending for this person.
pub async fn insert_person_deletion(
    pool: &PgPool,
    person_id: &str,
    pull_request: u64,
    change_name: &str,
    requested_by: &str,
) -> Result<bool, sqlx::Error> {
    let pull_request = i64::try_from(pull_request).unwrap_or(i64::MAX);
    let inserted = sqlx::query(
        "INSERT INTO person_deletions (person_id, pull_request, change_name, requested_by) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (person_id) DO NOTHING",
    )
    .bind(person_id)
    .bind(pull_request)
    .bind(change_name)
    .bind(requested_by)
    .execute(pool)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

pub async fn person_deletion(
    pool: &PgPool,
    person_id: &str,
) -> Result<Option<PersonDeletion>, sqlx::Error> {
    sqlx::query_as::<_, PersonDeletion>(
        "SELECT person_id, pull_request, change_name FROM person_deletions WHERE person_id = $1",
    )
    .bind(person_id)
    .fetch_optional(pool)
    .await
}

pub async fn person_deletions(pool: &PgPool) -> Result<Vec<PersonDeletion>, sqlx::Error> {
    sqlx::query_as::<_, PersonDeletion>(
        "SELECT person_id, pull_request, change_name FROM person_deletions ORDER BY requested_at",
    )
    .fetch_all(pool)
    .await
}

/// Remembers when the link of the invitation just sent to `person_id` expires, and the project
/// it leads into; sent again without one, it keeps the project it had (PF-108).
pub async fn set_person_invitation(
    pool: &PgPool,
    person_id: &str,
    expires_at: OffsetDateTime,
    project: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO person_invitations (person_id, expires_at, project) VALUES ($1, $2, $3) \
         ON CONFLICT (person_id) DO UPDATE SET expires_at = EXCLUDED.expires_at, \
         project = COALESCE(EXCLUDED.project, person_invitations.project)",
    )
    .bind(person_id)
    .bind(expires_at)
    .bind(project)
    .execute(pool)
    .await
    .map(drop)
}

/// The invitation expiries of the given people, by id; a person the Portal sent none is absent.
pub async fn person_invitations(
    pool: &PgPool,
    person_ids: &[String],
) -> Result<std::collections::HashMap<String, OffsetDateTime>, sqlx::Error> {
    let rows: Vec<(String, OffsetDateTime)> = sqlx::query_as(
        "SELECT person_id, expires_at FROM person_invitations WHERE person_id = ANY($1)",
    )
    .bind(person_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The project the invitation of `person_id` leads into, when it names one.
pub async fn person_invitation_project(
    pool: &PgPool,
    person_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT project FROM person_invitations WHERE person_id = $1",
    )
    .bind(person_id)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

pub async fn remove_person_invitation(pool: &PgPool, person_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM person_invitations WHERE person_id = $1")
        .bind(person_id)
        .execute(pool)
        .await
        .map(drop)
}

pub async fn remove_person_deletion(pool: &PgPool, person_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM person_deletions WHERE person_id = $1")
        .bind(person_id)
        .execute(pool)
        .await
        .map(drop)
}

#[cfg(test)]
mod rollback_tests {
    use super::tolerate_newer;

    /// A rollback boots on a newer release's compatible migrations and stops on a breaking
    /// one, naming its version (T-5105).
    #[test]
    fn a_rollback_tolerates_compatible_migrations_and_stops_on_a_breaking_one() {
        assert_eq!(tolerate_newer(38, &[]).ok(), Some(false));
        let compatible = [(39, "space notes".to_owned())];
        assert_eq!(tolerate_newer(38, &compatible).ok(), Some(true));
        let breaking = [
            (39, "space notes".to_owned()),
            (40, "breaking drop names".to_owned()),
        ];
        assert_eq!(
            tolerate_newer(38, &breaking).err().map(|e| e.to_string()),
            Some("migration 40 is newer than the latest applied migration 38".to_owned())
        );
    }

    /// The lease sweep and the activity trim each have an index the planner takes (F-POR1-026,
    /// F-POR1-027): the sweep's own query plans on the partial index.
    #[tokio::test]
    async fn the_lease_sweep_and_the_activity_trim_read_an_index() {
        let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = super::connect(&url).await.expect("migrate");
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT indexname::text FROM pg_indexes WHERE indexname IN \
             ('agent_runs_live_expires', 'activity_time') ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .expect("indexes");
        assert_eq!(names, ["activity_time", "agent_runs_live_expires"]);
        let mut tx = pool.begin().await.expect("begin");
        sqlx::query("SET LOCAL enable_seqscan = off")
            .execute(&mut *tx)
            .await
            .expect("set");
        let plan: Vec<String> = sqlx::query_scalar(
            "EXPLAIN SELECT id FROM agent_runs \
             WHERE status NOT IN ('published', 'failed', 'cancelled', 'expired') \
             AND expires_at < now() ORDER BY expires_at ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("explain");
        assert!(
            plan.iter()
                .any(|line| line.contains("agent_runs_live_expires")),
            "{plan:?}"
        );
    }

    /// The same on a live database: the boot accepts a compatible migration of a newer release
    /// and stops on a breaking one. A scratch database of its own, so no parallel test boots on
    /// the rows it plants.
    #[tokio::test]
    async fn a_live_rollback_boots_on_a_compatible_migration_and_not_on_a_breaking_one() {
        let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let admin = sqlx::PgPool::connect(&url).await.expect("connect");
        let name = format!("jc_rollback_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name}"
        )))
        .execute(&admin)
        .await
        .expect("drop");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create");
        let mut scratch = url::Url::parse(&url).expect("url");
        scratch.set_path(&name);
        let db = super::connect(scratch.as_str()).await.expect("migrate");
        let newest: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&db)
            .await
            .expect("newest");
        let plant = |v: i64, d: &'static str| {
            sqlx::query(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, \
                 execution_time) VALUES ($1, $2, true, '\\x00', 0)",
            )
            .bind(v)
            .bind(d)
        };
        plant(newest + 1, "space notes")
            .execute(&db)
            .await
            .expect("plant");
        let compatible = super::connect(scratch.as_str())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        plant(newest + 2, "breaking drop names")
            .execute(&db)
            .await
            .expect("plant");
        let breaking = super::connect(scratch.as_str())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        db.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop");
        assert_eq!(compatible, Ok(()));
        assert_eq!(
            breaking,
            Err(format!(
                "migration {} is newer than the latest applied migration {newest}",
                newest + 2
            ))
        );
    }
}

#[cfg(test)]
mod pool_tests {
    use super::pool_options;
    use std::time::Duration;

    /// The pool gives idle backends back to Postgres after a minute (T-6201).
    #[test]
    fn the_pool_releases_idle_connections_after_a_minute() {
        let options = pool_options();
        assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(60)));
        assert_eq!(options.get_min_connections(), 0);
        assert_eq!(options.get_max_connections(), 5);
    }
}
