//! The daily data-quality run (DM-74, API/01 §27).
//!
//! A write is checked when it happens (PL-59, DM-61); data written before its model changed, or
//! by a pipeline that stopped, is not. Once a day the leading replica reads every entity of every
//! space that names a model through the space surface, as its own client, holds each one to the
//! model the way a pipeline's validation stage does, and compares the age of the newest entity a
//! pipeline writes with the pipeline's freshness target. The result is held in memory per space
//! and, with a database, kept there too (T-5066), so a restarted Portal shows the last report. A
//! space is due when its report is missing or a day old, so a run a restart cut short, or a space
//! whose read failed, is taken up at the next try, an hour on; a failed space keeps its previous
//! report until then. Each space reads with a token of its own, so a long run never outlives its
//! token and leaves the spaces at the end of the list unread. A rule
//! and path a space breaks that its previous report did not is new, and opens an `invalid` alert.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use jc_time::TimestampTz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::pipeline_validation::{ModelSchemas, Problem};
use crate::reconciler::drift::Watch;
use crate::store::{ListOptions, Mirror};

/// Entities one page reads, the gateway's own page size.
pub const PAGE: usize = 1_000;
/// Entities one run reads of one space at most: the run's resource budget.
pub const PER_SPACE: usize = 20_000;
/// How often a run is due.
pub const EVERY: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);
/// The least time between two runs: how soon a space that was not read is tried again.
const RETRY: jiff::SignedDuration = jiff::SignedDuration::from_hours(1);
/// Example ids kept per rule.
const EXAMPLES: usize = 5;
/// The least slack a freshness target allows past the interval, in seconds.
const MIN_SLACK: u64 = 600;
/// The pause between two pages, so a run never competes with the people reading a space.
const BETWEEN_PAGES: Duration = Duration::from_millis(100);

/// One failing rule of a space: a SHACL component and the path it is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RuleCount {
    pub rule: String,
    pub path: String,
    pub count: u64,
    /// At most five ids of entities that break it.
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FreshState {
    Fresh,
    Stale,
    Empty,
    Untargeted,
}

/// How recent the data of one pipeline is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Freshness {
    pub pipeline: String,
    /// The pipeline's output type; empty when it names none and the whole space counts.
    #[serde(rename = "type")]
    pub entity_type: String,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub newest: Option<TimestampTz>,
    pub target_seconds: Option<u64>,
    pub state: FreshState,
    pub paused: bool,
}

impl Freshness {
    /// Whether the operators' alert fires: the data is stale and the pipeline is meant to run
    /// (T-5020). A paused pipeline's data ages by choice.
    pub fn alerting(&self) -> bool {
        self.state == FreshState::Stale && !self.paused
    }
}

/// What one run found in one space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceQuality {
    #[schema(value_type = String, format = DateTime)]
    pub observed_at: TimestampTz,
    pub checked: u64,
    pub invalid: u64,
    pub truncated: bool,
    pub rules: Vec<RuleCount>,
    pub freshness: Vec<Freshness>,
    /// Per entity type: how complete each attribute is, the newest change, numeric ranges and
    /// outliers (T-3252).
    pub types: Vec<TypeQuality>,
}

/// What one run found of one entity type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeQuality {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub count: u64,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub newest: Option<TimestampTz>,
    /// Every attribute an entity of the type carries, the most complete first.
    pub attributes: Vec<AttributeQuality>,
}

/// One attribute of one type: how many entities carry it and, for numbers, their range and the
/// values far outside the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttributeQuality {
    pub name: String,
    /// Entities of the type that carry the attribute.
    pub present: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Values outside the Tukey fences (1.5 interquartile ranges past the quartiles).
    pub outliers: u64,
    /// At most five ids of entities holding such a value.
    pub outlier_examples: Vec<String>,
}

/// What a run keeps of one type while it reads it: ids by position, so a number keeps a `u32`.
#[derive(Debug, Default)]
struct TypeTally {
    ids: Vec<String>,
    present: BTreeMap<String, u64>,
    numbers: BTreeMap<String, Vec<(f64, u32)>>,
}

/// The fewest numbers an attribute needs before a value is called an outlier.
const OUTLIER_FROM: usize = 8;

/// The count of one space's entities while a run reads them.
#[derive(Debug, Default)]
pub struct Tally {
    pub checked: u64,
    pub invalid: u64,
    pub truncated: bool,
    rules: BTreeMap<(String, String), (u64, Vec<String>)>,
    /// The newest `modifiedAt` per type.
    newest: BTreeMap<String, TimestampTz>,
    types: BTreeMap<String, TypeTally>,
}

/// The number an attribute holds: a Property's numeric `value`, the first of several instances.
fn number_of(attribute: &Value) -> Option<f64> {
    let first = match attribute {
        Value::Array(items) => items.first()?,
        other => other,
    };
    first
        .get("value")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
}

impl Tally {
    /// Counts one entity with what the model said of it, and its `modifiedAt`.
    pub fn add(&mut self, entity: &Value, modified: Option<TimestampTz>, problems: &[Problem]) {
        self.checked += 1;
        if let (Some(kind), Some(at)) = (entity.get("type").and_then(Value::as_str), modified) {
            let newest = self.newest.entry(kind.to_owned()).or_insert(at);
            *newest = (*newest).max(at);
        }
        if let (Some(kind), Some(object)) = (
            entity.get("type").and_then(Value::as_str),
            entity.as_object(),
        ) {
            let tally = self.types.entry(kind.to_owned()).or_default();
            let at = u32::try_from(tally.ids.len()).unwrap_or(u32::MAX);
            tally.ids.push(
                entity
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
            for (name, attribute) in object {
                if matches!(name.as_str(), "id" | "type" | "@context") || attribute.is_null() {
                    continue;
                }
                *tally.present.entry(name.clone()).or_default() += 1;
                if let Some(number) = number_of(attribute) {
                    tally
                        .numbers
                        .entry(name.clone())
                        .or_default()
                        .push((number, at));
                }
            }
        }
        if problems.is_empty() {
            return;
        }
        self.invalid += 1;
        let id = entity.get("id").and_then(Value::as_str).unwrap_or_default();
        // One entity counts once per rule and path, however many values break it.
        let mut seen = std::collections::BTreeSet::new();
        for problem in problems {
            let key = (problem.rule.clone(), problem.path.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            let (count, examples) = self.rules.entry(key).or_default();
            *count += 1;
            if examples.len() < EXAMPLES && !id.is_empty() {
                examples.push(id.to_owned());
            }
        }
    }

    /// The newest entity of `entity_type`, or of the whole space when it is empty.
    pub fn newest(&self, entity_type: &str) -> Option<TimestampTz> {
        match entity_type {
            "" => self.newest.values().max().copied(),
            kind => self.newest.get(kind).copied(),
        }
    }

    pub fn into_quality(self, observed_at: TimestampTz, freshness: Vec<Freshness>) -> SpaceQuality {
        let mut rules: Vec<RuleCount> = self
            .rules
            .into_iter()
            .map(|((rule, path), (count, examples))| RuleCount {
                rule,
                path,
                count,
                examples,
            })
            .collect();
        // Most frequent first; ties by rule and path, so two runs list them alike.
        rules.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| (&a.rule, &a.path).cmp(&(&b.rule, &b.path)))
        });
        let types = self
            .types
            .into_iter()
            .map(|(entity_type, tally)| {
                let newest = self.newest.get(&entity_type).copied();
                type_quality(entity_type, newest, tally)
            })
            .collect();
        SpaceQuality {
            observed_at,
            checked: self.checked,
            invalid: self.invalid,
            truncated: self.truncated,
            rules,
            freshness,
            types,
        }
    }
}

