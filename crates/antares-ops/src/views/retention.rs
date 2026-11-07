// SPDX-License-Identifier: EUPL-1.2
//! `/q/retention/{tenant}`: how long a Tenant keeps each Entity type's
//! current state and history (T-3900, platform ADR-N-046 2.2 and 2.3).
//!
//! One document per Tenant, stored beside the view documents and reloaded
//! with them. A rule names an expanded type IRI and one or two durations:
//! `current` removes an Entity of that type that carries no `expiresAt` of
//! its own once it has not been modified for that long; `history` removes
//! its attribute instances older than that. A type without a rule, and a
//! Tenant without a document, lose nothing. Once a minute every pod sweeps
//! (deleting twice what is already gone deletes nothing), in bounded batches,
//! and counts what it deleted in `antares_retention_deleted_total`.

use super::{bad_tenant, serve, Reloader};
use antares_api::AppState;
use antares_model::TenantId;
use axum::body::Bytes;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// The id prefix a Tenant's retention document is stored under, beside the
/// view documents; a view id is `[a-z0-9-]`, so none shares a key with it.
pub const RETENTION_ID_PREFIX: &str = "retention:";
/// The most rules one Tenant's document holds.
pub const MAX_RETENTION_RULES: usize = 256;
/// The largest retention document `PUT` accepts, in bytes.
pub const MAX_RETENTION_BYTES: usize = 64 * 1024;
/// How often every pod sweeps.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// Rows one statement deletes, and statements one rule runs per sweep:
/// a backlog drains over several minutes instead of holding one long lock.
const BATCH: usize = 1000;
const ROUNDS: usize = 50;
/// How long one sweep of one pod may spend deleting: past it the sweep stops
/// and the next minute goes on, so a backlog takes at most a quarter of each
/// minute (T-4084: an 8-hour rule over a 117 GB history held Postgres at its
/// limit and starved every other query of the broker). The steps a spent
/// budget left run first in the next sweep (`Turns`).
pub const SWEEP_BUDGET: Duration = Duration::from_secs(15);
/// The most rows a preview counts per rule and kind, in one statement.
pub const PREVIEW_CAP: usize = 100_000;
/// The cut of a `history: "off"` rule: every stored instant is before it.
const OFF_CUT: &str = "9999-12-31T23:59:59.999Z";

/// The type a Tenant's default rule names (DM-97): every type without a
/// history rule of its own.
pub const ANY_TYPE: &str = "*";
/// The `type` label of what an Entity's own `historyRetention` deleted, and
/// of what the platform default deleted; a metric labelled by Entity id
/// would grow without bound.
pub const ENTITY_LABEL: &str = "@entity";
/// See [`ENTITY_LABEL`].
pub const PLATFORM_LABEL: &str = "@platform";
/// The `type` label of what the audit sweep deleted, under Tenant `*`: it
/// runs across Tenants (T-6337).
pub const AUDIT_LABEL: &str = "@audit";
/// The most Entities with their own `historyRetention` one sweep reads.
// ponytail: one bounded read per sweep; page it once a deployment holds more.
const MAX_OVERRIDES: usize = 10_000;

/// One rule of a Tenant's document.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuleDoc {
    #[serde(rename = "type")]
    type_iri: String,
    #[serde(default)]
    current: Option<String>,
    #[serde(default)]
    history: Option<String>,
    /// How the type records history (T-4665, ADR-N-054 2.2): `every`,
    /// `observed`, `never` (T-5056) or `onChange`, with the three settings
    /// after it.
    #[serde(default)]
    record: Option<String>,
    #[serde(default)]
    heartbeat: Option<String>,
    #[serde(default)]
    deadband: Option<f64>,
    #[serde(default)]
    min_distance: Option<f64>,
    /// Per expanded attribute: how it records, overriding the type.
    #[serde(default)]
    attributes: std::collections::BTreeMap<String, RecordDoc>,
}

impl RuleDoc {
    /// The type's own `record` and its settings.
    fn record(&self) -> RecordDoc {
        RecordDoc {
            record: self.record.clone(),
            heartbeat: self.heartbeat.clone(),
            deadband: self.deadband,
            min_distance: self.min_distance,
        }
    }
}

/// `record` and its settings, of a type or of one attribute.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecordDoc {
    #[serde(default)]
    record: Option<String>,
    #[serde(default)]
    heartbeat: Option<String>,
    #[serde(default)]
    deadband: Option<f64>,
    #[serde(default)]
    min_distance: Option<f64>,
}

