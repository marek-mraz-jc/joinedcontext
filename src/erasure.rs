//! The data subject's erasure in a space (API/01 §46, DM-126, DM-127, ADR-N-054 §2.6, T-4668).
//!
//! An erasure is a row of `erasures`, the erasure log: the subject's keyed hash, never the
//! value, the Entities that named the subject, grouped by class with the Attributes that hold
//! their personal data, and one state per store the erasure reaches. The request answers at
//! once; each store is asked then and again every minute until it reports done, so a store that
//! is down keeps the erasure open instead of finishing it early. After [`MAX_ATTEMPTS`] a store
//! is `failed` and the erasure stays open for a person to read why.
//!
//! Four eyes (DM-142, T-5356): an owner's request is a proposal that erases nothing. Another
//! person with `approve` on the ContextSpace approves it with the subject typed again, whose
//! keyed hash must be the proposal's, or either owner declines it; a proposal nobody decided in
//! [`PROPOSAL_DAYS`] days reads `expired`. Only an approved erasure reaches a store.
//!
//! The stores: the broker (current state, every version, dead letters; it keeps its own record
//! and applies it again after a restore), the Portal's own copies (the trash, the comments), the
//! assistant's stored conversations, and the export files and entity caches, which this platform
//! does not keep: the gateway streams `file.csv` and answers every read `private`.

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;

use crate::state::AppState;

/// Every store an erasure reaches, in the order a person reads them.
///
/// The catalogue keeps no copy since T-5060: CKAN holds no rows, its datasets point at the
/// Endpoint. An erasure logged before then still lists its catalogue part; a pending one is
/// done with nothing to erase on the next attempt, since the next publication drops the old
/// table resources and CKAN drops their tables with them.
pub const STORES: [&str; 5] = ["broker", "portal", "conversations", "exports", "caches"];
/// How often a store is asked before it is `failed`: an hour of the minute loop.
pub const MAX_ATTEMPTS: u32 = 60;
/// The longest subject value or reason a request carries.
pub const MAX_SUBJECT: usize = 256;
pub const MAX_REASON: usize = 500;
/// How long a proposal waits for its second owner before it reads `expired`.
pub const PROPOSAL_DAYS: i64 = 7;

/// Where an erasure stands with its four eyes (DM-142).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// One owner asked; nothing is erased.
    Proposed,
    /// A second person approved it: the stores are asked.
    Approved,
    Declined,
}

impl Approval {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Declined => "declined",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "proposed" => Self::Proposed,
            "declined" => Self::Declined,
            _ => Self::Approved,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreState {
    #[default]
    Pending,
    Done,
    Failed,
}

/// One store's part of an erasure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub state: StoreState,
    /// What went: Entities, rows, conversations.
    pub count: u64,
    /// The broker's history rows, the versions that went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<u64>,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Store {
    /// Done, with nothing to do: a store this platform does not keep.
    fn none_kept() -> Self {
        Self {
            state: StoreState::Done,
            ..Self::default()
        }
    }

    /// The store after one more attempt: done with what went, else pending again until
    /// [`MAX_ATTEMPTS`], then failed. The error is the store's own sentence, never a value.
    pub fn after(&self, outcome: Result<(u64, Option<u64>), String>) -> Self {
        let attempts = self.attempts.saturating_add(1);
        match outcome {
            Ok((count, versions)) => Self {
                state: StoreState::Done,
                count,
                versions,
                attempts,
                error: None,
            },
            Err(error) => Self {
                state: if attempts >= MAX_ATTEMPTS {
                    StoreState::Failed
                } else {
                    StoreState::Pending
                },
                attempts,
                error: Some(error.chars().take(300).collect()),
                ..self.clone()
            },
        }
    }
}

/// The Entities of one class that name the subject, and the Attributes to erase of them;
/// `attrs: None` erases the whole Entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub class: String,
    pub entities: Vec<String>,
    pub attrs: Option<Vec<String>>,
}

/// One erasure as the log holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Erasure {
    pub id: String,
    pub project: String,
    pub space: String,
    pub subject: String,
    pub reason: String,
    /// Who proposed it.
    pub actor: String,
    pub requested_at: time::OffsetDateTime,
    pub completed_at: Option<time::OffsetDateTime>,
    pub groups: Vec<Group>,
    pub stores: BTreeMap<String, Store>,
    pub approval: Approval,
    /// Who approved or declined it, and when; `None` while proposed and for the erasures
    /// logged before the four eyes.
    pub decided_by: Option<String>,
    pub decided_at: Option<time::OffsetDateTime>,
    pub decline_reason: Option<String>,
}

impl Erasure {
    /// When an undecided proposal stops being approvable.
    pub fn expires_at(&self) -> time::OffsetDateTime {
        self.requested_at + time::Duration::days(PROPOSAL_DAYS)
    }

