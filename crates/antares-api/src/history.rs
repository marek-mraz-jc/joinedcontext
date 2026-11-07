// SPDX-License-Identifier: EUPL-1.2
//! The temporal seam's producer side (ADR-0013): the write path pushes
//! `TemporalEvent`s into a per-request buffer; the buffer is drained ONCE
//! per request — after the handler, before the response leaves — so the
//! driver sees the whole request in one `event_list` call and a client
//! reading its own history right after the write always finds it.
//!
//! Outside a request (background jobs, tests without the router) there is
//! no buffer, and a push drains immediately: the seam degrades to today's
//! per-change recording rather than losing events. A driver error in the
//! drain is logged and counted (`/q/health` temporalDrainErrors) — it never
//! changes the response of a write that already committed.

#[cfg(not(target_arch = "wasm32"))]
use crate::provenance::PROVENANCE;
use crate::state::{AppState, RecordMode};
use antares_model::TenantId;
use antares_store::{TemporalDriverExt as _, TemporalEvent};
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static DRAIN_ERRORS: AtomicU64 = AtomicU64::new(0);

/// Drains that failed in the driver (the events of that request are lost;
/// the write itself stood).
pub fn drain_errors() -> u64 {
    DRAIN_ERRORS.load(Ordering::Relaxed)
}

// The buffer rides the request's task; tokio task-locals need the `rt`
// feature the single-threaded wasm build does not carry, so wasm records
// immediately.
#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    static BUFFER: std::cell::RefCell<Vec<TemporalEvent>>;
    static STAGED: std::cell::RefCell<Vec<TemporalEvent>>;
    static CHANGES: std::cell::RefCell<Vec<crate::mirror::Change>>;
    static TENANT: Option<TenantId>;
}

/// `ANTARES_HISTORY_BACKFILL=on` (T-4665, ADR-N-054 2.1): a Core API write of
/// an instance older than the one the Entity holds goes to history only.
/// Off (the default) keeps NGSI-LD's replace, which the ETSI suites assume.
fn backfill_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ANTARES_HISTORY_BACKFILL").as_deref() == Ok("on"))
}