impl RecordDoc {
    /// `Ok(None)` when unset, the mode it names otherwise; the reason a
    /// setting is out of place.
    fn parse(&self) -> Result<Option<antares_api::RecordMode>, String> {
        let bound = |name: &str, v: Option<f64>| match v {
            // JSON carries no NaN or infinity, so the sign is the whole check
            Some(v) if v < 0.0 => Err(format!("{name} must be a number of at least 0")),
            _ => Ok(v),
        };
        match self.record.as_deref() {
            Some("onChange") => Ok(Some(antares_api::RecordMode::OnChange(
                antares_api::OnChange {
                    heartbeat: parse_duration(
                        self.heartbeat
                            .as_deref()
                            .ok_or("record onChange needs a heartbeat")?,
                    )?,
                    deadband: bound("deadband", self.deadband)?,
                    min_distance: bound("minDistance", self.min_distance)?,
                },
            ))),
            None | Some("every" | "observed" | "never") => {
                if self.heartbeat.is_some()
                    || self.deadband.is_some()
                    || self.min_distance.is_some()
                {
                    Err("heartbeat, deadband and minDistance belong to record onChange".into())
                } else {
                    Ok(match self.record.as_deref() {
                        Some("every") => Some(antares_api::RecordMode::Every),
                        Some("observed") => Some(antares_api::RecordMode::Observed),
                        Some("never") => Some(antares_api::RecordMode::Never),
                        _ => None,
                    })
                }
            }
            Some(other) => Err(format!(
                "record {other:?} is none of every, observed, never and onChange"
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    rules: Vec<RuleDoc>,
}

/// One rule as the sweep runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// The expanded Entity type, or [`ANY_TYPE`] for the Tenant's default.
    pub type_iri: String,
    /// How long an Entity not modified lives on, if the rule says.
    pub current: Option<Duration>,
    /// How long an attribute instance is kept, if the rule says.
    pub history: Option<Duration>,
    /// `history: "off"` (T-4275): nothing is recorded for the type, and what
    /// it holds is swept.
    pub history_off: bool,
    /// `history: "forever"` (DM-97): the type keeps all of it, and no level
    /// below (the Tenant's default, the platform's) reaches it.
    pub history_forever: bool,
    /// How it records history, when some attribute records other than
    /// `every`.
    pub recording: Option<antares_api::Recording>,
}

impl Rule {
    /// The rule says something of history, so the levels below leave the
    /// type alone.
    fn rules_history(&self) -> bool {
        self.history.is_some() || self.history_off || self.history_forever
    }
}

/// Every Tenant's rules, as the last reload found them.
pub type Rules = Vec<(TenantId, Vec<Rule>)>;

pub use antares_api::history::{parse_duration, parse_duration_within};

/// The rules of one document, or the first reason it holds none.
///
/// # Errors
/// A document that is not one, a type that is no absolute IRI, a type named
/// twice, a duration out of bounds, a `*` rule with a `current`, or more
/// than [`MAX_RETENTION_RULES`].
pub fn parse(body: &[u8]) -> Result<Vec<Rule>, String> {
    let doc: Document =
        serde_json::from_slice(body).map_err(|e| format!("not a retention document: {e}"))?;
    if doc.rules.len() > MAX_RETENTION_RULES {
        return Err(format!(
            "a document holds at most {MAX_RETENTION_RULES} rules"
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    doc.rules
        .into_iter()
        .map(|r| {
            let any = r.type_iri == ANY_TYPE;
            // An expanded IRI, as the store holds types: a short name would
            // depend on a @context the document does not carry.
            if !any
                && !r.type_iri.split_once(':').is_some_and(|(scheme, rest)| {
                    !scheme.is_empty()
                        && !rest.is_empty()
                        && !r.type_iri.contains(char::is_whitespace)
                })
            {
                return Err(format!("type {:?} is not an expanded IRI", r.type_iri));
            }
            // A default `current` would remove every Entity of the Tenant
            // nobody wrote for a while, of types no one chose (DM-97).
            if any && r.current.is_some() {
                return Err("the \"*\" rule sets history only, never current".into());
            }
            if !seen.insert(r.type_iri.clone()) {
                return Err(format!("type {:?} has two rules", r.type_iri));
            }
            if any && (r.record() != RecordDoc::default() || !r.attributes.is_empty()) {
                return Err("the \"*\" rule sets history only, never how a type records".into());
            }
            let mut recording = antares_api::Recording {
                default: r.record().parse()?,
                ..Default::default()
            };
            for (attr, doc) in &r.attributes {
                if !attr.contains(':') || attr.contains(char::is_whitespace) {
                    return Err(format!("attribute {attr:?} is not an expanded IRI"));
                }
                // a setting without `record` takes the type's
                if let Some(mode) = doc.parse()? {
                    recording.attributes.insert(attr.clone(), Some(mode));
                }
            }
            let names_a_mode = recording.default.is_some() || !recording.attributes.is_empty();
            let word = r
                .history
                .as_deref()
                .filter(|h| ["off", "forever"].contains(h));
            Ok(Rule {
                current: r.current.as_deref().map(parse_duration).transpose()?,
                history: r
                    .history
                    .as_deref()
                    .filter(|_| word.is_none())
                    .map(parse_duration)
                    .transpose()?,
                history_off: word == Some("off"),
                history_forever: word == Some("forever"),
                recording: names_a_mode.then_some(recording),
                type_iri: r.type_iri,
            })
        })
        .collect()
}

/// The rules of the stored retention documents among `docs`, by Tenant; a
/// stored document that no longer parses is skipped with a warning, so the
/// sweep deletes nothing for that Tenant rather than guess.
pub(super) fn load<'a>(docs: impl Iterator<Item = (&'a str, &'a Value)>) -> Rules {
    super::tenant_rules(docs, parse, "retention")
}

/// What the write path's history gate 3 needs of `rules` (T-4275, DM-97).
pub(super) fn history_off(rules: &Rules) -> antares_api::HistoryRules {
    let mut out = antares_api::HistoryRules::default();
    for (t, list) in rules {
        for r in list {
            let tenant = t.as_str().to_owned();
            if r.type_iri == ANY_TYPE {
                if r.history_off {
                    out.space_off.insert(tenant);
                } else if r.rules_history() {
                    out.space_on.insert(tenant);
                }
                continue;
            }
            if r.history_off {
                out.off.insert((tenant.clone(), r.type_iri.clone()));
            }
            if let Some(recording) = &r.recording {
                out.recording
                    .insert((tenant.clone(), r.type_iri.clone()), recording.clone());
            }
            if r.rules_history() {
                out.ruled.insert((tenant, r.type_iri.clone()));
            }
        }
    }
    out
}

/// Which writes keep [`antares_api::SHORT_HISTORY`] of history or less
/// (T-5249): a type rule or the Tenant's `*` rule with such a duration.
/// `off` records nothing and `forever` keeps it all; both count as not short
/// for a type, so its other types' short rules do not reach it.
pub(super) fn short_history(rules: &Rules) -> antares_api::ShortHistory {
    let mut out = antares_api::ShortHistory::default();
    for (t, list) in rules {
        for r in list {
            let short = !r.history_off
                && !r.history_forever
                && r.history.is_some_and(|d| d <= antares_api::SHORT_HISTORY);
            if r.type_iri == ANY_TYPE {
                if short {
                    out.tenants.insert(t.as_str().to_owned());
                }
            } else if r.rules_history() {
                out.types
                    .insert((t.as_str().to_owned(), r.type_iri.clone()), short);
            }
        }
    }
    out
}

/// What one sweep deleted of one Tenant's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    /// The Tenant.
    pub tenant: String,
    /// The expanded type, [`ANY_TYPE`] for the Tenant's default,
    /// [`ENTITY_LABEL`] or [`PLATFORM_LABEL`].
    pub type_iri: String,
    /// `entity` or `history`.
    pub kind: &'static str,
    /// How many rows.
    pub count: u64,
}

/// One sweep at `now` over the rules the last reload found: every rule's
/// current state, then its history, each in at most [`ROUNDS`] statements
/// of [`BATCH`] rows. Logs, counts and totals per pod what it deleted, never
/// what it held.
pub(super) async fn sweep(r: &Reloader, now: chrono::DateTime<chrono::Utc>) -> Vec<Deleted> {
    sweep_within(r, now, SWEEP_BUDGET).await
}

/// [`sweep`] with `budget` in place of [`SWEEP_BUDGET`].
pub(super) async fn sweep_within(
    r: &Reloader,
    now: chrono::DateTime<chrono::Utc>,
    budget: Duration,
) -> Vec<Deleted> {
    let plan = Plan {
        rules: &r.retention_rules(),
        platform: r.platform_history,
        now,
        dry_run: false,
        entities: true,
    };
    let out = retain(&r.state, &plan, budget, Some(&r.turns)).await;
    sweep_audit(r, now).await;
    let mut totals = r
        .deleted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for d in &out {
        let (tenant, ty, kind, count) = (d.tenant.as_str(), d.type_iri.as_str(), d.kind, d.count);
        metrics::counter!("antares_retention_deleted_total",
            "tenant" => tenant.to_owned(), "type" => ty.to_owned(), "kind" => kind)
        .increment(count);
        tracing::info!(tenant, r#type = ty, kind, count, "retention deleted");
        let total = totals
            .entry((tenant.to_owned(), ty.to_owned(), kind))
            .or_default();
        *total = total.saturating_add(count);
    }
    out
}

/// The audit sweep (T-6337, ADR-N-054 2.7): audit rows older than the
/// audit retention, at most [`BATCH`] per history table each sweep; the
/// history rules never delete them. Logs and counts how many, never which.
async fn sweep_audit(r: &Reloader, now: chrono::DateTime<chrono::Utc>) {
    match r
        .state
        .temporal
        .retain_audit(&cut(now, r.audit_retention), BATCH)
        .await
    {
        Ok(0) => {}
        Ok(count) => {
            metrics::counter!("antares_retention_deleted_total",
                "tenant" => ANY_TYPE, "type" => AUDIT_LABEL, "kind" => "audit")
            .increment(count);
            tracing::info!(count, "retention deleted audit rows");
        }
        Err(e) => {
            tracing::warn!(error = %e, "retention: audit sweep failed; it runs again next sweep");
        }
    }
}

/// What one run of [`retain`] covers.
struct Plan<'a> {
    rules: &'a Rules,
    /// The platform default (`ANTARES_RETENTION_HISTORY_DEFAULT`), applied to
    /// every Tenant without a `*` rule; `None` keeps what nothing rules.
    platform: Option<Duration>,
    now: chrono::DateTime<chrono::Utc>,
    dry_run: bool,
    /// Run level 1, the Entities' own retention; a preview of one draft
    /// document leaves it out, as the draft does not change it.
    entities: bool,
}

/// One statement kind the sweep repeats, owning what it names.
enum Job {
    /// Current state of one type (DM-83).
    Current(String),
    /// History of one Entity, by its own `historyRetention`.
    Entity(String),
    /// History of one type, but not the Entities that rule themselves.
    Type {
        type_iri: String,
        except_ids: Vec<String>,
    },
    /// History of every type without a history rule of its own.
    Rest {
        except_types: Vec<String>,
        except_ids: Vec<String>,
    },
}

impl Job {
    /// The history the job reaches, or the type whose current state it does.
    fn scope(&self) -> Result<antares_api::HistoryScope<'_>, &str> {
        match self {
            Job::Current(type_iri) => Err(type_iri),
            Job::Entity(id) => Ok(antares_api::HistoryScope::Entity(id)),
            Job::Type {
                type_iri,
                except_ids,
            } => Ok(antares_api::HistoryScope::Type {
                type_iri,
                except_ids,
            }),
            Job::Rest {
                except_types,
                except_ids,
            } => Ok(antares_api::HistoryScope::Rest {
                except_types,
                except_ids,
            }),
        }
    }
}

/// One job of one Tenant before one cut, recorded under `label`.
struct Step {
    tenant: TenantId,
    label: String,
    job: Job,
    before: String,
}

/// The Entities with a valid `historyRetention` of their own, by Tenant,
/// with the cut each keeps, and whether that is all of them; one that is not
/// a duration nor `off` is logged and left to its type's rule.
async fn overrides(
    state: &AppState,
    now: chrono::DateTime<chrono::Utc>,
    cap: usize,
) -> (
    std::collections::BTreeMap<String, Vec<(String, String)>>,
    bool,
) {
    let mut out: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    let found = match state.store.history_overrides(cap).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "retention: entities with their own historyRetention not read");
            return (out, false);
        }
    };
    let whole = found.len() < cap;
    if !whole {
        tracing::warn!(
            cap,
            "retention: more entities carry their own historyRetention than one sweep reads; only their own retention runs"
        );
    }
    for (tenant, id, own) in found {
        let before = match antares_api::history::retention_of(&own) {
            Ok(antares_api::history::OwnRetention::Off) => OFF_CUT.to_owned(),
            Ok(antares_api::history::OwnRetention::Keep(keep)) => cut(now, keep),
            Err(why) => {
                tracing::warn!(
                    tenant,
                    entity = id.as_str(),
                    reason = why.as_str(),
                    "historyRetention ignored; the type's rule applies"
                );
                continue;
            }
        };
        out.entry(tenant).or_default().push((id, before));
    }
    (out, whole)
}