    /// `proposed` until a second person decides (`expired` after [`PROPOSAL_DAYS`]), `declined`;
    /// once approved `done` when every store is, `failed` once none is pending and one gave up,
    /// else `open`.
    pub fn state(&self) -> &'static str {
        self.state_at(time::OffsetDateTime::now_utc())
    }

    pub fn state_at(&self, now: time::OffsetDateTime) -> &'static str {
        match self.approval {
            Approval::Declined => return "declined",
            Approval::Proposed if now >= self.expires_at() => return "expired",
            Approval::Proposed => return "proposed",
            Approval::Approved => {}
        }
        let states = || self.stores.values().map(|s| s.state);
        if states().all(|s| s == StoreState::Done) {
            "done"
        } else if states().any(|s| s == StoreState::Pending) {
            "open"
        } else {
            "failed"
        }
    }

    /// Every Entity the erasure names.
    pub fn urns(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|g| g.entities.iter().cloned())
            .collect()
    }

    /// The erasure as API/01 §46 answers it: its state, the counts per store, each store's part.
    pub fn view(&self) -> Value {
        let rfc = |at: time::OffsetDateTime| {
            at.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        };
        let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
        for (name, store) in &self.stores {
            counts.insert(name.as_str(), store.count);
            if let Some(versions) = store.versions {
                counts.insert("versions", versions);
            }
        }
        let mut view = json!({
            "id": self.id,
            "state": self.state(),
            "requestedAt": rfc(self.requested_at),
            "completedAt": self.completed_at.map(rfc),
            "proposedBy": self.actor,
            "entities": self.urns().len(),
            "counts": counts,
            "stores": self.stores,
        });
        match self.approval {
            Approval::Proposed => view["expiresAt"] = json!(rfc(self.expires_at())),
            Approval::Approved => view["approvedBy"] = json!(self.decided_by),
            Approval::Declined => view["declinedBy"] = json!(self.decided_by),
        }
        if let Some(at) = self.decided_at {
            view["decidedAt"] = json!(rfc(at));
        }
        view
    }
}

/// The dry run of an erasure (`?dryRun=All`, API/01 §46): the Entities and Attributes it would
/// take, per class, so the steward confirms before anything is erased.
pub fn preview(groups: &[Group]) -> Value {
    json!({
        "dryRun": true,
        "entities": groups.iter().map(|g| g.entities.len()).sum::<usize>(),
        "groups": groups,
    })
}

/// A fresh proposal: every store pending but the two this platform does not keep, none asked
/// until a second person approves it.
pub fn begin(
    (project, space): (&str, &str),
    subject: String,
    reason: &str,
    actor: &str,
    groups: Vec<Group>,
) -> Erasure {
    let now = time::OffsetDateTime::now_utc();
    let stores = STORES
        .iter()
        .map(|&name| {
            let store = match name {
                "exports" | "caches" => Store::none_kept(),
                _ => Store::default(),
            };
            (name.to_owned(), store)
        })
        .collect();
    Erasure {
        id: new_id(now),
        project: project.to_owned(),
        space: space.to_owned(),
        subject,
        reason: reason.trim().to_owned(),
        actor: actor.to_owned(),
        requested_at: now,
        completed_at: None,
        groups,
        stores,
        approval: Approval::Proposed,
        decided_by: None,
        decided_at: None,
        decline_reason: None,
    }
}

