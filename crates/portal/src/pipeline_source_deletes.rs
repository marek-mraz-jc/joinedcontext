//! A snapshot pipeline follows what its source removed (PL-97, PL-100, T-4891).
//!
//! The runner's outcome sink tells the Portal every id a run wrote; for a pipeline whose
//! `spec.sourceDeletes.mode` is `snapshot` each id goes into the pipeline's source ledger with the
//! run that read it. When the run queue sees a run end, [`run_ended`] judges it: a run that did not
//! finish, failed on a record or read nothing deletes nothing and counts as no miss; a complete run
//! adds a miss to every ledger id it did not read and clears the misses of the ones it did. The ids
//! missing from `misses` complete runs in a row are deleted, unless they are more than `maxShare`
//! of the ledger: then nothing goes, the run is held and the pipeline's subscribers are alerted
//! until a person with `approve` on Pipeline releases or keeps them.
//!
//! The Portal never deletes itself. It hands the runner a one-shot stream that deletes the ids
//! through the pipeline's output Endpoint as the pipeline's own account, with the reason in
//! `JC-Reason`, so the gateway judges, audits and tombstones every delete like a write (DM-115);
//! the stream reports what went back here and only those rows leave the ledger.
//!
//! An `events` pipeline (PL-98, T-5360) feeds the same ledger. Its stream reads `deletedWhen` on
//! each mapped record and posts the ids it marked to [`marked_at_source`]; only ids in the ledger
//! are kept, and they go in one delete stream when the run ends, or every [`SWEEP`] for a pipeline
//! that waits for pushes and has no runs. Before any delete the stream reads each entity's
//! `modifiedAt`: one someone changed after the pipeline last wrote it stays and is reported
//! (PL-100).

use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use std::time::Duration;

use jc_core::kinds::{SourceDeleteMode, SourceDeletes};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::AppState;

/// The most ids one delete stream carries; the rest stay doomed and go after the next complete
/// run. ponytail: 5000 per run; page the stream when a source drops more at once.
pub const MAX_PER_STREAM: usize = 5000;
/// Most ids one chunk's batch delete names: the gateway's `MAX_DELETE_IDS` (Antares'
/// `ANTARES_MAX_BATCH_ITEMS`).
const DELETE_IDS: usize = 1000;
/// Most bytes of a chunk's ids joined by `,`: the broker refuses a Query whose id list passes
/// `MAX_URI_BYTES` (antares-api bounds.rs, 8192) with 400.
const QUERY_ID_BYTES: usize = 8192;
/// How long a delete stream is left on the runner before it is removed.
const STREAM_WINDOW: Duration = Duration::from_secs(120);
/// The longest entity id the ledger keeps: the longest Antares stores (antares-model `id.rs`
/// `MAX_ENTITY_ID_BYTES`), so every Entity a pipeline wrote can be swept. A ledger key of this id
/// with 253-byte project and pipeline names still fits a btree row (2,704 B on an 8 KB page).
const MAX_ID: usize = 2048;
/// How often the marks of a pipeline without runs (a push source) go to their delete stream.
pub const SWEEP: Duration = Duration::from_secs(300);
/// Seconds a `modifiedAt` may pass the pipeline's last write before it counts as someone else's
/// change: the write is confirmed a moment after the broker stamps it.
const EDIT_SLACK: u64 = 5;

/// Why a delete stream deletes: a snapshot run missed the id, or the source marked it (PL-100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Snapshot,
    Event,
}

impl Cause {
    fn as_str(self) -> &'static str {
        match self {
            Cause::Snapshot => "source-snapshot",
            Cause::Event => "source-event",
        }
    }

    fn parse(text: &str) -> Self {
        if text == "source-event" {
            Cause::Event
        } else {
            Cause::Snapshot
        }
    }

    /// The cause of a pipeline's deletes, from its spec.
    fn of(spec: &Value) -> Self {
        if events_rule(spec).is_some() {
            Cause::Event
        } else {
            Cause::Snapshot
        }
    }
}

/// Why a run deleted nothing and alerts the pipeline's subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HoldReason {
    /// It would delete more than `maxShare` of the ledger.
    ShareExceeded,
    /// It finished having read nothing: an outage that answers an empty list.
    Empty,
    /// It failed on a record or its source refused a request.
    Incomplete,
}

impl HoldReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ShareExceeded => "share-exceeded",
            Self::Empty => "empty",
            Self::Incomplete => "incomplete",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "share-exceeded" => Self::ShareExceeded,
            "empty" => Self::Empty,
            _ => Self::Incomplete,
        }
    }
}

/// A run that deleted nothing, as the pipeline's page and its alert say it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineSourceHold)]
pub struct Hold {
    pub run: String,
    pub reason: HoldReason,
    /// The ids the run would have deleted.
    pub would: usize,
    /// The ids in the ledger.
    pub ledger: usize,
}

impl Hold {
    /// The alert's words.
    pub fn detail(&self) -> String {
        match self.reason {
            HoldReason::ShareExceeded => format!(
                "source deletes held: run {} would delete {} of {} entities, more than the \
                 share the pipeline allows; release or keep them on the pipeline's page",
                self.run, self.would, self.ledger
            ),
            HoldReason::Empty => format!(
                "source deletes held: run {} read no record, so nothing was deleted",
                self.run
            ),
            HoldReason::Incomplete => format!(
                "source deletes held: run {} did not read its source completely, so nothing was \
                 deleted",
                self.run
            ),
        }
    }
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub last_run: String,
    pub misses: u8,
}

/// A snapshot rule the runtime may act on: `None` for any other mode and for numbers outside
/// PL-96's bounds, which the reconciler would otherwise read unchecked (a `misses` of 0 deletes
/// what the run just read).
pub fn snapshot_rule(spec: &Value) -> Option<SourceDeletes> {
    let rule: SourceDeletes = serde_json::from_value(spec.get("sourceDeletes")?.clone()).ok()?;
    (rule.mode == SourceDeleteMode::Snapshot
        && (1..=jc_core::kinds::pipeline_source_deletes::MISSES_MAX).contains(&rule.misses())
        && (1..=100).contains(&rule.max_share()))
    .then_some(rule)
}

/// The `deletedWhen` of an `events` pipeline (PL-98); `None` for any other mode.
pub fn events_rule(spec: &Value) -> Option<String> {
    let rule: SourceDeletes = serde_json::from_value(spec.get("sourceDeletes")?.clone()).ok()?;
    if rule.mode != SourceDeleteMode::Events {
        return None;
    }
    rule.deleted_when.filter(|query| !query.trim().is_empty())
}

/// Whether the pipeline keeps a source ledger: a snapshot or an events pipeline.
fn ledgered(spec: &Value) -> bool {
    snapshot_rule(spec).is_some() || events_rule(spec).is_some()
}

/// What a complete run decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Delete(Vec<String>),
    Held(Hold),
}

/// Judges a complete run against the ledger: every row's new misses, and the ids to delete or
/// the hold. A run that read none of the ledger is held as empty with the misses unchanged.
pub fn judge(
    rows: &BTreeMap<String, Row>,
    run: &str,
    rule: &SourceDeletes,
) -> (Vec<(String, u8)>, Verdict) {
    let hold = |reason, would| {
        Verdict::Held(Hold {
            run: run.to_owned(),
            reason,
            would,
            ledger: rows.len(),
        })
    };
    if !rows.is_empty() && rows.values().all(|row| row.last_run != run) {
        return (Vec::new(), hold(HoldReason::Empty, 0));
    }
    let misses: Vec<(String, u8)> = rows
        .iter()
        .map(|(id, row)| {
            let missed = if row.last_run == run {
                0
            } else {
                row.misses.saturating_add(1)
            };
            (id.clone(), missed)
        })
        .collect();
    let doomed: Vec<String> = misses
        .iter()
        .filter(|(_, missed)| *missed >= rule.misses())
        .map(|(id, _)| id.clone())
        .collect();
    if doomed.len() * 100 > usize::from(rule.max_share()) * rows.len() {
        return (misses, hold(HoldReason::ShareExceeded, doomed.len()));
    }
    (misses, Verdict::Delete(doomed))
}

/// Whether an id may be put in a delete: a URI the gateway can name, nothing a stream could read
/// as anything else.
pub fn deletable(id: &str) -> bool {
    id.len() <= MAX_ID
        && id.contains(':')
        && id
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'$' | b'{' | b'}'))
}

type Key = (String, String);

#[derive(Default)]
struct Book {
    rows: BTreeMap<String, Row>,
    hold: Option<Hold>,
    /// The last complete run and when it was judged.
    complete: Option<(String, String)>,
    /// What the pipeline deleted, oldest first, at most `REMOVALS_KEPT`.
    removed: Vec<(String, Removal)>,
    /// The ids the source marked as deleted, with the run that marked them (PL-98).
    marked: BTreeMap<String, String>,
    /// When the pipeline last wrote each id, unix seconds (PL-100).
    written: HashMap<String, i64>,
}

enum Inner {
    /// Every statement runs in a `db::tenant_tx` of its project's key (T-7198 PK-1f).
    Postgres(sqlx::PgPool, std::sync::Arc<crate::store::Mirror>),
    Memory(RwLock<HashMap<Key, Book>>),
}

/// The ledgers and holds of every snapshot pipeline: durable with a database, in memory without.
pub struct Ledger {
    inner: Inner,
}