/// One type's report from what the run kept of it.
fn type_quality(entity_type: String, newest: Option<TimestampTz>, tally: TypeTally) -> TypeQuality {
    let TypeTally {
        ids,
        present,
        mut numbers,
    } = tally;
    let mut attributes: Vec<AttributeQuality> = present
        .into_iter()
        .map(|(name, present)| {
            let mut values = numbers.remove(&name).unwrap_or_default();
            values.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (outliers, outlier_examples) = outliers_of(&values, &ids);
            AttributeQuality {
                min: values.first().map(|v| v.0),
                max: values.last().map(|v| v.0),
                name,
                present,
                outliers,
                outlier_examples,
            }
        })
        .collect();
    attributes.sort_by(|a, b| b.present.cmp(&a.present).then_with(|| a.name.cmp(&b.name)));
    TypeQuality {
        entity_type,
        count: ids.len() as u64,
        newest,
        attributes,
    }
}

/// The values of `sorted` outside the Tukey fences, and up to five ids holding them; nothing
/// below [`OUTLIER_FROM`] values, where quartiles say nothing.
fn outliers_of(sorted: &[(f64, u32)], ids: &[String]) -> (u64, Vec<String>) {
    if sorted.len() < OUTLIER_FROM {
        return (0, Vec::new());
    }
    let quartile = |q: f64| {
        let at = q * (sorted.len() - 1) as f64;
        let (low, high) = (at.floor() as usize, at.ceil() as usize);
        sorted[low].0 + (sorted[high].0 - sorted[low].0) * (at - low as f64)
    };
    let (q1, q3) = (quartile(0.25), quartile(0.75));
    let fence = 1.5 * (q3 - q1);
    let outside: Vec<&(f64, u32)> = sorted
        .iter()
        .filter(|(value, _)| *value < q1 - fence || *value > q3 + fence)
        .collect();
    let examples = outside
        .iter()
        .filter_map(|(_, at)| ids.get(*at as usize))
        .filter(|id| !id.is_empty())
        .take(EXAMPLES)
        .cloned()
        .collect();
    (outside.len() as u64, examples)
}

/// The entity's `modifiedAt`, which `options=sysAttrs` adds. The model check takes the entity as
/// stored: the system members of DM-130 pass a closed class and are never reported (DM-131).
pub fn modified_of(entity: &Value) -> Option<TimestampTz> {
    jc_time::parse_rfc3339_relaxed(entity.get("modifiedAt")?.as_str()?).map(TimestampTz)
}

/// Seconds between two runs of a five-field cron `schedule`, or `None` for one this reading does
/// not understand. Lists and ranges count as their field's unit: `0 6,18 * * *` reads as daily,
/// which only makes the target looser, never a false alarm.
pub fn schedule_seconds(schedule: &str) -> Option<u64> {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields.as_slice() else {
        return None;
    };
    let step = |field: &str| {
        field
            .strip_prefix("*/")
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n > 0)
    };
    if [minute, hour]
        .iter()
        .any(|f| f.starts_with("*/") && step(f).is_none())
    {
        return None;
    }
    let every = if *minute == "*" {
        60
    } else if let Some(n) = step(minute) {
        if *hour != "*" {
            return None;
        }
        n.checked_mul(60)?
    } else if *hour == "*" {
        3_600
    } else if let Some(n) = step(hour) {
        n.checked_mul(3_600)?
    } else if *weekday != "*" {
        7 * 86_400
    } else if *day != "*" && *month != "*" {
        366 * 86_400
    } else if *day != "*" {
        31 * 86_400
    } else {
        86_400
    };
    Some(every)
}

/// The freshness target of a pipeline: its interval plus the larger of a twelfth of it and ten
/// minutes; `None` for a pipeline its source drives.
pub fn target_seconds(period: Option<u64>, schedule: Option<&str>) -> Option<u64> {
    let every = match (period, schedule) {
        (Some(seconds), _) if seconds > 0 => seconds,
        (_, Some(schedule)) => schedule_seconds(schedule)?,
        _ => return None,
    };
    Some(every.saturating_add((every / 12).max(MIN_SLACK)))
}

pub fn fresh_state(
    newest: Option<TimestampTz>,
    target: Option<u64>,
    now: TimestampTz,
) -> FreshState {
    match (newest, target) {
        (_, None) => FreshState::Untargeted,
        (None, Some(_)) => FreshState::Empty,
        (Some(at), Some(target)) if (now - at).as_secs() > target as i64 => FreshState::Stale,
        (Some(_), Some(_)) => FreshState::Fresh,
    }
}

/// The last report of every space, and whether a run is under way.
#[derive(Default)]
pub struct Store {
    by_space: RwLock<BTreeMap<(String, String), SpaceQuality>>,
    /// Per space, the `rule path` pairs its last report broke and the one before did not.
    new_rules: RwLock<BTreeMap<(String, String), Vec<String>>>,
    running: AtomicBool,
    /// When the last run started; the next starts no sooner than `RETRY` after it.
    last_start: RwLock<Option<TimestampTz>>,
    db: Option<sqlx::PgPool>,
    /// What places each project in its Organization: every row is read and written in a
    /// `db::tenant_tx` of its project's key (T-7197 PK-1e).
    mirror: Arc<Mirror>,
}