/// `er-20261019-<12 hex>`.
fn new_id(now: time::OffsetDateTime) -> String {
    use sha2::Digest;
    // Six bytes of the CSPRNG the CSRF token is minted with.
    let drawn = Sha256::digest(crate::auth::csrf::new_token().as_bytes());
    let hex: String = drawn[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "er-{:04}{:02}{:02}-{hex}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

/// Whether `id` is shaped as `new_id` mints them.
pub fn is_id(id: &str) -> bool {
    id.len() == 24
        && id.starts_with("er-")
        && id[3..11].bytes().all(|b| b.is_ascii_digit())
        && id.as_bytes()[11] == b'-'
        && id[12..]
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The subject's keyed hash in one space: 32 hex of an HMAC-SHA256 under `key`, so the log and
/// the broker's record name the subject without holding a value anyone can look up.
pub fn subject_hash(key: &str, project: &str, space: &str, subject: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
    mac.update(format!("joinedcontext/erasure-subject/v1/{project}/{space}").as_bytes());
    mac.update(&[0]);
    mac.update(subject.trim().as_bytes());
    mac.finalize().into_bytes()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Why `subject` cannot name a data subject, if it cannot: 1 to [`MAX_SUBJECT`] characters with
/// no control character and no quote or backslash, so it goes into an NGSI-LD `q` as one string.
pub fn subject_refusal(subject: &str) -> Option<String> {
    let trimmed = subject.trim();
    let bad = trimmed.is_empty()
        || trimmed.chars().count() > MAX_SUBJECT
        || trimmed
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\');
    bad.then(|| {
        format!("subject is 1 to {MAX_SUBJECT} characters, with no quote, backslash or control character")
    })
}

/// A class of the space's model that names its data subject: the slot that holds the subject
/// (`dataSubjectRef`) and the Attributes to erase, `None` when the whole class is personal.
/// `hashed` when the subject slot is ingested as `keyedHash` (DM-121): the broker holds the
/// value's keyed hash, so the subject is found only under the space's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectClass {
    pub class: String,
    pub slot: String,
    pub attrs: Option<Vec<String>>,
    pub hashed: bool,
}

/// The classes of a LinkML model that declare `dataSubjectRef` (Architecture/11 §9), with the
/// slots marked `personalData` on the slot, in the class's `slot_usage` or its `attributes`,
/// and the subject slot itself. A class marked `personalData` as a whole goes whole.
pub fn subject_classes(linkml: &str) -> Vec<SubjectClass> {
    use serde_norway::Value as Yaml;
    let Ok(model) = serde_norway::from_str::<Yaml>(linkml) else {
        return Vec::new();
    };
    // `annotations: {name: value}` or `{name: {tag: name, value: value}}`.
    let annotation = |node: &Yaml, name: &str| -> Option<Yaml> {
        let found = node.get("annotations")?.get(name)?;
        Some(found.get("value").cloned().unwrap_or_else(|| found.clone()))
    };
    let truthy = |value: Option<Yaml>| match value {
        Some(Yaml::Bool(b)) => b,
        Some(Yaml::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    let Some(classes) = model.get("classes").and_then(Yaml::as_mapping) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, class) in classes {
        let (Some(name), Some(slot)) = (
            name.as_str(),
            annotation(class, "dataSubjectRef").and_then(|s| s.as_str().map(str::to_owned)),
        ) else {
            continue;
        };
        let attrs = if truthy(annotation(class, "personalData")) {
            None
        } else {
            let listed = class
                .get("slots")
                .and_then(Yaml::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(Yaml::as_str)
                .map(str::to_owned);
            let own = class
                .get("attributes")
                .and_then(Yaml::as_mapping)
                .into_iter()
                .flatten()
                .filter_map(|(k, _)| k.as_str().map(str::to_owned));
            let mut attrs: Vec<String> = listed
                .chain(own)
                .filter(|s| {
                    let marked = |node: Option<&Yaml>| {
                        node.is_some_and(|n| truthy(annotation(n, "personalData")))
                    };
                    *s == slot
                        || marked(model.get("slots").and_then(|all| all.get(s.as_str())))
                        || marked(class.get("slot_usage").and_then(|u| u.get(s.as_str())))
                        || marked(class.get("attributes").and_then(|a| a.get(s.as_str())))
                })
                .collect();
            if !attrs.contains(&slot) {
                attrs.push(slot.clone());
            }
            attrs.sort();
            attrs.dedup();
            Some(attrs)
        };
        // The subject slot's `ingest`, the class's own word on it first.
        let hashed = [
            class.get("slot_usage").and_then(|u| u.get(slot.as_str())),
            class.get("attributes").and_then(|a| a.get(slot.as_str())),
            model.get("slots").and_then(|all| all.get(slot.as_str())),
        ]
        .into_iter()
        .flatten()
        .find_map(|node| annotation(node, "ingest"))
        .is_some_and(|ingest| ingest.as_str() == Some("keyedHash"));
        out.push(SubjectClass {
            class: name.to_owned(),
            slot,
            attrs,
            hashed,
        });
    }
    out
}

/// Why a subject cannot be searched for when a class holds it as a keyed hash and the Portal
/// holds no key for the space: the broker holds the HMAC under the space's key, so asking for
/// the raw value would find nothing and end `done` with no Entities, a silent miss.
pub fn hashed_refusal(classes: &[SubjectClass]) -> Option<String> {
    let class = classes.iter().find(|c| c.hashed)?;
    Some(format!(
        "the subject slot '{}' of '{}' is ingested as keyedHash and this Portal holds no key for the space: set spec.personalData.keyRef to a secret of at least 32 bytes, or check the Portal's log for why it did not resolve; nothing was read or erased",
        class.slot, class.class
    ))
}

/// The `q` of each class, in order: a class holding the subject as a keyed hash is searched for
/// the subject's hash under the space's key, the way the gateway stored it (DM-121). Without the
/// key the raw value would find nothing, so the lookup is refused with [`hashed_refusal`].
pub fn subject_queries(
    classes: &[SubjectClass],
    subject: &str,
    key: Option<&jc_core::Secret>,
) -> Result<Vec<String>, String> {
    if key.is_none() {
        if let Some(why) = hashed_refusal(classes) {
            return Err(why);
        }
    }
    Ok(classes
        .iter()
        .map(|class| match key.filter(|_| class.hashed) {
            Some(key) => subject_query(
                &class.slot,
                &crate::personal_keys::hash(key, subject.trim()),
            ),
            None => subject_query(&class.slot, subject),
        })
        .collect())
}

/// The NGSI-LD `q` that finds the Entities naming `subject` in `slot`.
pub fn subject_query(slot: &str, subject: &str) -> String {
    format!("{slot}==\"{}\"", subject.trim())
}

// --- the log ------------------------------------------------------------------------------

/// The columns every read of the log selects, in [`Row`]'s order.
macro_rules! columns {
    () => {
        "id, project, space, subject, reason, actor, requested_at, completed_at, groups, stores, \
         approval, decided_by, decided_at, decline_reason"
    };
}

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    time::OffsetDateTime,
    Option<time::OffsetDateTime>,
    Value,
    Value,
    String,
    Option<String>,
    Option<time::OffsetDateTime>,
    Option<String>,
);

fn from_row(row: Row) -> Erasure {
    let (
        id,
        project,
        space,
        subject,
        reason,
        actor,
        requested_at,
        completed_at,
        groups,
        stores,
        approval,
        decided_by,
        decided_at,
        decline_reason,
    ) = row;
    Erasure {
        id,
        project,
        space,
        subject,
        reason,
        actor,
        requested_at,
        completed_at,
        groups: serde_json::from_value(groups).unwrap_or_default(),
        stores: serde_json::from_value(stores).unwrap_or_default(),
        approval: Approval::parse(&approval),
        decided_by,
        decided_at,
        decline_reason,
    }
}

pub async fn insert(pool: &PgPool, erasure: &Erasure) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO erasures (id, project, space, subject, reason, actor, requested_at, groups, stores, approval) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(&erasure.id)
    .bind(&erasure.project)
    .bind(&erasure.space)
    .bind(&erasure.subject)
    .bind(&erasure.reason)
    .bind(&erasure.actor)
    .bind(erasure.requested_at)
    .bind(json!(erasure.groups))
    .bind(json!(erasure.stores))
    .bind(erasure.approval.as_str())
    .execute(pool)
    .await
    .map(|_| ())
}

/// One erasure of the space, or `None`.
pub async fn get(
    pool: &PgPool,
    project: &str,
    space: &str,
    id: &str,
) -> Result<Option<Erasure>, sqlx::Error> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM erasures WHERE id = $1 AND project = $2 AND space = $3"
    ))
    .bind(id)
    .bind(project)
    .bind(space)
    .fetch_optional(pool)
    .await
    .map(|row| row.map(from_row))
}

