//! Alerts a person chooses (API/01 §37, PL-71, T-3261).
//!
//! A person subscribes to a pipeline, a space or a type in a space. After every sync the leader
//! reads the pipelines' state: a pipeline in `Error` is a `failure`, a `StreamWriting` condition
//! that is `False` is `zero` output, the daily quality run's `stale` freshness is `stale`
//! (DM-74), and a rule the run finds broken in a space for the first time is `invalid` (T-5066). An incident opens when a signal starts and closes when it clears; each unmuted
//! subscriber whose subscription covers the pipeline gets one notice for the opening and one for
//! the recovery, or, on `digest`, one notice a day. Durable with a database, in memory without.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::activity::{chrono_to_odt, odt_to_chrono};
use crate::resource::Phase;
use crate::store::Mirror;

/// The most notices one person keeps; the oldest go first.
pub const KEPT: i64 = 200;
/// The hour (UTC) after which the day's digest is written.
pub const DIGEST_HOUR: u32 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = AlertScope)]
pub enum Scope {
    Pipeline,
    Space,
    Type,
    /// An agent profile's runs in the project (T-5301): its target is the profile's name.
    Agent,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(as = AlertEvent)]
pub enum Event {
    Failure,
    Stale,
    Zero,
    /// The daily quality run found a rule and path the space's data did not break the day before.
    Invalid,
    /// `REFUSAL_SPIKE` or more refused tool calls of one agent profile within an hour (AG-136).
    #[serde(rename = "refusalSpike")]
    RefusalSpike,
    /// A run of the profile used `SPEND_SHARE_PERCENT` of its token cap or more.
    #[serde(rename = "spendShare")]
    SpendShare,
    /// A run of the profile wrote outside office hours.
    #[serde(rename = "offHoursWrite")]
    OffHoursWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = AlertDelivery)]
pub enum Delivery {
    Portal,
    Digest,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
            Self::Space => "space",
            Self::Type => "type",
            Self::Agent => "agent",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pipeline" => Self::Pipeline,
            "space" => Self::Space,
            "type" => Self::Type,
            "agent" => Self::Agent,
            _ => return None,
        })
    }
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::Stale => "stale",
            Self::Zero => "zero",
            Self::Invalid => "invalid",
            Self::RefusalSpike => "refusalSpike",
            Self::SpendShare => "spendShare",
            Self::OffHoursWrite => "offHoursWrite",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "failure" => Self::Failure,
            "stale" => Self::Stale,
            "zero" => Self::Zero,
            "invalid" => Self::Invalid,
            "refusalSpike" => Self::RefusalSpike,
            "spendShare" => Self::SpendShare,
            "offHoursWrite" => Self::OffHoursWrite,
            _ => return None,
        })
    }
    /// Whether the event is about an agent profile's runs, not a pipeline.
    pub fn is_agent(self) -> bool {
        matches!(
            self,
            Self::RefusalSpike | Self::SpendShare | Self::OffHoursWrite
        )
    }
}

impl Delivery {
    fn as_str(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::Digest => "digest",
        }
    }
}

/// One subscription as the API shows it.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AlertSubscription)]
pub struct Subscription {
    pub id: i64,
    pub project: String,
    pub scope: Scope,
    pub target: String,
    pub events: Vec<Event>,
    pub delivery: Delivery,
    /// RFC 3339; none when it is not muted.
    pub muted_until: Option<String>,
}

/// What a person asks to be told about.
#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(as = NewAlertSubscription)]
pub struct NewSubscription {
    pub project: String,
    pub scope: String,
    pub target: String,
    pub events: Vec<String>,
    pub delivery: String,
}

/// One notice left for a subscriber.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AlertNotice)]
pub struct Notice {
    pub id: i64,
    /// The subscription it came from, which "Mute" names.
    pub subscription: i64,
    pub project: String,
    /// The pipeline, or the agent profile for `refusalSpike`, `spendShare` and `offHoursWrite`.
    pub pipeline: String,
    pub event: Event,
    /// `opened`, `recovered` or `digest`.
    pub change: String,
    pub detail: String,
    /// RFC 3339.
    pub created_at: String,
    pub read: bool,
}

/// A subscription as the evaluator reads it: whose, and when they last got a digest.
#[derive(Debug, Clone)]
pub struct Held {
    pub subject: String,
    pub subscription: Subscription,
    pub muted_until: Option<DateTime<Utc>>,
    pub digest_at: Option<DateTime<Utc>>,
}

/// `(project, pipeline, event)`.
pub type IncidentKey = (String, String, Event);

/// An incident as the digest reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub key: IncidentKey,
    pub detail: String,
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct Memory {
    next: i64,
    subscriptions: Vec<Held>,
    incidents: Vec<(Incident, Option<DateTime<Utc>>)>,
    notices: Vec<(String, Notice)>,
}

pub struct AlertStore {
    inner: Inner,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<Memory>),
}

type Stamp = time::OffsetDateTime;

pub(crate) fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// One row of `alert_notices` as Postgres answers it.
type NoticeRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    Stamp,
    Option<Stamp>,
);

type SubscriptionRow = (
    i64,
    String,
    String,
    String,
    String,
    Vec<String>,
    String,
    Option<Stamp>,
    Option<Stamp>,
);

fn held_of(row: SubscriptionRow) -> Option<Held> {
    let (id, subject, project, scope, target, events, delivery, muted_until, digest_at) = row;
    let muted_until = muted_until.map(odt_to_chrono);
    let digest_at = digest_at.map(odt_to_chrono);
    Some(Held {
        subject,
        subscription: Subscription {
            id,
            project,
            scope: Scope::parse(&scope)?,
            target,
            events: events.iter().filter_map(|e| Event::parse(e)).collect(),
            delivery: if delivery == "digest" {
                Delivery::Digest
            } else {
                Delivery::Portal
            },
            muted_until: muted_until.map(rfc3339),
        },
        muted_until,
        digest_at,
    })
}

/// A subscription checked: its scope, events and delivery read, each event once.
pub fn checked(new: &NewSubscription) -> Result<(Scope, Vec<Event>, Delivery), String> {
    let scope = Scope::parse(&new.scope)
        .ok_or_else(|| format!("`{}` is not pipeline, space, type or agent", new.scope))?;
    let (agent, names) = if scope == Scope::Agent {
        (true, "refusalSpike, spendShare or offHoursWrite")
    } else {
        (false, "failure, stale, zero or invalid")
    };
    if new.events.is_empty() {
        return Err(format!("events: name at least one of {names}"));
    }
    let mut events = BTreeSet::new();
    for event in &new.events {
        let parsed = Event::parse(event)
            .filter(|e| e.is_agent() == agent)
            .ok_or_else(|| format!("events: `{event}` is not {names}"))?;
        events.insert(parsed);
    }
    if new.target.is_empty() {
        return Err("target: name what the alert watches".into());
    }
    let delivery =
        match new.delivery.as_str() {
            "portal" => Delivery::Portal,
            "digest" => Delivery::Digest,
            "email" => return Err(
                "this Portal sends no e-mail yet: it has no mail relay; choose portal or digest"
                    .into(),
            ),
            other => return Err(format!("`{other}` is not portal or digest")),
        };
    if scope == Scope::Type
        && new.target.split_once('/').is_none_or(|(space, kind)| {
            space.is_empty() || !crate::ops::data_views::is_type_name(kind)
        })
    {
        return Err("a type is named `{space}/{Type}`, the Type as the gateway names it".into());
    }
    Ok((scope, events.into_iter().collect(), delivery))
}

/// The most subscriptions one person holds: each is scanned on every sync (F-R2POR1-014).
pub const MAX_SUBSCRIPTIONS: usize = 200;