/// Whether `incoming`, about to replace `current` of `attr` on the Entity
/// `doc` (expanded, as stored), is a backfill: both observed and `incoming`
/// the older. A backfill is recorded into the request's history here, and
/// the caller keeps `current`. Outside a request, or with backfill off,
/// nothing is one.
pub(crate) fn backfilled(doc: &Value, attr: &str, current: &Value, incoming: &Value) -> bool {
    if !backfill_on() {
        return false;
    }
    let at = |i: &Value| {
        i.get("observedAt").and_then(Value::as_str).and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&antares_store::filter::canonical_datetime(s)).ok()
        })
    };
    let (Some(was), Some(now)) = (at(current), at(incoming)) else {
        return false;
    };
    let Some(id) = doc.get("id").and_then(Value::as_str) else {
        return false;
    };
    if now >= was {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(Some(tenant)) = TENANT.try_with(Clone::clone) else {
            return false;
        };
        let mut instance = incoming.clone();
        if let Some(o) = instance.as_object_mut() {
            let iid = crate::notify::instance_id(id, attr, o);
            o.entry("instanceId".to_owned())
                .or_insert(Value::String(iid));
            o.insert(
                antares_store::PROVENANCE_KEY.to_owned(),
                provenance("backfill"),
            );
        }
        let mut shell = Map::new();
        for k in ["id", "type", "createdAt", "modifiedAt", "scope"] {
            if let Some(v) = doc.get(k) {
                shell.insert(k.to_owned(), v.clone());
            }
        }
        let event = TemporalEvent {
            op: antares_store::TemporalOp::AttrModified,
            tenant,
            entity_id: id.to_owned(),
            shell: Value::Object(shell),
            attr: attr.to_owned(),
            instance,
        };
        stage(event)
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// Holds a backfill's event until its write is known to commit: the
/// closure that found it may still be refused or rerun (F-ANT2-016), and
/// [`settle`] buffers it only then. `false` outside a request.
#[cfg(not(target_arch = "wasm32"))]
fn stage(event: TemporalEvent) -> bool {
    STAGED.try_with(|b| b.borrow_mut().push(event)).is_ok()
}

/// A read-modify-write of Entity `id` (re)starts: the backfills an earlier
/// run of its closure staged are void (F-ANT2-016). Called first thing in
/// every closure that can reach [`backfilled`].
pub(crate) fn restage(id: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = STAGED.try_with(|b| b.borrow_mut().retain(|e| e.entity_id != id));
    #[cfg(target_arch = "wasm32")]
    let _ = id;
}

/// The read-modify-write of Entity `id` returned: its staged backfills go
/// to the request's history when the write committed, and are dropped when
/// it was refused or found nothing.
pub(crate) fn settle(id: &str, committed: bool) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mine = STAGED
            .try_with(|b| {
                let mut b = b.borrow_mut();
                let (mine, rest) = std::mem::take(&mut *b)
                    .into_iter()
                    .partition::<Vec<_>, _>(|e| e.entity_id == id);
                *b = rest;
                mine
            })
            .unwrap_or_default();
        if committed && !mine.is_empty() {
            let _ = BUFFER.try_with(|b| b.borrow_mut().extend(mine));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (id, committed);
}

/// [`settle`] for each id of a batch read-modify-write, by its slot.
pub(crate) fn settle_all<T, E>(ids: &[String], res: &[Option<Result<T, E>>]) {
    for (id, r) in ids.iter().zip(res) {
        settle(id, matches!(r, Some(Ok(_))));
    }
}

/// The provenance of the request in flight with its `operation` and
/// `writtenAt`, when the broker stored it (T-4665, T-5361); outside a
/// request, a write of the broker's own: `api`.
pub(crate) fn provenance(operation: &str) -> Value {
    #[cfg(not(target_arch = "wasm32"))]
    let request = PROVENANCE.try_with(Clone::clone).ok();
    #[cfg(target_arch = "wasm32")]
    let request: Option<Value> = None;
    let base = request.unwrap_or_else(|| serde_json::json!({"channel": "api"}));
    let mut out = crate::provenance::with_operation(&base, operation);
    if let Some(o) = out.as_object_mut() {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        o.insert("writtenAt".into(), Value::String(now));
    }
    out
}

/// Whether the request in flight came through an Endpoint that audits its
/// changes (ADR-N-054 2.8, DM-136): each change is then kept as a version
/// whatever the type's history rule says.
fn audited() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return PROVENANCE
        .try_with(crate::provenance::audited)
        .unwrap_or(false);
    #[cfg(target_arch = "wasm32")]
    false
}

/// The temporal `doc` as a read without `versions=all` sees it (platform
/// ADR-N-054 2.9, DM-146): an Entity whose type keeps no history answers no
/// Attribute, though the version rows an audited write kept are there, so a
/// chart and a public Endpoint see nothing a version request added.
pub(crate) fn as_recorded<'a>(
    st: &AppState,
    tenant: &TenantId,
    doc: &'a Value,
    versions: bool,
) -> std::borrow::Cow<'a, Value> {
    if versions || records_history(st, tenant, doc) {
        return std::borrow::Cow::Borrowed(doc);
    }
    let shell: Map<String, Value> = ["id", "type", "scope"]
        .into_iter()
        .filter_map(|k| Some((k.to_owned(), doc.get(k)?.clone())))
        .collect();
    std::borrow::Cow::Owned(Value::Object(shell))
}

