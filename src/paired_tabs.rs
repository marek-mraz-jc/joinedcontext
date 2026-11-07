//! Portal tabs a person connected to one of their connectors (T-5967, GOAL-connectors §6).
//!
//! "Connect this tab" pairs the open tab with a connector: while it is paired, the connector's
//! MCP client (Claude Code, say) may send the person's browser the assistant's UI actions
//! (navigate, open_form, highlight, show_diff, filter_table), and only that tab runs them. A
//! pairing belongs to the person signed in in the tab and to their session: another person, a
//! later sign-in or a bearer token cannot listen on it, and it ends with the session, a
//! disconnect, or the next tab the person pairs with the same connector. Nothing of the session
//! (cookie, token) is kept here or reaches the connector; a tab hears only actions.
//!
//! In memory, like the live views (`crate::live`): a Portal restart ends every pairing and the
//! tab offers to connect again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use serde_json::Value;
use tokio::sync::broadcast;

/// Actions a tab may fall behind before it misses some.
const BUFFER: usize = 32;

/// Seconds past its end a pairing still takes the same session's reconnect: an edge token
/// lives minutes, the tab's stream ends with it and comes back on the refreshed token, which
/// renews the pairing (JC2-19).
const RENEW_GRACE: i64 = 120;

/// One paired tab of one connector.
struct Pairing {
    tab: String,
    owner: String,
    /// The session's `issued_at`: a later sign-in of the same person is another session.
    issued_at: i64,
    /// Unix seconds the session ends, and the pairing with it.
    expires_at: i64,
    sender: broadcast::Sender<Arc<Value>>,
}

/// Why an action reached no tab, in words the connector's model passes on to the person.
#[derive(Debug, PartialEq, Eq)]
pub enum Undelivered {
    /// No tab is paired with the connector, or the pairing ended.
    NotPaired,
    /// A tab is paired but none is listening now (closed, or reloading).
    NotListening,
}

impl Undelivered {
    pub fn reason(&self) -> &'static str {
        match self {
            Undelivered::NotPaired => {
                "no Portal tab is connected to this connector; ask the person to open My \
                 connectors in the Portal and press Connect this tab"
            }
            Undelivered::NotListening => {
                "the Portal tab connected to this connector is closed or reloading; ask the \
                 person to open it again"
            }
        }
    }
}

#[derive(Default)]
pub struct PairedTabs {
    by_connector: Mutex<HashMap<String, Pairing>>,
}

