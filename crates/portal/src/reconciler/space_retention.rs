//! The data of a removed Context Space (T-6717, P-21): its broker tenant keeps every entity and
//! history for a grace period after the removal applied, listed on its project's status, and is
//! purged at the broker (`DELETE /q/tenants/{tenant}`) when the grace ends. The removal itself was
//! confirmed already: any ContextSpace removal is held until a Change marked `risk: destructive`
//! confirms it ([`super::removal_hold`]). A revert within the grace brings the space back over
//! its data untouched.
//!
//! The tenants a run applied and the retained ones live in one file on the reconciler's state
//! directory, so a space removed while the Portal restarts is still seen as removed. Without a
//! state directory they live in memory: a restart then forgets a retained tenant and its data
//! stays, as before this module.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::store::Mirror;

/// How long a removed space's data is kept before it is purged (decided by the supervisor,
/// owner may override: 7 days).
pub const GRACE_SECS: u64 = 7 * 24 * 60 * 60;

/// The condition a project with retained space data carries.
pub const CONDITION: &str = "SpaceDataRetained";

/// One Context Space as the run applied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub project: String,
    pub space: String,
    /// The broker the tenant lives on (`main` or a `ContextBroker` name); `""` for none.
    pub broker: String,
}

/// A removed space's data, kept until `removed_at + GRACE_SECS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retained {
    #[serde(flatten)]
    pub space: Space,
    /// Unix seconds when the removal applied.
    pub removed_at: u64,
}

impl Retained {
    pub fn purge_at(&self) -> u64 {
        self.removed_at.saturating_add(GRACE_SECS)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    /// The applied spaces by tenant.
    live: BTreeMap<String, Space>,
    /// The removed spaces' data by tenant.
    retained: BTreeMap<String, Retained>,
}

/// The applied and retained tenants, in memory and, with a state directory, in one file there.
pub struct SpaceRetention {
    path: Option<PathBuf>,
    state: Mutex<State>,
}

impl SpaceRetention {
    pub fn new(path: Option<PathBuf>) -> Self {
        let state = path
            .as_deref()
            .and_then(|path| match std::fs::read(path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| tracing::warn!(path = %path.display(), %error, "the retained spaces file does not parse; no removed space's data is purged until a space is removed again"))
                    .ok(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "the retained spaces file was not read; no removed space's data is purged until a space is removed again");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records one run's spaces (`(project, name, tenant)`, with each tenant's broker): a tenant
    /// back in the run leaves the retained set, and a tenant the run lacks is retained from `now`,
    /// unless `held` (removals wait for a confirm: the tenant stays applied).
    pub fn observe(
        &self,
        spaces: &[(String, String, String)],
        brokers: &BTreeMap<String, String>,
        held: bool,
        now: u64,
    ) {
        let mut state = self.lock();
        let before = state.clone();
        let current: BTreeMap<String, Space> = spaces
            .iter()
            .map(|(project, space, tenant)| {
                let broker = brokers.get(tenant).cloned().unwrap_or_default();
                (
                    tenant.clone(),
                    Space {
                        project: project.clone(),
                        space: space.clone(),
                        broker,
                    },
                )
            })
            .collect();
        // A space is the ContextSpace (project, name): an Update of its urnSegment moves it to a
        // new tenant with no removal and no destructive confirm, so its earlier tenant is never
        // retained or purged while the space exists.
        let applied: std::collections::BTreeSet<(&str, &str)> = current
            .values()
            .map(|space| (space.project.as_str(), space.space.as_str()))
            .collect();
        let exists =
            |space: &Space| applied.contains(&(space.project.as_str(), space.space.as_str()));
        state.retained.retain(|tenant, retained| {
            let back = current.contains_key(tenant) || exists(&retained.space);
            if back {
                tracing::info!(%tenant, project = %retained.space.project, space = %retained.space.space, "a removed Context Space is back within its grace: its data is kept");
            }
            !back
        });
        if held {
            state.live.extend(current);
        } else {
            let gone: Vec<(String, Space)> = state
                .live
                .iter()
                .filter(|(tenant, space)| !current.contains_key(*tenant) && !exists(space))
                .map(|(tenant, space)| (tenant.clone(), space.clone()))
                .collect();
            for (tenant, space) in gone {
                tracing::info!(%tenant, project = %space.project, space = %space.space, purge_after_secs = GRACE_SECS, "a Context Space was removed: its data is kept for the grace, then purged");
                state.retained.entry(tenant).or_insert(Retained {
                    space,
                    removed_at: now,
                });
            }
            state.live = current;
        }
        if *state != before {
            self.write(&state);
        }
    }

    /// The retained tenants whose grace has ended at `now`.
    pub fn due(&self, now: u64) -> Vec<(String, Retained)> {
        self.lock()
            .retained
            .iter()
            .filter(|(_, retained)| retained.purge_at() <= now)
            .map(|(tenant, retained)| (tenant.clone(), retained.clone()))
            .collect()
    }

    /// Every retained tenant.
    pub fn retained(&self) -> BTreeMap<String, Retained> {
        self.lock().retained.clone()
    }

    /// Forgets a tenant the broker purged.
    pub fn purged(&self, tenant: &str) {
        let mut state = self.lock();
        if state.retained.remove(tenant).is_some() {
            self.write(&state);
        }
    }

    /// Written beside the file and renamed, so a crash leaves the old file or the new one.
    fn write(&self, state: &State) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let next = path.with_extension("next");
        let written = serde_json::to_vec(state)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&next, bytes))
            .and_then(|()| std::fs::rename(&next, path));
        if let Err(error) = written {
            tracing::warn!(path = %path.display(), %error, "the retained spaces were not written; a restart reads the last file");
        }
    }
}

