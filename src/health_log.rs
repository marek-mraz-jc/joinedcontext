//! The health log (T-5067): what is broken in a project and what to do about it. After the alert
//! run of the leader, every Pipeline, ContextSpace, Endpoint, DataModel and DataSource of the
//! mirror is judged by its checks; each check keeps one row with its status, since when that
//! status holds, when it was last ok, what it said (masked) and a fix hint naming the manifest
//! field or the page to change. The statuses over time are kept 90 days, one point per change or
//! per hour, for the sparkline. The alerts read the same signals (`alerts::read_pipelines`).
//!
//! Checks: `reconcile` for every resource (its status phase and conditions); for a pipeline also
//! `runs` (its run health and Error phase, T-5048) and `freshness` (the space's quality report).
//! ponytail: the surface probes (NGSI-LD, OData, OGC ... with the space's read account) and the
//! SHACL report of T-5066 add checks here once they exist; the table already takes their rows.

use std::collections::BTreeMap;
use std::sync::RwLock;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::activity::{chrono_to_odt, odt_to_chrono};
use crate::alerts::{rfc3339, Event, Signal};
use crate::resource::{Condition, Phase, ResourceEnvelope};
use crate::store::Mirror;

/// How long the history is kept.
pub const KEEP: Duration = Duration::days(90);
/// A status that holds gets a history point once an hour.
const SAMPLE: Duration = Duration::hours(1);
/// The leader writes the log at most this often; a broken surface turns red within it.
pub const EVERY: Duration = Duration::minutes(2);
/// The history one row carries: the last 7 days.
const SHOWN: Duration = Duration::days(7);
/// The longest detail kept, in characters.
const MAX_DETAIL: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = HealthCheckKind)]
pub enum Kind {
    Pipeline,
    Space,
    Endpoint,
    Model,
    Datasource,
}

/// The manifest kind of each health kind, in the order the page lists them.
const KINDS: [(&str, Kind); 5] = [
    ("Pipeline", Kind::Pipeline),
    ("ContextSpace", Kind::Space),
    ("Endpoint", Kind::Endpoint),
    ("DataModel", Kind::Model),
    ("DataSource", Kind::Datasource),
];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
            Self::Space => "space",
            Self::Endpoint => "endpoint",
            Self::Model => "model",
            Self::Datasource => "datasource",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        KINDS
            .iter()
            .map(|(_, kind)| *kind)
            .find(|kind| kind.as_str() == text)
    }

    /// The manifest kind the permission check asks about.
    pub fn manifest_kind(self) -> &'static str {
        KINDS
            .iter()
            .find(|(_, kind)| *kind == self)
            .map_or("", |(manifest, _)| manifest)
    }

    /// The Portal page of a resource of this kind.
    fn page(self, project: &str, name: &str) -> String {
        let plural = match self {
            Self::Pipeline => "pipelines",
            Self::Space => "spaces",
            Self::Endpoint => "endpoints",
            Self::Model => "models",
            Self::Datasource => "datasources",
        };
        format!("/projects/{project}/{plural}/{name}")
    }
}

/// Errors first, then warnings, then what is well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = HealthCheckStatus)]
pub enum Status {
    Error,
    Warn,
    Ok,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "ok" => Self::Ok,
            "warn" => Self::Warn,
            _ => Self::Error,
        }
    }
}

/// What one check found in one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub project: String,
    pub kind: Kind,
    pub name: String,
    pub check: String,
    pub status: Status,
    pub detail: Text,
    pub fix: Text,
    pub evidence_url: String,
}

type Key = (String, Kind, String, String);

impl Finding {
    fn key(&self) -> Key {
        (
            self.project.clone(),
            self.kind,
            self.name.clone(),
            self.check.clone(),
        )
    }
}

/// One point of a check's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[schema(as = HealthCheckPoint)]
pub struct Point {
    /// RFC 3339.
    pub at: String,
    pub status: Status,
}

/// A message of the health log: its key under `health.say` in the UI's locales with the values
/// it names, and its English text for the API and the logs. A text a component wrote (a broker's
/// error) has no key: it is shown as it came, credentials masked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = HealthCheckText)]
pub struct Text {
    /// The key under `health.say`; empty for a component's own words.
    pub code: String,
    /// The values the message names, by placeholder.
    pub args: BTreeMap<String, String>,
    /// The message in English.
    pub text: String,
}