/// Versions on request (platform ADR-N-054 2.9, DM-145): on an audited change
/// of `attr` whose history holds no row of the changed instance's datasetId
/// yet, the value it replaces (in `before`) goes to history first, so the
/// first change already reads as the old value and the new. Its provenance is
/// what the current state knew of its writer (`modifiedBy`, DM-138), with
/// `operation: create`, never the request's: the broker names no writer of
/// the old value it did not record. A store that cannot answer whether a row
/// exists keeps nothing extra, and the write stands.
pub(crate) async fn keep_replaced(
    st: &AppState,
    tenant: &TenantId,
    shell: &Value,
    attr: &str,
    before: Option<&Value>,
    instance: &Value,
) {
    if !audited() {
        return;
    }
    let ds = |i: &Value| i.get("datasetId").cloned();
    let same = |i: &&Value| ds(i) == ds(instance);
    let was = match before {
        Some(Value::Array(all)) => all.iter().find(same),
        Some(one) => Some(one).filter(same),
        None => None,
    };
    let (Some(was), Some(entity)) = (was, shell.get("id").and_then(Value::as_str)) else {
        return;
    };
    let attrs = [attr.to_owned()];
    let held = antares_store::filter::TemporalFilter {
        attrs: Some(&attrs),
        last_n: Some(1),
        ..Default::default()
    };
    let kept = match st.temporal.get_temporal(tenant, entity, &held).await {
        Ok(doc) => doc
            .as_ref()
            .and_then(|d| d.get(attr))
            .is_some_and(|a| match a {
                Value::Array(all) => all.iter().any(|i| same(&i)),
                one => same(&one),
            }),
        Err(e) => {
            tracing::warn!("versions on request: the replaced value is not kept: {e}");
            true
        }
    };
    if kept {
        return;
    }
    let mut old = was.clone();
    let Some(o) = old.as_object_mut() else {
        return;
    };
    let mut p = o
        .remove(antares_jsonld::compact::MODIFIED_BY_KEY)
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    if let Some(po) = p.as_object_mut() {
        po.insert("operation".into(), Value::String("create".into()));
        po.insert(crate::provenance::AUDIT.into(), Value::Bool(true));
        if let Some(at) = o.get("modifiedAt").or_else(|| o.get("createdAt")) {
            po.insert("writtenAt".into(), at.clone());
        }
    }
    let iid = crate::notify::instance_id(entity, attr, o);
    o.entry("instanceId".to_owned())
        .or_insert(Value::String(iid));
    o.insert(antares_store::PROVENANCE_KEY.to_owned(), p);
    let event = TemporalEvent {
        op: antares_store::TemporalOp::AttrModified,
        tenant: tenant.clone(),
        entity_id: entity.to_owned(),
        shell: shell.clone(),
        attr: attr.to_owned(),
        instance: old,
    };
    push(st, event).await;
}

/// Buffer one entity change for the request in flight so the matcher
/// receives the whole request at once. Handed back when no request is in
/// flight — the caller then gives it to the matcher on the spot.
pub(crate) fn buffer_change(change: crate::mirror::Change) -> Option<crate::mirror::Change> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut slot = Some(change);
        let _ = CHANGES.try_with(|b| {
            if let Some(c) = slot.take() {
                b.borrow_mut().push(c);
            }
        });
        slot
    }
    #[cfg(target_arch = "wasm32")]
    Some(change)
}

/// Hand one event to the seam: buffered when a request is in flight,
/// drained on the spot otherwise.
pub(crate) async fn push(st: &AppState, ev: TemporalEvent) {
    #[cfg(not(target_arch = "wasm32"))]
    let ev = {
        let mut slot = Some(ev);
        let buffered = BUFFER
            .try_with(|b| {
                if let Some(ev) = slot.take() {
                    b.borrow_mut().push(ev);
                }
            })
            .is_ok();
        if buffered {
            return;
        }
        match slot {
            Some(ev) => ev,
            None => return,
        }
    };
    drain(st, vec![ev]).await;
}

/// The gate chain: an event enters history only if every gate admits it.
/// Gate 1 (value-change) runs in the producer — an unchanged instance never
/// becomes an event (`changed_instances`) — and so does gate 3, retention
/// ([`records_history`]), which needs the whole Entity. Adding a gate = one more entry
/// here; producers and drivers stay untouched.
const GATES: &[fn(&AppState, &TemporalEvent) -> bool] = &[observed_gate];

/// Gate 2: ANTARES_TEMPORAL_RECORD. `all` admits everything; `observed`
/// keeps only instances that carry `observedAt` — the spec's own
/// measurement axis (4.5.7: observedAt is the default timeproperty), so
/// metadata-shaped writes leave no history; `none` admits nothing.
fn observed_gate(st: &AppState, ev: &TemporalEvent) -> bool {
    use crate::state::TemporalRecord::*;
    // An audited write (DM-136) is kept without `observedAt` too.
    let audited = || {
        ev.instance
            .get(antares_store::PROVENANCE_KEY)
            .is_some_and(crate::provenance::audited)
    };
    match st.temporal_record {
        All | Ruled => true,
        Observed => ev.instance.get("observedAt").is_some() || audited(),
        None => false,
    }
}

