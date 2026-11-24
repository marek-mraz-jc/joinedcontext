//! A set the host registry's watcher (`modules::portal`) publishes after every rebuild, and the
//! modules that follow it (T-7180's sign-in pattern, shared since T-7186): a follower registered
//! late gets the last set at once, and a set equal to the last one reaches nobody.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

type Follower<T> = Arc<dyn Fn(&T) + Send + Sync>;

struct Held<T> {
    /// The set the registry served last; `None` before its first rebuild.
    last: Option<T>,
    /// Keyed by module, so a module built again replaces its follower.
    by_module: BTreeMap<&'static str, Follower<T>>,
}

/// One published set and its followers.
pub struct Published<T> {
    held: Mutex<Held<T>>,
}

impl<T: PartialEq> Published<T> {
    pub const fn new() -> Self {
        Self {
            held: Mutex::new(Held {
                last: None,
                by_module: BTreeMap::new(),
            }),
        }
    }

    fn held(&self) -> MutexGuard<'_, Held<T>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Calls `follow` with every set published from now on, and at once with the last one when
    /// the registry was read before this module was built. Followers are called under the lock,
    /// so a set published meanwhile never reaches a follower before an older one. A follower only
    /// swaps a table or hands the set to a task; it never calls [`Self::follow`] or
    /// [`Self::publish`].
    pub fn follow(&self, module: &'static str, follow: impl Fn(&T) + Send + Sync + 'static) {
        let follow: Follower<T> = Arc::new(follow);
        let mut held = self.held();
        if let Some(last) = &held.last {
            follow(last);
        }
        held.by_module.insert(module, follow);
    }

    /// Hands `set` to every follower when it differs from the last one; `true` when it did.
    #[cfg(feature = "portal")]
    pub fn publish(&self, set: T) -> bool {
        let mut held = self.held();
        if held.last.as_ref() == Some(&set) {
            return false;
        }
        for follow in held.by_module.values() {
            follow(&set);
        }
        held.last = Some(set);
        true
    }
}

/// The Organizations the registry runs beyond jc.toml (T-7186): each `active` row jc.toml does
/// not name, as the section its served hosts make (`app::registry_hosts`; `catalogue_id` empty
/// while the row has none), and every `removed` row's name, jc.toml's included. A module that
/// sets an Organization up at start (dataroom's catalogue, the Portal's catalogue entry) sets up
/// the run-time ones from here and takes a removed one out.
#[cfg(feature = "portal")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Organizations {
    pub run_time: Vec<crate::config::OrganizationSection>,
    pub removed: Vec<String>,
}

#[cfg(feature = "portal")]
pub static ORGANIZATIONS: Published<Organizations> = Published::new();

/// The Organization jc runs each project and branch under, by name (the project registry,
/// ARCH/TEN-04): the gateway keeps each Organization's host to its own projects by it (T-7411).
#[cfg(all(feature = "portal", feature = "gateway"))]
pub static PROJECT_TENANTS: Published<BTreeMap<String, String>> = Published::new();

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> (Arc<Mutex<Vec<u32>>>, impl Fn(&u32) + Send + Sync + 'static) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&seen);
        (seen, move |set: &u32| into.lock().unwrap().push(*set))
    }

    #[test]
    fn a_follower_before_the_first_set_waits_and_a_late_one_gets_the_last_at_once() {
        let published = Published::<u32>::new();
        let (early, follow) = recorder();
        published.follow("early", follow);
        assert!(early.lock().unwrap().is_empty(), "nothing published yet");
        #[cfg(feature = "portal")]
        {
            assert!(published.publish(1));
            assert!(published.publish(2));
            let (late, follow) = recorder();
            published.follow("late", follow);
            assert_eq!(*late.lock().unwrap(), vec![2], "the last set only, at once");
            assert_eq!(*early.lock().unwrap(), vec![1, 2]);
        }
    }

    #[cfg(feature = "portal")]
    #[test]
    fn a_set_equal_to_the_last_reaches_nobody() {
        let published = Published::<u32>::new();
        let (seen, follow) = recorder();
        published.follow("m", follow);
        assert!(published.publish(7));
        assert!(!published.publish(7), "an equal set is not handed out");
        assert!(published.publish(8));
        assert_eq!(*seen.lock().unwrap(), vec![7, 8]);
    }

    #[cfg(feature = "portal")]
    #[test]
    fn a_module_built_again_replaces_its_follower() {
        let published = Published::<u32>::new();
        let (first, follow) = recorder();
        published.follow("m", follow);
        let (second, follow) = recorder();
        published.follow("m", follow);
        assert!(published.publish(3));
        assert!(first.lock().unwrap().is_empty(), "the old follower is gone");
        assert_eq!(*second.lock().unwrap(), vec![3]);
    }

    #[cfg(feature = "portal")]
    #[test]
    fn a_follower_that_panicked_leaves_the_set_usable() {
        let published = Arc::new(Published::<u32>::new());
        published.follow("bad", |set: &u32| assert_ne!(*set, 1, "refuses one"));
        let inner = Arc::clone(&published);
        let crashed = std::thread::spawn(move || inner.publish(1)).join();
        assert!(crashed.is_err(), "the follower panicked under the lock");
        let (seen, follow) = recorder();
        published.follow("good", follow);
        assert!(published.publish(2), "a poisoned lock is taken over");
        assert_eq!(*seen.lock().unwrap(), vec![2]);
    }
}