/// A UTC date for a person, from unix seconds.
fn date(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| secs.to_string())
}

/// Marks each project holding retained space data (the Organization for `org`) with the
/// `SpaceDataRetained` condition naming each space and its purge date; a project with none
/// loses the condition.
pub fn mark(mirror: &Mirror, retained: &BTreeMap<String, Retained>) {
    let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for retained in retained.values() {
        by_project
            .entry(retained.space.project.as_str())
            .or_default()
            .push(format!(
                "ContextSpace {} until {}",
                retained.space.space,
                date(retained.purge_at())
            ));
    }
    for mut envelope in mirror.matching(|env| env.kind == "Project" || env.kind == "Organization") {
        let project = if envelope.kind == "Organization" {
            crate::api::blueprints::ORG_NAMESPACE
        } else {
            envelope.metadata.name.as_str()
        };
        let spaces = by_project.get(project).cloned();
        let Some(status) = envelope.status.as_mut() else {
            continue;
        };
        let had = status.conditions.iter().any(|c| c.r#type == CONDITION);
        if spaces.is_none() && !had {
            continue;
        }
        status.conditions.retain(|c| c.r#type != CONDITION);
        if let Some(spaces) = spaces {
            status.conditions.push(super::streams::make_condition(
                CONDITION,
                "True",
                "RemovedSpaceKept",
                &format!(
                    "the data of a removed Context Space is kept, then purged at the broker: {}. \
                     Restore the ContextSpace manifest before that date to keep its data",
                    spaces.join("; ")
                ),
            ));
        }
        mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces(names: &[&str]) -> Vec<(String, String, String)> {
        names
            .iter()
            .map(|name| ("p".to_owned(), (*name).to_owned(), format!("org-p-{name}")))
            .collect()
    }

    fn brokers() -> BTreeMap<String, String> {
        [("org-p-air", "main"), ("org-p-noise", "main")]
            .into_iter()
            .map(|(t, b)| (t.to_owned(), b.to_owned()))
            .collect()
    }

    const T0: u64 = 1_800_000_000;

    #[test]
    fn a_removed_space_is_kept_before_the_grace_and_due_after_it() {
        let retention = SpaceRetention::new(None);
        retention.observe(&spaces(&["air", "noise"]), &brokers(), false, T0);
        assert!(retention.retained().is_empty());
        retention.observe(&spaces(&["noise"]), &brokers(), false, T0 + 60);
        let retained = retention.retained();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained["org-p-air"].space.broker, "main");
        assert!(
            retention.due(T0 + 60 + GRACE_SECS - 1).is_empty(),
            "kept before the grace"
        );
        let due = retention.due(T0 + 60 + GRACE_SECS);
        assert_eq!(due.len(), 1, "purged once the grace ends");
        assert_eq!(due[0].0, "org-p-air");
        retention.purged("org-p-air");
        assert!(retention.retained().is_empty());
        // A later run does not retain it again: it is no longer applied.
        retention.observe(&spaces(&["noise"]), &brokers(), false, T0 + 2 * GRACE_SECS);
        assert!(retention.retained().is_empty());
    }

    #[test]
    fn a_space_restored_within_the_grace_keeps_its_data() {
        let retention = SpaceRetention::new(None);
        retention.observe(&spaces(&["air"]), &brokers(), false, T0);
        retention.observe(&[], &brokers(), false, T0 + 1);
        assert_eq!(retention.retained().len(), 1);
        retention.observe(&spaces(&["air"]), &brokers(), false, T0 + 2);
        assert!(retention.retained().is_empty());
        assert!(retention.due(T0 + 2 * GRACE_SECS).is_empty());
    }

    #[test]
    fn a_space_whose_tenant_changed_is_not_retained_or_purged() {
        let retention = SpaceRetention::new(None);
        retention.observe(&spaces(&["air"]), &brokers(), false, T0);
        // An Update of the space's urnSegment: same (project, name), a new tenant.
        let renamed = vec![("p".to_owned(), "air".to_owned(), "org-p-air2".to_owned())];
        retention.observe(&renamed, &brokers(), false, T0 + 1);
        assert!(
            retention.retained().is_empty(),
            "the space still exists: its old tenant is never purged"
        );
        assert!(retention.due(T0 + 2 * GRACE_SECS).is_empty());
        // Removed later: only the tenant it applied last is retained.
        retention.observe(&[], &brokers(), false, T0 + 2);
        assert_eq!(
            retention.retained().keys().collect::<Vec<_>>(),
            ["org-p-air2"]
        );
    }

    #[test]
    fn a_removed_space_back_under_a_new_tenant_keeps_the_old_data() {
        let retention = SpaceRetention::new(None);
        retention.observe(&spaces(&["air"]), &brokers(), false, T0);
        retention.observe(&[], &brokers(), false, T0 + 1);
        assert_eq!(retention.retained().len(), 1);
        let back = vec![("p".to_owned(), "air".to_owned(), "org-p-air2".to_owned())];
        retention.observe(&back, &brokers(), false, T0 + 2);
        assert!(
            retention.retained().is_empty(),
            "the ContextSpace is back: its earlier tenant is not purged"
        );
    }

    #[test]
    fn a_held_run_retains_nothing() {
        let retention = SpaceRetention::new(None);
        retention.observe(&spaces(&["air"]), &brokers(), false, T0);
        retention.observe(&[], &brokers(), true, T0 + 1);
        assert!(
            retention.retained().is_empty(),
            "a held removal is not applied yet"
        );
        retention.observe(&[], &brokers(), false, T0 + 2);
        assert_eq!(retention.retained()["org-p-air"].removed_at, T0 + 2);
    }

    #[test]
    fn a_space_removed_across_a_restart_is_still_retained() {
        let dir = std::env::temp_dir().join(format!("jc-retained-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("retained-spaces.json");
        SpaceRetention::new(Some(path.clone())).observe(&spaces(&["air"]), &brokers(), false, T0);
        let restarted = SpaceRetention::new(Some(path.clone()));
        restarted.observe(&[], &brokers(), false, T0 + 5);
        let again = SpaceRetention::new(Some(path));
        assert_eq!(again.retained()["org-p-air"].removed_at, T0 + 5);
        assert_eq!(again.due(T0 + 5 + GRACE_SECS).len(), 1);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