/// An Entity's own history retention (DM-97), read from its
/// [`antares_store::HISTORY_RETENTION`] Property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnRetention {
    /// `off`: the Entity keeps its current state only.
    Off,
    /// A duration: its history is kept that long, whatever its type says.
    Keep(Duration),
}

/// What the Entity document `doc` (expanded, as stored) says of its own
/// history: `None` when it carries no `historyRetention`, `Some(Err)` with
/// the reason when the value is neither a duration nor `off` (a typo,
/// `forever`), which the rules of its type then override.
pub fn own_retention(doc: &Value) -> Option<Result<OwnRetention, String>> {
    doc.get(antares_store::HISTORY_RETENTION).map(retention_of)
}

/// What one stored `historyRetention` Property (one instance, or an array
/// whose first instance counts) says.
///
/// # Errors
/// Why it is neither a duration nor `off`.
pub fn retention_of(own: &Value) -> Result<OwnRetention, String> {
    let first = match own {
        Value::Array(all) => all.first(),
        one => Some(one),
    };
    let Some(raw) = first.and_then(|p| p.get("value")).and_then(Value::as_str) else {
        return Err("historyRetention is not a Property with a string value".into());
    };
    if raw == "off" {
        Ok(OwnRetention::Off)
    } else {
        parse_duration(raw).map(OwnRetention::Keep)
    }
}

/// Gate 3 (T-4275, DM-97), run in the producer where the whole Entity is at
/// hand: whether a write of `doc` in `tenant` records history. The most
/// specific level wins: the Entity's own `historyRetention`, then the rule
/// of one of its types (any `off` among them records nothing), then the
/// Tenant's `*` rule. Below them all, `ANTARES_TEMPORAL_RECORD=ruled`
/// records nothing (T-5056: history is opt-in), any other mode everything.
pub(crate) fn records_history(st: &AppState, tenant: &TenantId, doc: &Value) -> bool {
    if audited() {
        return true;
    }
    match own_retention(doc) {
        Some(Ok(OwnRetention::Off)) => return false,
        Some(Ok(OwnRetention::Keep(_))) => return true,
        Some(Err(_)) | None => {}
    }
    let rules = st
        .history_off
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let opt_in = st.temporal_record == crate::state::TemporalRecord::Ruled;
    if rules.is_empty() && !opt_in {
        return true;
    }
    let tenant = tenant.as_str();
    let types: Vec<&str> = match doc.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let has = |set: &std::collections::HashSet<(String, String)>| {
        types
            .iter()
            .any(|t| set.contains(&(tenant.to_owned(), (*t).to_owned())))
    };
    if has(&rules.off) {
        return false;
    }
    if opt_in {
        return has(&rules.ruled) || rules.space_on.contains(tenant);
    }
    has(&rules.ruled) || !rules.space_off.contains(tenant)
}

/// The last row each attribute recorded `onChange` wrote, by (Tenant,
/// Entity, attribute, datasetId) (T-4665).
type LastRows = std::collections::HashMap<(String, String, String, String), LastRow>;

/// An attribute's newest row, and the last late row written below it.
struct LastRow {
    payload: Value,
    at: chrono::DateTime<chrono::Utc>,
    /// A late instance (older than `at`) is judged against the last late row,
    /// or against `at` before there is one, so a replayed or retried batch
    /// of older seconds records one row per heartbeat, not one per second
    /// (T-5157: a Vehicle wrote one row in 4.5 s under a 30 s heartbeat).
    late: Option<(Value, chrono::DateTime<chrono::Utc>)>,
}
static LAST_ROWS: std::sync::LazyLock<std::sync::Mutex<LastRows>> =
    std::sync::LazyLock::new(Default::default);
/// The most attributes [`LAST_ROWS`] remembers.
// ponytail: one map per process, cleared when full or restarted; the next
// write of each attribute then records a row, which costs a row, never one
// lost. A shared store if replicas ever record far more than one row per
// heartbeat between them.
const LAST_ROWS_MAX: usize = 500_000;