fn new_tab_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl PairedTabs {
    fn pairings(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pairing>> {
        self.by_connector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Pairs a new tab of the person's session with `connector`, ending the tab paired before.
    /// The caller has checked that the person owns the connector. Returns the tab's id.
    pub fn pair(&self, connector: &str, owner: &str, issued_at: i64, expires_at: i64) -> String {
        let tab = new_tab_id();
        let (sender, _) = broadcast::channel(BUFFER);
        self.pairings().insert(
            connector.to_owned(),
            Pairing {
                tab: tab.clone(),
                owner: owner.to_owned(),
                issued_at,
                expires_at,
                sender,
            },
        );
        tab
    }

    /// The actions of `tab`, when it is the connector's paired tab of this person's session.
    /// The stream ends when the pairing does (disconnect, a newer tab, the session's end). A
    /// listen of the same session on a refreshed token (`expires_at` later) renews the pairing,
    /// within `RENEW_GRACE` seconds of its end.
    pub fn listen(
        &self,
        connector: &str,
        tab: &str,
        owner: &str,
        issued_at: i64,
        expires_at: i64,
        now: i64,
    ) -> Option<(broadcast::Receiver<Arc<Value>>, i64)> {
        let mut pairings = self.pairings();
        let pairing = pairings.get_mut(connector)?;
        if pairing.expires_at.saturating_add(RENEW_GRACE) <= now {
            pairings.remove(connector);
            return None;
        }
        if pairing.tab != tab || pairing.owner != owner || pairing.issued_at != issued_at {
            return None;
        }
        pairing.expires_at = pairing.expires_at.max(expires_at);
        (pairing.expires_at > now).then(|| (pairing.sender.subscribe(), pairing.expires_at))
    }

    /// Ends the person's pairing of `connector`; `false` when they had none.
    pub fn unpair(&self, connector: &str, owner: &str) -> bool {
        let mut pairings = self.pairings();
        if pairings.get(connector).is_some_and(|p| p.owner == owner) {
            pairings.remove(connector);
            return true;
        }
        false
    }

    /// Whether the person has a live pairing of `connector`: what makes its UI tools listed.
    pub fn is_paired(&self, connector: &str, owner: &str, now: i64) -> bool {
        self.pairings()
            .get(connector)
            .is_some_and(|p| p.owner == owner && p.expires_at > now)
    }

    /// Hands one checked action to the connector's paired tab of `owner`.
    pub fn deliver(
        &self,
        connector: &str,
        owner: &str,
        action: Value,
        now: i64,
    ) -> Result<(), Undelivered> {
        let pairings = self.pairings();
        let Some(pairing) = pairings.get(connector) else {
            return Err(Undelivered::NotPaired);
        };
        if pairing.owner != owner {
            return Err(Undelivered::NotPaired);
        }
        // Kept for the same session's reconnect (`listen`), which may still renew it.
        if pairing.expires_at <= now {
            return Err(Undelivered::NotPaired);
        }
        pairing
            .sender
            .send(Arc::new(action))
            .map(|_| ())
            .map_err(|_| Undelivered::NotListening)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_paired_tab_of_the_owners_session_hears_an_action() {
        let tabs = PairedTabs::default();
        assert_eq!(
            tabs.deliver("c1", "ana", json!({}), 10),
            Err(Undelivered::NotPaired)
        );
        let tab = tabs.pair("c1", "ana", 5, 100);
        assert_eq!(tab.len(), 32);
        assert!(tabs.is_paired("c1", "ana", 10));
        assert!(!tabs.is_paired("c1", "ben", 10));
        // Paired but nobody listening: the agent hears that, not "done".
        assert_eq!(
            tabs.deliver("c1", "ana", json!({}), 10),
            Err(Undelivered::NotListening)
        );
        // Another person, another session of the same person, another tab id: no stream.
        assert!(tabs.listen("c1", &tab, "ben", 5, 100, 10).is_none());
        assert!(tabs.listen("c1", &tab, "ana", 6, 100, 10).is_none());
        assert!(tabs
            .listen("c1", "0".repeat(32).as_str(), "ana", 5, 100, 10)
            .is_none());
        let (mut rx, until) = tabs.listen("c1", &tab, "ana", 5, 100, 10).expect("paired");
        assert_eq!(until, 100);
        // Another person's connector id never reaches ana's tab.
        assert_eq!(
            tabs.deliver("c1", "ben", json!({"x": 1}), 10),
            Err(Undelivered::NotPaired)
        );
        tabs.deliver("c1", "ana", json!({"x": 2}), 10)
            .expect("delivered");
        assert_eq!(*rx.try_recv().expect("action"), json!({"x": 2}));
    }

    #[test]
    fn a_pairing_ends_with_a_disconnect_a_newer_tab_or_the_session() {
        let tabs = PairedTabs::default();
        let first = tabs.pair("c1", "ana", 5, 100);
        let (mut old, _) = tabs
            .listen("c1", &first, "ana", 5, 100, 10)
            .expect("paired");
        let second = tabs.pair("c1", "ana", 5, 100);
        assert!(tabs.listen("c1", &first, "ana", 5, 100, 10).is_none());
        assert_eq!(old.try_recv(), Err(broadcast::error::TryRecvError::Closed));
        assert!(!tabs.unpair("c1", "ben"));
        assert!(tabs.unpair("c1", "ana"));
        assert!(tabs.listen("c1", &second, "ana", 5, 100, 10).is_none());
        tabs.pair("c1", "ana", 5, 100);
        assert_eq!(
            tabs.deliver("c1", "ana", json!({}), 100),
            Err(Undelivered::NotPaired)
        );
        assert!(!tabs.is_paired("c1", "ana", 100));
    }

    /// JC2-19: an edge token lives minutes; the tab comes back on the refreshed token of the
    /// same session and keeps its pairing, while another session or a late return does not.
    #[test]
    fn the_same_session_on_a_refreshed_token_keeps_its_pairing() {
        let tabs = PairedTabs::default();
        let tab = tabs.pair("c1", "ana", 5, 100);
        assert!(!tabs.is_paired("c1", "ana", 105));
        assert_eq!(
            tabs.deliver("c1", "ana", json!({}), 105),
            Err(Undelivered::NotPaired)
        );
        assert!(tabs.listen("c1", &tab, "ana", 6, 400, 105).is_none());
        let (_, until) = tabs
            .listen("c1", &tab, "ana", 5, 400, 105)
            .expect("renewed");
        assert_eq!(until, 400);
        assert!(tabs.is_paired("c1", "ana", 200));
        let late = tabs.pair("c2", "ana", 5, 100);
        assert!(tabs
            .listen("c2", &late, "ana", 5, 600, 100 + RENEW_GRACE)
            .is_none());
    }
}
