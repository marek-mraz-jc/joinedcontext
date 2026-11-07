// SPDX-License-Identifier: EUPL-1.2
//! Erasure of personal data in the stores (T-4665, platform ADR-N-054 2.6):
//! the current state, the history with every version, and the dead letters.
//! The admin route that asks for it and the log it leaves live in
//! `antares-ops` (`/q/erasures`).

use crate::state::AppState;
use antares_model::TenantId;
use antares_store::{CurrentStateDriverExt as _, Kind};
use serde_json::{json, Value};

/// A fresh id for one erasure's record.
#[must_use]
pub fn record_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The instant of a stored member (an Entity or one of its Attributes) a
/// re-applied erasure compares with its own: `observedAt`, else
/// `modifiedAt`, else `createdAt`; `None` when it carries none.
fn stamped(member: &Value) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let one = |m: &Value| {
        ["observedAt", "modifiedAt", "createdAt"]
            .iter()
            .find_map(|k| m.get(*k).and_then(Value::as_str))
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    match member {
        Value::Array(instances) => instances.iter().filter_map(one).max(),
        m => one(m),
    }
}

/// Erases `attrs` (None: the whole Entity) of `entities` from the current
/// state, the history and the dead letters; what went from each. `before`
/// (a re-applied erasure's own time) keeps what was written after it: an
/// Entity created or an Attribute stamped later, a row observed later. A
/// member with no stamp goes.
///
/// # Errors
/// A `before` that is no RFC 3339 instant, before anything goes; the
/// stores', as text.
pub async fn erase(
    st: &AppState,
    t: &TenantId,
    entities: &[String],
    attrs: Option<&[String]>,
    before: Option<&str>,
) -> Result<Value, String> {
    // a cut that does not parse erases nothing: read as no cut it would
    // erase what was written after the erasure
    let cut = before
        .map(|b| {
            chrono::DateTime::parse_from_rfc3339(b)
                .map_err(|e| format!("erasure time {b:?} is no RFC 3339 instant: {e}"))
        })
        .transpose()?;
    let goes = |member: &Value| match (cut, stamped(member)) {
        (Some(cut), Some(at)) => at <= cut,
        _ => true,
    };
    let (mut current, mut rows) = (0u64, 0u64);
    for id in entities {
        let hit = match attrs {
            None => st
                .store
                .delete_entity_if(t, id, &|doc: &Value| goes(doc))
                .await
                .map_err(|e| e.to_string())?,
            Some(attrs) => st
                .store
                .mutate(t, Kind::Entity, id, |doc| {
                    let o = doc.as_object_mut().ok_or(())?;
                    let named: Vec<&String> = attrs
                        .iter()
                        .filter(|a| o.get(a.as_str()).is_some_and(goes))
                        .collect();
                    for a in &named {
                        o.remove(a.as_str());
                    }
                    Ok::<bool, ()>(!named.is_empty())
                })
                .await
                .map_err(|e| e.to_string())?
                .and_then(Result::ok)
                .unwrap_or(false),
        };
        current = current.saturating_add(u64::from(hit));
        let gone = st
            .temporal
            .erase(t, id, attrs, before)
            .await
            .map_err(|e| e.to_string())?;
        rows = rows.saturating_add(gone);
    }
    // a dead letter goes whole when it notified one of the Entities; one
    // that merely names an Entity (a Relationship object) stays. A letter
    // with no readable notification goes when its text names one.
    let mut doomed = Vec::new();
    crate::csource::walk_docs(st, t, Kind::DeadLetter, |letter| {
        let hit = match notified(&letter) {
            Some(ids) => ids.iter().any(|id| entities.contains(id)),
            None => {
                let text = letter.to_string();
                entities
                    .iter()
                    .any(|id| text.contains(&format!("\"{id}\"")))
            }
        };
        if hit {
            if let Some(id) = letter.get("id").and_then(Value::as_str) {
                doomed.push(id.to_owned());
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut letters = 0u64;
    for id in doomed {
        let gone = st
            .store
            .delete(t, Kind::DeadLetter, &id)
            .await
            .map_err(|e| e.to_string())?;
        letters = letters.saturating_add(u64::from(gone));
    }
    Ok(json!({"entities": current, "history": rows, "deadLetters": letters}))
}

/// The ids of the Entities a dead letter notified: `data` of its
/// Notification (5.3.1), or the Features of a GeoJSON one (6.3.15); `None`
/// when it carries no readable notification.
fn notified(letter: &Value) -> Option<Vec<String>> {
    let out = antares_notifier::Outbound::from_dead_letter(letter).ok()?;
    let ids = ["data", "features"]
        .iter()
        .filter_map(|k| out.body.get(*k).and_then(Value::as_array))
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    Some(ids)
}