impl Ledger {
    pub fn new(db: Option<sqlx::PgPool>, mirror: std::sync::Arc<crate::store::Mirror>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool, mirror),
                None => Inner::Memory(RwLock::new(HashMap::new())),
            },
        }
    }

    fn memory(&self) -> Option<std::sync::RwLockWriteGuard<'_, HashMap<Key, Book>>> {
        match &self.inner {
            Inner::Memory(map) => Some(map.write().unwrap_or_else(|e| e.into_inner())),
            Inner::Postgres(..) => None,
        }
    }

    /// Notes that `run` read these ids: known ones keep their misses until the run is judged, and
    /// new ones join the ledger when the pipeline `wrote` them; a record the model refused only
    /// keeps a known row alive, since an id the pipeline never wrote is never its to delete.
    pub async fn seen(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        ids: &[String],
        wrote: bool,
    ) -> Result<(), sqlx::Error> {
        let ids: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| deletable(id))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let book = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                for id in ids {
                    if wrote {
                        book.rows.entry(id.to_owned()).or_insert(Row {
                            last_run: String::new(),
                            misses: 0,
                        });
                    }
                    if let Some(row) = book.rows.get_mut(id) {
                        row.last_run = run.to_owned();
                        if wrote {
                            book.written
                                .insert(id.to_owned(), jc_time::TimestampTz::now().as_second());
                        }
                    }
                }
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        let sql = if wrote {
            "INSERT INTO pipeline_source_ledger (project, pipeline, entity_id, last_run, \
             last_written) SELECT $1, $2, i, $3, now() FROM UNNEST($4::text[]) AS t(i) \
             ON CONFLICT (project, pipeline, entity_id) DO UPDATE SET last_run = $3, \
             last_seen = now(), last_written = now()"
        } else {
            "UPDATE pipeline_source_ledger SET last_run = $3, last_seen = now() \
             WHERE project = $1 AND pipeline = $2 AND entity_id = ANY($4::text[])"
        };
        sqlx::query(sql)
            .bind(project)
            .bind(pipeline)
            .bind(run)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The pipeline's ledger and its hold.
    pub async fn book(
        &self,
        project: &str,
        pipeline: &str,
    ) -> Result<(BTreeMap<String, Row>, Option<Hold>), sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok((BTreeMap::new(), None));
            };
            let rows: Vec<(String, String, i16)> = sqlx::query_as(
                "SELECT entity_id, last_run, misses FROM pipeline_source_ledger \
                 WHERE project = $1 AND pipeline = $2",
            )
            .bind(project)
            .bind(pipeline)
            .fetch_all(&mut *tx)
            .await?;
            let hold: Option<(String, String, i32, i32)> = sqlx::query_as(
                "SELECT run, reason, would, ledger FROM pipeline_source_holds \
                 WHERE project = $1 AND pipeline = $2",
            )
            .bind(project)
            .bind(pipeline)
            .fetch_optional(&mut *tx)
            .await?;
            let rows = rows
                .into_iter()
                .map(|(id, last_run, misses)| {
                    let misses = u8::try_from(misses.max(0)).unwrap_or(u8::MAX);
                    (id, Row { last_run, misses })
                })
                .collect();
            let hold = hold.map(|(run, reason, would, ledger)| Hold {
                run,
                reason: HoldReason::parse(&reason),
                would: usize::try_from(would).unwrap_or_default(),
                ledger: usize::try_from(ledger).unwrap_or_default(),
            });
            tx.commit().await?;
            return Ok((rows, hold));
        }
        let map = self.memory();
        let book = map
            .as_ref()
            .and_then(|map| map.get(&(project.to_owned(), pipeline.to_owned())));
        Ok(book.map_or_else(Default::default, |book| {
            (book.rows.clone(), book.hold.clone())
        }))
    }

    /// Writes a judged run: the new misses, and the hold set or cleared.
    pub async fn settle(
        &self,
        project: &str,
        pipeline: &str,
        misses: &[(String, u8)],
        hold: Option<&Hold>,
    ) -> Result<(), sqlx::Error> {
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let book = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                for (id, missed) in misses {
                    if let Some(row) = book.rows.get_mut(id) {
                        row.misses = *missed;
                    }
                }
                book.hold = hold.cloned();
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        let (ids, counts): (Vec<&str>, Vec<i16>) = misses
            .iter()
            .map(|(id, missed)| (id.as_str(), i16::from(*missed)))
            .unzip();
        sqlx::query(
            "UPDATE pipeline_source_ledger AS l SET misses = t.m \
             FROM UNNEST($3::text[], $4::smallint[]) AS t(i, m) \
             WHERE l.project = $1 AND l.pipeline = $2 AND l.entity_id = t.i",
        )
        .bind(project)
        .bind(pipeline)
        .bind(&ids)
        .bind(&counts)
        .execute(&mut *tx)
        .await?;
        set_hold(&mut tx, project, pipeline, hold).await?;
        tx.commit().await
    }

    /// Takes deleted ids out of the ledger.
    pub async fn forget(
        &self,
        project: &str,
        pipeline: &str,
        ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                if let Some(book) = map.get_mut(&(project.to_owned(), pipeline.to_owned())) {
                    ids.iter().for_each(|id| {
                        book.rows.remove(id);
                        book.marked.remove(id);
                        book.written.remove(id);
                    });
                }
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        sqlx::query(
            "DELETE FROM pipeline_source_ledger WHERE project = $1 AND pipeline = $2 \
             AND entity_id = ANY($3::text[])",
        )
        .bind(project)
        .bind(pipeline)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// "Keep": every id counts as read again and the hold is cleared.
    pub async fn keep(&self, project: &str, pipeline: &str) -> Result<(), sqlx::Error> {
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                if let Some(book) = map.get_mut(&(project.to_owned(), pipeline.to_owned())) {
                    book.rows.values_mut().for_each(|row| row.misses = 0);
                    book.hold = None;
                }
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        sqlx::query(
            "UPDATE pipeline_source_ledger SET misses = 0 WHERE project = $1 AND pipeline = $2",
        )
        .bind(project)
        .bind(pipeline)
        .execute(&mut *tx)
        .await?;
        set_hold(&mut tx, project, pipeline, None).await?;
        tx.commit().await
    }

    /// Marks the ids `run`'s source said are deleted (PL-98) and answers the ones the ledger holds:
    /// only those may go, since the pipeline never wrote the others.
    pub async fn mark(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        ids: &[String],
    ) -> Result<Vec<String>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok(Vec::new());
            };
            let answer = sqlx::query_scalar(
                "UPDATE pipeline_source_ledger SET marked_run = $3 \
                 WHERE project = $1 AND pipeline = $2 AND entity_id = ANY($4::text[]) \
                 RETURNING entity_id",
            )
            .bind(project)
            .bind(pipeline)
            .bind(run)
            .bind(ids)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(answer);
        }
        let mut map = self.memory();
        let Some(book) = map
            .as_mut()
            .and_then(|map| map.get_mut(&(project.to_owned(), pipeline.to_owned())))
        else {
            return Ok(Vec::new());
        };
        let held: Vec<String> = ids
            .iter()
            .filter(|id| book.rows.contains_key(*id))
            .cloned()
            .collect();
        for id in &held {
            book.marked.insert(id.clone(), run.to_owned());
        }
        Ok(held)
    }

    /// Takes the marks of the pipeline's oldest marked run, at most [`MAX_PER_STREAM`]: the run
    /// and its ids, which are no longer marked once taken.
    pub async fn take_marked(
        &self,
        project: &str,
        pipeline: &str,
    ) -> Result<Option<(String, Vec<String>)>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok(None);
            };
            let rows: Vec<(String, String)> = sqlx::query_as(
                "WITH oldest AS (SELECT min(marked_run) AS run FROM pipeline_source_ledger \
                 WHERE project = $1 AND pipeline = $2 AND marked_run IS NOT NULL), \
                 taken AS (SELECT l.entity_id FROM pipeline_source_ledger l, oldest \
                 WHERE l.project = $1 AND l.pipeline = $2 AND l.marked_run = oldest.run \
                 ORDER BY l.entity_id LIMIT $3 FOR UPDATE OF l) \
                 UPDATE pipeline_source_ledger l SET marked_run = NULL FROM taken, oldest \
                 WHERE l.project = $1 AND l.pipeline = $2 AND l.entity_id = taken.entity_id \
                 RETURNING oldest.run, l.entity_id",
            )
            .bind(project)
            .bind(pipeline)
            .bind(i64::try_from(MAX_PER_STREAM).unwrap_or(i64::MAX))
            .fetch_all(&mut *tx)
            .await?;
            let run = rows.first().map(|(run, _)| run.clone());
            tx.commit().await?;
            return Ok(run.map(|run| (run, rows.into_iter().map(|(_, id)| id).collect())));
        }
        let mut map = self.memory();
        let Some(book) = map
            .as_mut()
            .and_then(|map| map.get_mut(&(project.to_owned(), pipeline.to_owned())))
        else {
            return Ok(None);
        };
        let Some(run) = book.marked.values().min().cloned() else {
            return Ok(None);
        };
        let ids: Vec<String> = book
            .marked
            .iter()
            .filter(|(_, at)| **at == run)
            .map(|(id, _)| id.clone())
            .take(MAX_PER_STREAM)
            .collect();
        ids.iter().for_each(|id| {
            book.marked.remove(id);
        });
        Ok(Some((run, ids)))
    }

    /// The pipelines with marks waiting, for the sweep.
    pub async fn marking(&self) -> Result<Vec<(String, String)>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            // Project by project, each in its own key (STO-07).
            let mut marking = Vec::new();
            for (project, key) in crate::project_ids::keyed_projects(pool, mirror).await? {
                let mut tx = crate::db::tenant_tx(pool, key).await?;
                marking.extend(
                    sqlx::query_as::<_, (String, String)>(
                        "SELECT DISTINCT project, pipeline FROM pipeline_source_ledger \
                         WHERE project = $1 AND marked_run IS NOT NULL",
                    )
                    .bind(&project)
                    .fetch_all(&mut *tx)
                    .await?,
                );
                tx.commit().await?;
            }
            return Ok(marking);
        }
        let map = self.memory();
        Ok(map
            .iter()
            .flat_map(|map| map.iter())
            .filter(|(_, book)| !book.marked.is_empty())
            .map(|((project, pipeline), _)| (project.clone(), pipeline.clone()))
            .collect())
    }

    /// When the pipeline last wrote each of `ids`, unix seconds; an id it never confirmed a write
    /// of is missing and counts as changed by someone else.
    pub async fn written(
        &self,
        project: &str,
        pipeline: &str,
        ids: &[String],
    ) -> Result<HashMap<String, i64>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok(HashMap::new());
            };
            let rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT entity_id, EXTRACT(EPOCH FROM COALESCE(last_written, last_seen))::bigint \
                 FROM pipeline_source_ledger WHERE project = $1 AND pipeline = $2 \
                 AND entity_id = ANY($3::text[])",
            )
            .bind(project)
            .bind(pipeline)
            .bind(ids)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(rows.into_iter().collect());
        }
        let map = self.memory();
        let book = map
            .as_ref()
            .and_then(|map| map.get(&(project.to_owned(), pipeline.to_owned())));
        Ok(ids
            .iter()
            .filter_map(|id| Some((id.clone(), *book?.written.get(id)?)))
            .collect())
    }

    /// Every hold, for the alerts.
    pub async fn holds(&self) -> Result<Vec<(String, String, Hold)>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            // Project by project, each in its own key (STO-07).
            let mut rows: Vec<(String, String, String, String, i32, i32)> = Vec::new();
            for (project, key) in crate::project_ids::keyed_projects(pool, mirror).await? {
                let mut tx = crate::db::tenant_tx(pool, key).await?;
                rows.extend(
                    sqlx::query_as::<_, (String, String, String, String, i32, i32)>(
                        "SELECT project, pipeline, run, reason, would, ledger \
                         FROM pipeline_source_holds WHERE project = $1",
                    )
                    .bind(&project)
                    .fetch_all(&mut *tx)
                    .await?,
                );
                tx.commit().await?;
            }
            return Ok(rows
                .into_iter()
                .map(|(project, pipeline, run, reason, would, ledger)| {
                    let hold = Hold {
                        run,
                        reason: HoldReason::parse(&reason),
                        would: usize::try_from(would).unwrap_or_default(),
                        ledger: usize::try_from(ledger).unwrap_or_default(),
                    };
                    (project, pipeline, hold)
                })
                .collect());
        }
        let map = self.memory();
        Ok(map
            .iter()
            .flat_map(|map| map.iter())
            .filter_map(|((project, pipeline), book)| {
                let hold = book.hold.clone()?;
                Some((project.clone(), pipeline.clone(), hold))
            })
            .collect())
    }
}

/// One entity a snapshot pipeline deleted because its source no longer listed it (T-4892): the
/// tombstone's "removed at the source" as the pipeline's panel and the space say it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineSourceRemoval)]
pub struct Removal {
    pub id: String,
    pub pipeline: String,
    pub run: String,
    /// When the gateway confirmed the delete, RFC 3339.
    pub at: String,
    /// The `JC-Reason` the delete carried.
    pub reason: String,
}

/// The removals the memory ledger keeps per pipeline; the database keeps `REMOVED_DAYS`.
const REMOVALS_KEPT: usize = 1000;
/// How long a removal is remembered.
const REMOVED_DAYS: i32 = 400;

impl Ledger {
    /// Notes the last run that read the source completely (T-4892).
    pub async fn completed(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
    ) -> Result<(), sqlx::Error> {
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                map.entry((project.to_owned(), pipeline.to_owned()))
                    .or_default()
                    .complete = Some((run.to_owned(), jc_time::TimestampTz::now().to_rfc3339()));
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO pipeline_source_complete (project, pipeline, run) VALUES ($1, $2, $3) \
             ON CONFLICT (project, pipeline) DO UPDATE SET run = $3, at = now()",
        )
        .bind(project)
        .bind(pipeline)
        .bind(run)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The last complete run and when it was judged.
    pub async fn last_complete(
        &self,
        project: &str,
        pipeline: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok(None);
            };
            let row: Option<(String, time::OffsetDateTime)> = sqlx::query_as(
                "SELECT run, at FROM pipeline_source_complete WHERE project = $1 AND pipeline = $2",
            )
            .bind(project)
            .bind(pipeline)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(row.map(|(run, at)| (run, crate::pipeline_log::rfc3339(at))));
        }
        let map = self.memory();
        Ok(map
            .as_ref()
            .and_then(|map| map.get(&(project.to_owned(), pipeline.to_owned())))
            .and_then(|book| book.complete.clone()))
    }

    /// Remembers the ids a delete stream removed from `space`, with the run that decided it.
    pub async fn removed(
        &self,
        project: &str,
        space: &str,
        pipeline: &str,
        (run, cause): (&str, Cause),
        ids: &[String],
    ) -> Result<(), sqlx::Error> {
        if ids.is_empty() {
            return Ok(());
        }
        let (pool, mirror) = match &self.inner {
            Inner::Postgres(pool, mirror) => (pool, mirror),
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let book = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                let at = jc_time::TimestampTz::now().to_rfc3339();
                for id in ids {
                    book.removed.retain(|(_, removal)| &removal.id != id);
                    let removal = Removal {
                        id: id.clone(),
                        pipeline: pipeline.to_owned(),
                        run: run.to_owned(),
                        at: at.clone(),
                        reason: reason(cause, pipeline, run),
                    };
                    book.removed.push((space.to_owned(), removal));
                }
                let over = book.removed.len().saturating_sub(REMOVALS_KEPT);
                book.removed.drain(..over);
                return Ok(());
            }
        };
        let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO pipeline_source_removed (project, space, entity_id, pipeline, run, \
             cause) SELECT $1, $2, i, $3, $4, $6 FROM UNNEST($5::text[]) AS t(i) \
             ON CONFLICT (project, space, entity_id) DO UPDATE SET pipeline = $3, run = $4, \
             cause = $6, at = now()",
        )
        .bind(project)
        .bind(space)
        .bind(pipeline)
        .bind(run)
        .bind(ids)
        .bind(cause.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM pipeline_source_removed WHERE project = $1 \
             AND at < now() - make_interval(days => $2)",
        )
        .bind(project)
        .bind(REMOVED_DAYS)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// The newest removals, at most `limit`: of one pipeline, or of every pipeline in one space.
    pub async fn removals(
        &self,
        project: &str,
        of: RemovalsOf<'_>,
        limit: usize,
    ) -> Result<Vec<Removal>, sqlx::Error> {
        let (pipeline, space) = match of {
            RemovalsOf::Pipeline(name) => (Some(name), None),
            RemovalsOf::Space(name) => (None, Some(name)),
        };
        if let Inner::Postgres(pool, mirror) = &self.inner {
            let Some(mut tx) = crate::project_ids::tx_of(pool, mirror, project).await? else {
                return Ok(Vec::new());
            };
            let rows: Vec<(String, String, String, String, time::OffsetDateTime)> = sqlx::query_as(
                "SELECT entity_id, pipeline, run, cause, at FROM pipeline_source_removed \
                 WHERE project = $1 AND ($2::text IS NULL OR pipeline = $2) \
                 AND ($3::text IS NULL OR space = $3) ORDER BY at DESC, entity_id LIMIT $4",
            )
            .bind(project)
            .bind(pipeline)
            .bind(space)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(rows
                .into_iter()
                .map(|(id, pipeline, run, cause, at)| Removal {
                    reason: reason(Cause::parse(&cause), &pipeline, &run),
                    id,
                    pipeline,
                    run,
                    at: crate::pipeline_log::rfc3339(at),
                })
                .collect());
        }
        let map = self.memory();
        let mut found: Vec<Removal> = map
            .iter()
            .flat_map(|map| map.iter())
            .filter(|((p, name), _)| p == project && pipeline.is_none_or(|want| want == name))
            .flat_map(|(_, book)| book.removed.iter())
            .filter(|(at_space, _)| space.is_none_or(|want| want == at_space))
            .map(|(_, removal)| removal.clone())
            .collect();
        found.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        found.truncate(limit);
        Ok(found)
    }
}