/// The English of every message the checks say, `{name}` a placeholder.
const SAY: &[(&str, &str)] = &[
    ("notApplied", "not applied yet"),
    ("waitingApproval", "waiting for approval"),
    ("noNewData", "no new observation in the window its schedule promises"),
    ("countFellToZero", "the space held {before} entities and now holds none"),
    ("countHalved", "the space fell from {before} to {now} entities"),
    ("counted", "{entities} entities, {history} history rows"),
    ("notCounted", "the broker could not count the space: {reason}"),
    ("invalid", "{invalid} of {checked} entities break the model's rules, most often {rule}"),
    ("valid", "{checked} entities follow the model"),
    ("staleType", "{type} has no observation newer than {newest}, written by {pipeline}"),
    ("noReader", "no pipeline reads this source"),
    ("readBy", "read by {pipelines}"),
    ("readerSaid", "{pipeline}: {said}"),
    ("answered", "{surface} answered {status} in {ms} ms"),
    ("slow", "{surface} answered in {ms} ms, over 3 s"),
    ("anonymousRefused", "{surface} refused an anonymous reader ({status}) though the Endpoint is public"),
    ("notServed", "{surface} answered {status}"),
    ("noAnswer", "{surface} did not answer: {reason}"),
    ("policyWrite", "The broker refused the write: the target Endpoint's Policy must grant pl-{name} create and update, or spec.outputs[].targetEndpoint must name an Endpoint without policyRef."),
    ("policy", "A Policy refuses this operation: check the Endpoint's policyRef and the Policy's rules on the Policies page."),
    ("token", "The account's token was refused: check the service account and its secretRef, then sync again."),
    ("robots", "The source's robots.txt disallows the reader: point spec.source.url at an allowed address or ask the publisher."),
    ("sourceDown", "The source or the broker did not answer: check that spec.source.url opens, then use Run now on the pipeline page."),
    ("serviceDown", "A service did not answer: wait for the next check; if it stays red, read the broker's status on the Administration page."),
    ("runLog", "Open the run log on the pipeline page and fix the step its newest failed line names."),
    ("manifest", "Open the {kind} manifest, change the field the message names and propose the change."),
    ("approve", "Approve its change on the Approvals page; the next sync applies it."),
    ("drift", "The platform holds something else than the repository: open it and keep the platform's state or restore the repository's."),
    ("drop", "The newest run sent far less than the week before: compare the source with the pipeline's filter and mapping on the pipeline page."),
    ("schedule", "Check spec.schedule and the source, then use Run now on the pipeline page."),
    ("countFell", "Open the pipelines writing this space and their newest runs: a source that answered empty or a retention rule may have removed the entities."),
    ("validation", "Open the space's Quality tab: it lists each broken rule with examples; fix the pipeline's mapping or the model."),
    ("retention", "The history grows without a bound: set spec.retention on the Context Space."),
    ("addReader", "Add a pipeline that reads this source, or remove the source."),
    ("publicRefused", "The Endpoint is public but its surface asks for a token: check spec.audience and the Policy named in spec.policyRef."),
    ("surfaceMissing", "The gateway does not serve this surface: check spec.enabledRepresentations and that the Endpoint is applied."),
    ("slowSurface", "The surface answers slowly: check the broker's load and the Endpoint's filter."),
];

impl Text {
    /// Message `code` with `args`, its English filled in.
    pub fn say(code: &'static str, args: &[(&str, String)]) -> Self {
        let mut text = SAY
            .iter()
            .find(|(key, _)| *key == code)
            .map_or(code, |(_, english)| english)
            .to_owned();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        Self {
            code: code.to_owned(),
            args: args
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
            text,
        }
    }

    /// A component's own words, masked.
    pub fn raw(text: &str) -> Self {
        Self {
            text: shown(text),
            ..Self::default()
        }
    }

    /// The text as its two columns hold it: the English, and `{"code", "args"}` as JSON.
    fn from_columns(text: String, say: &str) -> Self {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Say {
            code: String,
            args: BTreeMap<String, String>,
        }
        let say: Say = serde_json::from_str(say).unwrap_or_default();
        Self {
            code: say.code,
            args: say.args,
            text,
        }
    }

    fn say_json(&self) -> String {
        serde_json::json!({ "code": self.code, "args": self.args }).to_string()
    }
}

/// One check of one resource as the Health page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HealthCheck)]
pub struct Check {
    pub project: String,
    pub kind: Kind,
    pub name: String,
    /// `reconcile`, `runs`, `freshness`, `entities`, `storage`, `validation`, `reachable`, or a
    /// surface an Endpoint serves (`ngsi-ld`, `odata`, `mcp`, ...).
    pub check: String,
    pub status: Status,
    /// Since when this status holds, RFC 3339.
    pub since: String,
    /// When the check was last ok, RFC 3339; absent when it never was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<String>,
    /// When the leader last judged it, RFC 3339.
    pub checked_at: String,
    /// What the check found, credentials masked.
    pub detail: Text,
    /// The manifest field or the page to change; empty when there is nothing to fix.
    pub fix: Text,
    /// The Portal page of the resource.
    pub evidence_url: String,
    /// The statuses of the last 7 days, oldest first.
    pub history: Vec<Point>,
}

/// The message a resource's status gives: the first condition that does not hold, else any.
fn said(envelope: &ResourceEnvelope) -> String {
    let Some(status) = &envelope.status else {
        return String::new();
    };
    let text = |c: &Condition| c.message.clone().or_else(|| c.reason.clone());
    status
        .conditions
        .iter()
        .filter(|c| c.status == "False")
        .find_map(text)
        .or_else(|| status.conditions.iter().find_map(text))
        .unwrap_or_default()
}

/// A detail fit to show: a quoted JSON body in its words, credentials masked, one line, at most [`MAX_DETAIL`] characters.
fn shown(detail: &str) -> String {
    let one_line = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::pipeline_outcomes::mask_text(&crate::pipeline_outcomes::json_in_words(&one_line))
        .chars()
        .take(MAX_DETAIL)
        .collect()
}