/// The Attributes of the temporal `doc` its type records `onChange`
/// (ADR-N-054 2.5): a query carries their level into a window and weighs
/// their values by the time each held.
pub(crate) fn held_attrs(
    st: &AppState,
    tenant: &TenantId,
    doc: &Value,
) -> std::collections::HashSet<String> {
    let rules = st
        .history_off
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (false, Some(obj)) = (rules.recording.is_empty(), doc.as_object()) else {
        return Default::default();
    };
    let types: Vec<&str> = match obj.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let recordings: Vec<_> = types
        .iter()
        .filter_map(|t| {
            rules
                .recording
                .get(&(tenant.as_str().to_owned(), (*t).to_owned()))
        })
        .collect();
    obj.keys()
        .filter(|k| {
            recordings
                .iter()
                .any(|r| matches!(r.of(k), Some(RecordMode::OnChange(_))))
        })
        .cloned()
        .collect()
}

/// Does some type of `tenant` record an Attribute `onChange`? Its
/// aggregations are then computed here, never pushed into the store.
pub(crate) fn records_on_change(st: &AppState, tenant: &TenantId) -> bool {
    st.history_off
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .recording
        .iter()
        .any(|((t, _), r)| t == tenant.as_str() && r.has_on_change())
}

/// Gate 4 (T-4665, ADR-N-054 2.2): whether an instance of `attr`, changed
/// on the Entity `entity` with `types` in `tenant`, writes a row. With no
/// rule the mode is `every`, or `observed` under `ruled` (T-5056). `every`
/// always does, `never` never; `onChange` when its value moved past the
/// deadband or minimum distance since the attribute's last row, when it
/// differs at all from a row of the same `observedAt` (a correction), or
/// once the heartbeat passed. An instance older than that row (a backfill,
/// a retried batch) is judged the same way against the nearest row this
/// process knows below it.
/// `observed` when its `observedAt` is newer than that of the instance it
/// replaces (`before`, the attribute as the Entity held it).
pub(crate) fn admits_change(
    st: &AppState,
    tenant: &TenantId,
    types: &[&str],
    entity: &str,
    attr: &str,
    before: Option<&Value>,
    instance: &Value,
) -> bool {
    // An audited write keeps every change (DM-136); gate 1 already dropped
    // an exact repeat.
    if audited() {
        return true;
    }
    let default = if st.temporal_record == crate::state::TemporalRecord::Ruled {
        RecordMode::Observed
    } else {
        RecordMode::Every
    };
    let mode = {
        let rules = st
            .history_off
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        types.iter().find_map(|t| {
            rules
                .recording
                .get(&(tenant.as_str().to_owned(), (*t).to_owned()))
                .and_then(|r| r.of(attr))
        })
    };
    let mode = match mode.unwrap_or(default) {
        RecordMode::Every => return true,
        RecordMode::Never => return false,
        RecordMode::Observed => return newly_observed(before, instance),
        RecordMode::OnChange(mode) => mode,
    };
    let at = observed_at(instance).unwrap_or_else(chrono::Utc::now);
    let payload = payload_of(instance);
    let key = (
        tenant.as_str().to_owned(),
        entity.to_owned(),
        attr.to_owned(),
        instance
            .get("datasetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    let mut last = LAST_ROWS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let admits = |was: &Value, when: chrono::DateTime<chrono::Utc>| {
        if at == when {
            return payload != *was;
        }
        let apart = at.signed_duration_since(when).abs();
        apart.to_std().is_ok_and(|d| d >= mode.heartbeat) || moved(&mode, was, &payload)
    };
    match last.get_mut(&key) {
        Some(row) if at < row.at => {
            let (was, when) = row
                .late
                .as_ref()
                .map_or((&row.payload, row.at), |(p, t)| (p, *t));
            let writes = admits(was, when);
            if writes {
                row.late = Some((payload, at));
            }
            writes
        }
        Some(row) if !admits(&row.payload, row.at) => false,
        _ => {
            if last.len() >= LAST_ROWS_MAX {
                last.clear();
            }
            last.insert(
                key,
                LastRow {
                    payload,
                    at,
                    late: None,
                },
            );
            true
        }
    }
}

/// The instant `instance` was observed at, if it says.
fn observed_at(instance: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = instance.get("observedAt")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(&antares_store::filter::canonical_datetime(s))
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// `record: observed`: whether `instance` carries an `observedAt` newer than
/// the instance of its datasetId in `before` (an array or one instance).
fn newly_observed(before: Option<&Value>, instance: &Value) -> bool {
    let Some(now) = observed_at(instance) else {
        return false;
    };
    let ds = |i: &Value| i.get("datasetId").cloned();
    let was = match before {
        Some(Value::Array(all)) => all.iter().find(|b| ds(b) == ds(instance)),
        Some(one) if ds(one) == ds(instance) => Some(one),
        _ => None,
    };
    was.and_then(observed_at).is_none_or(|was| now > was)
}

/// An instance without its stamps and provenance: what a row records.
fn payload_of(instance: &Value) -> Value {
    let mut p = instance.clone();
    if let Some(o) = p.as_object_mut() {
        for k in [
            "instanceId",
            "observedAt",
            "createdAt",
            "modifiedAt",
            antares_store::PROVENANCE_KEY,
            antares_jsonld::compact::MODIFIED_BY_KEY,
        ] {
            o.remove(k);
        }
    }
    p
}

/// Whether `now` moved from `was` enough to be a row under `mode`.
fn moved(mode: &crate::state::OnChange, was: &Value, now: &Value) -> bool {
    let number = |v: &Value| v.get("value").and_then(Value::as_f64);
    if let (Some(band), Some(a), Some(b)) = (mode.deadband, number(was), number(now)) {
        return (a - b).abs() > band;
    }
    let point = |v: &Value| {
        let g = v.get("value")?;
        let c = g.get("coordinates")?.as_array()?;
        (g.get("type")?.as_str()? == "Point").then_some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
    };
    if let (Some(metres), Some(a), Some(b)) = (mode.min_distance, point(was), point(now)) {
        return haversine(a, b) > metres;
    }
    was != now
}

/// Metres between two (longitude, latitude) points on a sphere of the
/// Earth's mean radius: within a fraction of a percent at a city's scale.
fn haversine((lon1, lat1): (f64, f64), (lon2, lat2): (f64, f64)) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let (dp, dl) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * a.sqrt().asin()
}

/// The bounds a duration has to sit in: one minute to ten years.
const SHORTEST: Duration = Duration::from_secs(60);
const LONGEST: Duration = Duration::from_hours(87_600);

/// A duration as ISO 8601 (`PT5M`, `PT8H`, `P30D`, `P1DT12H`) or in short
/// form (`90s`, `5m`, `8h`, `14d`), between one minute and ten years.
///
/// # Errors
/// Why `raw` is not one.
pub fn parse_duration(raw: &str) -> Result<Duration, String> {
    parse_duration_within(raw, SHORTEST, LONGEST)
}

/// A duration as [`parse_duration`] reads it, between `shortest` and
/// `longest` (the quality sweep's `maxAge` starts at one second, T-4018).
///
/// # Errors
/// Why `raw` is not one.
pub fn parse_duration_within(
    raw: &str,
    shortest: Duration,
    longest: Duration,
) -> Result<Duration, String> {
    let bad = || format!("{raw:?} is not a duration such as PT5M, PT8H, P30D, 5m, 8h or 14d");
    let secs = if let Some(iso) = raw.strip_prefix('P') {
        let (days, time) = match iso.split_once('T') {
            Some((d, t)) if !t.is_empty() => (d, Some(t)),
            Some(_) => return Err(bad()),
            None => (iso, None),
        };
        let mut total = 0u64;
        if !days.is_empty() {
            total = days
                .strip_suffix('D')
                .and_then(|n| n.parse::<u64>().ok())
                .ok_or_else(bad)?
                .checked_mul(86_400)
                .ok_or_else(bad)?;
        } else if time.is_none() {
            return Err(bad());
        }
        let mut rest = time.unwrap_or_default();
        for (unit, scale) in [('H', 3600u64), ('M', 60), ('S', 1)] {
            if let Some((n, tail)) = rest.split_once(unit) {
                let n = n.parse::<u64>().map_err(|_| bad())?;
                total = n
                    .checked_mul(scale)
                    .and_then(|n| total.checked_add(n))
                    .ok_or_else(bad)?;
                rest = tail;
            }
        }
        if !rest.is_empty() {
            return Err(bad());
        }
        total
    } else {
        let split = raw.len().checked_sub(1).ok_or_else(bad)?;
        let (n, unit) = (
            raw.get(..split).ok_or_else(bad)?,
            raw.get(split..).ok_or_else(bad)?,
        );
        let scale = match unit {
            "s" => 1u64,
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => return Err(bad()),
        };
        n.parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(scale))
            .ok_or_else(bad)?
    };
    let d = Duration::from_secs(secs);
    if d < shortest || d > longest {
        return Err(format!(
            "{raw:?} is outside {}s to {}s",
            shortest.as_secs(),
            longest.as_secs()
        ));
    }
    Ok(d)
}