/// Whose removals [`Ledger::removals`] lists.
#[derive(Debug, Clone, Copy)]
pub enum RemovalsOf<'a> {
    Pipeline(&'a str),
    Space(&'a str),
}

async fn set_hold(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project: &str,
    pipeline: &str,
    hold: Option<&Hold>,
) -> Result<(), sqlx::Error> {
    let Some(hold) = hold else {
        sqlx::query("DELETE FROM pipeline_source_holds WHERE project = $1 AND pipeline = $2")
            .bind(project)
            .bind(pipeline)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO pipeline_source_holds (project, pipeline, run, reason, would, ledger) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (project, pipeline) DO UPDATE SET \
         run = $3, reason = $4, would = $5, ledger = $6, at = now()",
    )
    .bind(project)
    .bind(pipeline)
    .bind(&hold.run)
    .bind(hold.reason.as_str())
    .bind(i32::try_from(hold.would).unwrap_or(i32::MAX))
    .bind(i32::try_from(hold.ledger).unwrap_or(i32::MAX))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// A run the queue saw end (PL-78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEnd {
    pub project: String,
    pub pipeline: String,
    pub run: String,
    /// The runner finished it; a timeout, a lost run or a refused start is `false`.
    pub finished: bool,
    /// The runner's own count of the run's outcome reports it could not deliver (F-POR3-006).
    pub reports: ReportCount,
    /// The [`stream_hash`](crate::reconciler::run_queue::stream_hash) of the stream it ran
    /// (T-5825).
    pub stream: String,
}

/// What the runner counted of a run's outcome reports that never reached the Portal: the
/// stream's `outcome_drop` counter at the run's end less the same counter at its start. The
/// runner counts it beside the report path, so a lost report cannot hide itself (F-POR3-006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportCount {
    /// Every report arrived.
    Complete,
    /// The runner dropped this many reports.
    Lost(u64),
    /// No count, and why: the runner did not answer, does not export the counter, restarted,
    /// or this Portal did not see the run start.
    Unknown(&'static str),
}

/// Feeds the ledger from one outcome report of a snapshot or events pipeline: what was written and
/// what the model refused both exist at the source.
pub async fn note_seen(
    state: &AppState,
    project: &str,
    pipeline: &str,
    run: &str,
    ids: &[String],
    wrote: bool,
) {
    let tracked = state
        .mirror
        .get(project, "Pipeline", pipeline)
        .is_some_and(|p| ledgered(&p.spec));
    if !tracked || ids.is_empty() {
        return;
    }
    if let Err(error) = state
        .source_ledger
        .seen(project, pipeline, run, ids, wrote)
        .await
    {
        tracing::warn!(project, pipeline, %error, "the source ledger missed a report's ids");
    }
}

/// Judges one ended run of a snapshot pipeline and starts its deletes (PL-97); for an events
/// pipeline, starts the deletes its source marked (PL-98), whether the run finished or not, since
/// each mark is the source's own word.
pub async fn run_ended(state: &AppState, end: RunEnd) {
    let RunEnd {
        project,
        pipeline,
        run,
        finished,
        reports,
        stream,
    } = end;
    // A restarted Portal compares a replace pipeline's stream with the one its newest run ran
    // (T-5825).
    if let Err(error) = state
        .pipeline_log
        .ran_stream(&project, &pipeline, &run, &stream)
        .await
    {
        tracing::warn!(%project, %pipeline, %run, %error, "the run's stream hash was not kept: a restart plans the pipeline by its clock");
    }
    let Some(spec) = state
        .mirror
        .get(&project, "Pipeline", &pipeline)
        .map(|p| p.spec)
    else {
        return;
    };
    if events_rule(&spec).is_some() {
        launch_marked(state, &project, &pipeline).await;
        return;
    }
    let Some(rule) = snapshot_rule(&spec) else {
        return;
    };
    if let Err(error) =
        judge_and_delete(state, &project, &pipeline, &run, finished, &reports, &rule).await
    {
        tracing::warn!(%project, %pipeline, %run, %error, "the run's source deletes were not judged");
    }
}

async fn judge_and_delete(
    state: &AppState,
    project: &str,
    pipeline: &str,
    run: &str,
    finished: bool,
    reports: &ReportCount,
    rule: &SourceDeletes,
) -> Result<(), sqlx::Error> {
    let ledger = &state.source_ledger;
    let (rows, _) = ledger.book(project, pipeline).await?;
    let logged = state
        .pipeline_log
        .runs(project, pipeline, crate::pipeline_log::RUNS_KEPT)
        .await?
        .into_iter()
        .find(|logged| logged.run == run);
    // A run whose reports the runner could not all deliver, or could not count, left ids it read
    // out of the ledger: it is judged as incomplete and never deletes on a partial count.
    let unreported = *reports != ReportCount::Complete;
    if unreported {
        tracing::warn!(
            project,
            pipeline,
            run,
            ?reports,
            "the run's outcome reports are not all in; its source deletes are held"
        );
    }
    // A run that did not finish or failed on a record read part of its source at best.
    if !finished || unreported || logged.as_ref().is_some_and(|logged| logged.failed > 0) {
        let hold = Hold {
            run: run.to_owned(),
            reason: HoldReason::Incomplete,
            would: 0,
            ledger: rows.len(),
        };
        return ledger.settle(project, pipeline, &[], Some(&hold)).await;
    }
    let rejected = logged.map_or(0, |logged| logged.rejected);
    crate::telemetry::pipeline_missing(
        project,
        pipeline,
        usize::try_from(rejected).unwrap_or(usize::MAX),
    );
    if rows.is_empty() {
        return ledger.completed(project, pipeline, run).await;
    }
    let (misses, verdict) = judge(&rows, run, rule);
    if !matches!(&verdict, Verdict::Held(hold) if hold.reason == HoldReason::Empty) {
        ledger.completed(project, pipeline, run).await?;
    }
    let orphans = misses.iter().filter(|(_, missed)| *missed > 0).count();
    crate::telemetry::pipeline_orphans(project, pipeline, orphans);
    match verdict {
        Verdict::Held(hold) => ledger.settle(project, pipeline, &misses, Some(&hold)).await,
        Verdict::Delete(doomed) => {
            ledger.settle(project, pipeline, &misses, None).await?;
            if !doomed.is_empty() {
                if let Err(reason) = launch(state, project, pipeline, run, &doomed).await {
                    tracing::warn!(project, pipeline, run, %reason, "the source deletes were not started; they go after the next complete run");
                }
            }
            Ok(())
        }
    }
}

/// Starts one delete stream for the marks of the pipeline's oldest marked run (PL-98). Marks a
/// stream could not take are put back for the next run end or sweep.
async fn launch_marked(state: &AppState, project: &str, pipeline: &str) {
    let taken = match state.source_ledger.take_marked(project, pipeline).await {
        Ok(Some(taken)) => taken,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(project, pipeline, %error, "the source's delete marks were not read");
            return;
        }
    };
    let (run, ids) = taken;
    if let Err(reason) = launch(state, project, pipeline, &run, &ids).await {
        tracing::warn!(project, pipeline, run, %reason, "the marked deletes were not started; they go with the next run");
        if let Err(error) = state
            .source_ledger
            .mark(project, pipeline, &run, &ids)
            .await
        {
            tracing::warn!(project, pipeline, %error, "the source's delete marks were lost");
        }
    }
}

/// Starts the marked deletes of every events pipeline that has no runs to end (PL-98): one whose
/// source waits for pushes, with neither a `period` nor a `schedule`. Called every [`SWEEP`].
pub async fn sweep_marks(state: &AppState) {
    let marking = match state.source_ledger.marking().await {
        Ok(marking) => marking,
        Err(error) => {
            tracing::warn!(%error, "the pipelines with delete marks were not read");
            return;
        }
    };
    for (project, pipeline) in marking {
        let Some(spec) = state
            .mirror
            .get(&project, "Pipeline", &pipeline)
            .map(|p| p.spec)
        else {
            continue;
        };
        let pushed = spec.get("period").is_none() && spec.get("schedule").is_none();
        if pushed && events_rule(&spec).is_some() {
            launch_marked(state, &project, &pipeline).await;
        }
    }
}

/// What an events stream posts for one message (PL-98): the ids `deletedWhen` marked and the
/// records it could not judge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marks {
    #[serde(default)]
    pub run: Option<String>,
    pub ids: Vec<String>,
    #[serde(default)]
    pub failed: Vec<Failed>,
}

/// Takes one post of an events stream: ids the ledger holds are marked for the run's delete
/// stream; any other id the pipeline never wrote, so it is reported on the activity feed and never
/// deleted; a record `deletedWhen` could not judge is a failed line of the run's log.
pub async fn marked_at_source(
    state: &AppState,
    project: &str,
    pipeline: &str,
    marks: Marks,
) -> Result<(), sqlx::Error> {
    let run = crate::pipeline_log::run_name(marks.run.as_deref());
    let ids: Vec<String> = marks.ids.into_iter().filter(|id| deletable(id)).collect();
    let held = state
        .source_ledger
        .mark(project, pipeline, &run, &ids)
        .await?;
    let lines: Vec<crate::pipeline_log::NewLine> = marks
        .failed
        .iter()
        .map(|failed| crate::pipeline_log::NewLine {
            record_id: failed.id.clone(),
            step: None,
            outcome: crate::pipeline_log::Outcome::Failed,
            message: format!(
                "deletedWhen: {}",
                crate::pipeline_outcomes::mask_text(&failed.message)
            ),
        })
        .collect();
    if !lines.is_empty() {
        state
            .pipeline_log
            .append(project, pipeline, &run, &lines)
            .await?;
    }
    let strangers: Vec<&String> = ids.iter().filter(|id| !held.contains(id)).collect();
    if strangers.is_empty() {
        return Ok(());
    }
    let event = crate::activity::ActivityEvent {
        time: jc_time::TimestampTz::now(),
        project: project.to_owned(),
        space: None,
        kind: "pipeline.source-delete".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "pipeline {pipeline}'s source marked {} records as deleted that the pipeline never wrote; nothing of them is deleted (run {run})",
            strangers.len()
        ),
        severity: "warning".to_owned(),
        correlation_id: None,
        details: json!({
            "object": format!("pipelines/{pipeline}"),
            "run": run,
            "notInLedger": strangers.iter().take(20).collect::<Vec<_>>(),
        }),
    };
    if let Err(error) = state.activity.append(&[event]).await {
        tracing::warn!(project, pipeline, %error, "the marks outside the ledger are not on the activity feed");
    }
    Ok(())
}

/// The ids whose misses reached the rule: what the next delete takes, and what a release deletes.
pub fn doomed(rows: &BTreeMap<String, Row>, rule: &SourceDeletes) -> Vec<String> {
    rows.iter()
        .filter(|(_, row)| row.misses >= rule.misses())
        .map(|(id, _)| id.clone())
        .collect()
}

/// The spec of the Endpoint the pipeline's first output writes through.
fn output_endpoint(state: &AppState, project: &str, spec: &Value) -> Option<Value> {
    let spec: jc_core::kinds::PipelineSpec = serde_json::from_value(spec.clone()).ok()?;
    let output = spec.outputs().into_iter().next()?;
    let endpoint = state
        .mirror
        .get(project, "Endpoint", output.target_endpoint.local_id())?;
    Some(endpoint.spec)
}

/// The Endpoint slug of the pipeline's first output.
fn target(state: &AppState, project: &str, spec: &Value) -> Option<String> {
    Some(
        output_endpoint(state, project, spec)?
            .get("slug")?
            .as_str()?
            .to_owned(),
    )
}

/// The space the pipeline's first output writes into.
pub fn space_of(state: &AppState, project: &str, spec: &Value) -> Option<String> {
    let endpoint = output_endpoint(state, project, spec)?;
    crate::api::assistant::ref_name(endpoint.get("contextSpaceRef")?)
}

/// Hands the runner the stream that deletes `ids` (PL-97, PL-98): through the output Endpoint, as
/// the pipeline's account, its answers reported to the Portal's internal listener. The cause
/// follows the pipeline's mode, and each id carries when the pipeline last wrote it (PL-100).
pub async fn launch(
    state: &AppState,
    project: &str,
    pipeline: &str,
    run: &str,
    ids: &[String],
) -> Result<usize, String> {
    let spec = state
        .mirror
        .get(project, "Pipeline", pipeline)
        .map(|p| p.spec)
        .ok_or("the pipeline is gone")?;
    let slug = target(state, project, &spec).ok_or("the pipeline writes through no Endpoint")?;
    let runner = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or("no pipeline runner is configured")?
        .replace("{project}", project)
        .trim_end_matches('/')
        .to_owned();
    let internal = state
        .config
        .pipeline_test_capture_url
        .as_deref()
        .ok_or("no internal listener is configured for the runner")?
        .trim_end_matches('/')
        .to_owned();
    let ids: Vec<String> = ids
        .iter()
        .filter(|id| deletable(id))
        .take(MAX_PER_STREAM)
        .cloned()
        .collect();
    let written = state
        .source_ledger
        .written(project, pipeline, &ids)
        .await
        .map_err(|error| error.to_string())?;
    let wrote: Vec<i64> = ids
        .iter()
        .map(|id| written.get(id).copied().unwrap_or(0))
        .collect();
    let report = format!("{internal}/internal/pipelines/{project}/{pipeline}/source-deletes");
    let mut config = render(
        project,
        pipeline,
        &slug,
        (run, Cause::of(&spec)),
        (&ids, &wrote),
        &report,
    );
    if state.config.pipeline_identity {
        crate::reconciler::streams::as_pipeline(
            &mut config,
            project,
            pipeline,
            state.config.pipeline_credential,
        );
    }
    let stream = delete_stream(&runner, project, pipeline);
    let http = crate::api::pipelines::runner_http(&state.config);
    match http.post(&stream).json(&config).send().await {
        Ok(answer) if answer.status().is_success() => {}
        Ok(answer) => return Err(format!("the runner answered {}", answer.status())),
        Err(error) => return Err(error.to_string()),
    }
    tokio::spawn(async move {
        tokio::time::sleep(STREAM_WINDOW).await;
        if let Err(error) = http.delete(&stream).send().await {
            tracing::warn!(%error, "a source-delete stream was not removed");
        }
    });
    Ok(ids.len())
}

/// The runner stream of one launch. Each launch has its own: a second launch inside the window
/// of the first (a run end, then a release) neither deletes the first's stream mid-run nor has
/// its own deleted by the first's timer, which would lose marks already taken from the ledger.
fn delete_stream(runner: &str, project: &str, pipeline: &str) -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    format!(
        "{runner}/streams/{}.source-deletes.{:016x}",
        crate::reconciler::streams::stream_id(project, pipeline),
        OsRng.next_u64()
    )
}

/// The reason a source delete names (PL-100): its cause, the pipeline and the run that decided it.
fn reason(cause: Cause, pipeline: &str, run: &str) -> String {
    format!("{}: pipeline {pipeline} run {run}", cause.as_str())
}

/// The most characters of a reason the gateway takes (DM-113).
pub(crate) const MAX_REASON: usize = 500;

