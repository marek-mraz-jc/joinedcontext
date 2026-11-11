//! Removals held back for a mass deletion (T-6708, P-6): a revision that takes more than a
//! deployment's share of a project's objects, or any ContextSpace, removes nothing live until a
//! Change marked `risk: destructive` confirms it.
//!
//! The comparison is against the objects the last applied run held, kept beside the assembly on
//! the reconciler's state directory so a restart compares with the same set. A held project keeps
//! its applied entry, so the next run holds again until the confirm lands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::store::Mirror;

/// The share of a project's objects one revision may remove before it is held (owner decision
/// via the supervisor, 2026-11-11; `JC_PORTAL_REMOVAL_HOLD_PERCENT`).
pub const DEFAULT_PERCENT: u8 = 25;

/// The merge commit line the Portal writes for an approved red-lane Change, whose approver typed
/// the object's name to confirm it (CC-19, PF-13).
pub const DESTRUCTIVE_TRAILER: &str = "Risk: destructive";

/// Every mirrored object by namespace, as `Kind/name`.
pub type Objects = BTreeMap<String, BTreeSet<String>>;

/// The objects `mirror` holds, by namespace.
pub fn objects(mirror: &Mirror) -> Objects {
    let mut out = Objects::new();
    for envelope in mirror.matching(|_| true) {
        let key = envelope.key();
        out.entry(key.namespace)
            .or_default()
            .insert(format!("{}/{}", key.kind, key.name));
    }
    out
}

/// One project's share of objects a revision removes beyond what may go unconfirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shrink {
    pub project: String,
    pub removed: usize,
    pub total: usize,
    /// The ContextSpaces among them, by name.
    pub spaces: Vec<String>,
}

impl Shrink {
    /// What the project's status says, and what a person does about it.
    pub fn message(&self, percent: u8) -> String {
        let spaces = if self.spaces.is_empty() {
            String::new()
        } else {
            format!(", ContextSpace {}", self.spaces.join(", "))
        };
        format!(
            "this revision removes {} of {} objects{spaces}, more than this deployment lets go \
             unconfirmed ({percent} % or any ContextSpace), so nothing live is removed: approve \
             the removal as a red-lane Change in the Portal (it asks you to type the name), or \
             restore the manifests",
            self.removed, self.total
        )
    }
}

/// The projects of `applied` whose objects `current` lacks beyond `percent`, or lacks a
/// ContextSpace of. A project in `unloaded` did not load this run and is not judged here.
pub fn shrinks(
    applied: &Objects,
    current: &Objects,
    percent: u8,
    unloaded: &[String],
) -> Vec<Shrink> {
    let empty = BTreeSet::new();
    applied
        .iter()
        .filter(|(project, before)| !before.is_empty() && !unloaded.contains(project))
        .filter_map(|(project, before)| {
            let after = current.get(project).unwrap_or(&empty);
            let removed: Vec<&String> = before.difference(after).collect();
            let spaces: Vec<String> = removed
                .iter()
                .filter_map(|key| key.strip_prefix("ContextSpace/").map(str::to_owned))
                .collect();
            let beyond = removed.len() * 100 > usize::from(percent) * before.len();
            (beyond || !spaces.is_empty()).then(|| Shrink {
                project: project.clone(),
                removed: removed.len(),
                total: before.len(),
                spaces,
            })
        })
        .collect()
}

/// Whether a commit message is a confirmed destructive Change's merge.
pub fn confirmed(message: &str) -> bool {
    message
        .lines()
        .any(|line| line.trim() == DESTRUCTIVE_TRAILER)
}

/// The next applied set: `current`, but a project in `keep` keeps what `applied` held of it.
pub fn next_applied(applied: Option<&Objects>, current: &Objects, keep: &[String]) -> Objects {
    let mut next = current.clone();
    for project in keep {
        match applied.and_then(|applied| applied.get(project)) {
            Some(before) => next.insert(project.clone(), before.clone()),
            None => next.remove(project),
        };
    }
    next
}