/// Whether `held`, a person's subscriptions, leaves room for `new` in `scope`: a change to one
/// they hold always does.
pub fn room_for(held: &[Subscription], new: &NewSubscription, scope: Scope) -> Result<(), String> {
    let changes_one = held
        .iter()
        .any(|s| s.project == new.project && s.scope == scope && s.target == new.target);
    if changes_one || held.len() < MAX_SUBSCRIPTIONS {
        Ok(())
    } else {
        Err(format!(
            "you hold {MAX_SUBSCRIPTIONS} alerts, the most one person keeps; stop one you no \
             longer read before adding another"
        ))
    }
}

impl AlertStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new(Memory::default())),
            },
        }
    }

    /// The person's subscriptions, oldest first.
    pub async fn subscriptions_of(&self, subject: &str) -> Result<Vec<Subscription>, sqlx::Error> {
        Ok(self
            .all()
            .await?
            .into_iter()
            .filter(|held| held.subject == subject)
            .map(|held| held.subscription)
            .collect())
    }

    /// Every subscription, for the evaluator.
    pub async fn all(&self) -> Result<Vec<Held>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<SubscriptionRow> = sqlx::query_as(
                    "SELECT id, subject, project, scope, target, events, delivery, muted_until, digest_at FROM alert_subscriptions ORDER BY id",
                )
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().filter_map(held_of).collect())
            }
            Inner::Memory(lock) => Ok(lock
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .subscriptions
                .clone()),
        }
    }

    /// Subscribes, or changes the person's subscription of the same target; a mute is kept.
    pub async fn upsert(
        &self,
        subject: &str,
        project: &str,
        scope: Scope,
        target: &str,
        events: &[Event],
        delivery: Delivery,
    ) -> Result<Subscription, sqlx::Error> {
        let names: Vec<String> = events.iter().map(|e| e.as_str().to_owned()).collect();
        match &self.inner {
            Inner::Postgres(pool) => {
                let row: SubscriptionRow = sqlx::query_as(
                    "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
                     VALUES ($1, $2, $3, $4, $5, $6) \
                     ON CONFLICT (subject, project, scope, target) DO UPDATE SET events = EXCLUDED.events, delivery = EXCLUDED.delivery \
                     RETURNING id, subject, project, scope, target, events, delivery, muted_until, digest_at",
                )
                .bind(subject)
                .bind(project)
                .bind(scope.as_str())
                .bind(target)
                .bind(&names)
                .bind(delivery.as_str())
                .fetch_one(pool)
                .await?;
                held_of(row)
                    .map(|held| held.subscription)
                    .ok_or(sqlx::Error::RowNotFound)
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                if let Some(held) = memory.subscriptions.iter_mut().find(|held| {
                    held.subject == subject
                        && held.subscription.project == project
                        && held.subscription.scope == scope
                        && held.subscription.target == target
                }) {
                    held.subscription.events = events.to_vec();
                    held.subscription.delivery = delivery;
                    return Ok(held.subscription.clone());
                }
                memory.next += 1;
                let subscription = Subscription {
                    id: memory.next,
                    project: project.to_owned(),
                    scope,
                    target: target.to_owned(),
                    events: events.to_vec(),
                    delivery,
                    muted_until: None,
                };
                memory.subscriptions.push(Held {
                    subject: subject.to_owned(),
                    subscription: subscription.clone(),
                    muted_until: None,
                    digest_at: None,
                });
                Ok(subscription)
            }
        }
    }

    /// Removes one of the person's subscriptions, and its notices; `false` when none has that id.
    pub async fn remove(&self, subject: &str, id: i64) -> Result<bool, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query(
                "DELETE FROM alert_subscriptions WHERE id = $1 AND subject = $2",
            )
            .bind(id)
            .bind(subject)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let before = memory.subscriptions.len();
                memory
                    .subscriptions
                    .retain(|held| !(held.subject == subject && held.subscription.id == id));
                let removed = memory.subscriptions.len() < before;
                if removed {
                    memory
                        .notices
                        .retain(|(_, notice)| notice.subscription != id);
                }
                Ok(removed)
            }
        }
    }

    /// Mutes one of the person's subscriptions until `until`, or unmutes it with `None`.
    pub async fn mute(
        &self,
        subject: &str,
        id: i64,
        until: Option<DateTime<Utc>>,
    ) -> Result<Option<Subscription>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let row: Option<SubscriptionRow> = sqlx::query_as(
                    "UPDATE alert_subscriptions SET muted_until = $3 WHERE id = $1 AND subject = $2 \
                     RETURNING id, subject, project, scope, target, events, delivery, muted_until, digest_at",
                )
                .bind(id)
                .bind(subject)
                .bind(until.map(chrono_to_odt))
                .fetch_optional(pool)
                .await?;
                Ok(row.and_then(held_of).map(|held| held.subscription))
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                Ok(memory
                    .subscriptions
                    .iter_mut()
                    .find(|held| held.subject == subject && held.subscription.id == id)
                    .map(|held| {
                        held.muted_until = until;
                        held.subscription.muted_until = until.map(rfc3339);
                        held.subscription.clone()
                    }))
            }
        }
    }

    /// Records that the subscription's digest was written at `at`.
    async fn digested(&self, id: i64, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                sqlx::query("UPDATE alert_subscriptions SET digest_at = $2 WHERE id = $1")
                    .bind(id)
                    .bind(chrono_to_odt(at))
                    .execute(pool)
                    .await?;
                Ok(())
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                if let Some(held) = memory
                    .subscriptions
                    .iter_mut()
                    .find(|held| held.subscription.id == id)
                {
                    held.digest_at = Some(at);
                }
                Ok(())
            }
        }
    }

    /// The incidents open now.
    pub async fn open_incidents(&self) -> Result<BTreeMap<IncidentKey, Incident>, sqlx::Error> {
        Ok(self
            .incidents_since(None)
            .await?
            .into_iter()
            .filter(|(_, recovered)| recovered.is_none())
            .map(|(incident, _)| (incident.key.clone(), incident))
            .collect())
    }

    /// Incidents opened after `since` (every open one when `None`), with when they recovered.
    async fn incidents_since(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<(Incident, Option<DateTime<Utc>>)>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<(String, String, String, String, Stamp, Option<Stamp>)> = sqlx::query_as(
                    "SELECT project, pipeline, event, detail, opened_at, recovered_at FROM alert_incidents \
                     WHERE ($1::timestamptz IS NULL AND recovered_at IS NULL) OR opened_at > $1 ORDER BY id",
                )
                .bind(since.map(chrono_to_odt))
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .filter_map(
                        |(project, pipeline, event, detail, opened_at, recovered_at)| {
                            Some((
                                Incident {
                                    key: (project, pipeline, Event::parse(&event)?),
                                    detail,
                                    opened_at: odt_to_chrono(opened_at),
                                },
                                recovered_at.map(odt_to_chrono),
                            ))
                        },
                    )
                    .collect())
            }
            Inner::Memory(lock) => {
                let memory = lock.read().unwrap_or_else(|e| e.into_inner());
                Ok(memory
                    .incidents
                    .iter()
                    .filter(|(incident, recovered)| match since {
                        None => recovered.is_none(),
                        Some(since) => incident.opened_at > since,
                    })
                    .cloned()
                    .collect())
            }
        }
    }

    async fn open(&self, signal: &Signal, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO alert_incidents (project, pipeline, event, detail, opened_at) VALUES ($1, $2, $3, $4, $5) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(&signal.key.0)
                .bind(&signal.key.1)
                .bind(signal.key.2.as_str())
                .bind(&signal.detail)
                .bind(chrono_to_odt(at))
                .execute(pool)
                .await?;
                Ok(())
            }
            Inner::Memory(lock) => {
                lock.write()
                    .unwrap_or_else(|e| e.into_inner())
                    .incidents
                    .push((
                        Incident {
                            key: signal.key.clone(),
                            detail: signal.detail.clone(),
                            opened_at: at,
                        },
                        None,
                    ));
                Ok(())
            }
        }
    }

    async fn close(&self, key: &IncidentKey, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                sqlx::query(
                    "UPDATE alert_incidents SET recovered_at = $4 \
                     WHERE project = $1 AND pipeline = $2 AND event = $3 AND recovered_at IS NULL",
                )
                .bind(&key.0)
                .bind(&key.1)
                .bind(key.2.as_str())
                .bind(chrono_to_odt(at))
                .execute(pool)
                .await?;
                Ok(())
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                for (incident, recovered) in memory.incidents.iter_mut() {
                    if &incident.key == key && recovered.is_none() {
                        *recovered = Some(at);
                    }
                }
                Ok(())
            }
        }
    }

    /// Leaves a notice for `recipient`, dropping their oldest beyond [`KEPT`].
    async fn notify(
        &self,
        recipient: &str,
        subscription: i64,
        key: &IncidentKey,
        change: &str,
        detail: &str,
    ) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO alert_notices (recipient, subscription, project, pipeline, event, change, detail) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(recipient)
                .bind(subscription)
                .bind(&key.0)
                .bind(&key.1)
                .bind(key.2.as_str())
                .bind(change)
                .bind(detail)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "DELETE FROM alert_notices WHERE recipient = $1 AND id NOT IN \
                     (SELECT id FROM alert_notices WHERE recipient = $1 ORDER BY id DESC LIMIT $2)",
                )
                .bind(recipient)
                .bind(KEPT)
                .execute(&mut *tx)
                .await?;
                tx.commit().await
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                memory.next += 1;
                let notice = Notice {
                    id: memory.next,
                    subscription,
                    project: key.0.clone(),
                    pipeline: key.1.clone(),
                    event: key.2,
                    change: change.to_owned(),
                    detail: detail.to_owned(),
                    created_at: rfc3339(Utc::now()),
                    read: false,
                };
                memory.notices.push((recipient.to_owned(), notice));
                let theirs = memory
                    .notices
                    .iter()
                    .filter(|(r, _)| r == recipient)
                    .count();
                if theirs > KEPT as usize {
                    let mut drop = theirs - KEPT as usize;
                    memory.notices.retain(|(r, _)| {
                        if r == recipient && drop > 0 {
                            drop -= 1;
                            false
                        } else {
                            true
                        }
                    });
                }
                Ok(())
            }
        }
    }

    /// The person's notices, newest first, and how many are unread.
    pub async fn notices(&self, recipients: &[String]) -> Result<Vec<Notice>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<NoticeRow> =
                    sqlx::query_as(
                        "SELECT id, subscription, project, pipeline, event, change, detail, created_at, read_at \
                         FROM alert_notices WHERE recipient = ANY($1) ORDER BY id DESC LIMIT $2",
                    )
                    .bind(recipients)
                    .bind(KEPT)
                    .fetch_all(pool)
                    .await?;
                Ok(rows
                    .into_iter()
                    .filter_map(
                        |(
                            id,
                            subscription,
                            project,
                            pipeline,
                            event,
                            change,
                            detail,
                            created_at,
                            read_at,
                        )| {
                            Some(Notice {
                                id,
                                subscription,
                                project,
                                pipeline,
                                event: Event::parse(&event)?,
                                change,
                                detail,
                                created_at: rfc3339(odt_to_chrono(created_at)),
                                read: read_at.is_some(),
                            })
                        },
                    )
                    .collect())
            }
            Inner::Memory(lock) => {
                let memory = lock.read().unwrap_or_else(|e| e.into_inner());
                Ok(memory
                    .notices
                    .iter()
                    .rev()
                    .filter(|(r, _)| recipients.contains(r))
                    .map(|(_, notice)| notice.clone())
                    .collect())
            }
        }
    }

    /// Marks one of the person's notices read; `false` when none of theirs has that id.
    pub async fn mark_read(&self, recipients: &[String], id: i64) -> Result<bool, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query(
                "UPDATE alert_notices SET read_at = coalesce(read_at, now()) WHERE id = $1 AND recipient = ANY($2)",
            )
            .bind(id)
            .bind(recipients)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let mut found = false;
                for (recipient, notice) in memory.notices.iter_mut() {
                    if notice.id == id && recipients.contains(recipient) {
                        notice.read = true;
                        found = true;
                    }
                }
                Ok(found)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// What the pipelines say.