/// The consumer side: one `event_list` call per drained batch.
pub(crate) async fn drain(st: &AppState, evs: Vec<TemporalEvent>) {
    let evs: Vec<TemporalEvent> = evs
        .into_iter()
        .filter(|ev| GATES.iter().all(|gate| gate(st, ev)))
        .collect();
    if evs.is_empty() {
        return;
    }
    if let Err(e) = st.temporal.event_list(&evs).await {
        DRAIN_ERRORS.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("antares_temporal_drain_errors_total").increment(1);
        tracing::warn!(events = evs.len(), "temporal drain failed: {e}");
    }
}

/// Router layer: scopes the buffer over the handler and drains it once the
/// response is built. Drained BEFORE the response is returned so
/// read-your-writes holds at any store latency (the ETSI temporal suites
/// read history straight after the write).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn layer(
    axum::extract::State(st): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let provenance = crate::provenance::of_request(req.headers(), st.provenance_from.as_deref());
    let tenant = crate::negotiate::tenant_from(req.headers()).ok();
    let (resp, evs, changes) = BUFFER
        .scope(std::cell::RefCell::new(Vec::new()), async {
            STAGED
                .scope(
                    std::cell::RefCell::new(Vec::new()),
                    CHANGES.scope(std::cell::RefCell::new(Vec::new()), async {
                        PROVENANCE
                            .scope(
                                provenance,
                                TENANT.scope(tenant, async {
                                    let resp = next.run(req).await;
                                    (resp, BUFFER.with(|b| b.take()), CHANGES.with(|c| c.take()))
                                }),
                            )
                            .await
                    }),
                )
                .await
        })
        .await;
    drain(&st, evs).await;
    if !changes.is_empty() {
        if let Some(flush) = &st.change_flush {
            flush(changes);
        }
    }
    resp
}