/// What a person changes when a check of `kind` failed with `detail`.
pub fn fix_hint(kind: Kind, name: &str, detail: &str) -> Text {
    let lower = detail.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&["403", "forbidden", "no policy grants"]) {
        return match kind {
            Kind::Pipeline => Text::say("policyWrite", &[("name", name.to_owned())]),
            _ => Text::say("policy", &[]),
        };
    }
    if has(&["401", "unauthorized", "invalid_client", "invalid token"]) {
        return Text::say("token", &[]);
    }
    if has(&["robots"]) {
        return Text::say("robots", &[]);
    }
    if has(&[
        "timed out",
        "timeout",
        "connection",
        "500",
        "502",
        "503",
        "504",
    ]) {
        return match kind {
            Kind::Pipeline | Kind::Datasource => Text::say("sourceDown", &[]),
            _ => Text::say("serviceDown", &[]),
        };
    }
    match kind {
        Kind::Pipeline => Text::say("runLog", &[]),
        _ => Text::say("manifest", &[("kind", kind.manifest_kind().to_owned())]),
    }
}

impl Finding {
    pub fn new(project: &str, kind: Kind, name: &str, check: &str) -> Self {
        Self {
            project: project.to_owned(),
            kind,
            name: name.to_owned(),
            check: check.to_owned(),
            status: Status::Ok,
            detail: Text::default(),
            fix: Text::default(),
            evidence_url: kind.page(project, name),
        }
    }

    pub fn judged(mut self, status: Status, detail: Text, fix: Text) -> Self {
        self.status = status;
        self.detail = detail;
        self.fix = fix;
        self
    }
}

/// The `reconcile` check of one resource: its phase and the condition that says why.
fn reconcile(project: &str, kind: Kind, envelope: &ResourceEnvelope) -> Finding {
    let name = &envelope.metadata.name;
    let detail = Text::raw(&said(envelope));
    let (status, detail, fix) = match envelope.status.as_ref().map(|s| s.phase) {
        None => (
            Status::Warn,
            Text::say("notApplied", &[]),
            Text::say("approve", &[]),
        ),
        Some(Phase::Live | Phase::Deploying) => (Status::Ok, detail, Text::default()),
        Some(Phase::Draft | Phase::Pending) => (
            Status::Warn,
            Text::say("waitingApproval", &[]),
            Text::say("approve", &[]),
        ),
        Some(Phase::Drifted) => (Status::Warn, detail, Text::say("drift", &[])),
        Some(Phase::Error) => {
            let fix = fix_hint(kind, name, &detail.text);
            (Status::Error, detail, fix)
        }
    };
    Finding::new(project, kind, name, "reconcile").judged(status, detail, fix)
}

/// The pipelines of the mirror that read DataSource `name` of `project`, by name.
fn readers_of(mirror: &Mirror, project: &str, name: &str) -> Vec<String> {
    let names = |spec: &serde_json::Value| -> bool {
        let reads = |source: &serde_json::Value| {
            source
                .get("dataSourceRef")
                .and_then(crate::resource::reference_name)
                == Some(name)
        };
        reads(spec)
            || spec.get("source").is_some_and(reads)
            || spec
                .get("sources")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|sources| sources.iter().any(reads))
    };
    let mut readers: Vec<String> = mirror
        .matching(|env| {
            env.kind == "Pipeline" && env.metadata.namespace.as_deref() == Some(project)
        })
        .into_iter()
        .filter(|env| names(&env.spec))
        .map(|env| env.metadata.name)
        .collect();
    readers.sort();
    readers
}

/// The checks a space's quality report answers (T-3252): its newest observation against each
/// writer's schedule, and how many entities break the model's rules.
fn space_quality(project: &str, name: &str, report: &crate::quality::SpaceQuality) -> Vec<Finding> {
    let stale = report.freshness.iter().find(|row| row.alerting());
    let freshness = Finding::new(project, Kind::Space, name, "freshness");
    let freshness = match stale {
        Some(row) => freshness.judged(
            Status::Warn,
            Text::say(
                "staleType",
                &[
                    (
                        "type",
                        if row.entity_type.is_empty() {
                            name.to_owned()
                        } else {
                            row.entity_type.clone()
                        },
                    ),
                    (
                        "newest",
                        row.newest.map_or_else(|| "none".to_owned(), rfc3339),
                    ),
                    ("pipeline", row.pipeline.clone()),
                ],
            ),
            Text::say("schedule", &[]),
        ),
        None => freshness,
    };
    let validation = Finding::new(project, Kind::Space, name, "validation");
    let validation = if report.invalid > 0 {
        let rule = report
            .rules
            .iter()
            .max_by_key(|rule| rule.count)
            .map_or_else(String::new, |rule| {
                format!("{} at {}", rule.rule, rule.path)
            });
        validation.judged(
            Status::Warn,
            Text::say(
                "invalid",
                &[
                    ("invalid", report.invalid.to_string()),
                    ("checked", report.checked.to_string()),
                    ("rule", rule),
                ],
            ),
            Text::say("validation", &[]),
        )
    } else {
        validation.judged(
            Status::Ok,
            Text::say("valid", &[("checked", report.checked.to_string())]),
            Text::default(),
        )
    };
    vec![freshness, validation]
}

