//! The realm's names this platform did not create (ADR-N-030 §3.5, AP-114, AP-115).
//!
//! A Keycloak client `app-{name}` or a group without the `managed-by: joinedcontext` attribute
//! belongs to whoever made it: the reconciler never takes one over. The write doors refuse a new
//! App or Group that would need one, before a Change exists, so the clash is met in the form
//! and not as a blocked App in the reconciler's report. The waves that list the realm anyway
//! record what they saw here; the doors read it.
//!
//! ponytail: per-replica and as old as the last reconcile run, so a name made in the console
//! since then passes the door; the reconciler still refuses to take it over (AP-114).

use std::collections::BTreeSet;
use std::sync::RwLock;

#[derive(Debug, Default)]
pub struct ForeignNames {
    clients: RwLock<BTreeSet<String>>,
    groups: RwLock<BTreeSet<String>>,
}

impl ForeignNames {
    /// The ids of the unmanaged `app-*` clients the last client run listed.
    pub fn set_clients(&self, ids: BTreeSet<String>) {
        *self
            .clients
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ids;
    }

    /// The names of the unmanaged groups the last group run listed.
    pub fn set_groups(&self, names: BTreeSet<String>) {
        *self
            .groups
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = names;
    }

    /// Whether the realm holds a client `id` this platform did not create.
    pub fn has_client(&self, id: &str) -> bool {
        self.clients
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(id)
    }

    /// Whether the realm holds a group `name` this platform does not manage.
    pub fn has_group(&self, name: &str) -> bool {
        self.groups
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_replaces_what_the_last_one_saw() {
        let names = ForeignNames::default();
        assert!(!names.has_client("app-board"));
        names.set_clients(BTreeSet::from(["app-board".to_owned()]));
        names.set_groups(BTreeSet::from(["admins".to_owned()]));
        assert!(names.has_client("app-board"));
        assert!(names.has_group("admins"));
        assert!(!names.has_group("app-board"), "a client is not a group");
        names.set_clients(BTreeSet::new());
        assert!(!names.has_client("app-board"));
    }
}