/// The rules of `now` that `before` did not break, as `rule path`. A space's first report has
/// nothing before it, so every rule it breaks is new.
fn rules_new(before: Option<&SpaceQuality>, now: &SpaceQuality) -> Vec<String> {
    now.rules
        .iter()
        .filter(|r| {
            !before.is_some_and(|b| b.rules.iter().any(|o| o.rule == r.rule && o.path == r.path))
        })
        .map(|r| format!("{} {}", r.rule, r.path))
        .collect()
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("by_space", &self.by_space)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl Store {
    pub fn new(db: Option<sqlx::PgPool>, mirror: Arc<Mirror>) -> Self {
        Self {
            db,
            mirror,
            ..Self::default()
        }
    }

    pub fn get(&self, project: &str, space: &str) -> Option<SpaceQuality> {
        self.by_space
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(project.to_owned(), space.to_owned()))
            .cloned()
    }

    /// The `rule path` pairs the space's last report broke for the first time.
    pub fn new_rules(&self, project: &str, space: &str) -> Vec<String> {
        self.new_rules
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(project.to_owned(), space.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    /// Keeps a space's report and answers the rules it breaks for the first time.
    pub(crate) fn put(&self, project: &str, space: &str, quality: SpaceQuality) -> Vec<String> {
        let key = (project.to_owned(), space.to_owned());
        let mut by_space = self.by_space.write().unwrap_or_else(|e| e.into_inner());
        let fresh = rules_new(by_space.get(&key), &quality);
        by_space.insert(key.clone(), quality);
        self.new_rules
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, fresh.clone());
        fresh
    }

    /// Forgets the spaces that no longer name a model.
    fn keep_only(&self, spaces: &[(String, String)]) {
        self.by_space
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key, _| spaces.contains(key));
        self.new_rules
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key, _| spaces.contains(key));
    }

    /// Reads the kept reports back after a restart, so only the spaces whose report is missing
    /// or a day old are read again. A row that no longer parses is skipped. Project by project in
    /// its key (STO-07), so it waits for the mirror's first sync: [`spawn_load`].
    pub async fn load(&self) -> Result<usize, sqlx::Error> {
        let Some(db) = &self.db else { return Ok(0) };
        let mut rows: Vec<(String, String, Value, Vec<String>)> = Vec::new();
        for (project, key) in crate::project_ids::keyed_projects(db, &self.mirror).await? {
            let mut tx = crate::db::tenant_tx(db, key).await?;
            rows.extend(
                sqlx::query_as::<_, (String, String, Value, Vec<String>)>(
                    "SELECT project, space, report, new_rules FROM space_quality WHERE project = $1",
                )
                .bind(&project)
                .fetch_all(&mut *tx)
                .await?,
            );
            tx.commit().await?;
        }
        let mut loaded = 0;
        for (project, space, report, fresh) in rows {
            let Ok(report) = serde_json::from_value::<SpaceQuality>(report) else {
                continue;
            };
            let key = (project, space);
            self.new_rules
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.clone(), fresh);
            self.by_space
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, report);
            loaded += 1;
        }
        Ok(loaded)
    }

    /// Keeps one space's report, replacing the one before; a project with no tenancy key keeps
    /// it in memory only.
    async fn save(&self, project: &str, space: &str, fresh: &[String]) -> Result<(), sqlx::Error> {
        let (Some(db), Some(report)) = (&self.db, self.get(project, space)) else {
            return Ok(());
        };
        let Some(key) = crate::project_ids::key_of(db, &self.mirror, project).await? else {
            return Ok(());
        };
        let report =
            serde_json::to_value(&report).map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
        let mut tx = crate::db::tenant_tx(db, key).await?;
        sqlx::query(
            "INSERT INTO space_quality (project, space, report, new_rules) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (project, space) DO UPDATE \
             SET report = EXCLUDED.report, new_rules = EXCLUDED.new_rules, at = now()",
        )
        .bind(project)
        .bind(space)
        .bind(report)
        .bind(fresh)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// Drops the kept reports of spaces that no longer name a model, project by project.
    async fn forget_others(&self, spaces: &[(String, String)]) -> Result<(), sqlx::Error> {
        let Some(db) = &self.db else { return Ok(()) };
        for (project, key) in crate::project_ids::keyed_projects(db, &self.mirror).await? {
            let kept: Vec<&str> = spaces
                .iter()
                .filter(|(p, _)| *p == project)
                .map(|(_, s)| s.as_str())
                .collect();
            let mut tx = crate::db::tenant_tx(db, key).await?;
            sqlx::query("DELETE FROM space_quality WHERE project = $1 AND NOT (space = ANY($2))")
                .bind(&project)
                .bind(&kept)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    /// The spaces of `spaces` whose report is missing or [`EVERY`] old at `now`.
    pub fn due(&self, spaces: &[(String, String)], now: TimestampTz) -> Vec<(String, String)> {
        let by_space = self.by_space.read().unwrap_or_else(|e| e.into_inner());
        spaces
            .iter()
            .filter(|key| {
                by_space
                    .get(*key)
                    .is_none_or(|report| now - report.observed_at >= EVERY)
            })
            .cloned()
            .collect()
    }

    /// Whether a run may start at `now`, an hour or more after the last one; when it may, this
    /// call claims it.
    pub fn claim(&self, now: TimestampTz) -> bool {
        let mut last = self.last_start.write().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now - at < RETRY) {
            return false;
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return false;
        }
        *last = Some(now);
        true
    }

    fn finish(&self) {
        self.running.store(false, Ordering::Release);
    }
}

/// Reads the kept reports back once the mirror holds its first sync: a project's key comes from
/// its Project's Organization, which lives only in Git (T-7197). Without a syncer the mirror is
/// what the caller gave, so the reports are read at once.
pub(crate) fn spawn_load(state: crate::state::AppState) {
    if state.db.is_none() {
        return;
    }
    tokio::spawn(async move {
        if let Some(syncer) = state.syncer.clone() {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            while syncer.status().revision.is_none() {
                tick.tick().await;
            }
        }
        if let Err(err) = state.quality.load().await {
            tracing::warn!(error = %err, "the kept data-quality reports were not read");
        }
    });
}

/// What a run reads with and where it leaves its reports.
pub struct Scanner {
    pub watch: Arc<Watch>,
    pub schemas: Arc<ModelSchemas>,
    pub mirror: Arc<Mirror>,
    pub org_domain: String,
    pub store: Arc<Store>,
}

impl Scanner {
    /// Starts a run in the background when a space is due; the reconciler calls it on every
    /// sync of the leader and never waits for it.
    pub fn start_if_due(self: &Arc<Self>) {
        let now = TimestampTz::now();
        let spaces = self.schemas.spaces();
        let due = self.store.due(&spaces, now);
        if due.is_empty() || !self.store.claim(now) {
            return;
        }
        let scanner = Arc::clone(self);
        tokio::spawn(async move {
            scanner.run(&spaces, &due).await;
            scanner.store.finish();
        });
    }

    async fn run(&self, spaces: &[(String, String)], due: &[(String, String)]) {
        for (project, space) in due {
            let read = match self.watch.scan_token().await {
                Ok(token) => self.space(&token, project, space).await,
                Err(err) => Err(err),
            };
            match read {
                Ok(quality) => {
                    for row in &quality.freshness {
                        crate::telemetry::pipeline_stale(project, &row.pipeline, row.alerting());
                    }
                    let fresh = self.store.put(project, space, quality);
                    if let Err(err) = self.store.save(project, space, &fresh).await {
                        tracing::warn!(project, space, error = %err, "a space's report was not kept");
                    }
                }
                // One space that cannot be read keeps its last report; the others go on.
                Err(err) => tracing::warn!(project, space, error = %err, "a space was not checked"),
            }
        }
        self.store.keep_only(spaces);
        if let Err(err) = self.store.forget_others(spaces).await {
            tracing::warn!(error = %err, "the reports of removed spaces were not dropped");
        }
    }

    async fn space(&self, token: &str, project: &str, space: &str) -> Result<SpaceQuality, String> {
        let schema = self
            .schemas
            .get(project, space)
            .ok_or_else(|| format!("{project}/{space} names no model any more"))?;
        let mut tally = Tally::default();
        'classes: for class in schema.classes.keys() {
            let mut offset = 0;
            loop {
                let page = self.watch.page(token, space, class, offset, PAGE).await?;
                let full = page.len() == PAGE;
                for entity in page {
                    if tally.checked as usize >= PER_SPACE {
                        tally.truncated = true;
                        break 'classes;
                    }
                    let modified = modified_of(&entity);
                    let problems = schema.check(&entity);
                    tally.add(&entity, modified, &problems);
                }
                if !full {
                    break;
                }
                offset += PAGE;
                tokio::time::sleep(BETWEEN_PAGES).await;
            }
        }
        let now = TimestampTz::now();
        let freshness = pipelines_into(&self.mirror, project, space)
            .into_iter()
            .map(|(pipeline, entity_type, target, paused)| {
                let newest = tally.newest(&entity_type);
                Freshness {
                    pipeline,
                    state: fresh_state(newest, target, now),
                    entity_type,
                    newest,
                    target_seconds: target,
                    paused,
                }
            })
            .collect();
        Ok(tally.into_quality(now, freshness))
    }
}