/// Marks each held project's manifest (the Organization for its own namespace) with the
/// `HeldRemoval` condition.
pub fn mark(mirror: &Mirror, held: &[Shrink], percent: u8) {
    for shrink in held {
        let org = shrink.project == crate::api::blueprints::ORG_NAMESPACE;
        let Some(mut envelope) = mirror.find(|env| {
            if org {
                env.kind == "Organization"
            } else {
                env.kind == "Project" && env.metadata.name == shrink.project
            }
        }) else {
            continue;
        };
        if let Some(status) = envelope.status.as_mut() {
            status.conditions.retain(|c| c.r#type != "RemovalsApplied");
            status.conditions.push(super::streams::make_condition(
                "RemovalsApplied",
                "False",
                "HeldRemoval",
                &shrink.message(percent),
            ));
            mirror.upsert(envelope);
        }
    }
}

/// The applied set, in memory and, with a state directory, in one file there.
pub struct Applied {
    path: Option<PathBuf>,
    objects: Mutex<Option<Objects>>,
}

impl Applied {
    pub fn new(path: Option<PathBuf>) -> Self {
        let objects = path.as_deref().and_then(|path| match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| tracing::warn!(path = %path.display(), %error, "the applied objects file does not parse; the next run compares with nothing"))
                .ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "the applied objects file was not read; the next run compares with nothing");
                None
            }
        });
        Self {
            path,
            objects: Mutex::new(objects),
        }
    }

    /// What the last applied run held; `None` before the first.
    pub fn get(&self) -> Option<Objects> {
        self.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Records `objects` as applied, and writes the file when they changed: written beside it
    /// and renamed, so a crash leaves the old file or the new one.
    pub fn set(&self, objects: Objects) {
        let mut held = self
            .objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.as_ref() == Some(&objects) {
            return;
        }
        if let Some(path) = self.path.as_deref() {
            let next = path.with_extension("next");
            let written = serde_json::to_vec(&objects)
                .map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(&next, bytes))
                .and_then(|()| std::fs::rename(&next, path));
            if let Err(error) = written {
                tracing::warn!(path = %path.display(), %error, "the applied objects were not written; a restart compares with the last file");
            }
        }
        *held = Some(objects);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(n: usize, spaces: usize) -> BTreeSet<String> {
        (0..n)
            .map(|i| format!("Endpoint/e{i}"))
            .chain((0..spaces).map(|i| format!("ContextSpace/s{i}")))
            .collect()
    }

    fn one(name: &str, objects: BTreeSet<String>) -> Objects {
        Objects::from([(name.to_owned(), objects)])
    }

    #[test]
    fn a_30_percent_removal_is_held_and_a_10_percent_one_is_not() {
        let applied = one("b", project(10, 0));
        let thirty = one("b", project(7, 0));
        let held = shrinks(&applied, &thirty, DEFAULT_PERCENT, &[]);
        assert_eq!(
            held,
            vec![Shrink {
                project: "b".into(),
                removed: 3,
                total: 10,
                spaces: vec![]
            }]
        );
        assert!(held[0].message(25).contains("removes 3 of 10 objects"));
        let ten = one("b", project(9, 0));
        assert!(shrinks(&applied, &ten, DEFAULT_PERCENT, &[]).is_empty());
        // Exactly the share goes unconfirmed: "more than" 25 %.
        let quarter = one("b", project(3, 0));
        assert!(shrinks(&one("b", project(4, 0)), &quarter, 25, &[]).is_empty());
    }

    #[test]
    fn a_context_space_removal_is_held_however_small() {
        let applied = one("b", project(40, 1));
        let held = shrinks(&applied, &one("b", project(40, 0)), DEFAULT_PERCENT, &[]);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].spaces, vec!["s0".to_owned()]);
        assert!(held[0].message(25).contains("ContextSpace s0"));
    }

    #[test]
    fn a_project_gone_whole_is_held_unless_it_did_not_load() {
        let applied = one("b", project(3, 0));
        assert_eq!(shrinks(&applied, &Objects::new(), 25, &[]).len(), 1);
        assert!(shrinks(&applied, &Objects::new(), 25, &["b".into()]).is_empty());
    }

    #[test]
    fn a_held_project_keeps_its_applied_entry_and_the_rest_move_on() {
        let applied = Objects::from([("a".into(), project(4, 0)), ("b".into(), project(10, 0))]);
        let current = Objects::from([("a".into(), project(3, 0)), ("b".into(), project(2, 0))]);
        let next = next_applied(Some(&applied), &current, &["b".into()]);
        assert_eq!(next["a"], project(3, 0));
        assert_eq!(next["b"], project(10, 0));
    }

    #[test]
    fn only_the_trailer_line_confirms() {
        assert!(confirmed(
            "Merge change proposal chg-1: x\n\nApproved in the Portal by eva@bb.sk\nRisk: destructive"
        ));
        assert!(!confirmed("Merge: Risk: destructive is what this is not"));
        assert!(!confirmed("Merge change proposal chg-1: x"));
    }

    #[test]
    fn the_applied_set_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("jc-applied-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("applied.json");
        let objects = one("b", project(2, 1));
        Applied::new(Some(path.clone())).set(objects.clone());
        assert_eq!(Applied::new(Some(path)).get(), Some(objects));
        assert_eq!(Applied::new(None).get(), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