/// `JC-Reason` as the gateway reads it: percent-encoded UTF-8, cut to [`MAX_REASON`] characters,
/// so a long pipeline or project name never turns the delete into `400 reason-required`.
pub(crate) fn reason_header(reason: &str) -> String {
    reason
        .chars()
        .take(MAX_REASON)
        .collect::<String>()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Splits `ids` into chunks whose comma-joined ids fit [`QUERY_ID_BYTES`] and whose count fits
/// [`DELETE_IDS`]; an id too long for the query alone is a chunk of its own (the ledger keeps
/// none that long, `MAX_ID`; three ids of `MAX_ID` fill a chunk).
fn chunk_ends(ids: &[String]) -> Vec<usize> {
    let mut ends = Vec::new();
    let (mut count, mut bytes) = (0usize, 0usize);
    for (i, id) in ids.iter().enumerate() {
        let joined = if count == 0 {
            id.len()
        } else {
            bytes + 1 + id.len()
        };
        if count > 0 && (count == DELETE_IDS || joined > QUERY_ID_BYTES) {
            ends.push(i);
            (count, bytes) = (1, id.len());
        } else {
            (count, bytes) = (count + 1, joined);
        }
    }
    if count > 0 {
        ends.push(ids.len());
    }
    ends
}

/// The delete stream: one message per chunk of ids ([`chunk_ends`]), each first read for its entities'
/// `modifiedAt` by a batch query (queryBatch, the read admission demands beside deleteBatch), so one someone changed after the pipeline last wrote it (`wrote`, unix seconds,
/// beside each id) is kept and reported as `edited` (PL-100); the rest is a batch delete on the
/// Endpoint, its answer reduced to what went, what was gone already and what the gateway
/// refused, and posted to `report`. A chunk whose read fails deletes nothing. The ids ride
/// base64-encoded, so no id is ever read as config.
pub fn render(
    project: &str,
    pipeline: &str,
    slug: &str,
    (run, cause): (&str, Cause),
    (ids, wrote): (&[String], &[i64]),
    report: &str,
) -> Value {
    use base64::Engine;
    let mut start = 0;
    let chunks: Vec<Value> = chunk_ends(ids)
        .into_iter()
        .map(|end| {
            let chunk = json!({ "run": run, "ids": &ids[start..end], "wrote": wrote.get(start..end).unwrap_or_default() });
            start = end;
            chunk
        })
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(Value::from(chunks).to_string());
    let oauth2 = crate::reconciler::streams::pipeline_oauth2(project);
    let entities = format!("${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1");
    let not_found = r#"e.error.status.or(0) == 404 || e.error.type.or("").string().has_suffix("ResourceNotFound")"#;
    json!({
        "input": { "generate": {
            "count": 1,
            "interval": "",
            "mapping": format!("root = \"{encoded}\".decode(\"base64\").parse_json()"),
        }},
        "pipeline": { "processors": [
            { "unarchive": { "format": "json_array" } },
            { "try": [
                { "branch": {
                    "request_map": "root = {\"type\": \"Query\", \"entities\": [{\"type\": \"*\", \"id\": this.ids}]}",
                    "processors": [{ "http": {
                        "url": format!("{entities}/entityOperations/query?options=sysAttrs,keyValues&limit={DELETE_IDS}"),
                        "verb": "POST",
                        "headers": { "Content-Type": "application/json", "Accept": "application/json" },
                        "oauth2": oauth2,
                        "timeout": "30s",
                        "retries": 1,
                    }}],
                    "result_map": "root.found = this.map_each(e -> {\"id\": e.id.or(\"\"), \"at\": e.modifiedAt.or(\"\").ts_unix().catch(0)})",
                }},
                { "mapping": format!(
                    "let found = this.found\n\
                     let wrote = this.wrote\n\
                     let edited = this.ids.enumerated().filter(p -> $found.any(f -> f.id == p.value && f.at > $wrote.index(p.index).or(0) + {EDIT_SLACK})).map_each(p -> p.value)\n\
                     root = this.without(\"found\", \"wrote\")\n\
                     root.edited = $edited\n\
                     root.ids = this.ids.filter(i -> !$edited.contains(i))"
                ) },
                { "branch": {
                    "request_map": "root = if this.ids.length() == 0 { deleted() } else { this.ids }",
                    "processors": [{ "http": {
                        "url": format!("{entities}/entityOperations/delete"),
                        "verb": "POST",
                        "headers": {
                            "Content-Type": "application/json",
                            "Accept": "application/json",
                            "JC-Reason": reason_header(&reason(cause, pipeline, run)),
                        },
                        "oauth2": oauth2,
                        "timeout": "30s",
                        "retries": 1,
                    }}],
                    "result_map": "root.answer = if content().length() == 0 { null } else { this }",
                }},
                { "mapping": format!(
                    "let errors = this.answer.errors.or([])\n\
                     root.run = this.run\n\
                     root.deleted = if this.answer == null {{ this.ids }} else {{ this.answer.success.or([]) }}\n\
                     root.gone = $errors.filter(e -> {not_found}).map_each(e -> e.entityId)\n\
                     root.failed = $errors.filter(e -> !({not_found})).map_each(e -> {{\"id\": e.entityId, \"message\": e.error.title.or(e.error.detail).or(\"refused\").string()}})\n\
                     root.edited = this.edited"
                ) },
            ]},
            { "catch": [{ "mapping":
                "root = {\"run\": this.run, \"deleted\": [], \"gone\": [], \"edited\": [], \"failed\": this.ids.map_each(id -> {\"id\": id, \"message\": error().or(\"the gateway did not answer\")})}"
            }]},
        ]},
        "output": { "http_client": {
            "url": report,
            "verb": "POST",
            "headers": { "Content-Type": "application/json" },
            "timeout": "10s",
            "oauth2": crate::reconciler::streams::pipeline_oauth2(project),
        }},
    })
}

/// What a delete stream reports per chunk.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub run: String,
    pub deleted: Vec<String>,
    pub gone: Vec<String>,
    pub failed: Vec<Failed>,
    /// Ids kept because someone changed the entity after the pipeline's last write (PL-100).
    #[serde(default)]
    pub edited: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failed {
    pub id: String,
    pub message: String,
}

/// Takes one report: what went and what was gone leave the ledger, and the activity feed says
/// which entities the pipeline deleted and why. Ids outside the ledger are ignored.
pub async fn reported(
    state: &AppState,
    project: &str,
    pipeline: &str,
    report: Report,
) -> Result<(), sqlx::Error> {
    let (rows, _) = state.source_ledger.book(project, pipeline).await?;
    let held = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .filter(|id| rows.contains_key(*id))
            .cloned()
            .collect()
    };
    let deleted = held(&report.deleted);
    // An entity someone changed after the pipeline's last write is theirs now: it leaves the
    // ledger without a delete, so no later run takes it either (PL-100).
    let edited = held(&report.edited);
    let left: Vec<String> = [deleted.clone(), held(&report.gone), edited.clone()].concat();
    state.source_ledger.forget(project, pipeline, &left).await?;
    let spec = state
        .mirror
        .get(project, "Pipeline", pipeline)
        .map(|p| p.spec);
    let cause = spec.as_ref().map_or(Cause::Snapshot, Cause::of);
    if let Some(space) = spec.and_then(|spec| space_of(state, project, &spec)) {
        state
            .source_ledger
            .removed(project, &space, pipeline, (&report.run, cause), &deleted)
            .await?;
    }
    if let Some(event) = feed_event(project, pipeline, cause, &report, &deleted, &edited) {
        if let Err(error) = state.activity.append(&[event]).await {
            tracing::warn!(project, pipeline, %error, "the source deletes are not on the activity feed");
        }
    }
    Ok(())
}

/// The activity feed's line for one report: what went, what someone changed and was kept, and
/// what the gateway refused; `None` when nothing happened.
fn feed_event(
    project: &str,
    pipeline: &str,
    cause: Cause,
    report: &Report,
    deleted: &[String],
    edited: &[String],
) -> Option<crate::activity::ActivityEvent> {
    let failed: Vec<String> = report
        .failed
        .iter()
        .take(20)
        .map(|f| {
            format!(
                "{}: {}",
                f.id,
                crate::pipeline_outcomes::mask_text(&f.message)
            )
        })
        .collect();
    if deleted.is_empty() && failed.is_empty() && edited.is_empty() {
        return None;
    }
    let why = match cause {
        Cause::Snapshot => "its source no longer lists",
        Cause::Event => "its source marked as deleted",
    };
    let mut summary = format!(
        "pipeline {pipeline} deleted {} entities {why} (run {})",
        deleted.len(),
        report.run
    );
    if !edited.is_empty() {
        summary.push_str(&format!(
            "; kept {} that someone changed after the pipeline's last write",
            edited.len()
        ));
    }
    if !failed.is_empty() {
        summary.push_str(&format!("; the gateway refused {}", report.failed.len()));
    }
    let quiet = failed.is_empty() && edited.is_empty();
    Some(crate::activity::ActivityEvent {
        time: jc_time::TimestampTz::now(),
        project: project.to_owned(),
        space: None,
        kind: "pipeline.source-delete".to_owned(),
        source: "portal".to_owned(),
        summary,
        severity: if quiet { "info" } else { "warning" }.to_owned(),
        correlation_id: None,
        details: json!({
            "object": format!("pipelines/{pipeline}"),
            "run": report.run,
            "reason": reason(cause, pipeline, &report.run),
            "deleted": deleted.iter().take(20).collect::<Vec<_>>(),
            "edited": edited.iter().take(20).collect::<Vec<_>>(),
            "refused": failed,
        }),
    })
}

/// The ledger as the pipeline's page shows it, with the dry run: what the next complete run that
/// misses them again would delete (PL-97).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineSourceDeletes)]
pub struct Overview {
    pub misses: u8,
    pub max_share: u8,
    /// The ids in the ledger.
    pub ledger: usize,
    /// The ids at their last miss: one more complete run without them deletes them.
    pub next: Vec<String>,
    /// The ids whose misses already reached the rule and wait for a delete or a release.
    pub due: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
    /// The space the pipeline writes into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// The last run that read the source completely: the source side of the report (T-4892).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_complete: Option<CompleteRun>,
    /// In the space, gone at the source: ids at least one complete run in a row did not read.
    pub orphans: usize,
    pub orphan_ids: Vec<String>,
    /// At the source, not in the space: records the last complete run read and the model refused.
    pub missing: usize,
    pub missing_ids: Vec<String>,
    /// The newest entities the pipeline deleted because its source dropped them.
    pub removed: Vec<Removal>,
    /// The caller may read the pipeline but not its space: the counts stay, every id is left out.
    pub ids_hidden: bool,
}

impl Overview {
    /// Leaves out every id, for a caller who may not read the space they are in.
    pub fn without_ids(mut self) -> Self {
        self.next.clear();
        self.due.clear();
        self.orphan_ids.clear();
        self.missing_ids.clear();
        self.removed.clear();
        self.ids_hidden = true;
        self
    }
}

/// The last run that read the source completely.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineSourceCompleteRun)]
pub struct CompleteRun {
    pub run: String,
    /// When the run was judged, RFC 3339.
    pub at: String,
    /// The records the run read: written and refused.
    pub read: u64,
}

/// The most ids each list of the overview names.
const LISTED: usize = 200;

/// The pipeline's report: the ledger and its dry run beside what the last complete run read,
/// what it could not write and what the pipeline deleted (T-4892). Ids only.
pub async fn report(
    state: &AppState,
    project: &str,
    pipeline: &str,
    spec: &Value,
    rule: &SourceDeletes,
) -> Result<Overview, sqlx::Error> {
    let ledger = &state.source_ledger;
    let (rows, hold) = ledger.book(project, pipeline).await?;
    let mut report = overview(&rows, hold, rule);
    report.space = space_of(state, project, spec);
    report.removed = ledger
        .removals(project, RemovalsOf::Pipeline(pipeline), LISTED)
        .await?;
    let Some((run, at)) = ledger.last_complete(project, pipeline).await? else {
        return Ok(report);
    };
    let log = &state.pipeline_log;
    let read = log
        .runs(project, pipeline, crate::pipeline_log::RUNS_KEPT)
        .await?
        .into_iter()
        .find(|logged| logged.run == run)
        .map_or(0, |logged| logged.sent + logged.rejected);
    let lines = log
        .lines(
            project,
            pipeline,
            &run,
            crate::pipeline_log::LINES_KEPT,
            None,
        )
        .await?;
    let missing: std::collections::BTreeSet<String> = lines
        .into_iter()
        .filter(|line| line.outcome == crate::pipeline_log::Outcome::Rejected)
        .map(|line| line.record_id)
        .collect();
    report.missing = missing.len();
    report.missing_ids = missing.into_iter().take(LISTED).collect();
    report.last_complete = Some(CompleteRun { run, at, read });
    Ok(report)
}