/// The space's newest erasures, proposals included, for its owners' list.
pub async fn list(pool: &PgPool, project: &str, space: &str) -> Result<Vec<Erasure>, sqlx::Error> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM erasures WHERE project = $1 AND space = $2 ORDER BY requested_at DESC LIMIT 50"
    ))
    .bind(project)
    .bind(space)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(from_row).collect())
}

/// Approves a proposal as `approver` with the Entities found now, in one statement, so two
/// approvals never both run it: `None` when it is no longer an approvable proposal of someone
/// else (decided, expired, or the approver's own).
pub async fn approve(
    pool: &PgPool,
    (project, space, id): (&str, &str, &str),
    approver: &str,
    groups: &[Group],
) -> Result<Option<Erasure>, sqlx::Error> {
    sqlx::query_as::<_, Row>(concat!(
        "UPDATE erasures SET approval = 'approved', decided_by = $4, decided_at = now(), groups = $5 \
         WHERE id = $1 AND project = $2 AND space = $3 AND approval = 'proposed' AND actor <> $4 \
           AND requested_at > now() - make_interval(days => $6) RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(project)
    .bind(space)
    .bind(approver)
    .bind(json!(groups))
    .bind(i32::try_from(PROPOSAL_DAYS).unwrap_or(i32::MAX))
    .fetch_optional(pool)
    .await
    .map(|row| row.map(from_row))
}

/// Declines a proposal still undecided and not expired; `None` otherwise.
pub async fn decline(
    pool: &PgPool,
    (project, space, id): (&str, &str, &str),
    decider: &str,
    reason: &str,
) -> Result<Option<Erasure>, sqlx::Error> {
    sqlx::query_as::<_, Row>(concat!(
        "UPDATE erasures SET approval = 'declined', decided_by = $4, decided_at = now(), decline_reason = $5 \
         WHERE id = $1 AND project = $2 AND space = $3 AND approval = 'proposed' \
           AND requested_at > now() - make_interval(days => $6) RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(project)
    .bind(space)
    .bind(decider)
    .bind(reason.trim())
    .bind(i32::try_from(PROPOSAL_DAYS).unwrap_or(i32::MAX))
    .fetch_optional(pool)
    .await
    .map(|row| row.map(from_row))
}

/// The approved erasures with a store still pending, oldest first.
pub async fn open(pool: &PgPool) -> Result<Vec<Erasure>, sqlx::Error> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM erasures WHERE completed_at IS NULL AND approval = 'approved' AND EXISTS \
         (SELECT 1 FROM jsonb_each(stores) s WHERE s.value->>'state' = 'pending') \
         ORDER BY requested_at LIMIT 100"
    ))
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(from_row).collect())
}