/// No buffer on wasm (pushes drain on the spot): the layer is a pass-through
/// so the router composes identically on both targets.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn layer(
    axum::extract::State(_st): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    next.run(req).await
}

/// delete_temporal_on_core_delete: entity deletion removes its temporal
/// representation too (suite configuration parity). Skipped on bus=nats
/// api pods — the recorder applies the entityDeleted fence instead.
pub(crate) async fn mirror_delete_entity(st: &AppState, tenant: &TenantId, id: &str) {
    if !st.record_locally() {
        return;
    }
    if let Err(e) = st.temporal.delete(tenant, id).await {
        tracing::warn!("temporal mirror delete failed: {e}");
    }
}

/// 4.5.7/4.5.8: "In case the Property is deleted, an instance of the
/// Property is recorded with its value set to the URI "urn:ngsi-ld:null"
/// and the deletedAt Temporal Property set" (object for a Relationship;
/// typed null shapes for the LanguageProperty/JsonProperty/Vocab/List
/// subtypes). Each recorded instance carries an instanceId — the clause
/// SHOULD that makes 5.6.14/5.6.15 selective modification possible.
pub(crate) async fn mirror_delete_attr(
    st: &AppState,
    tenant: &TenantId,
    id: &str,
    attr_iri: &str,
    dataset_id: Option<&str>,
    ts: &str,
) {
    let r = st
        .temporal
        .mutate(tenant, id, |doc| {
            // The mirror writes nothing into a document the temporal driver
            // handed back in a shape the contract forbids.
            let Some(target) = doc.as_object_mut() else {
                return Ok::<(), std::convert::Infallible>(());
            };
            if attr_iri == "scope" {
                // scope deletion: a deleted scope is the NGSI-LD Null (4.18:
                // urn:ngsi-ld:null appears only for deleted scopes; 4.5.0)
                let inst = serde_json::json!({
                    "type": "Property",
                    "value": "urn:ngsi-ld:null",
                    "instanceId": format!("urn:ngsi-ld:Instance:{}", uuid::Uuid::new_v4()),
                    "deletedAt": ts, "createdAt": ts, "modifiedAt": ts,
                });
                match target.get_mut("scope").and_then(Value::as_array_mut) {
                    Some(arr) if arr.first().is_some_and(|i| i.is_object()) => arr.push(inst),
                    _ => {
                        target.insert("scope".into(), Value::Array(vec![inst]));
                    }
                }
                return Ok::<(), std::convert::Infallible>(());
            }
            if let Some(arr) = target.get_mut(attr_iri).and_then(Value::as_array_mut) {
                if arr.is_empty() {
                    return Ok(());
                }
                let atype = arr
                    .first()
                    .and_then(|i| i.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("Property")
                    .to_owned();
                let mut inst = Map::new();
                inst.insert("type".into(), Value::String(atype.clone()));
                let null = Value::String("urn:ngsi-ld:null".into());
                match atype.as_str() {
                    "Relationship" => {
                        inst.insert("object".into(), null);
                    }
                    "LanguageProperty" => {
                        inst.insert(
                            "languageMap".into(),
                            serde_json::json!({"@none": "urn:ngsi-ld:null"}),
                        );
                    }
                    "JsonProperty" => {
                        inst.insert("json".into(), null);
                    }
                    "VocabProperty" => {
                        inst.insert("vocab".into(), null);
                    }
                    "ListProperty" => {
                        inst.insert("valueList".into(), null);
                    }
                    "ListRelationship" => {
                        inst.insert("objectList".into(), null);
                    }
                    _ => {
                        inst.insert("value".into(), null);
                    }
                }
                if let Some(ds) = dataset_id {
                    inst.insert("datasetId".into(), Value::String(ds.to_owned()));
                }
                inst.insert(
                    "instanceId".into(),
                    Value::String(format!("urn:ngsi-ld:Instance:{}", uuid::Uuid::new_v4())),
                );
                inst.insert("deletedAt".into(), Value::String(ts.to_owned()));
                // the tombstone instance is written at the deletion: a
                // timeproperty=createdAt|modifiedAt window finds it there
                inst.insert("createdAt".into(), Value::String(ts.to_owned()));
                inst.insert("modifiedAt".into(), Value::String(ts.to_owned()));
                inst.insert(antares_store::PROVENANCE_KEY.into(), provenance("delete"));
                arr.push(Value::Object(inst));
            }
            Ok(())
        })
        .await;
    if let Err(e) = r {
        tracing::warn!("temporal attr mirror failed: {e}");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn ev(id: &str) -> TemporalEvent {
        TemporalEvent {
            op: antares_store::TemporalOp::AttrModified,
            tenant: TenantId::new("t").expect("tenant"),
            entity_id: id.to_owned(),
            shell: serde_json::json!({"id": id}),
            attr: "https://example.org/level".to_owned(),
            instance: serde_json::json!({"type": "Property", "value": 1}),
        }
    }

    /// F-ANT2-016: a backfill found inside a read-modify-write reaches the
    /// request's history only when that write commits; a refused write
    /// leaves none, and a rerun closure records its backfill once.
    #[tokio::test]
    async fn a_backfill_is_recorded_only_for_a_committed_write() {
        let buffered = BUFFER
            .scope(std::cell::RefCell::new(Vec::new()), async {
                STAGED
                    .scope(std::cell::RefCell::new(Vec::new()), async {
                        // refused: nothing reaches history
                        restage("urn:a");
                        assert!(stage(ev("urn:a")));
                        settle("urn:a", false);
                        assert_eq!(BUFFER.with(|b| b.borrow().len()), 0);
                        // rerun twice, then committed: one event
                        restage("urn:b");
                        assert!(stage(ev("urn:b")));
                        restage("urn:b");
                        assert!(stage(ev("urn:b")));
                        // another Entity's staged event stays staged
                        assert!(stage(ev("urn:c")));
                        settle_all::<(), ()>(&["urn:b".to_owned()], &[Some(Ok(()))]);
                        assert_eq!(STAGED.with(|b| b.borrow().len()), 1);
                        BUFFER.with(|b| b.take())
                    })
                    .await
            })
            .await;
        let ids: Vec<&str> = buffered.iter().map(|e| e.entity_id.as_str()).collect();
        assert_eq!(ids, ["urn:b"]);
    }
}