/// Every check of every resource the mirror holds, judged with the alert signals of this run and
/// the spaces' quality reports.
pub fn derive(
    mirror: &Mirror,
    quality: &crate::quality::Store,
    signals: &[Signal],
) -> Vec<Finding> {
    let signal = |project: &str, name: &str, event: Event| {
        signals
            .iter()
            .find(|s| s.key.0 == project && s.key.1 == name && s.key.2 == event)
    };
    let mut findings = Vec::new();
    for (manifest, kind) in KINDS {
        for envelope in mirror.matching(|env| env.kind == manifest) {
            let Some(project) = envelope.metadata.namespace.clone() else {
                continue;
            };
            let name = envelope.metadata.name.clone();
            findings.push(reconcile(&project, kind, &envelope));
            match kind {
                Kind::Pipeline => {
                    let runs = Finding::new(&project, kind, &name, "runs");
                    findings.push(
                        match (
                            signal(&project, &name, Event::Failure),
                            signal(&project, &name, Event::Zero),
                        ) {
                            (Some(failure), _) => {
                                let detail = Text::raw(&failure.detail);
                                let fix = fix_hint(kind, &name, &detail.text);
                                runs.judged(Status::Error, detail, fix)
                            }
                            (None, Some(zero)) => runs.judged(
                                Status::Warn,
                                Text::raw(&zero.detail),
                                Text::say("drop", &[]),
                            ),
                            (None, None) => runs,
                        },
                    );
                    let freshness = Finding::new(&project, kind, &name, "freshness");
                    findings.push(if signal(&project, &name, Event::Stale).is_some() {
                        freshness.judged(
                            Status::Warn,
                            Text::say("noNewData", &[]),
                            Text::say("schedule", &[]),
                        )
                    } else {
                        freshness
                    });
                }
                Kind::Space => {
                    if let Some(report) = quality.get(&project, &name) {
                        findings.extend(space_quality(&project, &name, &report));
                    }
                }
                Kind::Datasource => {
                    let readers = readers_of(mirror, &project, &name);
                    let reachable = Finding::new(&project, kind, &name, "reachable");
                    let failing = readers.iter().find_map(|reader| {
                        Some((reader, signal(&project, reader, Event::Failure)?))
                    });
                    findings.push(match failing {
                        _ if readers.is_empty() => reachable.judged(
                            Status::Warn,
                            Text::say("noReader", &[]),
                            Text::say("addReader", &[]),
                        ),
                        Some((reader, failure)) => {
                            let said = Text::raw(&failure.detail).text;
                            let fix = fix_hint(kind, reader, &said);
                            reachable.judged(
                                Status::Error,
                                Text::say(
                                    "readerSaid",
                                    &[("pipeline", reader.clone()), ("said", said)],
                                ),
                                fix,
                            )
                        }
                        None => reachable.judged(
                            Status::Ok,
                            Text::say("readBy", &[("pipelines", readers.join(", "))]),
                            Text::default(),
                        ),
                    });
                }
                Kind::Endpoint | Kind::Model => {}
            }
        }
    }
    findings
}

#[derive(Debug, Clone)]
struct Latest {
    finding: Finding,
    since: DateTime<Utc>,
    last_ok: Option<DateTime<Utc>>,
    checked_at: DateTime<Utc>,
}

#[derive(Default)]
struct Memory {
    latest: BTreeMap<Key, Latest>,
    history: Vec<(Key, DateTime<Utc>, Status)>,
}

impl Memory {
    /// The same rules the Postgres statements of [`HealthStore::record`] hold.
    fn record(&mut self, now: DateTime<Utc>, findings: Vec<Finding>) {
        let mut latest = BTreeMap::new();
        for finding in findings {
            let key = finding.key();
            let before = self.latest.get(&key);
            let since = before
                .filter(|b| b.finding.status == finding.status)
                .map_or(now, |b| b.since);
            let last_ok = if finding.status == Status::Ok {
                Some(now)
            } else {
                before.and_then(|b| b.last_ok)
            };
            let newest = self
                .history
                .iter()
                .rev()
                .find(|(k, _, _)| *k == key)
                .map(|(_, at, status)| (*at, *status));
            if newest.is_none_or(|(at, status)| status != finding.status || at <= now - SAMPLE) {
                self.history.push((key.clone(), now, finding.status));
            }
            latest.insert(
                key,
                Latest {
                    finding,
                    since,
                    last_ok,
                    checked_at: now,
                },
            );
        }
        self.latest = latest;
        self.history.retain(|(_, at, _)| *at >= now - KEEP);
    }