/// Writes one store's part, and the completion once every store is done. One store per
/// statement, so two attempts of one erasure never overwrite each other.
pub async fn save(pool: &PgPool, id: &str, name: &str, store: &Store) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE erasures SET stores = jsonb_set(stores, ARRAY[$2], $3), \
           completed_at = CASE WHEN NOT EXISTS (SELECT 1 FROM jsonb_each(jsonb_set(stores, ARRAY[$2], $3)) s \
                                                WHERE s.value->>'state' <> 'done') \
                               THEN coalesce(completed_at, now()) ELSE completed_at END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(json!(store))
    .execute(pool)
    .await
    .map(|_| ())
}

// --- the stores ----------------------------------------------------------------------------

/// Asks every pending store once and saves each outcome. `words` is the
/// subject's own value, which the log never keeps: given on the first attempt only, so the
/// stored conversations are searched for it then and for the Entities' URNs on a retry.
pub async fn attempt(state: &AppState, pool: &PgPool, erasure: &Erasure, words: Option<&str>) {
    // Only a second person's approval lets an erasure reach a store (DM-142).
    if erasure.approval != Approval::Approved {
        return;
    }
    for (name, store) in &erasure.stores {
        if store.state != StoreState::Pending {
            continue;
        }
        let outcome = match name.as_str() {
            "broker" => broker(state, erasure).await,
            "portal" => portal(pool, erasure).await.map(|n| (n, None)),
            "conversations" => conversations(state, erasure, words)
                .await
                .map(|n| (n, None)),
            _ => Ok((0, None)),
        };
        if let Err(reason) = &outcome {
            tracing::warn!(erasure = %erasure.id, store = %name, %reason, "an erasure's store did not answer; asked again in a minute");
        }
        if let Err(err) = save(pool, &erasure.id, name, &store.after(outcome)).await {
            tracing::warn!(erasure = %erasure.id, error = %err, "an erasure's outcome was not saved");
        }
    }
}

async fn broker(state: &AppState, erasure: &Erasure) -> Result<(u64, Option<u64>), String> {
    let views = state
        .views
        .as_ref()
        .ok_or("this Portal has no broker address or admin token")?;
    let tenant = crate::spaces::segment(&state.mirror, &erasure.project, &erasure.space);
    let (mut entities, mut history) = (0, 0);
    for group in erasure.groups.iter().filter(|g| !g.entities.is_empty()) {
        let counts = views
            .erase(
                &tenant,
                &group.entities,
                group.attrs.as_deref(),
                (&erasure.subject, &erasure.actor, &erasure.reason),
            )
            .await?;
        let n = |k: &str| counts.get(k).and_then(Value::as_u64).unwrap_or(0);
        entities += n("entities");
        history += n("history");
    }
    Ok((entities, Some(history)))
}

/// The Portal's own copies of the Entities: the trash and the comments.
async fn portal(pool: &PgPool, erasure: &Erasure) -> Result<u64, String> {
    let urns = erasure.urns();
    let mut gone = 0;
    for sql in [
        "DELETE FROM entity_trash WHERE project = $1 AND space = $2 AND urn = ANY($3)",
        "DELETE FROM entity_comments WHERE project = $1 AND space = $2 AND urn = ANY($3)",
    ] {
        gone += sqlx::query(sql)
            .bind(&erasure.project)
            .bind(&erasure.space)
            .bind(&urns)
            .execute(pool)
            .await
            .map_err(|err| format!("the Portal's copies were not reached: {err}"))?
            .rows_affected();
    }
    Ok(gone)
}

