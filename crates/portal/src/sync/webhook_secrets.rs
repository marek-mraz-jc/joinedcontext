//! What authorises a run through the sync webhook route (MF-44, T-2297).
//!
//! `POST /api/v1/webhooks/sync/{project}/{name}` carries no session: the origin proves itself with
//! an HMAC-SHA256 of the request body. That signature covers the body and not the path, so one
//! secret shared between sources would let the origin of any one of them force a run of every
//! other, in projects it has no binding in. Each `SyncSource` therefore names its own, as
//! `spec.webhook.secretRef`, and the reconciler resolves it on each pass and holds it here.
//!
//! **Why it is held rather than resolved per request.** The door is unauthenticated until the
//! signature verifies, and resolving a reference means reading the repository this sync staged —
//! a tree fetched over the forge API. A resolution per request would hand an unauthenticated
//! caller a repository fetch per request, which is a worse door than the one this closes.
//!
//! The value never leaves this module: callers ask [`Accepted::verifies`], which answers yes or
//! no. A source the last pass resolved nothing for accepts nothing, and that is the same answer
//! as a bad signature — the route says `401` to both, so it tells nobody which sources exist
//! (PF-59, R20).

use std::collections::BTreeMap;
use std::sync::RwLock;

use jc_core::envelope::SecretRef;
use jc_core::kinds::sync::WebhookAuth;

/// The references of a `spec.webhook` block, current first: the order they are tried in.
pub fn references(block: &WebhookAuth) -> Vec<&SecretRef> {
    std::iter::once(&block.secret_ref)
        .chain(block.previous_secret_ref.as_ref())
        .collect()
}

/// The secrets each source accepts, by `(project, name)`.
///
/// Current first, then the one being retired: a rotation is written here in one commit and the
/// origin's own hook moved in another, and without that window every rotation is an outage
/// (T-0982).
#[derive(Debug, Default)]
pub struct Accepted {
    secrets: RwLock<BTreeMap<(String, String), Vec<String>>>,
}

impl Accepted {
    /// An empty set: nothing is reachable through the webhook until a pass has resolved it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `signature` is an HMAC one of this source's secrets makes over `body`.
    ///
    /// A source nothing is held for still costs one comparison, against a value no source has,
    /// so that "no such source" and "wrong signature" take the same work as well as the same
    /// answer.
    pub fn verifies(&self, project: &str, name: &str, body: &[u8], signature: &str) -> bool {
        let held = {
            let lock = self.secrets.read().unwrap_or_else(|p| p.into_inner());
            lock.get(&(project.to_owned(), name.to_owned())).cloned()
        };
        match held {
            Some(secrets) => secrets
                .iter()
                .any(|secret| crate::api::webhook::verify_signature(secret, body, signature)),
            None => {
                crate::api::webhook::verify_signature(NOBODYS_SECRET, body, signature);
                false
            }
        }
    }

    /// Replaces everything the previous pass resolved.
    ///
    /// Wholesale, like the mirror it is built beside: a source that was detached, or whose
    /// `spec.webhook` was removed, has to stop being reachable in the same pass that reads it.
    pub fn replace_all(&self, resolved: BTreeMap<(String, String), Vec<String>>) {
        let mut lock = self.secrets.write().unwrap_or_else(|p| p.into_inner());
        *lock = resolved;
    }

    /// How many sources are reachable through the webhook. For the log line of a pass and for
    /// tests; it counts sources and says nothing about any of them.
    pub fn len(&self) -> usize {
        self.secrets.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Whether no source is reachable through the webhook.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What a request for a source nothing is held for is compared against, so that door costs one
/// HMAC like every other and does not answer faster for a source that is not there.
///
/// A constant is enough: the comparison's result is thrown away and `verifies` answers `false`
/// whatever it says. Only the work it costs matters.
const NOBODYS_SECRET: &str = "no source of this platform is opened by this value";

#[cfg(test)]
mod tests {
    use super::*;

    /// The hex HMAC-SHA256 an origin presents, as Gitea computes it.
    fn signature(secret: &str, body: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        let mut mac = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).expect("any key");
        mac.update(body);
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn a_source_accepts_its_own_secret_and_the_one_it_is_retiring() {
        let held = Accepted::new();
        held.replace_all(BTreeMap::from([(
            ("ovzdusie".to_owned(), "regional".to_owned()),
            vec!["current".to_owned(), "retiring".to_owned()],
        )]));
        let body = br#"{"ref":"refs/heads/main"}"#;

        for secret in ["current", "retiring"] {
            assert!(
                held.verifies("ovzdusie", "regional", body, &signature(secret, body)),
                "{secret} was not accepted",
            );
        }
        assert!(
            !held.verifies("ovzdusie", "regional", body, &signature("stale", body)),
            "a secret nobody holds was accepted",
        );
        assert!(
            !held.verifies(
                "ovzdusie",
                "regional",
                b"another body",
                &signature("current", body)
            ),
            "the signature did not cover the body",
        );
    }

    #[test]
    fn a_secret_opens_the_source_it_belongs_to_and_no_other() {
        let held = Accepted::new();
        held.replace_all(BTreeMap::from([
            (
                ("ovzdusie".to_owned(), "regional".to_owned()),
                vec!["ours".to_owned()],
            ),
            (
                ("doprava".to_owned(), "regional".to_owned()),
                vec!["theirs".to_owned()],
            ),
        ]));
        let body = b"{}";
        let ours = signature("ours", body);

        assert!(held.verifies("ovzdusie", "regional", body, &ours));
        // The same signed body, on the other project's path: the whole point of a secret per
        // source is that this is refused (T-2297).
        assert!(!held.verifies("doprava", "regional", body, &ours));
        // And a source nothing is held for accepts nothing, whoever signed it.
        assert!(!held.verifies("ovzdusie", "gone", body, &ours));
        assert!(!held.verifies("ovzdusie", "gone", body, &signature("theirs", body)));
    }

    #[test]
    fn a_pass_that_resolves_nothing_closes_every_door_it_had_opened() {
        let held = Accepted::new();
        held.replace_all(BTreeMap::from([(
            ("ovzdusie".to_owned(), "regional".to_owned()),
            vec!["ours".to_owned()],
        )]));
        let body = b"{}";
        assert!(held.verifies("ovzdusie", "regional", body, &signature("ours", body)));

        // Detached, or its `spec.webhook` removed: the next pass holds nothing for it.
        held.replace_all(BTreeMap::new());
        assert!(held.is_empty());
        assert!(!held.verifies("ovzdusie", "regional", body, &signature("ours", body)));
    }
}