    fn list(&self, project: Option<&str>, from: DateTime<Utc>) -> Vec<Check> {
        self.latest
            .iter()
            .filter(|(key, _)| project.is_none_or(|p| key.0 == p))
            .map(|(key, held)| Check {
                project: key.0.clone(),
                kind: key.1,
                name: key.2.clone(),
                check: key.3.clone(),
                status: held.finding.status,
                since: rfc3339(held.since),
                last_ok: held.last_ok.map(rfc3339),
                checked_at: rfc3339(held.checked_at),
                detail: held.finding.detail.clone(),
                fix: held.finding.fix.clone(),
                evidence_url: held.finding.evidence_url.clone(),
                history: self
                    .history
                    .iter()
                    .filter(|(k, at, _)| k == key && *at > from)
                    .map(|(_, at, status)| Point {
                        at: rfc3339(*at),
                        status: *status,
                    })
                    .collect(),
            })
            .collect()
    }
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<Memory>),
}

/// One row of `health_check` with its history as Postgres answers it.
type Row = (
    String,
    String,
    String,
    String,
    String,
    time::OffsetDateTime,
    Option<time::OffsetDateTime>,
    time::OffsetDateTime,
    String,
    String,
    String,
    Vec<time::OffsetDateTime>,
    Vec<String>,
    String,
    String,
);

/// The health log, durable with a database, in memory without one.
pub struct HealthStore {
    inner: Inner,
    written: tokio::sync::Mutex<Option<DateTime<Utc>>>,
}

