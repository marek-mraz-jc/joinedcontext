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

/// The token the Portal publishes into a run-time Organization's catalogue with (T-7185):
/// minted here once per Organization and process, held only in memory and handed in process to
/// dataroom's grant and the Portal's catalogue entry, so no token file exists for it. `Err` when
/// the OS gives no randomness (nothing is minted then).
#[cfg(feature = "portal")]
pub fn catalogue_token(organization: &str) -> Result<Arc<jcctl::secrets::SecretValue>, String> {
    static TOKENS: Mutex<BTreeMap<String, Arc<jcctl::secrets::SecretValue>>> =
        Mutex::new(BTreeMap::new());
    let mut tokens = TOKENS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(token) = tokens.get(organization) {
        return Ok(token.clone());
    }
    let token = Arc::new(
        jcctl::secrets::SecretValue::generate()
            .map_err(|e| format!("no randomness for its catalogue token: {e}"))?,
    );
    tokens.insert(organization.to_owned(), token.clone());
    Ok(token)
}
