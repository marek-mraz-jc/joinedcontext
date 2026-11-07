//! A condition's `lastTransitionTime` moves only when the condition changes (T-2989).
//!
//! Every run compiles `status` afresh (MF-04), so a step that says `Ready False NoBuild` again
//! would stamp it with the run's own time, and a reader would see a transition every minute and
//! never learn since when the App has had no build. A run takes the conditions it starts from,
//! and a condition that still says what it said (its type, status and reason) keeps the time it
//! first said it.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::resource::ResourceEnvelope;
use crate::store::Mirror;

/// Namespace, kind, name and the condition's type.
type Key = (String, String, String, String);

/// The conditions a run started from: status, reason and the time of the last transition.
#[derive(Debug, Default)]
pub struct Transitions(HashMap<Key, (String, Option<String>, DateTime<Utc>)>);

fn key(envelope: &ResourceEnvelope, condition_type: &str) -> Key {
    (
        envelope.metadata.namespace.clone().unwrap_or_default(),
        envelope.kind.clone(),
        envelope.metadata.name.clone(),
        condition_type.to_owned(),
    )
}

fn has_conditions(envelope: &ResourceEnvelope) -> bool {
    envelope
        .status
        .as_ref()
        .is_some_and(|status| !status.conditions.is_empty())
}

impl Transitions {
    /// The conditions the mirror holds now.
    pub fn of(mirror: &Mirror) -> Self {
        let mut seen = HashMap::new();
        for envelope in mirror.matching(has_conditions) {
            for condition in envelope.status.iter().flat_map(|status| &status.conditions) {
                if let Some(at) = condition.last_transition_time {
                    seen.insert(
                        key(&envelope, &condition.r#type),
                        (condition.status.clone(), condition.reason.clone(), at),
                    );
                }
            }
        }
        Self(seen)
    }

    /// Gives each condition that says what it said at the run's start the time it first said
    /// it; a changed or new condition keeps its own. Whether anything changed.
    pub fn keep(&self, envelope: &mut ResourceEnvelope) -> bool {
        let keys: Vec<Key> = envelope
            .status
            .iter()
            .flat_map(|status| &status.conditions)
            .map(|condition| key(envelope, &condition.r#type))
            .collect();
        let Some(status) = envelope.status.as_mut() else {
            return false;
        };
        let mut changed = false;
        for (condition, key) in status.conditions.iter_mut().zip(keys) {
            let Some((was, why, at)) = self.0.get(&key) else {
                continue;
            };
            if *was == condition.status
                && *why == condition.reason
                && condition.last_transition_time != Some(*at)
            {
                condition.last_transition_time = Some(*at);
                changed = true;
            }
        }
        changed
    }

    /// [`Transitions::keep`] on every resource of a mirror that carries a condition.
    pub fn keep_all(&self, mirror: &Mirror) {
        for mut envelope in mirror.matching(has_conditions) {
            if self.keep(&mut envelope) {
                mirror.upsert(envelope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, Phase, Status, API_VERSION};
    use chrono::TimeZone;

    fn app(conditions: Vec<crate::resource::Condition>) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "App".into(),
            metadata: ObjectMeta {
                name: "alerts-desk".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: Some(Status {
                phase: Phase::Pending,
                observed_revision: None,
                source_url: None,
                conditions,
                build: None,
                shard: None,
                domain_verification: None,
                jobs: Vec::new(),
                notifications: None,
            }),
        }
    }

    fn ready(status: &str, reason: &str, at: DateTime<Utc>) -> crate::resource::Condition {
        let mut condition = super::super::streams::make_condition("Ready", status, reason, "said");
        condition.last_transition_time = Some(at);
        condition
    }

    fn time(envelope: &ResourceEnvelope) -> Option<DateTime<Utc>> {
        envelope
            .status
            .as_ref()?
            .conditions
            .first()?
            .last_transition_time
    }

    #[test]
    fn a_condition_that_says_the_same_keeps_its_time_and_a_changed_one_moves() {
        let first = Utc
            .with_ymd_and_hms(2026, 9, 25, 20, 26, 28)
            .single()
            .expect("a time");
        let now = Utc
            .with_ymd_and_hms(2026, 9, 25, 20, 27, 28)
            .single()
            .expect("a time");
        let mirror = Mirror::new();
        mirror.upsert(app(vec![ready("False", "NoBuild", first)]));
        let before = Transitions::of(&mirror);

        let mut again = app(vec![ready("False", "NoBuild", now)]);
        assert!(before.keep(&mut again));
        assert_eq!(time(&again), Some(first), "nothing transitioned");

        for mut changed in [
            app(vec![ready("False", "BuildMissing", now)]),
            app(vec![ready("True", "NoBuild", now)]),
        ] {
            assert!(!before.keep(&mut changed));
            assert_eq!(
                time(&changed),
                Some(now),
                "a new reason or status is a transition"
            );
        }
    }

    #[test]
    fn a_condition_the_run_did_not_start_from_keeps_its_own_time() {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 25, 20, 27, 28)
            .single()
            .expect("a time");
        let empty = Transitions::of(&Mirror::new());
        let mut new = app(vec![ready("False", "NoBuild", now)]);
        assert!(!empty.keep(&mut new));
        assert_eq!(time(&new), Some(now));

        // keep_all writes back only what it changed.
        let mirror = Mirror::new();
        mirror.upsert(app(vec![ready("False", "NoBuild", now)]));
        let before = Transitions::of(&mirror);
        let fresh = Mirror::new();
        let later = now + chrono::Duration::minutes(1);
        fresh.upsert(app(vec![ready("False", "NoBuild", later)]));
        before.keep_all(&fresh);
        let kept = fresh
            .get("helsinki", "App", "alerts-desk")
            .expect("the app");
        assert_eq!(time(&kept), Some(now));
    }
}