impl HealthStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new(Memory::default())),
            },
            written: tokio::sync::Mutex::new(None),
        }
    }

    /// Writes the findings when the last write is [`EVERY`] old or older.
    pub async fn record_due(
        &self,
        now: DateTime<Utc>,
        findings: Vec<Finding>,
    ) -> Result<(), sqlx::Error> {
        let mut written = self.written.lock().await;
        if written.is_some_and(|at| now - at < EVERY) {
            return Ok(());
        }
        self.record(now, findings).await?;
        *written = Some(now);
        Ok(())
    }

    /// Replaces the latest rows with `findings` (a check no longer found is gone with its
    /// resource), adds a history point per changed status or per hour, drops history past
    /// [`KEEP`].
    pub async fn record(
        &self,
        now: DateTime<Utc>,
        findings: Vec<Finding>,
    ) -> Result<(), sqlx::Error> {
        let pool = match &self.inner {
            Inner::Memory(memory) => {
                if let Ok(mut memory) = memory.write() {
                    memory.record(now, findings);
                }
                return Ok(());
            }
            Inner::Postgres(pool) => pool,
        };
        let column = |f: fn(&Finding) -> String| findings.iter().map(f).collect::<Vec<_>>();
        let at = chrono_to_odt(now);
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO health_check AS h (project, kind, name, \"check\", status, since, last_ok, \
             checked_at, detail, fix_hint, evidence_url, detail_say, fix_say) \
             SELECT p, k, n, c, s, $1, CASE WHEN s = 'ok' THEN $1 END, $1, d, f, e, ds::jsonb, \
             fs::jsonb \
             FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], \
             $8::text[], $9::text[], $10::text[], $11::text[]) AS t(p, k, n, c, s, d, f, e, ds, fs) \
             ON CONFLICT (project, kind, name, \"check\") DO UPDATE SET \
             status = EXCLUDED.status, \
             since = CASE WHEN h.status = EXCLUDED.status THEN h.since ELSE EXCLUDED.since END, \
             last_ok = COALESCE(EXCLUDED.last_ok, h.last_ok), checked_at = EXCLUDED.checked_at, \
             detail = EXCLUDED.detail, fix_hint = EXCLUDED.fix_hint, \
             evidence_url = EXCLUDED.evidence_url, detail_say = EXCLUDED.detail_say, \
             fix_say = EXCLUDED.fix_say",
        )
        .bind(at)
        .bind(column(|f| f.project.clone()))
        .bind(column(|f| f.kind.as_str().to_owned()))
        .bind(column(|f| f.name.clone()))
        .bind(column(|f| f.check.clone()))
        .bind(column(|f| f.status.as_str().to_owned()))
        .bind(column(|f| f.detail.text.clone()))
        .bind(column(|f| f.fix.text.clone()))
        .bind(column(|f| f.evidence_url.clone()))
        .bind(column(|f| f.detail.say_json()))
        .bind(column(|f| f.fix.say_json()))
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM health_check WHERE checked_at < $1")
            .bind(at)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO health_check_history (project, kind, name, \"check\", at, status) \
             SELECT h.project, h.kind, h.name, h.\"check\", h.checked_at, h.status \
             FROM health_check h LEFT JOIN LATERAL ( \
               SELECT x.at, x.status FROM health_check_history x \
               WHERE x.project = h.project AND x.kind = h.kind AND x.name = h.name \
                 AND x.\"check\" = h.\"check\" ORDER BY x.at DESC LIMIT 1) last ON true \
             WHERE h.checked_at = $1 \
               AND (last.at IS NULL OR last.status <> h.status OR last.at <= $1 - interval '1 hour') \
             ON CONFLICT DO NOTHING",
        )
        .bind(at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM health_check_history WHERE at < $1 - interval '90 days'")
            .bind(at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// The checks of one project, or of every project for `None`, errors first.
    pub async fn list(
        &self,
        project: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Check>, sqlx::Error> {
        self.list_from(project, now - SHOWN).await
    }

    /// The checks of one project as they stand, without their history: what a dataset's score
    /// reads on every catalogue request (T-5112).
    pub async fn current(
        &self,
        project: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<Check>, sqlx::Error> {
        self.list_from(Some(project), now).await
    }

    /// The checks with their history after `from`.
    async fn list_from(
        &self,
        project: Option<&str>,
        from: DateTime<Utc>,
    ) -> Result<Vec<Check>, sqlx::Error> {
        let mut checks = match &self.inner {
            Inner::Memory(memory) => memory
                .read()
                .map(|memory| memory.list(project, from))
                .unwrap_or_default(),
            Inner::Postgres(pool) => {
                let rows: Vec<Row> = sqlx::query_as(
                    "SELECT h.project, h.kind, h.name, h.\"check\", h.status, h.since, h.last_ok, \
                     h.checked_at, h.detail, h.fix_hint, h.evidence_url, \
                     COALESCE(p.ats, '{}'), COALESCE(p.statuses, '{}'), \
                     h.detail_say::text, h.fix_say::text \
                     FROM health_check h LEFT JOIN LATERAL ( \
                       SELECT array_agg(x.at ORDER BY x.at) AS ats, \
                              array_agg(x.status ORDER BY x.at) AS statuses \
                       FROM health_check_history x \
                       WHERE x.project = h.project AND x.kind = h.kind AND x.name = h.name \
                         AND x.\"check\" = h.\"check\" AND x.at > $2) p ON true \
                     WHERE $1::text IS NULL OR h.project = $1",
                )
                .bind(project)
                .bind(chrono_to_odt(from))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .filter_map(|row| {
                        Some(Check {
                            project: row.0,
                            kind: Kind::parse(&row.1)?,
                            name: row.2,
                            check: row.3,
                            status: Status::parse(&row.4),
                            since: rfc3339(odt_to_chrono(row.5)),
                            last_ok: row.6.map(|at| rfc3339(odt_to_chrono(at))),
                            checked_at: rfc3339(odt_to_chrono(row.7)),
                            detail: Text::from_columns(row.8, &row.13),
                            fix: Text::from_columns(row.9, &row.14),
                            evidence_url: row.10,
                            history: row
                                .11
                                .into_iter()
                                .zip(row.12)
                                .map(|(at, status)| Point {
                                    at: rfc3339(odt_to_chrono(at)),
                                    status: Status::parse(&status),
                                })
                                .collect(),
                        })
                    })
                    .collect()
            }
        };
        checks.sort_by(|a, b| {
            (a.status, &a.project, a.kind, &a.name, &a.check)
                .cmp(&(b.status, &b.project, b.kind, &b.name, &b.check))
        });
        Ok(checks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, API_VERSION};

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("a time")
    }

    fn envelope(kind: &str, name: &str, phase: Option<Phase>, message: &str) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("bbsk".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: phase.map(|phase| {
                let holds = if phase == Phase::Live {
                    "True"
                } else {
                    "False"
                };
                serde_json::from_value(serde_json::json!({
                    "phase": phase,
                    "conditions": [{"type": "Ready", "status": holds, "message": message}],
                }))
                .expect("a status")
            }),
        }
    }

    fn mirror(items: Vec<ResourceEnvelope>) -> Mirror {
        let mirror = Mirror::default();
        for item in items {
            mirror.upsert(item);
        }
        mirror
    }

    fn find<'a>(findings: &'a [Finding], name: &str, check: &str) -> &'a Finding {
        findings
            .iter()
            .find(|f| f.name == name && f.check == check)
            .expect("the finding")
    }

    #[test]
    fn every_kind_reports_ok_warn_and_error_from_its_status() {
        let findings = derive(
            &mirror(vec![
                envelope("ContextSpace", "air", Some(Phase::Live), "ready"),
                envelope(
                    "Endpoint",
                    "air-api",
                    Some(Phase::Error),
                    "policyRef open-air not found",
                ),
                envelope("DataModel", "events", Some(Phase::Pending), ""),
                envelope("DataSource", "rss", None, ""),
            ]),
            &crate::quality::Store::default(),
            &[],
        );
        assert_eq!(find(&findings, "air", "reconcile").status, Status::Ok);
        let endpoint = find(&findings, "air-api", "reconcile");
        assert_eq!(endpoint.status, Status::Error);
        assert_eq!(endpoint.kind, Kind::Endpoint);
        assert_eq!(endpoint.detail.text, "policyRef open-air not found");
        assert_eq!(
            endpoint.detail.code, "",
            "a component's own words stay as they came"
        );
        assert!(
            endpoint.fix.text.contains("Endpoint manifest"),
            "{}",
            endpoint.fix.text
        );
        assert_eq!(endpoint.evidence_url, "/projects/bbsk/endpoints/air-api");
        assert_eq!(find(&findings, "events", "reconcile").status, Status::Warn);
        assert_eq!(
            find(&findings, "events", "reconcile").evidence_url,
            "/projects/bbsk/models/events"
        );
        assert_eq!(find(&findings, "rss", "reconcile").status, Status::Warn);
    }

    #[test]
    fn a_refused_pipeline_write_is_an_error_whose_hint_names_the_policy() {
        let signals = [Signal {
            key: ("bbsk".into(), "bbsk-events".into(), Event::Failure),
            detail: "3 runs in a row sent nothing: 403 no policy grants this operation".into(),
        }];
        let findings = derive(
            &mirror(vec![envelope(
                "Pipeline",
                "bbsk-events",
                Some(Phase::Live),
                "",
            )]),
            &crate::quality::Store::default(),
            &signals,
        );
        let runs = find(&findings, "bbsk-events", "runs");
        assert_eq!(runs.status, Status::Error);
        assert!(
            runs.fix.text.contains("pl-bbsk-events"),
            "{}",
            runs.fix.text
        );
        assert!(runs.fix.text.contains("policyRef"), "{}", runs.fix.text);
        assert_eq!(runs.fix.code, "policyWrite");
        assert_eq!(
            runs.fix.args.get("name").map(String::as_str),
            Some("bbsk-events")
        );
        assert_eq!(
            find(&findings, "bbsk-events", "freshness").status,
            Status::Ok
        );
        assert_eq!(
            find(&findings, "bbsk-events", "reconcile").status,
            Status::Ok
        );
    }

    #[test]
    fn a_pipeline_that_dropped_or_went_stale_warns() {
        let key = |event| ("bbsk".to_owned(), "weather".to_owned(), event);
        let signals = [
            Signal {
                key: key(Event::Zero),
                detail: "the last run sent 2 records against a 7-day median of 400".into(),
            },
            Signal {
                key: key(Event::Stale),
                detail: String::new(),
            },
        ];
        let findings = derive(
            &mirror(vec![envelope("Pipeline", "weather", Some(Phase::Live), "")]),
            &crate::quality::Store::default(),
            &signals,
        );
        assert_eq!(find(&findings, "weather", "runs").status, Status::Warn);
        let fresh = find(&findings, "weather", "freshness");
        assert_eq!(fresh.status, Status::Warn);
        assert!(fresh.fix.text.contains("spec.schedule"));
    }

    #[test]
    fn a_detail_masks_credentials() {
        let findings = derive(
            &mirror(vec![envelope(
                "Endpoint",
                "leaky",
                Some(Phase::Error),
                "upstream said Bearer abcdefghijklmnopqrstuvwxyz0123456789 is 401",
            )]),
            &crate::quality::Store::default(),
            &[],
        );
        let leaky = find(&findings, "leaky", "reconcile");
        assert!(
            !leaky.detail.text.contains("abcdefghijklmnop"),
            "{}",
            leaky.detail.text
        );
        assert!(leaky.fix.text.contains("secretRef"), "{}", leaky.fix.text);
    }

    /// T-5734: the walker found the gateway's 403 body as raw JSON on /projects/helsinki/health.
    #[test]
    fn a_detail_tells_a_quoted_json_body_in_words() {
        let findings = derive(
            &mirror(vec![envelope(
                "Pipeline",
                "refused",
                Some(Phase::Error),
                r#"the last run failed for a cause outside its source: the write failed: processors failed: the target HTTP request returned unexpected response code (403): 403 Forbidden, Error: {"type":"about:blank","title":"Forbidden","status":403,"detail":"the Policy grants no write of this type"}; next try 22:45"#,
            )]),
            &crate::quality::Store::default(),
            &[],
        );
        let refused = find(&findings, "refused", "reconcile");
        assert_eq!(
            refused.detail.text,
            "the last run failed for a cause outside its source: the write failed: processors failed: the target HTTP request returned unexpected response code (403): 403 Forbidden, Error: the Policy grants no write of this type; next try 22:45"
        );
    }

    fn finding(status: Status) -> Finding {
        Finding {
            project: "bbsk".into(),
            kind: Kind::Pipeline,
            name: "events".into(),
            check: "runs".into(),
            status,
            detail: Text::default(),
            fix: Text::default(),
            evidence_url: String::new(),
        }
    }

    #[tokio::test]
    async fn a_check_keeps_since_while_it_holds_and_recovers_with_last_ok() {
        let store = HealthStore::new(None);
        let t0 = at("2026-10-21T08:00:00Z");
        store
            .record(t0, vec![finding(Status::Ok)])
            .await
            .expect("record");
        let t1 = t0 + Duration::minutes(2);
        store
            .record(t1, vec![finding(Status::Error)])
            .await
            .expect("record");
        let t2 = t1 + Duration::minutes(2);
        store
            .record(t2, vec![finding(Status::Error)])
            .await
            .expect("record");
        let held = &store.list(Some("bbsk"), t2).await.expect("list")[0];
        assert_eq!(held.status, Status::Error);
        assert_eq!(held.since, rfc3339(t1));
        assert_eq!(held.last_ok.as_deref(), Some(rfc3339(t0).as_str()));
        assert_eq!(held.history.len(), 2, "one point per change, not per run");
        let t3 = t2 + Duration::minutes(2);
        store
            .record(t3, vec![finding(Status::Ok)])
            .await
            .expect("record");
        let recovered = &store.list(None, t3).await.expect("list")[0];
        assert_eq!(
            (recovered.status, recovered.since.clone()),
            (Status::Ok, rfc3339(t3))
        );
        assert_eq!(recovered.history.len(), 3);
        assert!(store
            .list(Some("other"), t3)
            .await
            .expect("list")
            .is_empty());
    }

    #[tokio::test]
    async fn the_history_is_sampled_hourly_and_bounded_to_90_days() {
        let store = HealthStore::new(None);
        let t0 = at("2026-07-01T00:00:00Z");
        for hour in 0..3 {
            for minute in [0, 20, 40] {
                let now = t0 + Duration::hours(hour) + Duration::minutes(minute);
                store
                    .record(now, vec![finding(Status::Ok)])
                    .await
                    .expect("record");
            }
        }
        if let Inner::Memory(memory) = &store.inner {
            assert_eq!(
                memory.read().expect("lock").history.len(),
                3,
                "one point an hour"
            );
        }
        let later = t0 + KEEP + Duration::days(1);
        store
            .record(later, vec![finding(Status::Ok)])
            .await
            .expect("record");
        if let Inner::Memory(memory) = &store.inner {
            let memory = memory.read().expect("lock");
            assert_eq!(memory.history.len(), 1, "older than 90 days is gone");
        }
    }

    #[tokio::test]
    async fn a_removed_resource_leaves_the_log_and_writes_are_throttled() {
        let store = HealthStore::new(None);
        let t0 = at("2026-10-21T08:00:00Z");
        store
            .record_due(t0, vec![finding(Status::Ok)])
            .await
            .expect("record");
        store
            .record_due(t0 + Duration::minutes(1), vec![finding(Status::Error)])
            .await
            .expect("record");
        let list = store.list(None, t0).await.expect("list");
        assert_eq!(list[0].status, Status::Ok, "a write within EVERY waits");
        store
            .record_due(t0 + EVERY, Vec::new())
            .await
            .expect("record");
        assert!(store.list(None, t0 + EVERY).await.expect("list").is_empty());
    }

    #[test]
    fn a_data_source_is_judged_by_the_pipelines_that_read_it() {
        let mut reader = envelope("Pipeline", "rss-reader", Some(Phase::Live), "");
        reader.spec = serde_json::json!({"sources": [{"dataSourceRef": {"kind": "DataSource", "name": "rss"}}]});
        let mirror = mirror(vec![
            envelope("DataSource", "rss", Some(Phase::Live), ""),
            envelope("DataSource", "orphan", Some(Phase::Live), ""),
            reader,
        ]);
        let quality = crate::quality::Store::default();
        let ok = derive(&mirror, &quality, &[]);
        assert_eq!(find(&ok, "rss", "reachable").status, Status::Ok);
        assert_eq!(
            find(&ok, "rss", "reachable").detail.text,
            "read by rss-reader"
        );
        assert_eq!(find(&ok, "orphan", "reachable").status, Status::Warn);
        let signals = [Signal {
            key: ("bbsk".into(), "rss-reader".into(), Event::Failure),
            detail: "GET https://example.sk/rss answered robots.txt disallow".into(),
        }];
        let broken = derive(&mirror, &quality, &signals);
        let rss = find(&broken, "rss", "reachable");
        assert_eq!(
            (rss.status, rss.fix.code.as_str()),
            (Status::Error, "robots")
        );
        assert_eq!(rss.detail.code, "readerSaid");
    }

    #[test]
    fn a_space_report_flags_stale_types_and_broken_rules() {
        use crate::quality::{FreshState, Freshness, RuleCount, SpaceQuality};
        let report = SpaceQuality {
            observed_at: at("2026-10-21T08:00:00Z"),
            checked: 40,
            invalid: 3,
            truncated: false,
            rules: vec![RuleCount {
                rule: "required".into(),
                path: "name".into(),
                count: 3,
                examples: Vec::new(),
            }],
            freshness: vec![Freshness {
                pipeline: "air-feed".into(),
                entity_type: "AirQualityObserved".into(),
                newest: Some(at("2026-10-20T08:00:00Z")),
                target_seconds: Some(3600),
                state: FreshState::Stale,
                paused: false,
            }],
            types: Vec::new(),
        };
        let findings = space_quality("bbsk", "air", &report);
        let fresh = find(&findings, "air", "freshness");
        assert_eq!(
            (fresh.status, fresh.detail.code.as_str()),
            (Status::Warn, "staleType")
        );
        assert!(
            fresh.detail.text.contains("AirQualityObserved"),
            "{}",
            fresh.detail.text
        );
        let valid = find(&findings, "air", "validation");
        assert_eq!(valid.status, Status::Warn);
        assert!(
            valid.detail.text.contains("3 of 40"),
            "{}",
            valid.detail.text
        );
        assert!(
            valid.detail.text.contains("required at name"),
            "{}",
            valid.detail.text
        );
    }

    #[test]
    fn every_message_has_its_english_and_fills_its_placeholders() {
        let text = Text::say(
            "countHalved",
            &[("before", "100".into()), ("now", "40".into())],
        );
        assert_eq!(text.text, "the space fell from 100 to 40 entities");
        for (code, english) in SAY {
            assert!(!english.is_empty(), "{code}");
        }
        let stored = Text::from_columns(text.text.clone(), &text.say_json());
        assert_eq!(stored, text);
    }
}