/// The instant `keep` before `now`, as the store compares it.
fn cut(now: chrono::DateTime<chrono::Utc>, keep: Duration) -> String {
    // A rule holds at most ten years, so the cut is always in range.
    chrono::Duration::from_std(keep)
        .ok()
        .and_then(|k| now.checked_sub_signed(k))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The steps of `plan`, level by level (DM-97): the Entities' own retention,
/// then each type's rule, then the Tenant's `*` rule or the platform default
/// for the rest of every Tenant. History by type or by Tenant runs only when
/// every Entity that rules itself is known, as one left out of the exceptions
/// would lose what it asked to keep longer.
async fn steps(state: &AppState, plan: &Plan<'_>) -> Vec<Step> {
    let (own, whole) = overrides(state, plan.now, MAX_OVERRIDES).await;
    // ponytail: every Tenant in name order each sweep, the budget the bound;
    // rotate the start once a deployment's Tenants outgrow one sweep.
    let tenants = if plan.platform.is_some() && !plan.dry_run && whole {
        state.store.tenant_ids().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "retention: tenants not listed; the platform default waits");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    levels(plan, &own, whole, &tenants)
}

/// [`steps`] once the Entities that rule themselves (`own`, all of them when
/// `whole`) and the Tenants the platform default reaches are read.
fn levels(
    plan: &Plan<'_>,
    own: &std::collections::BTreeMap<String, Vec<(String, String)>>,
    whole: bool,
    tenants: &[String],
) -> Vec<Step> {
    let own_ids = |t: &str| -> Vec<String> {
        own.get(t)
            .into_iter()
            .flatten()
            .map(|(id, _)| id.clone())
            .collect()
    };
    let step = |tenant: &TenantId, label: &str, job, before| Step {
        tenant: tenant.clone(),
        label: label.to_owned(),
        job,
        before,
    };
    let mut out = Vec::new();
    if plan.entities {
        // A name the store hands back is a Tenant's; one that is not has nothing to sweep.
        let tenants = own
            .iter()
            .filter_map(|(t, list)| Some((TenantId::new(t).ok()?, list)));
        for (t, list) in tenants {
            for (id, before) in list {
                out.push(step(
                    &t,
                    ENTITY_LABEL,
                    Job::Entity(id.clone()),
                    before.clone(),
                ));
            }
        }
    }
    let mut ruled = std::collections::BTreeSet::new();
    for (t, list) in plan.rules {
        ruled.insert(t.as_str().to_owned());
        let except_ids = own_ids(t.as_str());
        for rule in list.iter().filter(|r| r.type_iri != ANY_TYPE) {
            if let Some(keep) = rule.current {
                let job = Job::Current(rule.type_iri.clone());
                out.push(step(t, &rule.type_iri, job, cut(plan.now, keep)));
            }
            if !whole {
                continue;
            }
            let before = match (rule.history_off, rule.history) {
                (true, _) => OFF_CUT.to_owned(),
                (false, Some(keep)) => cut(plan.now, keep),
                (false, None) => continue,
            };
            let job = Job::Type {
                type_iri: rule.type_iri.clone(),
                except_ids: except_ids.clone(),
            };
            out.push(step(t, &rule.type_iri, job, before));
        }
        if whole {
            out.extend(rest(plan, t, list, except_ids));
        }
    }
    // The platform default reaches every other Tenant too.
    let others = tenants
        .iter()
        .filter(|t| whole && !ruled.contains(*t))
        .filter_map(|t| TenantId::new(t).ok());
    for t in others {
        let except_ids = own_ids(t.as_str());
        out.extend(rest(plan, &t, &[], except_ids));
    }
    out
}

/// The step of the Tenant's `*` rule, or of the platform default, over every
/// type of `t` without a history rule of its own; none when it keeps all.
fn rest(plan: &Plan<'_>, t: &TenantId, list: &[Rule], except_ids: Vec<String>) -> Option<Step> {
    let (label, before) = match list.iter().find(|r| r.type_iri == ANY_TYPE) {
        Some(r) if r.history_off => (ANY_TYPE, OFF_CUT.to_owned()),
        Some(r) => (ANY_TYPE, cut(plan.now, r.history?)),
        None => (PLATFORM_LABEL, cut(plan.now, plan.platform?)),
    };
    let except_types = list
        .iter()
        .filter(|r| r.type_iri != ANY_TYPE && r.rules_history())
        .map(|r| r.type_iri.clone())
        .collect();
    Some(Step {
        tenant: t.clone(),
        label: label.to_owned(),
        job: Job::Rest {
            except_types,
            except_ids,
        },
        before,
    })
}

/// What `plan` deletes, or with `dry_run` would delete, step by step: a dry
/// run counts at most [`PREVIEW_CAP`] rows per step in one statement; a
/// delete starts no statement once `budget` has passed since it began, and
/// takes its order from `turns` and leaves it for the next sweep.
async fn retain(
    state: &AppState,
    plan: &Plan<'_>,
    budget: Duration,
    turns: Option<&std::sync::Mutex<Turns>>,
) -> Vec<Deleted> {
    let mut steps = steps(state, plan).await;
    // Partitions every Tenant in them outlived go whole, before the row sweep
    // spends its budget on their rows.
    let floors = floors(plan, &steps);
    if !plan.dry_run && !floors.is_empty() {
        dropped(state.temporal.drop_retained_history(&floors).await);
    }
    let batches = match turns {
        Some(t) => t
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .order(&mut steps),
        None => Vec::new(),
    };
    let (out, ran, failed) = run(
        &steps,
        budget,
        plan.dry_run,
        &batches,
        |step, batch| async move {
            match step.job.scope() {
                Ok(scope) => {
                    state
                        .temporal
                        .retain_history(&step.tenant, scope, &step.before, batch, plan.dry_run)
                        .await
                }
                Err(type_iri) => {
                    state
                        .store
                        .retain_current(&step.tenant, type_iri, &step.before, batch, plan.dry_run)
                        .await
                }
            }
        },
    )
    .await;
    if let Some(t) = turns {
        t.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ran(&steps, &ran, &failed);
    }
    out
}

/// Which sweep of this pod last ran each step (T-5157), so the next one runs
/// first what waited longest, then the shortest retention: a statement that
/// times out on one (Tenant, type), or a budget a backlog spent, delays the
/// others by one minute at most instead of starving them (on dev a timeout on
/// Bike docks kept a PT5M Vehicle history from ever being swept).
///
/// A step whose statement failed runs again 2, 4, 8 .. [`MAX_REST`] sweeps
/// later, with half the batch per failure (at least [`MIN_BATCH`]),
/// one doubling back per sweep it runs clean: on dev 36 timeouts an hour, 5 s
/// each, spent the budget the Vehicle backlog needed (T-5157 reopened).
#[derive(Debug, Default)]
pub(super) struct Turns {
    sweep: u64,
    last: std::collections::HashMap<StepKey, u64>,
    /// Failures in a row, and the first sweep that may run the step again.
    strikes: std::collections::HashMap<StepKey, (u32, u64)>,
}

/// The longest a failing step rests, in sweeps: every step runs at least
/// twice an hour.
const MAX_REST: u64 = 30;
/// The smallest batch a failing step shrinks to.
const MIN_BATCH: usize = 100;

/// A step's identity across sweeps: Tenant, label and job.
type StepKey = (String, String, String);

impl Turns {
    /// Sort `steps`: those that waited longest first, then the latest cut
    /// (the shortest retention); the order changes no step's rows, as no two
    /// steps reach the same row.
    /// The batch of each step in the new order; 0 for one that rests.
    fn order(&self, steps: &mut [Step]) -> Vec<usize> {
        let next = self.sweep.saturating_add(1);
        steps.sort_by(|a, b| {
            let waited = |s: &Step| self.last.get(&s.key()).copied().unwrap_or_default();
            waited(a)
                .cmp(&waited(b))
                .then_with(|| b.before.cmp(&a.before))
        });
        steps
            .iter()
            .map(|s| match self.strikes.get(&s.key()) {
                Some(&(_, at)) if at > next => 0,
                Some(&(n, _)) => BATCH.checked_shr(n).unwrap_or_default().max(MIN_BATCH),
                None => BATCH,
            })
            .collect()
    }

    /// Record which of `steps` this sweep started (`ran` by index) and which
    /// of those failed, and forget steps the rules no longer make.
    fn ran(&mut self, steps: &[Step], ran: &[usize], failed: &[usize]) {
        self.sweep = self.sweep.saturating_add(1);
        let keys: std::collections::HashSet<StepKey> = steps.iter().map(Step::key).collect();
        self.last.retain(|k, _| keys.contains(k));
        self.strikes.retain(|k, _| keys.contains(k));
        for (i, s) in ran.iter().filter_map(|&i| Some((i, steps.get(i)?))) {
            let key = s.key();
            self.last.insert(key.clone(), self.sweep);
            let n = self.strikes.get(&key).map_or(0, |&(n, _)| n);
            if failed.contains(&i) {
                let n = n.saturating_add(1);
                let rest = 1u64.checked_shl(n).unwrap_or(MAX_REST).min(MAX_REST);
                self.strikes
                    .insert(key, (n, self.sweep.saturating_add(rest)));
            } else if n > 1 {
                self.strikes.insert(key, (n.saturating_sub(1), 0));
            } else {
                self.strikes.remove(&key);
            }
        }
    }
}

impl Step {
    fn key(&self) -> StepKey {
        let job = match &self.job {
            Job::Current(_) => "current".to_owned(),
            Job::Entity(id) => id.clone(),
            Job::Type { .. } | Job::Rest { .. } => "history".to_owned(),
        };
        (self.tenant.as_str().to_owned(), self.label.clone(), job)
    }
}

/// Run `steps` in order, each in rounds of `delete` (`batches` rows by
/// step, [`BATCH`] past its end, none for a 0) until a short batch, an error (logged; the
/// next step goes on) or the spent `budget`; what each deleted under its
/// label, the index of every step it started and of every one that failed.
async fn run<'s, F, Fut>(
    steps: &'s [Step],
    budget: Duration,
    dry_run: bool,
    batches: &[usize],
    mut delete: F,
) -> (Vec<Deleted>, Vec<usize>, Vec<usize>)
where
    F: FnMut(&'s Step, usize) -> Fut,
    Fut: std::future::Future<Output = Result<u64, antares_model::NgsiError>>,
{
    let started = std::time::Instant::now();
    let (mut out, mut ran, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for (i, step) in steps.iter().enumerate() {
        let (batch, rounds) = if dry_run {
            (PREVIEW_CAP, 1)
        } else {
            (batches.get(i).copied().unwrap_or(BATCH), ROUNDS)
        };
        if batch == 0 {
            continue;
        }
        let begun = std::time::Instant::now();
        let (tenant, label) = (step.tenant.as_str(), step.label.as_str());
        let kind = if step.job.scope().is_ok() {
            "history"
        } else {
            "entity"
        };
        let mut count = 0u64;
        let mut spent = false;
        for round in 0..rounds {
            if !dry_run && started.elapsed() >= budget {
                spent = true;
                break;
            }
            if round == 0 {
                ran.push(i);
            }
            match delete(step, batch).await {
                Ok(n) => {
                    count = count.saturating_add(n);
                    if n < batch as u64 {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(tenant, r#type = label, kind, batch, error = %e, "retention sweep failed");
                    failed.push(i);
                    break;
                }
            }
        }
        // What the next fix measures: a step that holds the budget a second.
        let ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX);
        if !dry_run && ms >= 1000 {
            tracing::info!(
                tenant,
                r#type = label,
                kind,
                batch,
                count,
                ms,
                "retention sweep step slow"
            );
        }
        if count > 0 {
            out.push(Deleted {
                tenant: tenant.to_owned(),
                type_iri: label.to_owned(),
                kind,
                count,
            });
        }
        if spent {
            tracing::info!(
                budget_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
                "retention sweep budget spent; the next sweep goes on"
            );
            break;
        }
    }
    (out, ran, failed)
}

/// The floor of every Tenant whose whole history `steps` reach (T-4906): the
/// oldest cut of its history steps, when one of them is the rest of its
/// types (its `*` rule or the platform default) and no rule of it keeps a
/// type forever. No step keeps a row of it below the floor, so a past
/// partition of only such rows can go whole instead of row by row. The cuts
/// share one format ([`cut`], [`OFF_CUT`]), so the oldest sorts first.
fn floors(plan: &Plan<'_>, steps: &[Step]) -> Vec<(String, String)> {
    let mut out: std::collections::BTreeMap<&str, (bool, &str)> = std::collections::BTreeMap::new();
    for s in steps {
        let rest = match s.job {
            Job::Current(_) => continue,
            Job::Rest { .. } => true,
            Job::Entity(_) | Job::Type { .. } => false,
        };
        let floor = out
            .entry(s.tenant.as_str())
            .or_insert((false, s.before.as_str()));
        floor.0 |= rest;
        floor.1 = floor.1.min(s.before.as_str());
    }
    let forever = |t: &str| {
        plan.rules.iter().any(|(rt, list)| {
            rt.as_str() == t
                && list
                    .iter()
                    .any(|r| r.history_forever && r.type_iri != ANY_TYPE)
        })
    };
    out.into_iter()
        .filter(|(t, (rest, _))| *rest && !forever(t))
        .map(|(t, (_, floor))| (t.to_owned(), floor.to_owned()))
        .collect()
}

/// Log what the partition drop before the row sweep did (T-4906); a failure
/// leaves the partitions to the row sweep.
fn dropped(result: Result<Vec<String>, antares_model::NgsiError>) {
    match result {
        Ok(lines) => lines
            .iter()
            .for_each(|line| tracing::info!(what = line.as_str(), "retention dropped history")),
        Err(e) => {
            tracing::warn!(error = %e, "retention: partitions not dropped; the row sweep goes on");
        }
    }
}

/// `/q/retention/{tenant}`: GET the document as written, PUT one, DELETE
/// it; `POST …/preview` counts what a document would delete now, `GET
/// …/deleted` totals what this pod's sweeps deleted since it started.
pub(super) fn routes(weak: &Weak<Reloader>) -> Router {
    let (count, totals) = (weak.clone(), weak.clone());
    Router::new()
        .route(
            "/retention/{tenant}/preview",
            post(move |Path(tenant): Path<String>, body: Bytes| {
                serve(count.clone(), |r| preview(r, tenant, body))
            }),
        )
        .route(
            "/retention/{tenant}/deleted",
            get(move |Path(tenant): Path<String>| {
                serve(totals.clone(), |r| async move { Ok(deleted(&r, &tenant)) })
            }),
        )
        .route("/retention/{tenant}", DOC.handlers(weak))
}

static DOC: super::TextDoc = super::TextDoc {
    prefix: RETENTION_ID_PREFIX,
    what: "retention",
    max: MAX_RETENTION_BYTES,
    check: |body| parse(body).map(drop),
};

fn counts(rows: impl Iterator<Item = (String, &'static str, u64)>) -> Response {
    let rows: Vec<Value> = rows
        .map(|(ty, kind, count)| serde_json::json!({"type": ty, "kind": kind, "count": count}))
        .collect();
    axum::Json(serde_json::json!({ "counts": rows })).into_response()
}

async fn preview(r: Arc<Reloader>, tenant: String, body: Bytes) -> Result<Response, String> {
    let Ok(t) = TenantId::new(&tenant) else {
        return Ok(bad_tenant());
    };
    if let Some(refusal) = DOC.refused(&body) {
        return Ok(refusal);
    }
    let rules = vec![(t, parse(&body)?)];
    let plan = Plan {
        rules: &rules,
        platform: r.platform_history,
        now: chrono::Utc::now(),
        dry_run: true,
        entities: false,
    };
    let found = retain(&r.state, &plan, SWEEP_BUDGET, None).await;
    Ok(counts(
        found.into_iter().map(|d| (d.type_iri, d.kind, d.count)),
    ))
}

fn deleted(r: &Reloader, tenant: &str) -> Response {
    let Ok(t) = TenantId::new(tenant) else {
        return bad_tenant();
    };
    let totals = r
        .deleted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    counts(
        totals
            .into_iter()
            .filter(|((owner, _, _), _)| owner == t.as_str())
            .map(|((_, ty, kind), count)| (ty, kind, count)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_iso_and_short_forms_within_bounds() {
        let secs = |s: &str| parse_duration(s).map(|d| d.as_secs());
        assert_eq!(secs("PT5M"), Ok(300));
        assert_eq!(secs("PT8H"), Ok(28_800));
        assert_eq!(secs("P1DT12H"), Ok(129_600));
        assert_eq!(secs("P30D"), Ok(2_592_000));
        assert_eq!(secs("5m"), Ok(300));
        assert_eq!(secs("8h"), Ok(28_800));
        assert_eq!(secs("90s"), Ok(90));
        assert_eq!(secs("14d"), Ok(1_209_600));
        for bad in [
            "", "P", "PT", "PT30S", "59s", "0m", "-5m", "5w", "P3651D", "PT5X", "P1H", "5", "m",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_document_names_expanded_types_once() {
        let ok =
            br#"{"rules":[{"type":"https://example.org/Vehicle","current":"PT5M","history":"PT8H"},
                              {"type":"https://example.org/Stop"}]}"#;
        let rules = parse(ok).unwrap_or_default();
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules.first().and_then(|r| r.current),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            rules.get(1).map(|r| (r.current, r.history)),
            Some((None, None))
        );
        // T-4275: history "off" is no duration; any other word still is one or nothing.
        let off =
            parse(br#"{"rules":[{"type":"https://e.org/V","history":"off"}]}"#).unwrap_or_default();
        assert_eq!(
            off.first().map(|r| (r.history, r.history_off)),
            Some((None, true))
        );
        let pair = ("city".to_owned(), "https://e.org/V".to_owned());
        assert_eq!(
            history_off(&vec![(TenantId::new("city").expect("tenant"), off)]),
            antares_api::HistoryRules {
                off: std::collections::HashSet::from([pair.clone()]),
                ruled: std::collections::HashSet::from([pair]),
                space_off: std::collections::HashSet::new(),
                space_on: std::collections::HashSet::new(),
                recording: std::collections::HashMap::new(),
            }
        );
        assert!(rules.iter().all(|r| !r.history_off));
        for bad in [
            &br#"{"rules":[{"type":"https://e.org/V","history":"Off"}]}"#[..],
            br#"{"rules":[{"type":"https://e.org/V","history":"none"}]}"#,
            br#"{"rules":[{"type":"Vehicle","current":"PT5M"}]}"#,
            br#"{"rules":[{"type":"https://e.org/V"},{"type":"https://e.org/V"}]}"#,
            br#"{"rules":[{"type":"https://e.org/V","current":"PT0M"}]}"#,
            br#"{"rules":[{"type":"https://e.org/V","ttl":"PT5M"}]}"#,
            br#"{"rules":[{"type":":Vehicle"}]}"#,
            br#"{"rules":[{"type":"https:"}]}"#,
            br#"{"rules":[{"type":"https://e.org/A Vehicle"}]}"#,
            br#"{"rules":{}}"#,
            b"not json",
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// T-5249: PT1H or less is short, a longer duration, `off` and `forever`
    /// are not; a `*` rule makes its Tenant short.
    #[test]
    fn short_history_is_a_duration_of_an_hour_or_less() {
        let doc = br#"{"rules":[{"type":"*","history":"PT5M"},
                                 {"type":"https://e.org/Vehicle","history":"PT1H"},
                                 {"type":"https://e.org/Stop","history":"P30D"},
                                 {"type":"https://e.org/Off","history":"off"},
                                 {"type":"https://e.org/Keep","history":"forever"},
                                 {"type":"https://e.org/Now","current":"PT5M"}]}"#;
        let rules = parse(doc).unwrap_or_default();
        assert_eq!(rules.len(), 6);
        let short = short_history(&vec![(TenantId::new("city").expect("tenant"), rules)]);
        let pair = |t: &str| ("city".to_owned(), format!("https://e.org/{t}"));
        assert_eq!(
            short.types,
            std::collections::HashMap::from([
                (pair("Vehicle"), true),
                (pair("Stop"), false),
                (pair("Off"), false),
                (pair("Keep"), false),
            ])
        );
        assert_eq!(
            short.tenants,
            std::collections::HashSet::from(["city".to_owned()])
        );
    }

    /// DM-97: the `*` rule is the Tenant's default, history only; `forever`
    /// is a word like `off`, and a type with a history rule of any kind is
    /// out of the default's reach while a `current`-only rule is not.
    #[test]
    fn the_star_rule_is_history_only_and_forever_rules_a_type() {
        let doc = br#"{"rules":[{"type":"*","history":"off"},
                                 {"type":"https://e.org/Keep","history":"forever"},
                                 {"type":"https://e.org/Now","current":"PT5M"},
                                 {"type":"https://e.org/Week","history":"P7D"}]}"#;
        let rules = parse(doc).unwrap_or_default();
        assert_eq!(rules.len(), 4);
        assert_eq!(
            rules
                .get(1)
                .map(|r| (r.history, r.history_off, r.history_forever)),
            Some((None, false, true))
        );
        let gate = history_off(&vec![(TenantId::new("city").expect("tenant"), rules)]);
        let pair = |t: &str| ("city".to_owned(), format!("https://e.org/{t}"));
        assert_eq!(
            gate.space_off,
            std::collections::HashSet::from(["city".to_owned()])
        );
        assert!(gate.off.is_empty());
        assert!(gate.space_on.is_empty());
        // T-5056: a `*` duration opts every type of its Tenant in
        let on = parse(br#"{"rules":[{"type":"*","history":"P30D"}]}"#).unwrap_or_default();
        let opted = history_off(&vec![(TenantId::new("town").expect("tenant"), on)]);
        assert_eq!(
            opted.space_on,
            std::collections::HashSet::from(["town".to_owned()])
        );
        assert!(opted.space_off.is_empty());
        // a `*` rule that names no history opts nothing in, nor out
        let silent = parse(br#"{"rules":[{"type":"*"}]}"#).unwrap_or_default();
        assert_eq!(silent.len(), 1);
        let neither = history_off(&vec![(TenantId::new("town").expect("tenant"), silent)]);
        assert!(neither.space_on.is_empty());
        assert!(neither.space_off.is_empty());
        assert_eq!(
            gate.ruled,
            std::collections::HashSet::from([pair("Keep"), pair("Week")])
        );
        for bad in [
            &br#"{"rules":[{"type":"*","current":"PT5M"}]}"#[..],
            br#"{"rules":[{"type":"*","history":"P7D"},{"type":"*"}]}"#,
            br#"{"rules":[{"type":"**","history":"P7D"}]}"#,
            br#"{"rules":[{"type":"*","history":"always"}]}"#,
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_document_holds_at_most_the_rule_limit() {
        let doc = |n: usize| {
            let rules: Vec<_> = (0..n)
                .map(|i| serde_json::json!({"type": format!("https://e.org/T{i}")}))
                .collect();
            serde_json::json!({ "rules": rules }).to_string()
        };
        assert!(parse(doc(MAX_RETENTION_RULES).as_bytes()).is_ok());
        assert!(parse(doc(MAX_RETENTION_RULES.saturating_add(1)).as_bytes()).is_err());
    }

    #[test]
    fn load_skips_a_document_that_no_longer_parses_and_keeps_the_rest() {
        let good =
            serde_json::json!({"text": r#"{"rules":[{"type":"https://e.org/V","history":"5m"}]}"#});
        let bad = serde_json::json!({"text": "nope"});
        let rules = load([("city", &good), ("port", &bad), ("Not One", &good)].into_iter());
        assert_eq!(
            rules
                .iter()
                .map(|(t, r)| (t.as_str(), r.len()))
                .collect::<Vec<_>>(),
            [("city", 1)]
        );
    }

    /// T-4414: a read of the Entities that rule themselves that fills the
    /// cap, or fails, may leave one out of the exceptions, so only their own
    /// steps and current state run.
    #[test]
    fn without_every_entity_that_rules_itself_no_history_runs_by_type_or_tenant() {
        let t = TenantId::new("city").unwrap_or_else(|e| panic!("{e:?}"));
        let rules = vec![(
            t,
            parse(br#"{"rules":[{"type":"https://e.org/V","current":"P1D","history":"P30D"},{"type":"*","history":"P7D"}]}"#)
                .unwrap_or_default(),
        )];
        let plan = Plan {
            rules: &rules,
            platform: Some(Duration::from_hours(24)),
            now: chrono::Utc::now(),
            dry_run: false,
            entities: true,
        };
        let own = std::collections::BTreeMap::from([(
            "city".to_owned(),
            vec![(
                "urn:ngsi-ld:V:1".to_owned(),
                "2025-01-01T00:00:00.000Z".to_owned(),
            )],
        )]);
        let jobs = |whole, tenants: &[String]| {
            levels(&plan, &own, whole, tenants)
                .into_iter()
                .map(|s| match s.job {
                    Job::Current(_) => "current",
                    Job::Entity(_) => "entity",
                    Job::Type { .. } => "type",
                    Job::Rest { .. } => "rest",
                })
                .collect::<Vec<_>>()
        };
        let port = ["port".to_owned()];
        assert_eq!(
            jobs(true, &port),
            ["entity", "current", "type", "rest", "rest"]
        );
        assert_eq!(jobs(false, &port), ["entity", "current"]);
    }

    /// T-4906: a Tenant's floor is its oldest history cut, and only a Tenant
    /// whose every row some step reaches has one: none without a `*` rule or
    /// platform default, none with a type kept forever, none while the
    /// Entities that rule themselves are not all read.
    #[test]
    fn a_tenant_floor_is_its_oldest_cut_and_only_when_every_row_is_ruled() {
        let tenant = |n: &str| TenantId::new(n).unwrap_or_else(|e| panic!("{e:?}"));
        let doc = |d: &[u8]| parse(d).unwrap_or_else(|e| panic!("{e}"));
        let rules = vec![
            // mixed: a type kept 30 days, the rest 7, one Entity a year
            (tenant("city"), doc(br#"{"rules":[{"type":"https://e.org/V","history":"P30D"},{"type":"*","history":"P7D"}]}"#)),
            // a type kept forever: no row of the Tenant may go by partition
            (tenant("port"), doc(br#"{"rules":[{"type":"https://e.org/V","history":"forever"},{"type":"*","history":"P7D"}]}"#)),
            // no `*` rule and no platform default: the other types keep all
            (tenant("farm"), doc(br#"{"rules":[{"type":"https://e.org/V","history":"P1D"}]}"#)),
        ];
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-20T00:00:00Z")
            .expect("instant")
            .with_timezone(&chrono::Utc);
        let own = std::collections::BTreeMap::from([(
            "city".to_owned(),
            vec![(
                "urn:ngsi-ld:V:1".to_owned(),
                cut(now, Duration::from_hours(24 * 365)),
            )],
        )]);
        let floors_of = |platform, whole, tenants: &[String]| {
            let plan = Plan {
                rules: &rules,
                platform,
                now,
                dry_run: false,
                entities: true,
            };
            floors(&plan, &levels(&plan, &own, whole, tenants))
        };
        assert_eq!(
            floors_of(None, true, &[]),
            [("city".to_owned(), "2025-10-20T00:00:00.000Z".to_owned())]
        );
        // the platform default gives a Tenant without any document a floor
        let quiet = ["quiet".to_owned()];
        assert_eq!(
            floors_of(Some(Duration::from_hours(48)), true, &quiet),
            [
                ("city".to_owned(), "2025-10-20T00:00:00.000Z".to_owned()),
                ("farm".to_owned(), "2026-10-18T00:00:00.000Z".to_owned()),
                ("quiet".to_owned(), "2026-10-18T00:00:00.000Z".to_owned()),
            ]
        );
        assert_eq!(
            floors_of(Some(Duration::from_hours(48)), false, &quiet),
            Vec::<(String, String)>::new()
        );
    }

    /// T-4906: what the drop did is logged, and a failure is no panic.
    #[test]
    fn a_partition_drop_is_logged_and_a_failed_one_leaves_the_row_sweep() {
        dropped(Ok(vec![
            "dropped partition attr_instances_2026w38".to_owned()
        ]));
        dropped(Err(antares_model::NgsiError::InternalError(
            "lock timeout".into(),
        )));
    }

    /// T-4665: a type records `every` unless it says `onChange` with a
    /// heartbeat; an attribute may say otherwise; settings out of place, a
    /// short name and a `*` rule that names a recording are refused.
    #[test]
    fn a_rule_says_how_its_type_records_and_each_attribute_may_differ() {
        let rules = parse(
            br#"{"rules":[{"type":"https://e.org/Spot","history":"P90D","record":"onChange",
                 "heartbeat":"PT1H","deadband":1,
                 "attributes":{"https://e.org/count":{"record":"every"},
                               "https://e.org/location":{"record":"onChange","heartbeat":"PT15M","minDistance":5}}},
                {"type":"https://e.org/Flow","history":"P30D"}]}"#,
        )
        .expect("rules");
        let spot = rules[0].recording.as_ref().expect("onChange");
        let on_change = |heartbeat, deadband, min_distance| {
            Some(antares_api::RecordMode::OnChange(antares_api::OnChange {
                heartbeat: std::time::Duration::from_secs(heartbeat),
                deadband,
                min_distance,
            }))
        };
        assert_eq!(
            spot.of("https://e.org/status"),
            on_change(3600, Some(1.0), None)
        );
        assert_eq!(
            spot.of("https://e.org/count"),
            Some(antares_api::RecordMode::Every)
        );
        assert_eq!(
            spot.of("https://e.org/location"),
            on_change(900, None, Some(5.0))
        );
        assert!(rules[1].recording.is_none(), "every, the default");
        // T-5056: observed takes no settings, for the type or an attribute
        let observed = parse(
            br#"{"rules":[{"type":"https://e.org/Dock","history":"P7D","record":"observed",
                 "attributes":{"https://e.org/name":{"record":"every"}}}]}"#,
        )
        .expect("observed");
        let dock = observed[0].recording.as_ref().expect("observed");
        assert_eq!(
            dock.of("https://e.org/free"),
            Some(antares_api::RecordMode::Observed)
        );
        assert_eq!(
            dock.of("https://e.org/name"),
            Some(antares_api::RecordMode::Every)
        );
        let never = parse(
            br#"{"rules":[{"type":"https://e.org/Dock","history":"P7D",
                 "attributes":{"https://e.org/dateModified":{"record":"never"},"https://e.org/x":{}}}]}"#,
        )
        .expect("never");
        let dock = never[0].recording.as_ref().expect("never");
        assert_eq!(
            dock.of("https://e.org/dateModified"),
            Some(antares_api::RecordMode::Never)
        );
        assert_eq!(dock.of("https://e.org/x"), None, "unset takes the type's");
        assert!(parse(
            br#"{"rules":[{"type":"https://e.org/V","record":"observed","heartbeat":"PT1H"}]}"#
        )
        .is_err_and(|e| e.contains("belong")));
        let by_attribute = parse(
            br#"{"rules":[{"type":"https://e.org/V","attributes":{"https://e.org/a":{"record":"onChange","heartbeat":"PT1H"}}}]}"#,
        )
        .expect("an attribute alone records onChange");
        assert!(by_attribute[0].recording.is_some());
        let gate = history_off(&vec![(TenantId::new("city").expect("tenant"), rules)]);
        assert_eq!(gate.recording.len(), 1);
        for (bad, says) in [
            (&br#"{"rules":[{"type":"https://e.org/V","record":"onChange"}]}"#[..], "heartbeat"),
            (br#"{"rules":[{"type":"https://e.org/V","record":"sometimes"}]}"#, "none of"),
            (br#"{"rules":[{"type":"https://e.org/V","deadband":1}]}"#, "belong"),
            (br#"{"rules":[{"type":"https://e.org/V","record":"onChange","heartbeat":"PT1H","deadband":-1}]}"#, "at least 0"),
            (br#"{"rules":[{"type":"https://e.org/V","attributes":{"speed":{"record":"every"}}}]}"#, "expanded IRI"),
            (br#"{"rules":[{"type":"*","history":"P30D","record":"onChange","heartbeat":"PT1H"}]}"#, "never how"),
            (br#"{"rules":[{"type":"https://e.org/V","attributes":{"https://e.org/a":{"ttl":1}}}]}"#, "unknown field"),
            (br#"{"rules":[{"type":"https://e.org/V","heartbeat":"PT1H"}]}"#, "belong"),
            (br#"{"rules":[{"type":"https://e.org/V","minDistance":1}]}"#, "belong"),
            (br#"{"rules":[{"type":"https://e.org/V","record":"onChange","heartbeat":"soon"}]}"#, "not a duration"),
            (br#"{"rules":[{"type":"https://e.org/V","record":"onChange","heartbeat":"PT1H","minDistance":-1}]}"#, "at least 0"),
            (br#"{"rules":[{"type":"https://e.org/V","attributes":{"https://e.org/a b":{"record":"every"}}}]}"#, "expanded IRI"),
            (br#"{"rules":[{"type":"*","history":"P30D","attributes":{"https://e.org/a":{"record":"every"}}}]}"#, "never how"),
        ] {
            let err = parse(bad).expect_err("refused");
            assert!(err.contains(says), "{err}");
        }
    }

    fn history_step(tenant: &str, ty: &str, before: &str) -> Step {
        Step {
            tenant: TenantId::new(tenant).expect("tenant"),
            label: ty.to_owned(),
            job: Job::Type {
                type_iri: ty.to_owned(),
                except_ids: Vec::new(),
            },
            before: before.to_owned(),
        }
    }

    /// T-5157: a statement that times out on the first (Tenant, type) is
    /// logged and the other two are still swept in the same run, the second
    /// past one batch.
    #[tokio::test]
    async fn a_step_that_times_out_leaves_the_others_swept_in_the_same_run() {
        let steps = [
            history_step("helsinki", "Dock", "2026-09-21T00:00:00.000Z"),
            history_step("helsinki", "Vehicle", "2026-10-21T22:55:00.000Z"),
            history_step("praha", "Bin", "2026-09-21T00:00:00.000Z"),
        ];
        let left = std::sync::Mutex::new(BATCH.saturating_mul(2).saturating_add(7));
        let (out, ran, failed) = run(&steps, SWEEP_BUDGET, false, &[], |step, batch| {
            let slow = step.label == "Dock";
            let n = if step.label == "Vehicle" {
                let mut left = left.lock().expect("rows");
                let n = batch.min(*left);
                *left = left.saturating_sub(n);
                n
            } else {
                3
            };
            async move {
                if slow {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    return Err(antares_model::NgsiError::InternalError(
                        "statement timeout".into(),
                    ));
                }
                Ok(n as u64)
            }
        })
        .await;
        let swept: Vec<(&str, u64)> = out.iter().map(|d| (d.type_iri.as_str(), d.count)).collect();
        assert_eq!(swept, [("Vehicle", 2007), ("Bin", 3)]);
        assert_eq!(ran, [0, 1, 2]);
        assert_eq!(failed, [0]);
    }

    /// T-5157: the shortest retention runs first; what a spent budget left
    /// runs first in the next sweep; a step the rules dropped is forgotten.
    #[test]
    fn the_steps_that_waited_run_first_then_the_shortest_retention() {
        let names = |steps: &[Step]| steps.iter().map(|s| s.label.clone()).collect::<Vec<_>>();
        let mut turns = Turns::default();
        let mut steps = vec![
            history_step("helsinki", "Dock", "2026-09-21T00:00:00.000Z"),
            history_step("praha", "Bin", "2026-10-14T00:00:00.000Z"),
            history_step("helsinki", "Vehicle", "2026-10-21T22:55:00.000Z"),
        ];
        turns.order(&mut steps);
        assert_eq!(names(&steps), ["Vehicle", "Bin", "Dock"]);
        turns.ran(&steps, &[0], &[]);
        turns.order(&mut steps);
        assert_eq!(
            names(&steps),
            ["Bin", "Dock", "Vehicle"],
            "the budget left two"
        );
        turns.ran(&steps, &[0, 1, 2], &[]);
        turns.order(&mut steps);
        assert_eq!(names(&steps), ["Vehicle", "Bin", "Dock"]);
        steps.retain(|s| s.label != "Dock");
        turns.ran(&steps, &[], &[]);
        assert_eq!(turns.last.len(), 2);
    }

    /// T-5157 reopened: a step that fails rests 1, 3, 7 sweeps and returns
    /// with 500, 250, 125 rows a batch; each clean run doubles it back; the
    /// others keep the full batch, a resting step is never started, and a
    /// step past a second is logged as slow.
    #[tokio::test]
    async fn a_failing_step_rests_longer_and_comes_back_with_a_smaller_batch() {
        let mut turns = Turns::default();
        let mut steps = vec![
            history_step("helsinki", "Dock", "2026-10-14T00:00:00.000Z"),
            history_step("helsinki", "Vehicle", "2026-10-21T22:55:00.000Z"),
        ];
        let sweep = |turns: &mut Turns, steps: &mut [Step], dock_fails: bool| {
            let batches = turns.order(steps);
            let dock = steps.iter().position(|s| s.label == "Dock").expect("dock");
            let dock_batch = batches.get(dock).copied().expect("batch");
            let ran: Vec<usize> = (0..steps.len())
                .filter(|&i| batches.get(i) != Some(&0))
                .collect();
            let failed: Vec<usize> = ran
                .iter()
                .copied()
                .filter(|&i| dock_fails && i == dock)
                .collect();
            turns.ran(steps, &ran, &failed);
            dock_batch
        };
        assert_eq!(sweep(&mut turns, &mut steps, true), 1000);
        let rests: Vec<usize> = (0..12)
            .map(|_| sweep(&mut turns, &mut steps, true))
            .collect();
        assert_eq!(rests, [0, 500, 0, 0, 0, 250, 0, 0, 0, 0, 0, 0]);
        assert_eq!(sweep(&mut turns, &mut steps, false), 0);
        assert_eq!(sweep(&mut turns, &mut steps, false), 125);
        assert_eq!(sweep(&mut turns, &mut steps, false), 250);
        assert_eq!(sweep(&mut turns, &mut steps, false), 500);
        assert_eq!(sweep(&mut turns, &mut steps, false), 1000);
        assert!(turns.strikes.is_empty());

        let seen = std::sync::Mutex::new(Vec::new());
        let (_, ran, failed) = run(&steps, SWEEP_BUDGET, false, &[0, 250], |step, batch| {
            seen.lock().expect("seen").push((step.label.clone(), batch));
            async move {
                // Past a second: the step is logged as slow.
                tokio::time::sleep(Duration::from_millis(1000)).await;
                Ok(0)
            }
        })
        .await;
        assert_eq!((ran, failed), (vec![1], vec![]));
        let seen = seen.lock().expect("seen").clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen.first().map(|(_, b)| *b), Some(250));
    }
}