/// The assistant's stored conversations that mention an Entity or the subject's words (5
/// characters or more: a shorter word would match unrelated conversations).
async fn conversations(
    state: &AppState,
    erasure: &Erasure,
    words: Option<&str>,
) -> Result<u64, String> {
    let mut terms = erasure.urns();
    terms.extend(
        words
            .map(|w| w.trim().to_owned())
            .filter(|w| w.chars().count() >= 5),
    );
    if terms.is_empty() {
        return Ok(0);
    }
    if state.config.knowledge_url.is_none() {
        // A Portal without the assistant has no conversations to erase.
        return Ok(0);
    }
    let mut gone = 0;
    for batch in terms.chunks(1000) {
        let (status, body) = crate::api::knowledge::ask(
            state,
            reqwest::Method::POST,
            &format!("{}/knowledge/erasures", erasure.project),
            Some(json!({ "terms": batch })),
        )
        .await
        .map_err(|err| err.to_string())?;
        match status.as_u16() {
            200 => gone += body["conversations"].as_u64().unwrap_or(0),
            // A project the assistant never heard of holds no conversation.
            404 => {}
            other => return Err(format!("the assistant answered {other}")),
        }
    }
    Ok(gone)
}

/// Whether the broker's `records` hold the erasure of `group` for `subject`: one record of the
/// subject that names every Entity of the group.
pub fn recorded(records: &[Value], subject: &str, group: &Group) -> bool {
    records.iter().any(|record| {
        record["subject"].as_str() == Some(subject)
            && group.entities.iter().all(|id| {
                record["entities"]
                    .as_array()
                    .is_some_and(|named| named.iter().any(|n| n.as_str() == Some(id)))
            })
    })
}

/// The erasure log applied again to the brokers (DM-127): every erasure the broker finished whose
/// record the broker no longer holds, after its database was restored from a backup older than
/// the erasure, is sent again. Returns how many of how many logged erasures were sent.
///
/// ponytail: a resent erasure also takes what the space wrote about the Entities after the
/// first one; the broker's own re-apply keeps that, a resend with its `before` time when it matters.
pub(crate) async fn reapply(state: &AppState, pool: &PgPool) -> Result<(usize, usize), String> {
    let views = state
        .views
        .as_ref()
        .ok_or("this Portal has no broker address or admin token")?;
    let done: Vec<Erasure> = sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM erasures WHERE approval = 'approved' AND stores->'broker'->>'state' = 'done' \
         ORDER BY requested_at"
    ))
    .fetch_all(pool)
    .await
    .map_err(|err| err.to_string())?
    .into_iter()
    .map(from_row)
    .collect();
    let mut held: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut resent = 0;
    for erasure in &done {
        let tenant = crate::spaces::segment(&state.mirror, &erasure.project, &erasure.space);
        if !held.contains_key(&tenant) {
            held.insert(tenant.clone(), views.erasure_records(&tenant).await?);
        }
        let records = held.get(&tenant).map(Vec::as_slice).unwrap_or_default();
        let missing: Vec<&Group> = erasure
            .groups
            .iter()
            .filter(|g| !g.entities.is_empty() && !recorded(records, &erasure.subject, g))
            .collect();
        for group in &missing {
            views
                .erase(
                    &tenant,
                    &group.entities,
                    group.attrs.as_deref(),
                    (&erasure.subject, &erasure.actor, &erasure.reason),
                )
                .await?;
        }
        resent += usize::from(!missing.is_empty());
    }
    Ok((resent, done.len()))
}