pub fn overview(
    rows: &BTreeMap<String, Row>,
    hold: Option<Hold>,
    rule: &SourceDeletes,
) -> Overview {
    let at = |misses: u8| -> Vec<String> {
        rows.iter()
            .filter(|(_, row)| {
                row.misses == misses || (misses == rule.misses() && row.misses > misses)
            })
            .map(|(id, _)| id.clone())
            .take(LISTED)
            .collect()
    };
    Overview {
        misses: rule.misses(),
        max_share: rule.max_share(),
        ledger: rows.len(),
        next: at(rule.misses() - 1),
        due: at(rule.misses()),
        hold,
        space: None,
        last_complete: None,
        orphans: rows.values().filter(|row| row.misses > 0).count(),
        orphan_ids: rows
            .iter()
            .filter(|(_, row)| row.misses > 0)
            .map(|(id, _)| id.clone())
            .take(LISTED)
            .collect(),
        missing: 0,
        missing_ids: Vec::new(),
        removed: Vec::new(),
        ids_hidden: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic while the in-memory ledger was held poisons its lock; every later call still
    /// reads and writes the ledger instead of failing every pipeline run after it.
    #[tokio::test]
    async fn a_poisoned_memory_ledger_still_keeps_its_books() {
        let ledger = Ledger::new(None, std::sync::Arc::new(crate::store::Mirror::new()));
        std::thread::scope(|scope| {
            let _ = scope
                .spawn(|| {
                    let _held = ledger.memory();
                    panic!("poison the ledger's lock on purpose");
                })
                .join();
        });
        assert!(matches!(&ledger.inner, Inner::Memory(map) if map.is_poisoned()));
        let ids = vec![
            "urn:ngsi-ld:Garden:1".to_owned(),
            "urn:ngsi-ld:Garden:2".to_owned(),
        ];
        ledger
            .seen("p", "pl", "r1", &ids, true)
            .await
            .expect("seen");
        let (rows, _) = ledger.book("p", "pl").await.expect("book");
        assert_eq!(rows.len(), 2);
        ledger
            .settle("p", "pl", &[(ids[0].clone(), 1)], None)
            .await
            .expect("settle");
        ledger.keep("p", "pl").await.expect("keep");
        ledger.completed("p", "pl", "r1").await.expect("completed");
        ledger
            .removed("p", "space", "pl", ("r1", Cause::Snapshot), &ids[1..])
            .await
            .expect("removed");
        ledger.forget("p", "pl", &ids[..1]).await.expect("forget");
        let (rows, _) = ledger.book("p", "pl").await.expect("book");
        // The forgotten id is gone; the removed one waits for its run's outcome.
        assert_eq!(rows.keys().collect::<Vec<_>>(), [&ids[1]]);
    }

    /// An id of `len` bytes, distinct per `n`.
    fn id_of(len: usize, n: usize) -> String {
        let head = format!("urn:ngsi-ld:Garden:{n}:");
        format!("{head}{}", "x".repeat(len - head.len()))
    }

    /// The chunks render() puts in its stream, decoded from the generate mapping.
    fn rendered_chunks(ids: &[String]) -> Vec<Value> {
        use base64::Engine;
        let wrote: Vec<i64> = (0..ids.len() as i64).collect();
        let stream = render(
            "p",
            "pl",
            "slug",
            ("r1", Cause::Snapshot),
            (ids, &wrote),
            "http://report",
        );
        let mapping = stream["input"]["generate"]["mapping"].as_str().unwrap();
        let encoded = mapping.split('"').nth(1).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        serde_json::from_slice::<Vec<Value>>(&raw).unwrap()
    }

    fn joined_bytes(chunk: &Value) -> usize {
        let ids = chunk["ids"].as_array().unwrap();
        ids.iter().map(|i| i.as_str().unwrap().len()).sum::<usize>() + ids.len().saturating_sub(1)
    }

    /// Every chunk's query fits the broker's 8192-byte id list and is as full as it can be; the
    /// ids, in order, and their write times stay paired; a batch delete never passes 1000 ids.
    #[test]
    fn delete_chunks_fit_the_broker_id_cap_and_the_delete_limit() {
        // The longest real id (dev and prod exports, 274 B): 29 fit, 100 were 27,499 B.
        let long: Vec<String> = (0..100).map(|n| id_of(274, n)).collect();
        // praha-mesto Garden, mean 93.6 B: 100 of them were 9,459 B.
        let praha: Vec<String> = (0..500)
            .map(|n| id_of(if n % 5 < 3 { 94 } else { 93 }, n))
            .collect();
        // Short ids: the delete limit, not the bytes, closes the chunk.
        let short: Vec<String> = (0..2500).map(|n| format!("x{n:04}")).collect();
        for (name, ids, first) in [
            ("274 B", &long, 29),
            ("praha", &praha, 86),
            ("short", &short, 1000),
        ] {
            let chunks = rendered_chunks(ids);
            assert_eq!(chunks[0]["ids"].as_array().unwrap().len(), first, "{name}");
            let mut seen = Vec::new();
            for (k, chunk) in chunks.iter().enumerate() {
                let n = chunk["ids"].as_array().unwrap().len();
                assert!(
                    joined_bytes(chunk) <= QUERY_ID_BYTES,
                    "{name} chunk {k}: {} B",
                    joined_bytes(chunk)
                );
                assert!(n <= DELETE_IDS, "{name} chunk {k}: {n} ids");
                if let Some(next) = chunks.get(k + 1) {
                    let next_id = next["ids"][0].as_str().unwrap().len();
                    assert!(
                        n == DELETE_IDS || joined_bytes(chunk) + 1 + next_id > QUERY_ID_BYTES,
                        "{name} chunk {k} closed early"
                    );
                }
                let wrote: Vec<i64> = chunk["wrote"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w.as_i64().unwrap())
                    .collect();
                for (i, id) in chunk["ids"].as_array().unwrap().iter().enumerate() {
                    assert_eq!(
                        wrote[i] as usize,
                        seen.len(),
                        "{name}: write time paired with its id"
                    );
                    seen.push(id.as_str().unwrap().to_owned());
                }
            }
            assert_eq!(&seen, ids, "{name}: every id once, in order");
        }
        assert!(chunk_ends(&[]).is_empty());
        // An id past the cap alone is a chunk of its own (the ledger keeps none: MAX_ID).
        assert_eq!(chunk_ends(&[id_of(9000, 0), id_of(50, 1)]), [1, 2]);
        // The longest id the ledger keeps: three fill a chunk (6,146 B), a fourth would pass.
        let longest: Vec<String> = (0..7).map(|n| id_of(MAX_ID, n)).collect();
        assert_eq!(chunk_ends(&longest), [3, 6, 7]);
    }

    fn rule(misses: u8, max_share: u8) -> SourceDeletes {
        serde_json::from_value(
            json!({ "mode": "snapshot", "misses": misses, "maxShare": max_share }),
        )
        .expect("rule")
    }

    fn ledger(rows: &[(&str, &str, u8)]) -> BTreeMap<String, Row> {
        rows.iter()
            .map(|(id, run, misses)| {
                let row = Row {
                    last_run: (*run).to_owned(),
                    misses: *misses,
                };
                ((*id).to_owned(), row)
            })
            .collect()
    }

    fn fifty(dropped: &[usize], run: &str, before: u8) -> BTreeMap<String, Row> {
        (0..50)
            .map(|n| {
                let row = Row {
                    last_run: if dropped.contains(&n) { "r0" } else { run }.to_owned(),
                    misses: if dropped.contains(&n) { before } else { 0 },
                };
                (format!("urn:ngsi-ld:Dataset:{n}"), row)
            })
            .collect()
    }

    #[test]
    fn three_of_fifty_dropped_go_after_two_complete_runs_and_not_after_one() {
        let rule = rule(2, 10);
        let (misses, verdict) = judge(&fifty(&[3, 7, 9], "r1", 0), "r1", &rule);
        assert_eq!(verdict, Verdict::Delete(Vec::new()));
        assert_eq!(misses.iter().filter(|(_, m)| *m == 1).count(), 3);
        let (_, verdict) = judge(&fifty(&[3, 7, 9], "r2", 1), "r2", &rule);
        assert_eq!(
            verdict,
            Verdict::Delete(
                [
                    "urn:ngsi-ld:Dataset:3",
                    "urn:ngsi-ld:Dataset:7",
                    "urn:ngsi-ld:Dataset:9"
                ]
                .map(str::to_owned)
                .to_vec()
            )
        );
    }

    #[test]
    fn a_record_back_at_the_source_has_its_misses_cleared() {
        let rows = ledger(&[("urn:a", "r2", 1), ("urn:b", "r2", 0)]);
        let (misses, verdict) = judge(&rows, "r2", &rule(2, 100));
        assert_eq!(
            misses,
            vec![("urn:a".to_owned(), 0), ("urn:b".to_owned(), 0)]
        );
        assert_eq!(verdict, Verdict::Delete(Vec::new()));
    }

    #[test]
    fn more_than_the_share_is_held_and_deletes_nothing() {
        let rows = fifty(&[1, 2, 3, 4, 5, 6], "r2", 1);
        let (misses, verdict) = judge(&rows, "r2", &rule(2, 10));
        let hold = Hold {
            run: "r2".into(),
            reason: HoldReason::ShareExceeded,
            would: 6,
            ledger: 50,
        };
        assert!(hold.detail().contains("would delete 6 of 50"));
        assert_eq!(verdict, Verdict::Held(hold));
        // The misses still count, so the next complete run is judged the same way.
        assert_eq!(misses.iter().filter(|(_, m)| *m == 2).count(), 6);
        // Five of fifty is the share exactly and goes.
        let (_, verdict) = judge(&fifty(&[1, 2, 3, 4, 5], "r2", 1), "r2", &rule(2, 10));
        assert!(matches!(verdict, Verdict::Delete(ids) if ids.len() == 5));
    }

    #[test]
    fn a_run_that_read_none_of_the_ledger_is_held_empty_and_counts_no_miss() {
        let rows = fifty(&[], "r1", 0);
        let (misses, verdict) = judge(&rows, "r9", &rule(1, 100));
        assert!(misses.is_empty());
        assert!(matches!(
            verdict,
            Verdict::Held(Hold {
                reason: HoldReason::Empty,
                would: 0,
                ..
            })
        ));
    }

    /// Each hold reason keeps its kebab-case word in the database and reads back as itself; a
    /// word nobody wrote reads as incomplete, the cautious one. Each alert names its run.
    #[test]
    fn a_hold_reason_round_trips_its_word_and_each_alert_names_the_run() {
        for reason in [
            HoldReason::ShareExceeded,
            HoldReason::Empty,
            HoldReason::Incomplete,
        ] {
            assert_eq!(HoldReason::parse(reason.as_str()), reason);
            assert_eq!(
                json!(reason),
                json!(reason.as_str()),
                "the API says the same word"
            );
            let hold = Hold {
                run: "r7".into(),
                reason,
                would: 3,
                ledger: 9,
            };
            assert!(
                hold.detail().starts_with("source deletes held: run r7 "),
                "{}",
                hold.detail()
            );
        }
        assert_eq!(HoldReason::parse("unknown"), HoldReason::Incomplete);
        let incomplete = Hold {
            run: "r7".into(),
            reason: HoldReason::Incomplete,
            would: 0,
            ledger: 9,
        };
        assert!(incomplete
            .detail()
            .contains("did not read its source completely"));
    }

    #[test]
    fn only_a_snapshot_rule_within_bounds_acts() {
        let spec = |rule: Value| json!({ "sourceDeletes": rule });
        assert!(snapshot_rule(&spec(json!({ "mode": "snapshot" })))
            .is_some_and(|r| r.misses() == 2 && r.max_share() == 10));
        for refused in [
            json!({ "mode": "snapshot", "misses": 0 }),
            json!({ "mode": "snapshot", "misses": 11 }),
            json!({ "mode": "snapshot", "maxShare": 0 }),
            json!({ "mode": "snapshot", "maxShare": 101 }),
            json!({ "mode": "events", "deletedWhen": "this.gone" }),
            json!({ "mode": "never" }),
            json!({ "mode": "lease" }),
            json!({ "mode": "snapshot", "what": 1 }),
        ] {
            assert!(snapshot_rule(&spec(refused.clone())).is_none(), "{refused}");
        }
        assert!(snapshot_rule(&json!({})).is_none());
    }

    #[test]
    fn only_a_plain_uri_may_be_deleted() {
        assert!(deletable("urn:ngsi-ld:Dataset:sk.bbsk:open:ab-1"));
        assert!(deletable("https://data.example/ds/1"));
        for bad in [
            "",
            "no-colon",
            "urn:a b",
            "urn:${JC_CLIENT_SECRET_X}",
            "urn:\"x",
            "urn:a\\b",
            "urn:ž",
        ] {
            assert!(!deletable(bad), "{bad:?}");
        }
        assert!(deletable(&format!("urn:{}", "a".repeat(MAX_ID - 4))));
        assert!(!deletable(&format!("urn:{}", "a".repeat(MAX_ID - 3))));
    }

    #[test]
    fn the_delete_stream_goes_through_the_endpoint_with_the_reason_and_reports_back() {
        let ids: Vec<String> = (0..2500).map(|n| format!("x{n:04}")).collect();
        let wrote: Vec<i64> = (0..2500).collect();
        let stream = render(
            "helsinki",
            "registry",
            "ep-slug",
            ("2026-10-20T05:00:00Z", Cause::Snapshot),
            (&ids, &wrote),
            "http://portal-internal:8081/internal/pipelines/helsinki/registry/source-deletes",
        );
        let text = stream.to_string();
        // No id is in the config as text: Bento would read `${...}` in it.
        assert!(!text.contains("x0001"));
        let mapping = stream["input"]["generate"]["mapping"]
            .as_str()
            .expect("mapping");
        let encoded = mapping
            .strip_prefix("root = \"")
            .and_then(|m| m.split('"').next())
            .expect("encoded");
        use base64::Engine;
        let chunks: Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("base64"),
        )
        .expect("json");
        let chunks = chunks.as_array().expect("chunks");
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[2]["ids"].as_array().map(Vec::len), Some(500));
        assert_eq!(chunks[0]["run"], "2026-10-20T05:00:00Z");
        assert_eq!(chunks[2]["wrote"][0], 2000);
        // Each chunk first reads its entities' modifiedAt through the same Endpoint (PL-100).
        let read =
            &stream["pipeline"]["processors"][1]["try"][0]["branch"]["processors"][0]["http"];
        // POST entityOperations/query is the gateway's queryBatch, the read admission demands:
        // GET /entities would be queryEntity, which a pipeline's grants do not hold.
        assert!(crate::pipeline_expiry::SOURCE_DELETE_OPERATIONS
            .contains(&(jc_core::kinds::Operation::QueryBatch, "queryBatch")));
        assert_eq!(read["verb"], "POST");
        assert_eq!(
            read["url"],
            "${JC_GATEWAY_URL}/api/endpoint/ep-slug/ngsi-ld/v1/entityOperations/query?options=sysAttrs,keyValues&limit=1000"
        );
        assert_eq!(
            stream["pipeline"]["processors"][1]["try"][0]["branch"]["request_map"],
            r#"root = {"type": "Query", "entities": [{"type": "*", "id": this.ids}]}"#
        );
        let http =
            &stream["pipeline"]["processors"][1]["try"][2]["branch"]["processors"][0]["http"];
        assert_eq!(
            http["url"],
            "${JC_GATEWAY_URL}/api/endpoint/ep-slug/ngsi-ld/v1/entityOperations/delete"
        );
        assert_eq!(
            http["headers"]["JC-Reason"],
            "source-snapshot%3A%20pipeline%20registry%20run%202026-10-20T05%3A00%3A00Z"
        );
        assert_eq!(http["oauth2"]["client_key"], "helsinki-pipelines");
        assert_eq!(
            stream["output"]["http_client"]["url"],
            "http://portal-internal:8081/internal/pipelines/helsinki/registry/source-deletes"
        );
    }

    /// The stream as the runner gets it, for a Bento run against a gateway and a Portal stand-in:
    /// regenerate with `JC_UPDATE_GOLDEN=1`.
    #[test]
    fn the_delete_stream_matches_its_golden() {
        let ids: Vec<String> = (0..160).map(|n| format!("urn:x:{n}")).collect();
        let wrote = vec![1_792_000_000; ids.len()];
        let stream = render(
            "helsinki",
            "registry",
            "ep-slug",
            ("2026-10-20T05:00:00Z", Cause::Event),
            (&ids, &wrote),
            "http://127.0.0.1:18089/report",
        );
        let golden = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/source-deletes/stream.json"
        );
        let text = serde_json::to_string_pretty(&stream).expect("json") + "\n";
        let update = std::env::var_os("JC_UPDATE_GOLDEN").is_some();
        // JC_UPDATE_GOLDEN writes the golden; any other run writes a scratch copy beside the diff.
        let pid = std::process::id();
        let scratch = std::env::temp_dir().join(format!("jc-stream-golden-{pid}.json"));
        let target = [scratch.as_path(), std::path::Path::new(golden)][usize::from(update)];
        std::fs::write(target, &text).expect("golden written");
        assert_eq!(std::fs::read_to_string(golden).expect("golden"), text);
    }

    #[test]
    fn the_overview_lists_the_next_and_the_due_ids() {
        let rows = ledger(&[
            ("urn:a", "r", 0),
            ("urn:b", "r", 1),
            ("urn:c", "r", 2),
            ("urn:d", "r", 3),
        ]);
        let seen = overview(&rows, None, &rule(2, 10));
        assert_eq!(seen.ledger, 4);
        assert_eq!(seen.next, vec!["urn:b"]);
        assert_eq!(seen.due, vec!["urn:c", "urn:d"]);
        assert_eq!(doomed(&rows, &rule(2, 10)), vec!["urn:c", "urn:d"]);
    }

    #[test]
    fn every_launch_has_a_stream_of_its_own() {
        let first = delete_stream("http://runner", "helsinki", "registry");
        let second = delete_stream("http://runner", "helsinki", "registry");
        assert!(
            first.starts_with("http://runner/streams/helsinki.registry.source-deletes."),
            "{first}"
        );
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn the_memory_ledger_keeps_ids_misses_and_the_hold() {
        let ledger = Ledger::new(None, Default::default());
        let ids = ["urn:a", "urn:b", "bad id"].map(str::to_owned);
        ledger.seen("p", "x", "r1", &ids, true).await.expect("seen");
        // A refused record keeps a known row alive and adds no new one.
        let refused = ["urn:a", "urn:never-written"].map(str::to_owned);
        ledger
            .seen("p", "x", "r2", &refused, false)
            .await
            .expect("seen");
        let (rows, hold) = ledger.book("p", "x").await.expect("book");
        assert_eq!(rows.keys().collect::<Vec<_>>(), ["urn:a", "urn:b"]);
        assert_eq!(
            (
                rows["urn:a"].last_run.as_str(),
                rows["urn:b"].last_run.as_str()
            ),
            ("r2", "r1")
        );
        assert!(hold.is_none());
        let held = Hold {
            run: "r2".into(),
            reason: HoldReason::ShareExceeded,
            would: 1,
            ledger: 2,
        };
        ledger
            .settle("p", "x", &[("urn:a".into(), 2)], Some(&held))
            .await
            .expect("settle");
        assert_eq!(ledger.holds().await.expect("holds").len(), 1);
        ledger.keep("p", "x").await.expect("keep");
        let (rows, hold) = ledger.book("p", "x").await.expect("book");
        assert_eq!(rows["urn:a"].misses, 0);
        assert!(hold.is_none());
        ledger
            .forget("p", "x", &["urn:a".into()])
            .await
            .expect("forget");
        assert_eq!(ledger.book("p", "x").await.expect("book").0.len(), 1);
        // Another pipeline's ledger is its own.
        assert!(ledger.book("p", "y").await.expect("book").0.is_empty());
    }

    /// Project helsinki: the vehicles space's Endpoint and a snapshot pipeline writing it.
    fn snapshot_world(extra: Vec<crate::resource::ResourceEnvelope>) -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "Endpoint",
            "ep-vehicles",
            "helsinki",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "vehicles" } }),
        ));
        state
            .mirror
            .upsert(envelope("Pipeline", "feed", "helsinki", snapshot_spec()));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    fn snapshot_spec() -> Value {
        json!({
            "class": "scheduled",
            "schedule": "0 * * * *",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ep-vehicles" }],
            "sourceDeletes": { "mode": "snapshot", "misses": 2, "maxShare": 10 },
        })
    }

    #[tokio::test]
    async fn the_report_reads_the_ledger_the_last_complete_run_and_what_it_could_not_write() {
        use crate::pipeline_log::{NewLine, Outcome};
        let state = snapshot_world(Vec::new());
        let ids = ["urn:a", "urn:b"].map(str::to_owned);
        let ledger = &state.source_ledger;
        ledger
            .seen("helsinki", "feed", "r1", &ids, true)
            .await
            .expect("seen");
        let line = |id: &str, outcome| NewLine {
            record_id: id.to_owned(),
            step: None,
            outcome,
            message: String::new(),
        };
        state
            .pipeline_log
            .append(
                "helsinki",
                "feed",
                "r1",
                &[
                    line("urn:a", Outcome::Sent),
                    line("urn:c", Outcome::Rejected),
                ],
            )
            .await
            .expect("log");
        let rule = rule(2, 10);
        let before = report(&state, "helsinki", "feed", &snapshot_spec(), &rule)
            .await
            .expect("report");
        assert_eq!(
            (before.ledger, before.space.as_deref()),
            (2, Some("vehicles"))
        );
        assert!(
            before.last_complete.is_none(),
            "no run read its source completely yet"
        );
        ledger
            .completed("helsinki", "feed", "r1")
            .await
            .expect("completed");
        let after = report(&state, "helsinki", "feed", &snapshot_spec(), &rule)
            .await
            .expect("report");
        let complete = after.last_complete.as_ref().expect("the complete run");
        assert_eq!((complete.run.as_str(), complete.read), ("r1", 2));
        assert_eq!(
            (after.missing, after.missing_ids.clone()),
            (1, vec!["urn:c".to_owned()])
        );
    }

    fn events_world() -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let mut spec = snapshot_spec();
        spec["sourceDeletes"] = json!({ "mode": "events", "deletedWhen": "this.gone.or(false)" });
        spec.as_object_mut().map(|s| s.remove("schedule"));
        snapshot_world(vec![ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Pipeline".to_owned(),
            metadata: ObjectMeta::new("pushed", "helsinki"),
            spec,
            status: None,
        }])
    }

    async fn feed(state: &AppState) -> Vec<crate::activity::ActivityEvent> {
        let filter = crate::activity::ActivityFilter {
            limit: 50,
            ..Default::default()
        };
        state
            .activity
            .list("helsinki", &filter)
            .await
            .expect("feed")
            .items
    }

    /// PL-98, PL-100 (T-5360): a mark keeps only ids the pipeline wrote; another id is reported and
    /// never deleted, a record deletedWhen could not judge is a failed line of the run.
    #[tokio::test]
    async fn a_source_mark_takes_only_ledger_ids_and_reports_the_rest() {
        let state = events_world();
        let ledger = &state.source_ledger;
        let ids = ["urn:a", "urn:b"].map(str::to_owned);
        ledger
            .seen("helsinki", "pushed", "r0", &ids, true)
            .await
            .expect("seen");
        let marks: Marks = serde_json::from_value(json!({
            "run": "r1",
            "ids": ["urn:a", "urn:z", "bad id"],
            "failed": [{ "id": "urn:c", "message": "expected bool value, got string" }],
        }))
        .expect("marks");
        marked_at_source(&state, "helsinki", "pushed", marks)
            .await
            .expect("kept");
        assert_eq!(
            ledger.marking().await.expect("marking"),
            vec![("helsinki".to_owned(), "pushed".to_owned())]
        );
        let events = feed(&state).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, "warning");
        assert_eq!(events[0].details["notInLedger"], json!(["urn:z"]));
        let runs = state
            .pipeline_log
            .runs("helsinki", "pushed", 10)
            .await
            .expect("runs");
        assert_eq!(
            runs.iter().find(|r| r.run == "r1").map(|r| r.failed),
            Some(1)
        );
        let taken = ledger
            .take_marked("helsinki", "pushed")
            .await
            .expect("take");
        assert_eq!(taken, Some(("r1".to_owned(), vec!["urn:a".to_owned()])));
        assert_eq!(
            ledger
                .take_marked("helsinki", "pushed")
                .await
                .expect("take"),
            None
        );
        let written = ledger
            .written("helsinki", "pushed", &ids)
            .await
            .expect("written");
        assert_eq!(written.len(), 2, "each written id has its write time");
    }

    /// A mark the runner could not take (no runner here) is put back, for the run end and the sweep.
    #[tokio::test]
    async fn marks_a_stream_could_not_take_wait_for_the_next_run_end_or_sweep() {
        let state = events_world();
        let ledger = &state.source_ledger;
        ledger
            .seen("helsinki", "pushed", "r0", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        ledger
            .mark("helsinki", "pushed", "r1", &["urn:a".to_owned()])
            .await
            .expect("mark");
        sweep_marks(&state).await;
        run_ended(
            &state,
            RunEnd {
                project: "helsinki".to_owned(),
                pipeline: "pushed".to_owned(),
                run: "r1".to_owned(),
                finished: false,
                reports: ReportCount::Complete,
                stream: String::new(),
            },
        )
        .await;
        assert_eq!(
            ledger
                .take_marked("helsinki", "pushed")
                .await
                .expect("take"),
            Some(("r1".to_owned(), vec!["urn:a".to_owned()]))
        );
    }

    /// PL-100: what someone changed after the pipeline's last write leaves the ledger undeleted,
    /// and the feed says so with the source-event reason.
    #[tokio::test]
    async fn a_report_keeps_what_someone_changed_and_names_the_source_event() {
        let state = events_world();
        let ledger = &state.source_ledger;
        let ids = ["urn:a", "urn:b", "urn:c"].map(str::to_owned);
        ledger
            .seen("helsinki", "pushed", "r0", &ids, true)
            .await
            .expect("seen");
        let report: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": ["urn:a"], "gone": [], "failed": [], "edited": ["urn:b", "urn:q"],
        }))
        .expect("report");
        reported(&state, "helsinki", "pushed", report)
            .await
            .expect("reported");
        let (rows, _) = ledger.book("helsinki", "pushed").await.expect("book");
        assert_eq!(rows.keys().collect::<Vec<_>>(), ["urn:c"]);
        let removals = ledger
            .removals("helsinki", RemovalsOf::Pipeline("pushed"), 10)
            .await
            .expect("removals");
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].reason, "source-event: pipeline pushed run r1");
        let events = feed(&state).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].details["edited"], json!(["urn:b"]));
        assert!(
            events[0].summary.contains("marked as deleted"),
            "{}",
            events[0].summary
        );
        assert!(
            events[0].summary.contains("kept 1"),
            "{}",
            events[0].summary
        );
    }

    #[test]
    fn only_an_events_rule_with_a_query_marks() {
        assert_eq!(
            events_rule(
                &json!({ "sourceDeletes": { "mode": "events", "deletedWhen": "this.gone" } })
            ),
            Some("this.gone".to_owned())
        );
        assert_eq!(
            events_rule(&json!({ "sourceDeletes": { "mode": "events", "deletedWhen": "  " } })),
            None
        );
        assert_eq!(events_rule(&snapshot_spec()), None);
        assert_eq!(Cause::of(&snapshot_spec()), Cause::Snapshot);
    }

    // ---- the ledger without a database, for a pipeline it never saw, and its bounds ----

    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use std::sync::Arc;

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    /// A mirror that places `project` in an Organization, so a store on a dead pool reaches it.
    fn mirror_placing(project: &str) -> Arc<crate::store::Mirror> {
        let mirror = Arc::new(crate::store::Mirror::new());
        mirror.upsert(envelope(
            "Project",
            project,
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        ));
        mirror
    }

    fn dead_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool")
    }

    async fn database() -> sqlx::PgPool {
        let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
        crate::db::connect(&url).await.expect("connect + migrate")
    }

    #[tokio::test]
    async fn a_pipeline_the_memory_ledger_never_saw_answers_empty_and_changes_nothing() {
        let ledger = Ledger::new(None, Default::default());
        let ids = ["urn:a".to_owned()];
        ledger.forget("p", "x", &ids).await.expect("forget");
        ledger.keep("p", "x").await.expect("keep");
        assert!(ledger
            .mark("p", "x", "r1", &ids)
            .await
            .expect("mark")
            .is_empty());
        assert_eq!(ledger.take_marked("p", "x").await.expect("take"), None);
        assert!(ledger
            .written("p", "x", &ids)
            .await
            .expect("written")
            .is_empty());
        assert_eq!(ledger.last_complete("p", "x").await.expect("last"), None);
        assert!(ledger.marking().await.expect("marking").is_empty());
        assert!(ledger.holds().await.expect("holds").is_empty());
        assert!(ledger
            .removals("p", RemovalsOf::Pipeline("x"), 10)
            .await
            .expect("removals")
            .is_empty());
        // None of the above made a book: another read still finds nothing.
        assert!(ledger.book("p", "x").await.expect("book").0.is_empty());
    }

    #[tokio::test]
    async fn a_read_that_is_not_a_write_keeps_the_time_of_the_last_write() {
        let ledger = Ledger::new(None, Default::default());
        let ids = ["urn:a".to_owned()];
        ledger.seen("p", "x", "r1", &ids, true).await.expect("seen");
        let wrote = ledger.written("p", "x", &ids).await.expect("written");
        assert_eq!(wrote.len(), 1);
        ledger
            .seen("p", "x", "r2", &ids, false)
            .await
            .expect("seen");
        assert_eq!(
            ledger.written("p", "x", &ids).await.expect("written"),
            wrote
        );
        let (rows, _) = ledger.book("p", "x").await.expect("book");
        assert_eq!(rows["urn:a"].last_run, "r2");
        // An id nothing wrote has no write time.
        let other = ["urn:never".to_owned()];
        assert!(ledger
            .written("p", "x", &other)
            .await
            .expect("written")
            .is_empty());
    }

    #[tokio::test]
    async fn the_memory_ledger_keeps_the_newest_thousand_removals_and_one_per_id() {
        let ledger = Ledger::new(None, Default::default());
        let many: Vec<String> = (0..REMOVALS_KEPT + 5)
            .map(|n| format!("urn:x:{n}"))
            .collect();
        ledger
            .removed("p", "s1", "x", ("r1", Cause::Snapshot), &many)
            .await
            .expect("removed");
        let kept = ledger
            .removals("p", RemovalsOf::Pipeline("x"), 5000)
            .await
            .expect("removals");
        assert_eq!(kept.len(), REMOVALS_KEPT);
        assert!(
            kept.iter().all(|r| r.id != "urn:x:0"),
            "the oldest went first"
        );
        assert!(kept.iter().any(|r| r.id == "urn:x:1004"));
        // The same id again is one entry, now with the newer run and cause.
        ledger
            .removed(
                "p",
                "s1",
                "x",
                ("r2", Cause::Event),
                &["urn:x:1004".to_owned()],
            )
            .await
            .expect("removed");
        let again = ledger
            .removals("p", RemovalsOf::Pipeline("x"), 5000)
            .await
            .expect("removals");
        let same: Vec<&Removal> = again.iter().filter(|r| r.id == "urn:x:1004").collect();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].run, "r2");
        assert!(
            same[0].reason.starts_with("source-event"),
            "{}",
            same[0].reason
        );
        // Nothing to remove is nothing remembered; one space is not another, and a limit cuts.
        ledger
            .removed("p", "s1", "x", ("r3", Cause::Snapshot), &[])
            .await
            .expect("removed");
        let other_space = ledger
            .removals("p", RemovalsOf::Space("s2"), 10)
            .await
            .expect("removals");
        assert!(other_space.is_empty());
        let cut = ledger
            .removals("p", RemovalsOf::Space("s1"), 3)
            .await
            .expect("removals");
        assert_eq!(cut.len(), 3);
        let other_project = ledger
            .removals("q", RemovalsOf::Space("s1"), 3)
            .await
            .expect("removals");
        assert!(other_project.is_empty());
    }

    #[test]
    fn an_id_the_stream_could_misread_is_not_deletable() {
        assert!(deletable("urn:ngsi-ld:Garden:1"));
        assert!(!deletable(""));
        assert!(!deletable("no-colon"));
        assert!(!deletable("urn:a b"));
        assert!(!deletable("urn:a\"b"));
        assert!(!deletable("urn:${x}"));
        assert!(!deletable(&format!("urn:{}", "x".repeat(MAX_ID))));
        assert!(deletable(&format!("urn:{}", "x".repeat(MAX_ID - 4))));
    }

    // ---- the ledger on a database: a project without a key, and a database that is down ----

    #[tokio::test]
    async fn a_project_without_a_key_has_no_ledger_and_every_call_answers_empty() {
        let pool = database().await;
        let ledger = Ledger::new(Some(pool), Arc::new(crate::store::Mirror::new()));
        let ids = ["urn:a".to_owned()];
        ledger
            .seen("ghost", "x", "r1", &ids, true)
            .await
            .expect("seen");
        assert!(ledger.book("ghost", "x").await.expect("book").0.is_empty());
        ledger
            .settle("ghost", "x", &[], None)
            .await
            .expect("settle");
        ledger.forget("ghost", "x", &ids).await.expect("forget");
        ledger.keep("ghost", "x").await.expect("keep");
        assert!(ledger
            .mark("ghost", "x", "r1", &ids)
            .await
            .expect("mark")
            .is_empty());
        assert_eq!(ledger.take_marked("ghost", "x").await.expect("take"), None);
        assert!(ledger
            .written("ghost", "x", &ids)
            .await
            .expect("written")
            .is_empty());
        ledger
            .completed("ghost", "x", "r1")
            .await
            .expect("completed");
        assert_eq!(
            ledger.last_complete("ghost", "x").await.expect("last"),
            None
        );
        ledger
            .removed("ghost", "s", "x", ("r1", Cause::Snapshot), &ids)
            .await
            .expect("removed");
        assert!(ledger
            .removals("ghost", RemovalsOf::Pipeline("x"), 10)
            .await
            .expect("removals")
            .is_empty());
        assert!(ledger.marking().await.expect("marking").is_empty());
        assert!(ledger.holds().await.expect("holds").is_empty());
    }

    #[tokio::test]
    async fn a_database_that_is_down_fails_every_ledger_call() {
        let ledger = Ledger::new(Some(dead_pool()), mirror_placing("helsinki"));
        let ids = ["urn:a".to_owned()];
        assert!(ledger
            .seen("helsinki", "x", "r1", &ids, true)
            .await
            .is_err());
        assert!(ledger.book("helsinki", "x").await.is_err());
        assert!(ledger.settle("helsinki", "x", &[], None).await.is_err());
        assert!(ledger.forget("helsinki", "x", &ids).await.is_err());
        assert!(ledger.keep("helsinki", "x").await.is_err());
        assert!(ledger.mark("helsinki", "x", "r1", &ids).await.is_err());
        assert!(ledger.take_marked("helsinki", "x").await.is_err());
        assert!(ledger.written("helsinki", "x", &ids).await.is_err());
        assert!(ledger.completed("helsinki", "x", "r1").await.is_err());
        assert!(ledger.last_complete("helsinki", "x").await.is_err());
        assert!(ledger
            .removed("helsinki", "s", "x", ("r1", Cause::Event), &ids)
            .await
            .is_err());
        assert!(ledger
            .removals("helsinki", RemovalsOf::Space("s"), 10)
            .await
            .is_err());
        assert!(ledger.marking().await.is_err());
        assert!(ledger.holds().await.is_err());
    }

    // ---- the run end, on the memory ledger and on a database that is down ----

    fn run_end(pipeline: &str, run: &str, finished: bool, reports: ReportCount) -> RunEnd {
        RunEnd {
            project: "helsinki".to_owned(),
            pipeline: pipeline.to_owned(),
            run: run.to_owned(),
            finished,
            reports,
            stream: String::new(),
        }
    }

    /// The project's mirror with the Organization a dead database is asked about.
    fn dead_world() -> AppState {
        snapshot_world(vec![envelope(
            "Project",
            "helsinki",
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        )])
        .with_db(dead_pool())
    }

    #[tokio::test]
    async fn a_run_end_of_a_pipeline_the_mirror_lacks_changes_nothing() {
        let state = snapshot_world(Vec::new());
        state
            .source_ledger
            .seen("helsinki", "gone", "r0", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        run_ended(&state, run_end("gone", "r1", true, ReportCount::Complete)).await;
        let (rows, hold) = state
            .source_ledger
            .book("helsinki", "gone")
            .await
            .expect("book");
        assert_eq!(rows["urn:a"].misses, 0);
        assert!(hold.is_none());
    }

    #[tokio::test]
    async fn a_run_whose_reports_did_not_all_arrive_holds_its_deletes_as_incomplete() {
        for reports in [
            ReportCount::Lost(3),
            ReportCount::Unknown("the runner did not answer"),
        ] {
            let state = snapshot_world(Vec::new());
            let ids = ["urn:a", "urn:b"].map(str::to_owned);
            state
                .source_ledger
                .seen("helsinki", "feed", "r1", &ids, true)
                .await
                .expect("seen");
            run_ended(&state, run_end("feed", "r2", true, reports)).await;
            let (rows, hold) = state
                .source_ledger
                .book("helsinki", "feed")
                .await
                .expect("book");
            let hold = hold.expect("held");
            assert_eq!(hold.reason, HoldReason::Incomplete);
            assert_eq!((hold.run.as_str(), hold.ledger, hold.would), ("r2", 2, 0));
            assert!(
                rows.values().all(|row| row.misses == 0),
                "an incomplete run counts no miss"
            );
        }
    }

    #[tokio::test]
    async fn a_run_that_did_not_finish_holds_its_deletes_as_incomplete() {
        let state = snapshot_world(Vec::new());
        state
            .source_ledger
            .seen("helsinki", "feed", "r1", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        run_ended(&state, run_end("feed", "r2", false, ReportCount::Complete)).await;
        let (_, hold) = state
            .source_ledger
            .book("helsinki", "feed")
            .await
            .expect("book");
        assert_eq!(hold.map(|h| h.reason), Some(HoldReason::Incomplete));
    }

    #[tokio::test]
    async fn a_complete_run_over_an_empty_ledger_is_noted_as_the_last_complete_run() {
        let state = snapshot_world(Vec::new());
        run_ended(&state, run_end("feed", "r9", true, ReportCount::Complete)).await;
        let last = state
            .source_ledger
            .last_complete("helsinki", "feed")
            .await
            .expect("last");
        assert_eq!(last.map(|(run, _)| run), Some("r9".to_owned()));
        let (_, hold) = state
            .source_ledger
            .book("helsinki", "feed")
            .await
            .expect("book");
        assert!(hold.is_none());
    }

    #[tokio::test]
    async fn the_id_two_complete_runs_missed_is_due_even_when_its_delete_cannot_start() {
        let state = snapshot_world(Vec::new());
        let ledger = &state.source_ledger;
        let all: Vec<String> = (0..20).map(|n| format!("urn:a:{n}")).collect();
        ledger
            .seen("helsinki", "feed", "r0", &all, true)
            .await
            .expect("seen");
        for run in ["r1", "r2"] {
            ledger
                .seen("helsinki", "feed", run, &all[1..], false)
                .await
                .expect("seen");
            run_ended(&state, run_end("feed", run, true, ReportCount::Complete)).await;
        }
        let (rows, hold) = ledger.book("helsinki", "feed").await.expect("book");
        assert!(
            hold.is_none(),
            "one of twenty is within the share: {hold:?}"
        );
        assert_eq!(rows["urn:a:0"].misses, 2);
        assert_eq!(rows["urn:a:1"].misses, 0);
        assert_eq!(doomed(&rows, &rule(2, 10)), ["urn:a:0".to_owned()]);
        // This world's Endpoint has no slug, so the delete was not started and the id is still due.
        assert!(rows.contains_key("urn:a:0"));
    }

    #[tokio::test]
    async fn a_database_that_is_down_costs_a_run_end_its_warnings_and_nothing_else() {
        let state = dead_world();
        run_ended(&state, run_end("feed", "r1", true, ReportCount::Complete)).await;
        // An events pipeline's run end reads its marks and cannot.
        let events = events_world();
        events.mirror.upsert(envelope(
            "Project",
            "helsinki",
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        ));
        let events = events.with_db(dead_pool());
        run_ended(
            &events,
            run_end("pushed", "r1", true, ReportCount::Complete),
        )
        .await;
        sweep_marks(&events).await;
    }

    #[tokio::test]
    async fn a_report_or_a_seen_id_on_a_database_that_is_down_is_an_error_or_a_warning() {
        let state = dead_world();
        note_seen(
            &state,
            "helsinki",
            "feed",
            "r1",
            &["urn:a".to_owned()],
            true,
        )
        .await;
        let report: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": [], "gone": [], "failed": []
        }))
        .expect("report");
        assert!(reported(&state, "helsinki", "feed", report).await.is_err());
        let marks: Marks = serde_json::from_value(json!({ "ids": ["urn:a"] })).expect("marks");
        assert!(marked_at_source(&state, "helsinki", "feed", marks)
            .await
            .is_err());
        assert!(report_of(&state).await.is_err());
    }

    async fn report_of(state: &AppState) -> Result<Overview, sqlx::Error> {
        report(state, "helsinki", "feed", &snapshot_spec(), &rule(2, 10)).await
    }

    // ---- a mark of ids that are all the pipeline's, a feed that cannot be written ----

    #[tokio::test]
    async fn a_mark_of_ids_that_are_all_in_the_ledger_reports_nothing() {
        let state = events_world();
        let ids = ["urn:a", "urn:b"].map(str::to_owned);
        state
            .source_ledger
            .seen("helsinki", "pushed", "r0", &ids, true)
            .await
            .expect("seen");
        let marks: Marks =
            serde_json::from_value(json!({ "run": "r1", "ids": ["urn:a", "urn:b"] }))
                .expect("marks");
        marked_at_source(&state, "helsinki", "pushed", marks)
            .await
            .expect("kept");
        assert!(
            feed(&state).await.is_empty(),
            "every id was the pipeline's own"
        );
        let taken = state
            .source_ledger
            .take_marked("helsinki", "pushed")
            .await
            .expect("take");
        assert_eq!(taken.map(|(_, ids)| ids.len()), Some(2));
    }

    #[tokio::test]
    async fn a_mark_that_names_strangers_still_succeeds_when_the_feed_cannot_be_written() {
        let mut state = events_world();
        state.mirror.upsert(envelope(
            "Project",
            "helsinki",
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        ));
        state.activity =
            crate::activity::ActivityStore::new(Some(dead_pool()), Arc::clone(&state.mirror));
        let marks: Marks =
            serde_json::from_value(json!({ "run": "r1", "ids": ["urn:stranger"] })).expect("marks");
        marked_at_source(&state, "helsinki", "pushed", marks)
            .await
            .expect("the marks are kept; only the feed line is lost");
    }

    #[tokio::test]
    async fn a_report_still_succeeds_when_the_feed_cannot_be_written() {
        let mut state = events_world();
        state.mirror.upsert(envelope(
            "Project",
            "helsinki",
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        ));
        state.activity =
            crate::activity::ActivityStore::new(Some(dead_pool()), Arc::clone(&state.mirror));
        state
            .source_ledger
            .seen("helsinki", "pushed", "r0", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        let report: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": ["urn:a"], "gone": [], "failed": []
        }))
        .expect("report");
        reported(&state, "helsinki", "pushed", report)
            .await
            .expect("the ledger is settled; only the feed line is lost");
        let (rows, _) = state
            .source_ledger
            .book("helsinki", "pushed")
            .await
            .expect("book");
        assert!(rows.is_empty());
    }

    #[test]
    fn a_report_of_refusals_names_how_many_and_masks_what_the_gateway_said() {
        let report: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": [], "gone": [],
            "failed": [
                { "id": "urn:x", "message": "refused" },
                { "id": "urn:y", "message": "refused too" }
            ]
        }))
        .expect("report");
        let event = feed_event("p", "pl", Cause::Snapshot, &report, &[], &[]).expect("an event");
        assert!(
            event.summary.contains("the gateway refused 2"),
            "{}",
            event.summary
        );
        assert_eq!(event.severity, "warning");
        assert_eq!(event.details["refused"].as_array().map(Vec::len), Some(2));
        let first = event.details["refused"][0].as_str().expect("a line");
        assert!(first.starts_with("urn:x: "), "{first}");
    }

    #[test]
    fn a_report_of_nothing_has_no_feed_line_and_a_quiet_delete_is_info() {
        let nothing: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": [], "gone": [], "failed": []
        }))
        .expect("report");
        assert!(feed_event("p", "pl", Cause::Snapshot, &nothing, &[], &[]).is_none());
        let event = feed_event(
            "p",
            "pl",
            Cause::Snapshot,
            &nothing,
            &["urn:a".to_owned()],
            &[],
        )
        .expect("an event");
        assert_eq!(event.severity, "info");
        assert!(
            event.summary.contains("no longer lists"),
            "{}",
            event.summary
        );
    }

    // ---- sweeping marks: a pipeline the mirror lacks, and one that runs on a clock ----

    #[tokio::test]
    async fn the_sweep_leaves_the_marks_of_a_missing_or_clocked_pipeline_for_its_run_end() {
        use crate::resource::ResourceEnvelope as Env;
        let mut clocked_spec = snapshot_spec();
        clocked_spec["sourceDeletes"] = json!({ "mode": "events", "deletedWhen": "this.gone" });
        let clocked: Env = envelope("Pipeline", "clocked", "helsinki", clocked_spec);
        let state = snapshot_world(vec![clocked]);
        for pipeline in ["ghost", "clocked"] {
            state
                .source_ledger
                .seen("helsinki", pipeline, "r0", &["urn:a".to_owned()], true)
                .await
                .expect("seen");
            state
                .source_ledger
                .mark("helsinki", pipeline, "r1", &["urn:a".to_owned()])
                .await
                .expect("mark");
        }
        sweep_marks(&state).await;
        for pipeline in ["ghost", "clocked"] {
            let taken = state
                .source_ledger
                .take_marked("helsinki", pipeline)
                .await
                .expect("take");
            assert_eq!(
                taken,
                Some(("r1".to_owned(), vec!["urn:a".to_owned()])),
                "{pipeline}"
            );
        }
    }

    // ---- starting a delete stream ----

    fn runner_world(runner: Option<&str>, identity: bool, slug: Value) -> AppState {
        let mut config = crate::config::Config::for_tests();
        config.pipeline_runner_url = runner.map(str::to_owned);
        config.pipeline_test_capture_url = Some("http://portal-internal:9090".to_owned());
        config.pipeline_identity = identity;
        let state = AppState::new(config, None);
        state.mirror.upsert(envelope(
            "Endpoint",
            "ep-vehicles",
            "helsinki",
            json!({ "slug": slug, "contextSpaceRef": { "kind": "ContextSpace", "name": "vehicles" } }),
        ));
        state
            .mirror
            .upsert(envelope("Pipeline", "feed", "helsinki", snapshot_spec()));
        state
    }

    async fn runner_taking_streams() -> wiremock::MockServer {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let runner = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("^/streams/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&runner)
            .await;
        Mock::given(method("DELETE"))
            .and(path_regex("^/streams/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&runner)
            .await;
        runner
    }

    async fn requests_of(runner: &wiremock::MockServer, verb: &str) -> Vec<wiremock::Request> {
        runner
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.method.as_str() == verb)
            .collect()
    }

    #[tokio::test]
    async fn a_launch_names_why_it_cannot_start() {
        let ids = ["urn:a".to_owned()];
        let state = snapshot_world(Vec::new());
        assert_eq!(
            launch(&state, "helsinki", "missing", "r1", &ids).await,
            Err("the pipeline is gone".to_owned())
        );
        assert_eq!(
            launch(&state, "helsinki", "feed", "r1", &ids).await,
            Err("the pipeline writes through no Endpoint".to_owned())
        );
        let no_slug = runner_world(Some("http://runner"), false, json!(5));
        assert_eq!(
            launch(&no_slug, "helsinki", "feed", "r1", &ids).await,
            Err("the pipeline writes through no Endpoint".to_owned())
        );
        let no_runner = runner_world(None, false, json!("veh"));
        assert_eq!(
            launch(&no_runner, "helsinki", "feed", "r1", &ids).await,
            Err("no pipeline runner is configured".to_owned())
        );
        let mut config = crate::config::Config::for_tests();
        config.pipeline_runner_url = Some("http://runner".to_owned());
        let no_internal = AppState::new(config, None);
        no_internal.mirror.upsert(envelope(
            "Endpoint",
            "ep-vehicles",
            "helsinki",
            json!({ "slug": "veh" }),
        ));
        no_internal
            .mirror
            .upsert(envelope("Pipeline", "feed", "helsinki", snapshot_spec()));
        assert_eq!(
            launch(&no_internal, "helsinki", "feed", "r1", &ids).await,
            Err("no internal listener is configured for the runner".to_owned())
        );
    }

    #[tokio::test]
    async fn a_launch_cannot_read_when_the_ledger_is_down() {
        let state = runner_world(Some("http://runner"), false, json!("veh"));
        state.mirror.upsert(envelope(
            "Project",
            "helsinki",
            crate::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": "o" }),
        ));
        let state = state.with_db(dead_pool());
        let refused = launch(&state, "helsinki", "feed", "r1", &["urn:a".to_owned()]).await;
        assert!(refused.is_err(), "{refused:?}");
    }

    #[tokio::test]
    async fn a_runner_that_refuses_the_stream_or_is_not_there_fails_the_launch() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let ids = ["urn:a".to_owned()];
        let refusing = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&refusing)
            .await;
        let state = runner_world(Some(&refusing.uri()), false, json!("veh"));
        let answer = launch(&state, "helsinki", "feed", "r1", &ids).await;
        assert!(
            matches!(&answer, Err(reason) if reason.starts_with("the runner answered 500")),
            "{answer:?}"
        );
        let absent = runner_world(Some("http://127.0.0.1:1"), false, json!("veh"));
        let answer = launch(&absent, "helsinki", "feed", "r1", &ids).await;
        assert!(answer.is_err(), "{answer:?}");
    }

    #[tokio::test]
    async fn a_launch_posts_the_stream_under_the_pipelines_own_account_when_identity_is_on() {
        let runner = runner_taking_streams().await;
        let state = runner_world(Some(&runner.uri()), true, json!("veh"));
        let ids = ["urn:a".to_owned(), "bad id".to_owned()];
        let started = launch(&state, "helsinki", "feed", "r1", &ids).await;
        assert_eq!(started, Ok(1), "an id a stream could misread is left out");
        let posted = requests_of(&runner, "POST").await;
        assert_eq!(posted.len(), 1);
        let body = String::from_utf8_lossy(&posted[0].body).into_owned();
        assert!(body.contains("veh"), "{body}");
        assert!(
            posted[0].url.path().contains("source-deletes"),
            "{}",
            posted[0].url
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_started_stream_is_removed_when_its_window_has_passed() {
        let runner = runner_taking_streams().await;
        let state = runner_world(Some(&runner.uri()), false, json!("veh"));
        let started = launch(&state, "helsinki", "feed", "r1", &["urn:a".to_owned()]).await;
        assert_eq!(started, Ok(1));
        assert!(requests_of(&runner, "DELETE").await.is_empty());
        // The removal task starts its window sleep on its first poll, before the clock stops.
        tokio::task::yield_now().await;
        tokio::time::pause();
        tokio::time::advance(STREAM_WINDOW + Duration::from_secs(1)).await;
        let waited = std::time::Instant::now();
        let mut removed = Vec::new();
        while removed.is_empty() && waited.elapsed() < Duration::from_secs(10) {
            // A blocking wait parks the runtime (driving its I/O) without moving the paused clock.
            tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(2)))
                .await
                .expect("the wait");
            removed = requests_of(&runner, "DELETE").await;
        }
        assert_eq!(removed.len(), 1);
        assert!(removed[0].url.path().contains("source-deletes"));
    }

    /// The runner takes the stream and hangs up on its removal. A dropped wiremock server goes
    /// back to its pool and still answers, so the runner here is a socket of the test's own.
    #[tokio::test(flavor = "current_thread")]
    async fn a_stream_the_runner_can_no_longer_remove_is_only_a_warning() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let runner = format!("http://{}", listener.local_addr().expect("address"));
        let hung_up = std::sync::Arc::new(AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&hung_up);
        tokio::spawn(async move {
            let mut answered = false;
            loop {
                let (mut socket, _) = listener.accept().await.expect("a connection");
                let mut request = [0_u8; 8192];
                let _ = socket.read(&mut request).await;
                if answered {
                    drop(socket);
                    seen.store(true, Ordering::SeqCst);
                } else {
                    answered = true;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        )
                        .await;
                    // Read the rest of the request until the client closes, so no reset loses
                    // the answer.
                    while matches!(socket.read(&mut request).await, Ok(read) if read > 0) {}
                }
            }
        });
        let state = runner_world(Some(&runner), false, json!("veh"));
        let (started, log) =
            crate::test_log::logged(tracing::level_filters::LevelFilter::WARN, async {
                let started = launch(&state, "helsinki", "feed", "r1", &["urn:a".to_owned()]).await;
                // The removal task starts its window sleep on its first poll, before the clock stops.
                tokio::task::yield_now().await;
                tokio::time::pause();
                tokio::time::advance(STREAM_WINDOW + Duration::from_secs(1)).await;
                let waited = std::time::Instant::now();
                let mut after = 0;
                while after < 50 && waited.elapsed() < Duration::from_secs(10) {
                    // A blocking wait parks the runtime (driving its I/O) without moving the clock.
                    tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(2)))
                        .await
                        .expect("the wait");
                    after += usize::from(hung_up.load(Ordering::SeqCst));
                }
                started
            })
            .await;
        assert_eq!(started, Ok(1));
        assert!(
            log.contains("a source-delete stream was not removed"),
            "{log}"
        );
    }

    #[tokio::test]
    async fn a_judged_miss_of_an_id_the_memory_ledger_lacks_adds_no_row() {
        let ledger = Ledger::new(None, Default::default());
        ledger
            .seen("p", "x", "r1", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        ledger
            .settle(
                "p",
                "x",
                &[("urn:a".into(), 1), ("urn:stranger".into(), 2)],
                None,
            )
            .await
            .expect("settle");
        let (rows, _) = ledger.book("p", "x").await.expect("book");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["urn:a"].misses, 1);
        // A database ledger has no memory map to fall back on.
        assert!(Ledger::new(Some(dead_pool()), Default::default())
            .memory()
            .is_none());
    }

    #[tokio::test]
    async fn a_report_of_a_pipeline_the_mirror_lacks_settles_the_ledger_and_records_no_removal() {
        let state = snapshot_world(Vec::new());
        state
            .source_ledger
            .seen("helsinki", "gone", "r0", &["urn:a".to_owned()], true)
            .await
            .expect("seen");
        let report: Report = serde_json::from_value(json!({
            "run": "r1", "deleted": ["urn:a"], "gone": [], "failed": []
        }))
        .expect("report");
        reported(&state, "helsinki", "gone", report)
            .await
            .expect("reported");
        let (rows, _) = state
            .source_ledger
            .book("helsinki", "gone")
            .await
            .expect("book");
        assert!(rows.is_empty());
        assert!(state
            .source_ledger
            .removals("helsinki", RemovalsOf::Pipeline("gone"), 10)
            .await
            .expect("removals")
            .is_empty());
    }

    /// A launch that cannot start (no runner) puts the marks back; when the database refuses that
    /// too, the marks are lost and the log says so.
    #[tokio::test]
    async fn marks_the_database_will_not_take_back_are_logged_as_lost() {
        let scratch = crate::test_db::scratch().await;
        let state = events_world().with_db(scratch.pool.clone());
        crate::project_ids::placed(&scratch.pool, &state.mirror, "helsinki", "helsinki-org").await;
        let ids = ["urn:a".to_owned()];
        let ledger = &state.source_ledger;
        ledger
            .seen("helsinki", "pushed", "r0", &ids, true)
            .await
            .expect("seen");
        assert_eq!(
            ledger
                .mark("helsinki", "pushed", "r1", &ids)
                .await
                .expect("mark"),
            ids.to_vec()
        );
        // Refuses a new mark only: taking the marks (marked_run set to NULL) still works.
        sqlx::query(
            "CREATE FUNCTION jc_refuse_mark() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
             IF NEW.marked_run IS NOT NULL THEN RAISE EXCEPTION 'refused by the test'; END IF; \
             RETURN NEW; END $$",
        )
        .execute(&scratch.pool)
        .await
        .expect("trigger function");
        sqlx::query(
            "CREATE TRIGGER jc_refuse_mark BEFORE UPDATE ON pipeline_source_ledger \
             FOR EACH ROW EXECUTE FUNCTION jc_refuse_mark()",
        )
        .execute(&scratch.pool)
        .await
        .expect("trigger");
        let ((), log) = crate::test_log::logged(
            tracing::level_filters::LevelFilter::WARN,
            launch_marked(&state, "helsinki", "pushed"),
        )
        .await;
        assert!(log.contains("the marked deletes were not started"), "{log}");
        assert!(log.contains("the source's delete marks were lost"), "{log}");
        assert_eq!(
            ledger
                .take_marked("helsinki", "pushed")
                .await
                .expect("take"),
            None
        );
        scratch.remove().await;
    }
}