/// One pipeline as the evaluator matches subscriptions against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    pub project: String,
    pub name: String,
    /// The spaces its output Endpoints write into, by name.
    pub spaces: BTreeSet<String>,
    /// The types its outputs name.
    pub types: BTreeSet<String>,
}

/// A signal that holds now.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub key: IncidentKey,
    pub detail: String,
}

/// The output Endpoints' names and the output types of a pipeline spec, either version.
pub(crate) fn outputs_of(spec: &Value) -> Vec<(Option<String>, Option<String>)> {
    let endpoint = |urn: Option<&Value>| {
        urn.and_then(Value::as_str)
            .and_then(|u| u.rsplit(':').next())
            .map(str::to_owned)
    };
    let kind = |output: Option<&Value>| {
        output
            .and_then(|o| o.get("type"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match spec.get("outputs").and_then(Value::as_array) {
        Some(outputs) => outputs
            .iter()
            .map(|output| (endpoint(output.get("targetEndpoint")), kind(Some(output))))
            .collect(),
        None => vec![(
            endpoint(spec.get("targetEndpoint")),
            kind(spec.get("output")),
        )],
    }
}

/// Every pipeline of the mirror with what it writes, and the signals that hold for it now.
pub fn read_pipelines(
    mirror: &Mirror,
    quality: &crate::quality::Store,
) -> (Vec<Watched>, Vec<Signal>) {
    let mut watched = Vec::new();
    let mut signals = Vec::new();
    let mut pipelines = mirror.matching(|env| env.kind == "Pipeline");
    pipelines.sort_by(|a, b| {
        (&a.metadata.namespace, &a.metadata.name).cmp(&(&b.metadata.namespace, &b.metadata.name))
    });
    for pipeline in pipelines {
        let project = pipeline.metadata.namespace.clone().unwrap_or_default();
        let name = pipeline.metadata.name.clone();
        let mut spaces = BTreeSet::new();
        let mut types = BTreeSet::new();
        for (endpoint, kind) in outputs_of(&pipeline.spec) {
            if let Some(space) = endpoint
                .and_then(|e| mirror.get(&project, "Endpoint", &e))
                .and_then(|e| {
                    e.spec
                        .get("contextSpaceRef")
                        .and_then(crate::resource::reference_name)
                        .map(str::to_owned)
                })
            {
                spaces.insert(space);
            }
            if let Some(kind) = kind {
                types.insert(kind);
            }
        }
        if let Some(status) = &pipeline.status {
            if status.phase == Phase::Error {
                let said = status
                    .conditions
                    .iter()
                    .find_map(|c| c.message.clone())
                    .unwrap_or_default();
                signals.push(Signal {
                    key: (project.clone(), name.clone(), Event::Failure),
                    detail: said,
                });
            }
            if let Some(condition) = status
                .conditions
                .iter()
                .find(|c| c.r#type == "StreamWriting" && c.status == "False")
            {
                signals.push(Signal {
                    key: (project.clone(), name.clone(), Event::Zero),
                    detail: condition
                        .message
                        .clone()
                        .or_else(|| condition.reason.clone())
                        .unwrap_or_default(),
                });
            }
        }
        let fresh: Vec<String> = spaces
            .iter()
            .flat_map(|space| quality.new_rules(&project, space))
            .collect();
        if !fresh.is_empty() {
            // Rule and path only: the entity ids stay behind the space's Policy.
            signals.push(Signal {
                key: (project.clone(), name.clone(), Event::Invalid),
                detail: fresh.join(", "),
            });
        }
        for space in &spaces {
            if let Some(report) = quality.get(&project, space) {
                if report.freshness.iter().any(|row| {
                    row.pipeline == name && row.state == crate::quality::FreshState::Stale
                }) {
                    signals.push(Signal {
                        key: (project.clone(), name.clone(), Event::Stale),
                        detail: String::new(),
                    });
                    break;
                }
            }
        }
        watched.push(Watched {
            project,
            name,
            spaces,
            types,
        });
    }
    (watched, signals)
}

/// A held source delete is a `failure` of its pipeline, unless the pipeline already fails.
pub fn held_signals(
    signals: &mut Vec<Signal>,
    holds: Vec<(String, String, crate::pipeline_source_deletes::Hold)>,
) {
    for (project, name, hold) in holds {
        let key = (project, name, Event::Failure);
        if !signals.iter().any(|signal| signal.key == key) {
            signals.push(Signal {
                key,
                detail: hold.detail(),
            });
        }
    }
}

/// The incident a source's health opens (T-5048): `failure` after 3 failed runs in a row or for
/// an hour that lost more than 5 % of its records (T-5291), `zero` for a newest run more than
/// 80 % under the week's median.
pub fn health_signal(project: &str, health: &crate::pipeline_health::Health) -> Option<Signal> {
    if !health.alert {
        return None;
    }
    let key = |event| (project.to_owned(), health.pipeline.clone(), event);
    Some(
        if health.failed_runs >= crate::pipeline_health::ALERT_STREAK {
            Signal {
                key: key(Event::Failure),
                detail: format!(
                    "{} runs in a row sent nothing: {}",
                    health.failed_runs,
                    health
                        .last_error
                        .as_deref()
                        .unwrap_or("the log names no error")
                ),
            }
        } else if let Some(errors) = health.errors.filter(|errors| errors.over()) {
            Signal {
                key: key(Event::Failure),
                detail: format!(
                    "{} of {} records in the last hour failed or were refused ({} %): {}",
                    errors.errors,
                    errors.records,
                    errors.percent,
                    health
                        .last_error
                        .as_deref()
                        .unwrap_or("the log names no error")
                ),
            }
        } else {
            Signal {
                key: key(Event::Zero),
                detail: format!(
                    "the last run sent {} records against a 7-day median of {}",
                    health.last_sent.unwrap_or_default(),
                    health.median_sent.unwrap_or_default()
                ),
            }
        },
    )
}

/// Whether a subscription covers a pipeline.
pub fn covers(subscription: &Subscription, pipeline: &Watched) -> bool {
    if subscription.project != pipeline.project {
        return false;
    }
    match subscription.scope {
        Scope::Pipeline => subscription.target == pipeline.name,
        Scope::Space => pipeline.spaces.contains(&subscription.target),
        Scope::Type => subscription
            .target
            .split_once('/')
            .is_some_and(|(space, kind)| {
                pipeline.spaces.contains(space) && pipeline.types.contains(kind)
            }),
        Scope::Agent => false,
    }
}

/// Whether a subscription covers an incident: an agent event its profile's `agent`
/// subscription, any other event the pipeline it names.
pub fn covers_key(subscription: &Subscription, key: &IncidentKey, pipelines: &[Watched]) -> bool {
    if key.2.is_agent() {
        return subscription.scope == Scope::Agent
            && subscription.project == key.0
            && subscription.target == key.1;
    }
    pipelines
        .iter()
        .find(|p| p.project == key.0 && p.name == key.1)
        .is_some_and(|p| covers(subscription, p))
}

/// The incidents to open (signals with none open) and to close (open ones whose signal cleared).
pub fn step(
    open: &BTreeMap<IncidentKey, Incident>,
    signals: &[Signal],
) -> (Vec<Signal>, Vec<IncidentKey>) {
    let now: BTreeSet<&IncidentKey> = signals.iter().map(|signal| &signal.key).collect();
    let opening = signals
        .iter()
        .filter(|signal| !open.contains_key(&signal.key))
        .cloned()
        .collect();
    let closing = open
        .keys()
        .filter(|key| !now.contains(key))
        .cloned()
        .collect();
    (opening, closing)
}

/// Whether the day's digest is due for a subscription at `now`.
pub fn digest_due(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    if now.hour() < DIGEST_HOUR {
        return false;
    }
    let today = now
        .date_naive()
        .and_hms_opt(DIGEST_HOUR, 0, 0)
        .map(|at| at.and_utc());
    match (last, today) {
        (Some(last), Some(today)) => last < today,
        (None, _) => true,
        (_, None) => false,
    }
}

/// Runs after every sync of the leader; one run at a time.
pub struct Evaluator {
    pub mirror: Arc<Mirror>,
    pub quality: Arc<crate::quality::Store>,
    pub store: Arc<AlertStore>,
    /// The held source deletes, each a `failure` of its pipeline (PL-97, T-4891).
    holds: Option<Arc<crate::pipeline_source_deletes::Ledger>>,
    /// The runs whose health opens `failure` and `zero` incidents (T-5048), and the signals of
    /// the last hourly reading of them.
    runs: Option<Arc<crate::pipeline_log::LogStore>>,
    health: tokio::sync::Mutex<Option<(DateTime<Utc>, Vec<Signal>)>>,
    /// The health log every run's signals are written to (T-5067).
    log: Option<(
        Arc<crate::health_log::HealthStore>,
        Arc<crate::health_probe::Probes>,
    )>,
    /// The weekly link check, started from a run when it is due (T-5133).
    links: Option<Arc<crate::link_check::Checker>>,
    /// The agent runs whose refusal spikes, token-cap share and off-hours writes open incidents
    /// (T-5301).
    agents: Option<Arc<crate::agents::store::AgentStore>>,
    running: AtomicBool,
}

impl Evaluator {
    pub fn new(
        mirror: Arc<Mirror>,
        quality: Arc<crate::quality::Store>,
        store: Arc<AlertStore>,
    ) -> Self {
        Self {
            mirror,
            quality,
            store,
            holds: None,
            runs: None,
            health: tokio::sync::Mutex::new(None),
            log: None,
            links: None,
            agents: None,
            running: AtomicBool::new(false),
        }
    }

    /// Alerts the subscribers of a pipeline whose source deletes are held (PL-97).
    pub fn with_source_holds(
        mut self,
        ledger: Arc<crate::pipeline_source_deletes::Ledger>,
    ) -> Self {
        self.holds = Some(ledger);
        self
    }

    /// Alerts the subscribers of a source that failed 3 runs in a row or dropped by more than
    /// 80 % (T-5048).
    pub fn with_run_health(mut self, runs: Arc<crate::pipeline_log::LogStore>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Writes every resource's checks to the health log with this run's signals (T-5067).
    /// `probes` asks every live Endpoint's surfaces and counts every space for it.
    pub fn with_health_log(
        mut self,
        log: Arc<crate::health_log::HealthStore>,
        probes: Arc<crate::health_probe::Probes>,
    ) -> Self {
        self.log = Some((log, probes));
        self
    }

    /// Asks the stored links of every pipeline once a week (T-5133); `None` without a gateway.
    pub fn with_link_check(mut self, links: Option<Arc<crate::link_check::Checker>>) -> Self {
        self.links = links;
        self
    }

    /// Alerts the subscribers of an agent profile (T-5301).
    pub fn with_agent_runs(mut self, agents: Arc<crate::agents::store::AgentStore>) -> Self {
        self.agents = Some(agents);
        self
    }

    /// The agent signals of the last hour. When the run store does not answer, the open agent
    /// incidents hold, so a lost read never reads as a recovery.
    async fn add_agents(
        &self,
        open: &BTreeMap<IncidentKey, Incident>,
        signals: &mut Vec<Signal>,
        now: DateTime<Utc>,
    ) {
        let Some(agents) = &self.agents else {
            return;
        };
        match crate::api::agent_activity::signals(agents, &self.mirror, now).await {
            Ok(found) => signals.extend(found),
            Err(error) => {
                tracing::warn!(%error, "the agent runs were not read for the alerts");
                signals.extend(
                    open.values()
                        .filter(|i| i.key.2.is_agent())
                        .map(|i| Signal {
                            key: i.key.clone(),
                            detail: i.detail.clone(),
                        }),
                );
            }
        }
    }

    /// The health signals, read again once an hour: a run ends at most a few times a day.
    async fn add_health(
        &self,
        pipelines: &[Watched],
        signals: &mut Vec<Signal>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let Some(runs) = &self.runs else {
            return Ok(());
        };
        let mut cached = self.health.lock().await;
        if cached
            .as_ref()
            .is_none_or(|(at, _)| now - *at >= Duration::hours(1))
        {
            let mut read = Vec::new();
            for pipeline in pipelines {
                let health = crate::pipeline_health::of_pipelines(
                    runs,
                    &pipeline.project,
                    std::slice::from_ref(&pipeline.name),
                )
                .await?;
                for h in &health {
                    crate::telemetry::pipeline_error_ratio(
                        &pipeline.project,
                        &h.pipeline,
                        h.errors.map_or(0.0, |e| e.errors as f64 / e.records as f64),
                    );
                }
                read.extend(
                    health
                        .iter()
                        .filter_map(|h| health_signal(&pipeline.project, h)),
                );
            }
            *cached = Some((now, read));
        }
        for signal in cached.iter().flat_map(|(_, read)| read) {
            if !signals.iter().any(|held| held.key == signal.key) {
                signals.push(signal.clone());
            }
        }
        Ok(())
    }

    async fn add_holds(&self, signals: &mut Vec<Signal>) -> Result<(), sqlx::Error> {
        if let Some(ledger) = &self.holds {
            held_signals(signals, ledger.holds().await?);
        }
        Ok(())
    }

    /// Starts a run in the background unless one is under way; the sync never waits.
    pub fn start(self: &Arc<Self>) {
        if let Some(links) = &self.links {
            links.start_due(Utc::now());
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let evaluator = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = evaluator.run(Utc::now()).await {
                tracing::warn!(%error, "the alert run did not complete");
            }
            evaluator.running.store(false, Ordering::Release);
        });
    }

    /// One pass: open and close incidents, notify subscribers, write the digests that are due.
    pub async fn run(&self, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        let (pipelines, mut signals) = read_pipelines(&self.mirror, &self.quality);
        self.add_holds(&mut signals).await?;
        self.add_health(&pipelines, &mut signals, now).await?;
        // A mirror not loaded yet judged nothing: the rows and their `since` stay as the last
        // real run left them (F-POR4-017). A loaded mirror with no findings clears them.
        if let Some((log, probes)) = self.log.as_ref().filter(|_| !self.mirror.is_empty()) {
            let mut findings = crate::health_log::derive(&self.mirror, &self.quality, &signals);
            findings.extend(probes.findings(&self.mirror, now).await);
            if let Err(error) = log.record_due(now, findings).await {
                tracing::warn!(%error, "the health log was not written");
            }
        }
        let open = self.store.open_incidents().await?;
        self.add_agents(&open, &mut signals, now).await;
        let (opening, closing) = step(&open, &signals);
        let subscriptions = self.store.all().await?;
        let audience = |key: &IncidentKey| {
            subscriptions
                .iter()
                .filter(|held| {
                    held.subscription.delivery == Delivery::Portal
                        && held.subscription.events.contains(&key.2)
                        && held.muted_until.is_none_or(|until| until <= now)
                        && covers_key(&held.subscription, key, &pipelines)
                })
                .collect::<Vec<_>>()
        };
        for signal in &opening {
            self.store.open(signal, now).await?;
            for held in audience(&signal.key) {
                self.store
                    .notify(
                        &held.subject,
                        held.subscription.id,
                        &signal.key,
                        "opened",
                        &signal.detail,
                    )
                    .await?;
            }
        }
        for key in &closing {
            self.store.close(key, now).await?;
            for held in audience(key) {
                self.store
                    .notify(&held.subject, held.subscription.id, key, "recovered", "")
                    .await?;
            }
        }
        // The digests: one notice per incident opened since the last one, sent once a day.
        let digests: Vec<&Held> = subscriptions
            .iter()
            .filter(|held| {
                held.subscription.delivery == Delivery::Digest && digest_due(held.digest_at, now)
            })
            .collect();
        if !digests.is_empty() {
            let day = self
                .store
                .incidents_since(Some(now - Duration::days(1)))
                .await?;
            for held in digests {
                let muted = held.muted_until.is_some_and(|until| until > now);
                if !muted {
                    for (incident, recovered) in &day {
                        let since = held.digest_at.unwrap_or(now - Duration::days(1));
                        let covered = covers_key(&held.subscription, &incident.key, &pipelines);
                        if incident.opened_at > since
                            && covered
                            && held.subscription.events.contains(&incident.key.2)
                        {
                            let detail = match recovered {
                                Some(at) => format!("recovered {}", rfc3339(*at)),
                                None => incident.detail.clone(),
                            };
                            self.store
                                .notify(
                                    &held.subject,
                                    held.subscription.id,
                                    &incident.key,
                                    "digest",
                                    &detail,
                                )
                                .await?;
                        }
                    }
                }
                self.store.digested(held.subscription.id, now).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn watched(name: &str, space: &str, kind: &str) -> Watched {
        Watched {
            project: "helsinki".into(),
            name: name.into(),
            spaces: [space.to_owned()].into(),
            types: [kind.to_owned()].into(),
        }
    }

    fn subscription(scope: Scope, target: &str) -> Subscription {
        Subscription {
            id: 1,
            project: "helsinki".into(),
            scope,
            target: target.into(),
            events: vec![Event::Failure],
            delivery: Delivery::Portal,
            muted_until: None,
        }
    }

    #[test]
    fn a_subscription_covers_its_pipeline_its_space_and_its_type_there() {
        let bikes = watched("bikes", "mobility", "BikeHireDockingStation");
        assert!(covers(&subscription(Scope::Pipeline, "bikes"), &bikes));
        assert!(!covers(&subscription(Scope::Pipeline, "other"), &bikes));
        assert!(covers(&subscription(Scope::Space, "mobility"), &bikes));
        assert!(!covers(&subscription(Scope::Space, "air"), &bikes));
        assert!(covers(
            &subscription(Scope::Type, "mobility/BikeHireDockingStation"),
            &bikes
        ));
        assert!(!covers(
            &subscription(Scope::Type, "air/BikeHireDockingStation"),
            &bikes
        ));
        let mut elsewhere = subscription(Scope::Pipeline, "bikes");
        elsewhere.project = "espoo".into();
        assert!(!covers(&elsewhere, &bikes));
    }

    #[test]
    fn an_incident_opens_once_and_closes_when_its_signal_clears() {
        let key: IncidentKey = ("helsinki".into(), "bikes".into(), Event::Zero);
        let signal = Signal {
            key: key.clone(),
            detail: "stalled".into(),
        };
        let (opening, closing) = step(&BTreeMap::new(), std::slice::from_ref(&signal));
        assert_eq!((opening, closing), (vec![signal.clone()], vec![]));
        let open = BTreeMap::from([(
            key.clone(),
            Incident {
                key: key.clone(),
                detail: "stalled".into(),
                opened_at: at("2026-10-07T08:00:00Z"),
            },
        )]);
        assert_eq!(step(&open, std::slice::from_ref(&signal)), (vec![], vec![]));
        assert_eq!(step(&open, &[]), (vec![], vec![key]));
    }

    #[test]
    fn a_subscription_is_checked_and_e_mail_is_refused_with_its_reason() {
        let new = |scope: &str, target: &str, events: &[&str], delivery: &str| NewSubscription {
            project: "helsinki".into(),
            scope: scope.into(),
            target: target.into(),
            events: events.iter().map(|e| (*e).to_owned()).collect(),
            delivery: delivery.into(),
        };
        assert_eq!(
            checked(&new(
                "space",
                "bikes",
                &["zero", "failure", "zero"],
                "portal"
            )),
            Ok((
                Scope::Space,
                vec![Event::Failure, Event::Zero],
                Delivery::Portal
            ))
        );
        assert!(checked(&new("space", "bikes", &[], "portal")).is_err());
        assert!(checked(&new("planet", "bikes", &["zero"], "portal")).is_err());
        assert!(checked(&new("space", "bikes", &["boom"], "portal")).is_err());
        assert!(checked(&new("type", "Bikes", &["zero"], "portal")).is_err());
        assert!(checked(&new("type", "mobility/Bike", &["zero"], "digest")).is_ok());
        // F-R2POR1-014 (T-6225): the type half is a type name, bounded as the gateway bounds it.
        let long = format!("mobility/{}", "B".repeat(300));
        assert!(checked(&new("type", &long, &["zero"], "portal")).is_err());
        assert!(checked(&new("type", "mobility/Bike Station", &["zero"], "portal")).is_err());
        assert!(checked(&new("space", "bikes", &["zero"], "email"))
            .unwrap_err()
            .contains("no mail relay"));
    }

    /// F-R2POR1-014 (T-6225): a person holds at most MAX_SUBSCRIPTIONS; changing one they
    /// hold still works at the cap.
    #[test]
    fn a_person_at_the_cap_adds_no_alert_and_still_changes_one() {
        let held: Vec<Subscription> = (0..MAX_SUBSCRIPTIONS)
            .map(|i| Subscription {
                id: i64::try_from(i).expect("small"),
                project: "helsinki".into(),
                scope: Scope::Pipeline,
                target: format!("p{i}"),
                events: vec![Event::Failure],
                delivery: Delivery::Portal,
                muted_until: None,
            })
            .collect();
        let ask = |target: &str| NewSubscription {
            project: "helsinki".into(),
            scope: "pipeline".into(),
            target: target.into(),
            events: vec!["zero".into()],
            delivery: "portal".into(),
        };
        assert!(room_for(&held, &ask("p-new"), Scope::Pipeline).is_err());
        assert!(room_for(&held, &ask("p7"), Scope::Pipeline).is_ok());
        assert!(room_for(&held[1..], &ask("p-new"), Scope::Pipeline).is_ok());
    }

    #[test]
    fn an_agent_subscription_takes_only_agent_events_and_a_pipeline_one_none() {
        let new = |scope: &str, target: &str, events: &[&str]| NewSubscription {
            project: "helsinki".into(),
            scope: scope.into(),
            target: target.into(),
            events: events.iter().map(|e| (*e).to_owned()).collect(),
            delivery: "portal".into(),
        };
        assert_eq!(
            checked(&new("agent", "analyst", &["spendShare", "refusalSpike"])),
            Ok((
                Scope::Agent,
                vec![Event::RefusalSpike, Event::SpendShare],
                Delivery::Portal
            ))
        );
        assert_eq!(
            checked(&new("agent", "analyst", &["failure"])),
            Err("events: `failure` is not refusalSpike, spendShare or offHoursWrite".into())
        );
        assert_eq!(
            checked(&new("pipeline", "bikes", &["offHoursWrite"])),
            Err("events: `offHoursWrite` is not failure, stale, zero or invalid".into())
        );
        assert_eq!(
            checked(&new("agent", "", &["spendShare"])),
            Err("target: name what the alert watches".into())
        );
        assert_eq!(
            serde_json::to_value(Event::OffHoursWrite).unwrap(),
            json!("offHoursWrite")
        );
    }

    #[test]
    fn an_agent_incident_is_covered_only_by_its_profile_s_agent_subscription() {
        let bikes = [watched("analyst", "mobility", "Bike")];
        let key: IncidentKey = ("helsinki".into(), "analyst".into(), Event::RefusalSpike);
        assert!(covers_key(
            &subscription(Scope::Agent, "analyst"),
            &key,
            &bikes
        ));
        assert!(!covers_key(
            &subscription(Scope::Agent, "builder"),
            &key,
            &bikes
        ));
        assert!(
            !covers_key(&subscription(Scope::Pipeline, "analyst"), &key, &bikes),
            "a pipeline of the same name"
        );
        let pipeline_key: IncidentKey = ("helsinki".into(), "analyst".into(), Event::Failure);
        assert!(!covers_key(
            &subscription(Scope::Agent, "analyst"),
            &pipeline_key,
            &bikes
        ));
        assert!(covers_key(
            &subscription(Scope::Pipeline, "analyst"),
            &pipeline_key,
            &bikes
        ));
    }

    fn agent_run(id: &str, created_at: &str) -> crate::agents::run::AgentRun {
        serde_json::from_value(json!({
            "id": id, "project": "helsinki", "appName": "air", "endpointName": "air",
            "endpointSlug": "air", "endpoints": [], "profile": "analyst", "kind": "data",
            "unattended": false, "appClass": "ui", "visibility": "project", "prompt": "p",
            "promptDigest": "sha256:p", "dataNeeds": [], "allowsWrite": false, "branch": "b",
            "pathPrefix": "p/", "status": "building", "ticketHash": "x", "files": {},
            "steps": 0, "tokensUsed": 0, "createdBy": "ana@hel.fi", "origin": "person",
            "starter": null, "createdAt": created_at, "expiresAt": "2099-01-01T00:00:00Z",
        }))
        .expect("a run")
    }

    #[tokio::test]
    async fn a_refusal_spike_tells_the_profile_s_subscriber_once_and_recovers_after_the_hour() {
        let agents = Arc::new(crate::agents::store::AgentStore::new(None));
        let run = agent_run("r1", &rfc3339(Utc::now() - Duration::minutes(10)));
        agents.create_run(&run).await.unwrap();
        for _ in 0..crate::api::agent_activity::REFUSAL_SPIKE {
            agents
                .append_event(
                    "r1",
                    "tool",
                    json!({ "tool": "jc_resource_list", "status": "failed", "error": "no" }),
                )
                .await
                .unwrap();
        }
        // After the refusals: a signal counts only what happened up to the evaluator's now.
        let now = Utc::now() + Duration::seconds(1);
        let mirror = mirror();
        let store = Arc::new(AlertStore::new(None));
        let evaluator = Evaluator::new(
            Arc::clone(&mirror),
            Arc::new(crate::quality::Store::default()),
            Arc::clone(&store),
        )
        .with_agent_runs(Arc::clone(&agents));
        let jana = store
            .upsert(
                "jana",
                "helsinki",
                Scope::Agent,
                "analyst",
                &[Event::RefusalSpike],
                Delivery::Portal,
            )
            .await
            .unwrap();
        for (who, target, event) in [
            ("eeva", "analyst", Event::SpendShare),
            ("mikko", "builder", Event::RefusalSpike),
        ] {
            store
                .upsert(
                    who,
                    "helsinki",
                    Scope::Agent,
                    target,
                    &[event],
                    Delivery::Portal,
                )
                .await
                .unwrap();
        }
        evaluator.run(now).await.unwrap();
        evaluator.run(now + Duration::minutes(1)).await.unwrap();
        evaluator.run(now + Duration::hours(2)).await.unwrap();

        let told = store.notices(&["jana".into()]).await.unwrap();
        assert_eq!(
            told.iter().map(|n| n.change.as_str()).collect::<Vec<_>>(),
            vec!["recovered", "opened"]
        );
        assert!(told.iter().all(|n| n.subscription == jana.id
            && n.event == Event::RefusalSpike
            && n.pipeline == "analyst"));
        assert_eq!(told[1].detail, "5 refused tool calls in the last hour");
        assert!(
            store.notices(&["eeva".into()]).await.unwrap().is_empty(),
            "another event"
        );
        assert!(
            store.notices(&["mikko".into()]).await.unwrap().is_empty(),
            "another profile"
        );
    }

    #[test]
    fn the_digest_is_due_once_a_day_after_seven() {
        assert!(!digest_due(None, at("2026-10-07T06:59:00Z")));
        assert!(digest_due(None, at("2026-10-07T07:00:00Z")));
        assert!(digest_due(
            Some(at("2026-10-06T07:05:00Z")),
            at("2026-10-07T07:01:00Z")
        ));
        assert!(!digest_due(
            Some(at("2026-10-07T07:01:00Z")),
            at("2026-10-07T18:00:00Z")
        ));
    }

    fn pipeline(name: &str, status: Value) -> crate::resource::ResourceEnvelope {
        serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": name, "namespace": "helsinki" },
            "spec": { "class": "auto", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:mobility:bikes-write", "output": { "type": "BikeHireDockingStation" } },
            "status": status,
        }))
        .expect("a pipeline")
    }

    fn mirror() -> Arc<Mirror> {
        let mirror = Arc::new(Mirror::new());
        mirror.upsert(
            serde_json::from_value(json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Endpoint",
                "metadata": { "name": "bikes-write", "namespace": "helsinki" },
                "spec": { "contextSpaceRef": "mobility" },
            }))
            .expect("an endpoint"),
        );
        mirror
    }

    #[test]
    fn a_rule_broken_for_the_first_time_is_an_invalid_signal_naming_rule_and_path_only() {
        let mirror = mirror();
        mirror.upsert(pipeline("bikes", json!({ "phase": "Live" })));
        let quality = crate::quality::Store::default();
        let mut tally = crate::quality::Tally::default();
        let entity = json!({ "id": "urn:ngsi-ld:BikeHireDockingStation:secret-1", "type": "BikeHireDockingStation" });
        let problem = crate::pipeline_validation::Problem {
            rule: "MinInclusive".into(),
            path: "freeSlotNumber".into(),
            message: "below 0".into(),
        };
        tally.add(&entity, None, &[problem]);
        let report = tally.into_quality(chrono::Utc::now(), Vec::new());
        quality.put("helsinki", "mobility", report.clone());
        let (_, signals) = read_pipelines(&mirror, &quality);
        assert_eq!(
            signals,
            vec![Signal {
                key: ("helsinki".into(), "bikes".into(), Event::Invalid),
                detail: "MinInclusive freeSlotNumber".into()
            }]
        );
        quality.put("helsinki", "mobility", report);
        let (_, signals) = read_pipelines(&mirror, &quality);
        assert!(
            signals.is_empty(),
            "the same rule the next day opens nothing"
        );
    }

    #[test]
    fn a_pipeline_in_error_or_writing_nothing_is_a_signal_and_names_its_space_and_type() {
        let mirror = mirror();
        mirror.upsert(pipeline("bikes", json!({ "phase": "Live", "conditions": [{ "type": "StreamWriting", "status": "False", "reason": "Stalled", "message": "records in, nothing out" }] })));
        mirror.upsert(pipeline("broken", json!({ "phase": "Error", "conditions": [{ "type": "Ready", "status": "False", "message": "runner refused" }] })));
        mirror.upsert(pipeline("fine", json!({ "phase": "Live" })));
        let (watched, mut signals) = read_pipelines(&mirror, &crate::quality::Store::default());
        signals.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(
            signals,
            vec![
                Signal {
                    key: ("helsinki".into(), "bikes".into(), Event::Zero),
                    detail: "records in, nothing out".into()
                },
                Signal {
                    key: ("helsinki".into(), "broken".into(), Event::Failure),
                    detail: "runner refused".into()
                },
            ]
        );
        let bikes = watched.iter().find(|w| w.name == "bikes").unwrap();
        assert_eq!(bikes.spaces, ["mobility".to_owned()].into());
        assert_eq!(bikes.types, ["BikeHireDockingStation".to_owned()].into());
    }

    /// F-R2POR1-010 (T-6225): an Endpoint may name its space as a typed reference; the pipeline
    /// writing to it is still watched in that space.
    #[test]
    fn a_typed_space_reference_names_the_space_of_the_pipeline() {
        let mirror = Arc::new(Mirror::new());
        mirror.upsert(
            serde_json::from_value(json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Endpoint",
                "metadata": { "name": "bikes-write", "namespace": "helsinki" },
                "spec": { "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" } },
            }))
            .expect("an endpoint"),
        );
        mirror.upsert(pipeline("bikes", json!({ "phase": "Live" })));
        let (watched, _) = read_pipelines(&mirror, &crate::quality::Store::default());
        let bikes = watched.iter().find(|w| w.name == "bikes").expect("watched");
        assert_eq!(bikes.spaces, ["mobility".to_owned()].into());
    }

    #[tokio::test]
    async fn a_subscriber_is_told_once_when_it_starts_once_when_it_recovers_and_not_while_muted() {
        let mirror = mirror();
        let store = Arc::new(AlertStore::new(None));
        let evaluator = Evaluator::new(
            Arc::clone(&mirror),
            Arc::new(crate::quality::Store::default()),
            Arc::clone(&store),
        );
        let jana = store
            .upsert(
                "jana",
                "helsinki",
                Scope::Space,
                "mobility",
                &[Event::Zero],
                Delivery::Portal,
            )
            .await
            .unwrap();
        let muted = store
            .upsert(
                "eeva",
                "helsinki",
                Scope::Pipeline,
                "bikes",
                &[Event::Zero],
                Delivery::Portal,
            )
            .await
            .unwrap();
        store
            .mute("eeva", muted.id, Some(at("2026-10-08T00:00:00Z")))
            .await
            .unwrap();
        store
            .upsert(
                "mikko",
                "helsinki",
                Scope::Pipeline,
                "bikes",
                &[Event::Failure],
                Delivery::Portal,
            )
            .await
            .unwrap();

        let stalled = json!({ "phase": "Live", "conditions": [{ "type": "StreamWriting", "status": "False", "reason": "Stalled" }] });
        mirror.upsert(pipeline("bikes", stalled));
        evaluator.run(at("2026-10-07T08:00:00Z")).await.unwrap();
        evaluator.run(at("2026-10-07T08:01:00Z")).await.unwrap();
        mirror.upsert(pipeline("bikes", json!({ "phase": "Live" })));
        evaluator.run(at("2026-10-07T08:02:00Z")).await.unwrap();

        let told = store.notices(&["jana".into()]).await.unwrap();
        assert_eq!(
            told.iter().map(|n| n.change.as_str()).collect::<Vec<_>>(),
            vec!["recovered", "opened"]
        );
        assert!(told
            .iter()
            .all(|n| n.subscription == jana.id && n.event == Event::Zero && n.pipeline == "bikes"));
        assert!(
            store.notices(&["eeva".into()]).await.unwrap().is_empty(),
            "muted"
        );
        assert!(
            store.notices(&["mikko".into()]).await.unwrap().is_empty(),
            "another event"
        );
        assert!(store.open_incidents().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_source_failing_three_runs_in_a_row_opens_a_failure_once_an_hour_is_read() {
        let mirror = mirror();
        let store = Arc::new(AlertStore::new(None));
        let runs = Arc::new(crate::pipeline_log::LogStore::new(None));
        let evaluator = Evaluator::new(
            Arc::clone(&mirror),
            Arc::new(crate::quality::Store::default()),
            Arc::clone(&store),
        )
        .with_run_health(Arc::clone(&runs));
        store
            .upsert(
                "mikko",
                "helsinki",
                Scope::Pipeline,
                "bikes",
                &[Event::Failure],
                Delivery::Portal,
            )
            .await
            .unwrap();
        mirror.upsert(pipeline("bikes", json!({ "phase": "Live" })));
        let broken = |message: &str| crate::pipeline_log::NewLine {
            record_id: String::new(),
            step: None,
            outcome: crate::pipeline_log::Outcome::Failed,
            message: message.to_owned(),
        };
        for run in ["r1", "r2"] {
            runs.append(
                "helsinki",
                "bikes",
                run,
                &[broken("GET https://feed.example/x?key=k1 answered 404")],
            )
            .await
            .unwrap();
        }
        evaluator.run(at("2026-10-07T08:00:00Z")).await.unwrap();
        assert!(
            store.open_incidents().await.unwrap().is_empty(),
            "two failed runs"
        );
        runs.append(
            "helsinki",
            "bikes",
            "r3",
            &[broken("GET https://feed.example/x?key=k1 answered 404")],
        )
        .await
        .unwrap();
        evaluator.run(at("2026-10-07T08:30:00Z")).await.unwrap();
        assert!(
            store.open_incidents().await.unwrap().is_empty(),
            "read again after an hour"
        );
        evaluator.run(at("2026-10-07T09:00:00Z")).await.unwrap();
        let told = store.notices(&["mikko".into()]).await.unwrap();
        assert_eq!(told.len(), 1);
        assert_eq!(
            (told[0].event, told[0].change.as_str()),
            (Event::Failure, "opened")
        );
        assert_eq!(
            told[0].detail,
            "3 runs in a row sent nothing: GET https://feed.example/x answered 404"
        );
    }

    #[test]
    fn a_drop_is_a_zero_signal_and_a_healthy_source_none() {
        let mut health = crate::pipeline_health::health("bikes", &[], None);
        assert_eq!(health_signal("helsinki", &health), None);
        health.alert = true;
        health.last_sent = Some(3);
        health.median_sent = Some(40.0);
        assert_eq!(
            health_signal("helsinki", &health),
            Some(Signal {
                key: ("helsinki".into(), "bikes".into(), Event::Zero),
                detail: "the last run sent 3 records against a 7-day median of 40".into()
            })
        );
        // T-5291: an hour that lost a tenth of its records is a failure, named with the ratio.
        health.errors = Some(crate::pipeline_health::ErrorRatio {
            errors: 10,
            records: 100,
            percent: 10.0,
        });
        health.last_error = Some("step 2 threw: no date".into());
        assert_eq!(
            health_signal("helsinki", &health),
            Some(Signal {
                key: ("helsinki".into(), "bikes".into(), Event::Failure),
                detail: "10 of 100 records in the last hour failed or were refused (10 %): step 2 threw: no date".into()
            })
        );
    }

    #[tokio::test]
    async fn a_digest_names_the_day_s_incidents_once() {
        let mirror = mirror();
        let store = Arc::new(AlertStore::new(None));
        let evaluator = Evaluator::new(
            Arc::clone(&mirror),
            Arc::new(crate::quality::Store::default()),
            Arc::clone(&store),
        );
        store
            .upsert(
                "jana",
                "helsinki",
                Scope::Pipeline,
                "bikes",
                &[Event::Failure],
                Delivery::Digest,
            )
            .await
            .unwrap();
        mirror.upsert(pipeline("bikes", json!({ "phase": "Error" })));
        evaluator.run(at("2026-10-07T03:00:00Z")).await.unwrap();
        assert!(
            store.notices(&["jana".into()]).await.unwrap().is_empty(),
            "nothing before seven, nothing at once"
        );
        evaluator.run(at("2026-10-07T07:30:00Z")).await.unwrap();
        evaluator.run(at("2026-10-07T09:00:00Z")).await.unwrap();
        let told = store.notices(&["jana".into()]).await.unwrap();
        assert_eq!(told.len(), 1);
        assert_eq!(
            (told[0].change.as_str(), told[0].event),
            ("digest", Event::Failure)
        );
    }

    #[tokio::test]
    async fn a_person_keeps_their_newest_notices_and_reads_only_their_own() {
        let store = AlertStore::new(None);
        let sub = store
            .upsert(
                "jana",
                "helsinki",
                Scope::Pipeline,
                "bikes",
                &[Event::Zero],
                Delivery::Portal,
            )
            .await
            .unwrap();
        let key: IncidentKey = ("helsinki".into(), "bikes".into(), Event::Zero);
        for _ in 0..(KEPT + 5) {
            store
                .notify("jana", sub.id, &key, "opened", "")
                .await
                .unwrap();
        }
        let notices = store.notices(&["jana".into()]).await.unwrap();
        assert_eq!(notices.len(), KEPT as usize);
        assert!(!store
            .mark_read(&["eeva".into()], notices[0].id)
            .await
            .unwrap());
        assert!(store
            .mark_read(&["jana".into()], notices[0].id)
            .await
            .unwrap());
        assert!(store.notices(&["jana".into()]).await.unwrap()[0].read);
        assert!(store.remove("jana", sub.id).await.unwrap());
        assert!(
            store.notices(&["jana".into()]).await.unwrap().is_empty(),
            "notices go with their subscription"
        );
    }

    /// F-POR4-017: a run before the mirror loaded judged nothing, so the health log keeps its
    /// rows and their `since`.
    #[tokio::test]
    async fn a_run_over_an_unloaded_mirror_keeps_the_health_log() {
        let log = Arc::new(crate::health_log::HealthStore::new(None));
        let t0 = at("2026-10-21T08:00:00Z");
        let row = crate::health_log::Finding {
            project: "bbsk".into(),
            kind: crate::health_log::Kind::Pipeline,
            name: "air".into(),
            check: "source".into(),
            status: crate::health_log::Status::Error,
            detail: crate::health_log::Text::default(),
            fix: crate::health_log::Text::default(),
            evidence_url: String::new(),
        };
        log.record(t0, vec![row]).await.expect("record");
        let evaluator = Evaluator::new(
            Arc::new(Mirror::new()),
            Arc::new(crate::quality::Store::default()),
            Arc::new(AlertStore::new(None)),
        )
        .with_health_log(
            Arc::clone(&log),
            Arc::new(crate::health_probe::Probes::new(None, None)),
        );
        let later = t0 + crate::health_log::EVERY;
        evaluator.run(later).await.expect("run");
        let kept = log.list(Some("bbsk"), later).await.expect("list");
        assert_eq!(kept.len(), 1, "the unloaded run dropped the row");
    }
}