/// Every pipeline of `project` whose first output Endpoint writes into `space`: its name, output
/// type, freshness target and whether it is paused.
pub fn pipelines_into(
    mirror: &Mirror,
    project: &str,
    space: &str,
) -> Vec<(String, String, Option<u64>, bool)> {
    let mut rows: Vec<_> = mirror
        .list(project, "Pipeline", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| {
            let spec: jc_core::kinds::pipeline::PipelineSpec =
                serde_json::from_value(env.spec).ok()?;
            let output = spec.outputs().into_iter().next()?;
            let endpoint = mirror.get(project, "Endpoint", output.target_endpoint.local_id())?;
            let target_space = crate::api::assistant::ref_name(&endpoint.spec["contextSpaceRef"])?;
            (target_space == space).then(|| {
                (
                    env.metadata.name,
                    output.entity_type.unwrap_or_default(),
                    target_seconds(spec.period_seconds(), spec.schedule.as_deref()),
                    !spec.enabled,
                )
            })
        })
        .collect();
    rows.sort();
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> TimestampTz {
        text.parse().unwrap()
    }

    /// F-POR4-018: a state built `with_db` keeps its quality reports in that database, as the
    /// served state does; the two store wirings used to differ by this store.
    #[tokio::test]
    async fn a_state_with_a_database_keeps_its_quality_reports_there() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://portal@localhost/never-reached")
            .expect("a lazy pool");
        let state =
            crate::state::AppState::new(crate::config::Config::for_tests(), None).with_db(pool);
        assert!(state.quality.db.is_some());
    }

    /// A scanner of space `air` of project `helsinki`, whose model holds the class `Bus`, reading
    /// through a realm `city` on `server` that hands the Portal's client token `t`.
    async fn scanner_on(server: &wiremock::MockServer) -> Arc<Scanner> {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/realms/city/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "t"})))
            .mount(server)
            .await;
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            crate::auth::client_auth::ClientAuth::Secret("s".into()),
        );
        let schema = json!({ "definitions": { "Bus": { "type": "object", "properties": {
            "speed": { "type": "number", "x-ngsi-ld-kind": "Property" }
        }}}});
        let model = crate::pipeline_validation::ModelSchema::compile(
            "transport",
            "1.0.0",
            &schema,
            &["Bus".to_owned()],
            false,
        );
        let schemas = Arc::new(ModelSchemas::default());
        schemas.replace(std::collections::HashMap::from([(
            ("helsinki".to_owned(), "air".to_owned()),
            Arc::new(model),
        )]));
        let mirror = Arc::new(Mirror::new());
        Arc::new(Scanner {
            watch: Arc::new(watch),
            schemas,
            mirror: mirror.clone(),
            org_domain: "hel.fi".into(),
            store: Arc::new(Store::new(None, mirror)),
        })
    }

    fn bus(n: u32, speed: Value) -> Value {
        json!({ "id": format!("urn:ngsi-ld:Bus:{n}"), "type": "Bus",
                "modifiedAt": "2026-11-24T08:00:00Z",
                "speed": { "type": "Property", "value": speed } })
    }

    /// DM-74: a run reads every class of a due space page by page with the scan's token and keeps
    /// the space's report; a space nobody lists any more is dropped from the store.
    #[tokio::test]
    async fn a_run_reads_each_due_space_and_keeps_its_report() {
        use wiremock::matchers::{bearer_token, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let scanner = scanner_on(&server).await;
        Mock::given(method("GET"))
            .and(path("/cs/air/ngsi-ld/v1/entities"))
            .and(query_param("type", "Bus"))
            .and(bearer_token("t"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([bus(1, json!(40)), bus(2, json!("fast"))])),
            )
            .expect(1)
            .mount(&server)
            .await;
        scanner.store.put("helsinki", "gone", report(&[]));
        let spaces = scanner.schemas.spaces();
        scanner.run(&spaces, &spaces).await;
        let kept = scanner.store.get("helsinki", "air").expect("a report");
        assert_eq!((kept.checked, kept.invalid, kept.truncated), (2, 1, false));
        assert_eq!(kept.types[0].entity_type, "Bus");
        assert_eq!(kept.types[0].newest, Some(at("2026-11-24T08:00:00Z")));
        assert!(
            scanner.store.get("helsinki", "gone").is_none(),
            "no model, no report"
        );
    }

    /// A space whose read fails keeps its last report and the run goes on: a refused page, a
    /// realm that hands no token, a space whose model went away.
    #[tokio::test]
    async fn a_space_that_cannot_be_read_keeps_its_last_report() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let scanner = scanner_on(&server).await;
        Mock::given(method("GET"))
            .and(path("/cs/air/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let before = report(&[("sh:datatype", "speed")]);
        scanner.store.put("helsinki", "air", before.clone());
        let air = [("helsinki".to_owned(), "air".to_owned())];
        scanner.run(&air, &air).await;
        assert_eq!(scanner.store.get("helsinki", "air"), Some(before.clone()));
        assert_eq!(
            scanner.space("t", "helsinki", "air").await,
            Err("air answered 503 Service Unavailable for Bus".to_owned())
        );
        assert_eq!(
            scanner.space("t", "helsinki", "water").await,
            Err("helsinki/water names no model any more".to_owned())
        );

        let tokenless = MockServer::start().await;
        let mut refused = Arc::try_unwrap(scanner_on(&tokenless).await)
            .ok()
            .expect("one owner");
        refused.watch = Arc::new(Watch::new(
            tokenless.uri(),
            &format!("{}/realms/elsewhere", tokenless.uri()),
            "portal".to_owned(),
            crate::auth::client_auth::ClientAuth::Secret("s".into()),
        ));
        refused.store.put("helsinki", "air", before.clone());
        refused.run(&air, &air).await;
        assert_eq!(refused.store.get("helsinki", "air"), Some(before));
    }

    /// The reconciler's call starts a run only when a space is due and none runs; the run marks
    /// itself finished, and the next start waits an hour.
    #[tokio::test]
    async fn a_run_starts_when_a_space_is_due_and_finishes() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let scanner = scanner_on(&server).await;
        Mock::given(path("/cs/air/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([bus(1, json!(3))])))
            .mount(&server)
            .await;
        scanner.start_if_due();
        for _ in 0..200 {
            if scanner.store.get("helsinki", "air").is_some()
                && !scanner.store.running.load(Ordering::Acquire)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            scanner.store.get("helsinki", "air").map(|q| q.checked),
            Some(1)
        );
        assert!(!scanner.store.running.load(Ordering::Acquire), "finished");
        scanner.store.by_space.write().unwrap().clear();
        scanner.start_if_due();
        assert!(
            !scanner.store.running.load(Ordering::Acquire),
            "an hour has not passed"
        );
        let nothing_due = scanner_on(&MockServer::start().await).await;
        nothing_due
            .schemas
            .replace(std::collections::HashMap::new());
        nothing_due.start_if_due();
        assert!(
            nothing_due.store.last_start.read().unwrap().is_none(),
            "nothing due, no claim"
        );
    }

    /// A run already going refuses a second claim even when no start was recorded yet.
    #[test]
    fn a_running_scan_refuses_a_second_claim() {
        let store = Store::new(None, Arc::new(Mirror::new()));
        store.running.store(true, Ordering::Release);
        assert!(!store.claim(at("2026-11-24T09:00:00Z")));
        assert!(store.last_start.read().unwrap().is_none(), "no start kept");
        store.finish();
        assert!(store.claim(at("2026-11-24T09:00:00Z")));
    }

    /// The scan stops at PER_SPACE entities and says so, reading page after page while each
    /// page is full; the freshness rows name each pipeline writing into the space, and a store
    /// that cannot be reached keeps the report in memory and logs the failed save.
    #[tokio::test]
    async fn a_large_space_is_read_page_by_page_and_cut_at_its_limit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let scanner = scanner_on(&server).await;
        let page: Vec<Value> = (0..PAGE as u32).map(|n| bus(n, json!(n))).collect();
        Mock::given(method("GET"))
            .and(path("/cs/air/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(&server)
            .await;
        let m = &scanner.mirror;
        m.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: crate::resource::ObjectMeta {
                name: "air-in".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: json!({ "contextSpaceRef": "air" }),
            status: None,
        });
        let pipeline = |name: &str, spec: Value| crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Pipeline".into(),
            metadata: crate::resource::ObjectMeta {
                name: name.into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        let writing = |endpoint: &str| {
            json!({
                "class": "scheduled", "schedule": "0 * * * *", "enabled": false,
                "source": { "dataSourceRef": "feed" },
                "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:air:{endpoint}"),
                "output": { "type": "Bus", "mode": "upsert", "id": {
                    "mint": "template", "template": "urn:ngsi-ld:Bus:{n}"
                } }
            })
        };
        m.upsert(pipeline("buses", writing("air-in")));
        m.upsert(pipeline("elsewhere", writing("no-such-endpoint")));
        m.upsert(pipeline("broken", json!({ "class": 7 })));
        assert_eq!(
            pipelines_into(m, "helsinki", "air"),
            [(
                "buses".to_owned(),
                "Bus".to_owned(),
                target_seconds(None, Some("0 * * * *")),
                true
            )]
        );
        assert!(pipelines_into(m, "helsinki", "water").is_empty());
        let quality = scanner
            .space("t", "helsinki", "air")
            .await
            .expect("a report");
        assert!(quality.truncated, "cut at PER_SPACE");
        assert_eq!(quality.checked as usize, PER_SPACE);
        assert_eq!(quality.freshness.len(), 1, "{:?}", quality.freshness);
        assert_eq!(
            (
                quality.freshness[0].pipeline.as_str(),
                quality.freshness[0].paused
            ),
            ("buses", true)
        );
        let asked = server.received_requests().await.unwrap_or_default();
        assert_eq!(
            asked
                .iter()
                .filter(|r| r.url.path().ends_with("/entities"))
                .count(),
            PER_SPACE / PAGE + 1,
            "every page was full, so one more was asked and cut"
        );

        // A store whose database does not answer: the report stays in memory, the run goes on.
        let lazy = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy("postgres://portal@127.0.0.1:1/never-reached")
            .expect("a lazy pool");
        m.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Project".into(),
            metadata: crate::resource::ObjectMeta {
                name: "helsinki".into(),
                namespace: Some(crate::permissions::ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec: json!({ "organizationRef": "hel" }),
            status: None,
        });
        let down = Scanner {
            watch: scanner.watch.clone(),
            schemas: scanner.schemas.clone(),
            mirror: scanner.mirror.clone(),
            org_domain: "hel.fi".into(),
            store: Arc::new(Store::new(Some(lazy), scanner.mirror.clone())),
        };
        let air = [("helsinki".to_owned(), "air".to_owned())];
        down.run(&air, &air).await;
        assert!(down
            .store
            .get("helsinki", "air")
            .is_some_and(|q| q.truncated));
        assert!(down.store.save("helsinki", "air", &[]).await.is_err());
        assert!(down.store.forget_others(&air).await.is_err());
    }

    /// The first instance of several is the attribute's number; an entity with no type adds to
    /// no type's tally.
    #[test]
    fn several_instances_read_as_the_first_and_an_untyped_entity_has_no_tally() {
        assert_eq!(
            number_of(&json!([{ "type": "Property", "value": 3 }, { "value": 9 }])),
            Some(3.0)
        );
        assert_eq!(number_of(&json!([])), None);
        let mut tally = Tally::default();
        tally.add(&json!({ "id": "urn:ngsi-ld:Bus:1" }), None, &[]);
        tally.add(&json!(["not", "an", "entity"]), None, &[]);
        let quality = tally.into_quality(at("2026-11-24T09:00:00Z"), Vec::new());
        assert_eq!(quality.checked, 2);
        assert!(quality.types.is_empty(), "{:?}", quality.types);
    }

    /// Without a database there is nothing to read back, and no load starts.
    #[tokio::test]
    async fn without_a_database_nothing_is_read_back() {
        let store = Store::new(None, Arc::new(Mirror::new()));
        assert_eq!(store.load().await.expect("nothing to read"), 0);
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        spawn_load(state.clone());
        assert!(state.quality.get("helsinki", "air").is_none());
        // A database that does not answer: the load is logged as not read and the store stays
        // empty, so every space is due at the next start.
        let lazy = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy("postgres://portal@127.0.0.1:1/never-reached")
            .expect("a lazy pool");
        let down =
            crate::state::AppState::new(crate::config::Config::for_tests(), None).with_db(lazy);
        down.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Project".into(),
            metadata: crate::resource::ObjectMeta {
                name: "helsinki".into(),
                namespace: Some(crate::permissions::ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec: json!({ "organizationRef": "hel" }),
            status: None,
        });
        assert!(down.quality.load().await.is_err());
        spawn_load(down.clone());
        // Past the pool's 1 s acquire timeout, so the background load has failed by now.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(down.quality.get("helsinki", "air").is_none());
    }

    /// A thread that panicked holding the store's maps leaves them usable.
    #[test]
    fn a_poisoned_store_still_reads_and_writes() {
        let store = Store::new(None, Arc::new(Mirror::new()));
        std::thread::scope(|s| {
            let _ = s
                .spawn(|| {
                    let _a = store.by_space.write().unwrap();
                    let _b = store.new_rules.write().unwrap();
                    let _c = store.last_start.write().unwrap();
                    panic!("poison every map");
                })
                .join();
        });
        assert_eq!(
            store.put("p", "s", report(&[("In", "status")])),
            ["In status"]
        );
        assert!(store.get("p", "s").is_some());
        assert_eq!(store.new_rules("p", "s"), ["In status"]);
        assert_eq!(store.new_rules("p", "other"), Vec::<String>::new());
        let now = at("2026-11-24T09:00:00Z");
        assert!(
            store
                .due(&[("p".into(), "new".into())], now)
                .contains(&("p".to_owned(), "new".to_owned())),
            "a poisoned store still tells which spaces are due"
        );
        assert!(store.claim(now));
        store.keep_only(&[]);
        assert!(store.get("p", "s").is_none());
        assert!(format!("{store:?}").starts_with("Store {"));
    }

    fn problem(rule: &str, path: &str) -> Problem {
        Problem {
            rule: rule.into(),
            path: path.into(),
            message: "m".into(),
        }
    }

    // T-3252: per type, how many entities carry each attribute, the numeric range, and the one
    // value far outside the rest named by its entity.
    #[test]
    fn a_tally_reports_completeness_ranges_and_outliers_per_type() {
        let mut tally = Tally::default();
        for n in 0..10 {
            let capacity = if n == 7 { 400 } else { 10 + n };
            let mut station = json!({
                "id": format!("urn:ngsi-ld:Station:hel.fi:bikes:{n}"),
                "type": "Station",
                "capacity": { "type": "Property", "value": capacity },
            });
            if n % 2 == 0 {
                station["name"] = json!({ "type": "Property", "value": format!("S{n}") });
            }
            tally.add(&station, Some(at("2026-10-07T08:00:00Z")), &[]);
        }
        tally.add(
            &json!({ "id": "urn:ngsi-ld:Street:hel.fi:bikes:a", "type": "Street" }),
            None,
            &[],
        );
        let quality = tally.into_quality(at("2026-10-07T09:00:00Z"), Vec::new());

        let station = quality
            .types
            .iter()
            .find(|kind| kind.entity_type == "Station")
            .unwrap();
        assert_eq!(station.count, 10);
        assert_eq!(station.newest, Some(at("2026-10-07T08:00:00Z")));
        let names: Vec<(&str, u64)> = station
            .attributes
            .iter()
            .map(|a| (a.name.as_str(), a.present))
            .collect();
        assert_eq!(names, vec![("capacity", 10), ("name", 5)]);
        let capacity = &station.attributes[0];
        assert_eq!((capacity.min, capacity.max), (Some(10.0), Some(400.0)));
        assert_eq!(capacity.outliers, 1);
        assert_eq!(
            capacity.outlier_examples,
            vec!["urn:ngsi-ld:Station:hel.fi:bikes:7".to_owned()]
        );
        let name = &station.attributes[1];
        assert_eq!((name.min, name.outliers), (None, 0));

        let street = quality
            .types
            .iter()
            .find(|kind| kind.entity_type == "Street")
            .unwrap();
        assert_eq!((street.count, street.attributes.len()), (1, 0));
    }

    #[test]
    fn too_few_numbers_call_nothing_an_outlier() {
        let values: Vec<(f64, u32)> = vec![(1.0, 0), (2.0, 1), (1000.0, 2)];
        assert_eq!(
            outliers_of(&values, &["a".into(), "b".into(), "c".into()]),
            (0, Vec::new())
        );
    }

    #[test]
    fn a_tally_counts_entities_once_per_rule_and_keeps_five_examples() {
        let mut tally = Tally::default();
        for n in 0..7 {
            let entity =
                json!({"id": format!("urn:ngsi-ld:Station:hel.fi:bikes:{n}"), "type": "Station"});
            let problems = [
                problem("sh:minCount", "name"),
                problem("sh:minCount", "name"),
                problem("sh:in", "status"),
            ];
            tally.add(
                &entity,
                Some(at("2026-09-25T01:00:00Z")),
                &problems[..if n < 2 { 3 } else { 1 }],
            );
        }
        tally.add(
            &json!({"id": "urn:ok", "type": "Station"}),
            Some(at("2026-09-25T02:00:00Z")),
            &[],
        );
        let quality = tally.into_quality(at("2026-09-25T03:00:00Z"), Vec::new());
        assert_eq!((quality.checked, quality.invalid), (8, 7));
        assert_eq!(quality.rules[0].rule, "sh:minCount");
        assert_eq!(
            quality.rules[0].count, 7,
            "a record breaking a rule twice counts once"
        );
        assert_eq!(quality.rules[0].examples.len(), 5);
        assert_eq!(
            (quality.rules[1].rule.as_str(), quality.rules[1].count),
            ("sh:in", 2)
        );
    }

    #[test]
    fn the_newest_entity_is_per_type_or_of_the_whole_space() {
        let mut tally = Tally::default();
        tally.add(
            &json!({"id": "a", "type": "A"}),
            Some(at("2026-09-25T01:00:00Z")),
            &[],
        );
        tally.add(
            &json!({"id": "b", "type": "B"}),
            Some(at("2026-09-25T02:00:00Z")),
            &[],
        );
        tally.add(
            &json!({"id": "c", "type": "A"}),
            Some(at("2026-09-24T01:00:00Z")),
            &[],
        );
        assert_eq!(tally.newest("A"), Some(at("2026-09-25T01:00:00Z")));
        assert_eq!(tally.newest(""), Some(at("2026-09-25T02:00:00Z")));
        assert_eq!(tally.newest("C"), None);
    }

    /// DM-74, DM-131: a stored entity with its system members, an entity and an attribute
    /// `expiresAt` among them, gives no problem against a closed class; an undeclared attribute
    /// still does.
    #[test]
    fn a_stored_entity_with_its_system_members_passes_a_closed_class() {
        let model = jcctl::model_schema::ModelSchema::compile(
            "m",
            "1.0.0",
            &json!({ "definitions": { "Bus": {
                "additionalProperties": false,
                "properties": { "id": { "type": "string" }, "speed": { "type": "number", "x-ngsi-ld-kind": "Property" } }
            } } }),
            &["Bus".into()],
            false,
        );
        let mut entity = json!({
            "id": "urn:ngsi-ld:Bus:hsl:1", "type": "Bus", "scope": "/fi/helsinki",
            "createdAt": "2026-09-01T00:00:00Z", "modifiedAt": "2026-09-25T01:00:00Z",
            "expiresAt": "2026-09-25T01:05:00Z",
            "speed": {"type": "Property", "value": 9.5, "observedAt": "2026-09-25T01:00:00Z",
                      "expiresAt": "2026-09-25T01:05:00Z", "createdAt": "2026-09-01T00:00:00Z",
                      "modifiedAt": "2026-09-25T01:00:00Z", "datasetId": "urn:d"}
        });
        assert_eq!(modified_of(&entity), Some(at("2026-09-25T01:00:00Z")));
        assert_eq!(model.check(&entity), []);
        entity["colour"] = json!({"type": "Property", "value": "blue"});
        let rules: Vec<(String, String)> = model
            .check(&entity)
            .into_iter()
            .map(|p| (p.rule, p.path))
            .collect();
        assert_eq!(rules, [("sh:closed".to_owned(), "colour".to_owned())]);
    }

    #[test]
    fn a_cron_schedule_reads_as_its_interval() {
        assert_eq!(schedule_seconds("*/5 * * * *"), Some(300));
        assert_eq!(schedule_seconds("* * * * *"), Some(60));
        assert_eq!(schedule_seconds("15 * * * *"), Some(3_600));
        assert_eq!(schedule_seconds("0 */6 * * *"), Some(21_600));
        assert_eq!(schedule_seconds("0 2 * * *"), Some(86_400));
        assert_eq!(schedule_seconds("0 2 * * 1"), Some(604_800));
        assert_eq!(schedule_seconds("0 2 1 * *"), Some(2_678_400));
        assert_eq!(schedule_seconds("@daily"), None);
        assert_eq!(schedule_seconds("*/5 2 * * *"), None);
        assert_eq!(schedule_seconds("*/0 * * * *"), None);
        assert_eq!(schedule_seconds("*/x * * * *"), None);
        assert_eq!(schedule_seconds("0 */-1 * * *"), None);
        assert_eq!(schedule_seconds("* * * *"), None, "four fields");
        assert_eq!(
            schedule_seconds("0 2 1 6 *"),
            Some(366 * 86_400),
            "once a year"
        );
    }

    /// A step past what seconds can count reads as no schedule (found by the schedule fuzz
    /// target: `n * 60` overflowed and panicked on a manifest's cron text).
    #[test]
    fn a_step_too_large_to_count_reads_as_no_schedule() {
        let huge = format!("*/{} * * * *", u64::MAX / 2);
        assert_eq!(schedule_seconds(&huge), None);
        let huge = format!("0 */{} * * *", u64::MAX / 1000);
        assert_eq!(schedule_seconds(&huge), None);
        assert_eq!(
            target_seconds(Some(u64::MAX), None),
            Some(u64::MAX),
            "the slack saturates instead of wrapping to a target in the past"
        );
    }

    #[test]
    fn a_target_is_the_interval_and_its_slack() {
        // The owner's two examples: about ten minutes for a real-time feed, 26 h for a daily run.
        assert_eq!(target_seconds(Some(15), None), Some(615));
        assert_eq!(target_seconds(None, Some("0 2 * * *")), Some(93_600));
        assert_eq!(
            target_seconds(Some(300), Some("0 2 * * *")),
            Some(900),
            "the period wins"
        );
        assert_eq!(
            target_seconds(None, None),
            None,
            "a pipeline its source drives"
        );
        assert_eq!(target_seconds(None, Some("@hourly")), None);
    }

    #[test]
    fn data_is_fresh_stale_empty_or_untargeted() {
        let now = at("2026-09-25T03:00:00Z");
        assert_eq!(
            fresh_state(Some(at("2026-09-25T02:55:00Z")), Some(615), now),
            FreshState::Fresh
        );
        assert_eq!(
            fresh_state(Some(at("2026-09-25T02:40:00Z")), Some(615), now),
            FreshState::Stale
        );
        assert_eq!(fresh_state(None, Some(615), now), FreshState::Empty);
        assert_eq!(
            fresh_state(Some(at("2026-01-01T00:00:00Z")), None, now),
            FreshState::Untargeted
        );
    }

    // T-5020: dev's road-weather (every 10 min, target 20 min) read five weeks old and no one
    // was told. A planted stale source alerts; the same age on a paused pipeline does not.
    #[test]
    fn stale_data_of_a_running_pipeline_alerts() {
        let now = at("2026-10-21T12:40:00Z");
        let target = target_seconds(Some(600), Some("*/10 * * * *"));
        assert_eq!(target, Some(1_200), "twice the schedule");
        let row = |newest: &str, paused: bool| Freshness {
            pipeline: "road-weather".into(),
            entity_type: "WeatherObserved".into(),
            newest: Some(at(newest)),
            target_seconds: target,
            state: fresh_state(Some(at(newest)), target, now),
            paused,
        };
        assert!(row("2026-09-14T07:00:00Z", false).alerting());
        assert!(!row("2026-09-14T07:00:00Z", true).alerting());
        assert!(!row("2026-10-21T12:38:49Z", false).alerting());
        assert!(
            !row("2026-10-21T12:20:01Z", false).alerting(),
            "inside 2x the schedule"
        );
    }

    #[test]
    fn a_run_is_claimed_one_at_a_time_and_an_hour_apart() {
        let store = Store::default();
        let start = at("2026-09-25T02:00:00Z");
        assert!(store.claim(start));
        assert!(!store.claim(start), "one run at a time");
        store.finish();
        assert!(!store.claim(start + jiff::SignedDuration::from_mins(59)));
        assert!(store.claim(start + jiff::SignedDuration::from_hours(1)));
    }

    /// A run cut short by a restart, or a space whose read failed, leaves spaces without a
    /// report: those stay due, while a space read today is not (helsinki on dev, 2026-10-22).
    #[test]
    fn a_space_is_due_until_it_has_a_report_less_than_a_day_old() {
        let store = Store::default();
        let old = report(&[]);
        let day = old.observed_at + jiff::SignedDuration::from_hours(24);
        store.put("p", "old", old.clone());
        store.put(
            "p",
            "read",
            SpaceQuality {
                observed_at: day - jiff::SignedDuration::from_hours(1),
                ..old
            },
        );
        let spaces: Vec<(String, String)> = ["old", "read", "unread"]
            .iter()
            .map(|space| ("p".to_owned(), (*space).to_owned()))
            .collect();
        let due = |now| store.due(&spaces, now);
        assert_eq!(
            due(day),
            vec![
                ("p".to_owned(), "old".to_owned()),
                ("p".to_owned(), "unread".to_owned())
            ]
        );
        assert_eq!(due(day + jiff::SignedDuration::from_hours(23)), spaces);
    }

    fn report(rules: &[(&str, &str)]) -> SpaceQuality {
        let mut tally = Tally::default();
        for (rule, path) in rules {
            tally.add(
                &json!({ "id": "urn:ngsi-ld:T:1", "type": "T" }),
                None,
                &[problem(rule, path)],
            );
        }
        tally.into_quality(at("2026-10-21T02:00:00Z"), Vec::new())
    }

    #[test]
    fn a_rule_is_new_once_and_a_report_survives_its_json() {
        let store = Store::default();
        let first = report(&[("MinInclusive", "pm10")]);
        assert_eq!(
            store.put("p", "s", first.clone()),
            vec!["MinInclusive pm10"]
        );
        assert_eq!(
            store.put("p", "s", first.clone()),
            Vec::<String>::new(),
            "seen the day before"
        );
        let fresh = store.put(
            "p",
            "s",
            report(&[("MinInclusive", "pm10"), ("In", "status")]),
        );
        assert_eq!(fresh, vec!["In status"]);
        assert_eq!(store.new_rules("p", "s"), vec!["In status"]);
        store.keep_only(&[]);
        assert!(store.new_rules("p", "s").is_empty());
        let back: SpaceQuality =
            serde_json::from_value(serde_json::to_value(&first).expect("json")).expect("a report");
        assert_eq!(back, first, "what the database keeps reads back the same");
    }

    /// A restarted Portal reads the last reports back, with their new rules, and a space read
    /// today is not due again; a space that no longer names a model leaves the table.
    #[tokio::test]
    async fn a_kept_report_comes_back_after_a_restart() {
        let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let db = crate::db::connect(&url).await.expect("connect and migrate");
        // A project of its own run, placed in an Organization of the registry (T-7197).
        let run = jc_types::ProjectId::mint().path_segment();
        let t5066 = format!("t5066-{run}");
        let mirror = Arc::new(Mirror::new());
        crate::project_ids::placed(&db, &mirror, &t5066, &format!("t5066org-{run}")).await;
        let t5066 = t5066.as_str();
        let before = Store::new(Some(db.clone()), mirror.clone());
        let kept = report(&[("In", "status")]);
        let fresh = before.put(t5066, "air", kept.clone());
        before.save(t5066, "air", &fresh).await.expect("save");
        before.put(t5066, "gone", report(&[]));
        before.save(t5066, "gone", &[]).await.expect("save");
        before
            .forget_others(&[(t5066.into(), "air".into())])
            .await
            .expect("forget");
        // A project with no tenancy key keeps its report in memory only.
        before.put("t5066-unplaced", "air", report(&[]));
        before
            .save("t5066-unplaced", "air", &[])
            .await
            .expect("nothing to save");
        // A kept row that no longer reads as a report is skipped on load.
        let key = crate::project_ids::key_of(&db, &mirror, t5066)
            .await
            .expect("registry")
            .expect("a key");
        let mut tx = crate::db::tenant_tx(&db, key).await.expect("keyed");
        sqlx::query(
            "INSERT INTO space_quality (project, space, report, new_rules) VALUES ($1, 'old', $2, '{}')",
        )
        .bind(t5066)
        .bind(json!({ "not": "a report" }))
        .execute(&mut *tx)
        .await
        .expect("an old row");
        tx.commit().await.expect("commit");

        let after = Store::new(Some(db.clone()), mirror.clone());
        std::thread::scope(|s| {
            let _ = s
                .spawn(|| {
                    let _a = after.by_space.write().unwrap();
                    let _b = after.new_rules.write().unwrap();
                    panic!("poison the maps a load writes");
                })
                .join();
        });
        assert_eq!(
            after.load().await.expect("load"),
            1,
            "the old row is skipped"
        );
        assert_eq!(after.get(t5066, "air"), Some(kept.clone()));
        assert_eq!(after.get(t5066, "gone"), None);
        assert_eq!(after.new_rules(t5066, "air"), vec!["In status"]);
        assert!(
            after
                .due(
                    &[(t5066.into(), "air".into())],
                    kept.observed_at + jiff::SignedDuration::from_hours(23)
                )
                .is_empty(),
            "not due yet"
        );
        // spawn_load reads them back for a state with a database and no syncer.
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None)
            .with_db(db.clone());
        for env in mirror.matching(|e| matches!(e.kind.as_str(), "Project" | "Organization")) {
            state.mirror.upsert(env);
        }
        spawn_load(state.clone());
        let mut loaded = false;
        for _ in 0..200 {
            if state.quality.get(t5066, "air").is_some() {
                loaded = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(loaded, "spawn_load read the kept report back");
    }
}