/// Asks the pending stores of every open erasure once a minute, and applies the erasure log
/// again to the brokers when the Portal starts and every hour after (DM-127).
pub(crate) fn spawn_periodic(state: AppState) {
    let Some(pool) = state.db.clone() else {
        return;
    };
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            if tick.is_multiple_of(60) {
                match reapply(&state, &pool).await {
                    Ok((resent, logged)) => {
                        tracing::info!(resent, logged, "erasure log re-applied")
                    }
                    Err(reason) => {
                        tracing::warn!(%reason, "the erasure log was not re-applied; again in an hour")
                    }
                }
            }
            tick = tick.wrapping_add(1);
            match open(&pool).await {
                Ok(open) => {
                    for erasure in &open {
                        attempt(&state, &pool, erasure, None).await;
                    }
                }
                Err(err) => tracing::warn!(error = %err, "the open erasures could not be read"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_is_done_with_its_count_or_asked_again_until_it_fails() {
        let fresh = Store::default();
        let done = fresh.after(Ok((3, Some(12))));
        assert_eq!(
            (done.state, done.count, done.versions, done.attempts),
            (StoreState::Done, 3, Some(12), 1)
        );
        let mut store = fresh;
        for _ in 1..MAX_ATTEMPTS {
            store = store.after(Err("the broker did not answer".into()));
            assert_eq!(store.state, StoreState::Pending);
        }
        store = store.after(Err("x".repeat(1000)));
        assert_eq!(
            (store.state, store.attempts),
            (StoreState::Failed, MAX_ATTEMPTS)
        );
        assert_eq!(store.error.as_deref().map(str::len), Some(300));
    }

    #[test]
    fn an_erasure_is_open_until_every_store_is_done_and_failed_once_one_gave_up() {
        let mut erasure = begin(
            ("hel", "bikes"),
            "0".repeat(32),
            " Art. 17 ",
            "f:1:anna",
            vec![],
        );
        assert!(is_id(&erasure.id), "{}", erasure.id);
        assert_eq!(erasure.reason, "Art. 17");
        assert_eq!(erasure.stores["exports"].state, StoreState::Done);
        // T-5060: the catalogue holds no rows, so a new erasure has no catalogue part.
        assert_eq!(erasure.stores.len(), STORES.len());
        assert!(!erasure.stores.keys().any(|k| k.starts_with("ckan")));
        assert_eq!(
            erasure.state(),
            "proposed",
            "nothing runs on one owner's call"
        );
        erasure.approval = Approval::Approved;
        assert_eq!(erasure.state(), "open");
        for store in erasure.stores.values_mut() {
            *store = store.after(Ok((1, None)));
        }
        assert_eq!(erasure.state(), "done");
        erasure.stores.insert(
            "broker".into(),
            Store {
                state: StoreState::Failed,
                ..Store::default()
            },
        );
        assert_eq!(erasure.state(), "failed");
        erasure.stores.insert("portal".into(), Store::default());
        assert_eq!(erasure.state(), "open");
        let view = erasure.view();
        assert_eq!(view["counts"]["broker"], 0);
        assert!(
            view.to_string().find("Art. 17").is_none(),
            "the view carries no reason"
        );
    }

    #[test]
    fn a_broker_record_covers_a_group_only_for_the_same_subject_and_every_entity() {
        let group = Group {
            class: "BikeTrip".into(),
            entities: vec!["a".into(), "b".into()],
            attrs: None,
        };
        let record =
            |subject: &str, entities: Value| json!({"subject": subject, "entities": entities});
        assert!(recorded(
            &[record("s1", json!(["b", "a", "c"]))],
            "s1",
            &group
        ));
        assert!(!recorded(&[record("s1", json!(["a"]))], "s1", &group));
        assert!(!recorded(&[record("s2", json!(["a", "b"]))], "s1", &group));
        assert!(!recorded(&[], "s1", &group));
    }

    #[test]
    fn a_proposal_waits_for_a_second_person_and_expires_after_seven_days() {
        let mut erasure = begin(
            ("hel", "bikes"),
            "0".repeat(32),
            "Art. 17",
            "user:anna",
            vec![],
        );
        let at = erasure.requested_at;
        assert_eq!(erasure.state_at(at), "proposed");
        assert_eq!(erasure.state_at(at + time::Duration::days(6)), "proposed");
        assert_eq!(erasure.state_at(at + time::Duration::days(7)), "expired");
        let view = erasure.view();
        assert_eq!(view["proposedBy"], "user:anna");
        assert!(view["expiresAt"].is_string(), "{view}");
        assert!(view.get("approvedBy").is_none(), "{view}");
        erasure.approval = Approval::Declined;
        erasure.decided_by = Some("user:boris".into());
        erasure.decline_reason = Some("the person withdrew the request".into());
        assert_eq!(erasure.state_at(at + time::Duration::days(30)), "declined");
        let view = erasure.view();
        assert_eq!(view["declinedBy"], "user:boris");
        assert!(
            !view.to_string().contains("withdrew"),
            "the view carries no reason: {view}"
        );
    }

    #[test]
    fn ids_are_shaped_and_nothing_else_passes() {
        assert!(is_id("er-20261019-0a1b2c3d4e5f"));
        for bad in [
            "er-20261019-0A1B2C3D4E5F",
            "er-2026101-0a1b2c3d4e5f0",
            "../../etc/passwd",
            "",
        ] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_subject_hash_is_keyed_per_space_and_never_the_value() {
        let one = subject_hash("k1", "hel", "bikes", "Ján Nováček");
        assert_eq!(one.len(), 32);
        assert!(one.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(one, subject_hash("k1", "hel", "bikes", " Ján Nováček "));
        assert_ne!(one, subject_hash("k2", "hel", "bikes", "Ján Nováček"));
        assert_ne!(one, subject_hash("k1", "hel", "trips", "Ján Nováček"));
    }

    #[test]
    fn a_subject_goes_into_q_as_one_string_or_is_refused() {
        assert_eq!(subject_query("rider", " r-42 "), "rider==\"r-42\"");
        assert!(subject_refusal("r-42").is_none());
        assert!(subject_refusal("Ján Nováček").is_none());
        for bad in [
            "",
            "   ",
            "a\"|rider!=\"x",
            "a\\b",
            "a\nb",
            &"x".repeat(257),
        ] {
            assert!(subject_refusal(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn the_model_names_the_subject_slot_and_the_personal_attributes() {
        let linkml = r#"
id: https://example.org/bikes
slots:
  rider:
    range: string
  riderName:
    annotations:
      personalData: true
  station:
    range: string
classes:
  BikeTrip:
    annotations:
      dataSubjectRef: rider
    slots: [rider, riderName, station]
    slot_usage:
      station: {}
    attributes:
      phone:
        annotations:
          personalData:
            tag: personalData
            value: "true"
  Account:
    annotations:
      dataSubjectRef:
        tag: dataSubjectRef
        value: owner
      personalData: true
    slots: [owner]
  Station:
    slots: [station]
"#;
        assert_eq!(
            subject_classes(linkml),
            [
                SubjectClass {
                    class: "BikeTrip".into(),
                    slot: "rider".into(),
                    attrs: Some(vec!["phone".into(), "rider".into(), "riderName".into()]),
                    hashed: false,
                },
                SubjectClass {
                    class: "Account".into(),
                    slot: "owner".into(),
                    attrs: None,
                    hashed: false,
                },
            ]
        );
        assert!(subject_classes("not: [yaml").is_empty());
    }

    #[test]
    fn the_dry_run_counts_what_each_class_would_lose() {
        let groups = [
            Group {
                class: "BikeTrip".into(),
                entities: vec![
                    "urn:ngsi-ld:BikeTrip:1".into(),
                    "urn:ngsi-ld:BikeTrip:2".into(),
                ],
                attrs: Some(vec!["rider".into()]),
            },
            Group {
                class: "Account".into(),
                entities: vec![],
                attrs: None,
            },
        ];
        let view = preview(&groups);
        assert_eq!(view["dryRun"], true);
        assert_eq!(view["entities"], 2);
        assert_eq!(view["groups"][0]["attrs"], json!(["rider"]));
        assert_eq!(view["groups"][1]["attrs"], Value::Null);
        assert_eq!(preview(&[])["entities"], 0);
    }

    #[test]
    fn a_subject_slot_ingested_as_keyed_hash_is_marked_hashed_where_the_class_says_so() {
        let linkml = r#"
slots:
  rider:
    annotations: { personalData: true, ingest: keyedHash }
  owner:
    annotations: { ingest: keyedHash }
classes:
  BikeTrip:
    annotations: { dataSubjectRef: rider }
    slots: [rider]
  Account:
    annotations: { dataSubjectRef: owner }
    slots: [owner]
    slot_usage:
      owner:
        annotations: { ingest: { tag: ingest, value: keep } }
  Card:
    annotations: { dataSubjectRef: holder }
    attributes:
      holder:
        annotations: { ingest: keyedHash }
"#;
        let hashed: Vec<(String, bool)> = subject_classes(linkml)
            .into_iter()
            .map(|c| (c.class, c.hashed))
            .collect();
        assert_eq!(
            hashed,
            [
                ("BikeTrip".to_owned(), true),
                ("Account".to_owned(), false),
                ("Card".to_owned(), true),
            ]
        );
        let classes = subject_classes(linkml);
        let why = hashed_refusal(&classes).unwrap_or_default();
        assert!(
            why.contains("'rider' of 'BikeTrip'") && why.contains("keyedHash"),
            "{why}"
        );
        assert_eq!(hashed_refusal(&classes[1..2]), None);

        // T-5336: with the space's key the hashed class is searched for the gateway's HMAC of
        // the subject (`openssl dgst -sha256 -hmac`), the plain class for the subject itself.
        let key = jc_core::Secret::new("0123456789abcdef0123456789abcdef");
        let plain = SubjectClass {
            hashed: false,
            ..classes[0].clone()
        };
        let hashed = SubjectClass {
            hashed: true,
            ..classes[0].clone()
        };
        let both = [hashed, plain];
        let queries = subject_queries(&both, " r-42 ", Some(&key)).expect("the key is held");
        let slot = &both[0].slot;
        assert_eq!(
            queries,
            [
                format!(
                    "{slot}==\"606a62f1ce588d825f88912faeb84707ca3a1f7acdecffad13b0bd1d7ac56396\""
                ),
                format!("{slot}==\"r-42\""),
            ]
        );
        let refused = subject_queries(&both, "r-42", None).expect_err("no key");
        assert!(refused.contains("spec.personalData.keyRef"), "{refused}");
        assert_eq!(
            subject_queries(&both[1..], "r-42", None),
            Ok(vec![format!("{slot}==\"r-42\"")])
        );
        assert!(subject_classes("classes: {}").is_empty());
    }
}
