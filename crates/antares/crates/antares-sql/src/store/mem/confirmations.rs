// SPDX-License-Identifier: EUPL-1.2
//! Write confirmations of the memory and file stores (T-7507). One process
//! holds these stores, so its memory is the shared state.
//! ponytail: the file store does not persist them, a restart asks again;
//! write them to redb when a file-mode deployment needs an approval to survive one.

use antares_model::TenantId;
use antares_store::confirmations::{Caps, Confirmation, Subject, Taken};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

struct Row {
    subject: Subject,
    until: Instant,
    /// `None` for a spent nonce.
    question: Option<Confirmation>,
}

impl Row {
    fn shown(&self, now: Instant) -> Option<Confirmation> {
        let question = self.question.clone().filter(|_| self.until > now)?;
        Some(Confirmation {
            left: self.until.saturating_duration_since(now),
            ..question
        })
    }
}

/// The rows, by Tenant and id.
#[derive(Default)]
pub(crate) struct Confirmations(Mutex<HashMap<(String, String), Row>>);

impl Confirmations {
    fn rows(&self) -> MutexGuard<'_, HashMap<(String, String), Row>> {
        // a holder that panicked left the map whole: each change is one step
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn put(&self, tenant: &TenantId, id: &str, new: Row, caps: Caps, now: Instant) -> bool {
        let mut rows = self.rows();
        rows.retain(|_, r| r.until > now);
        let key = (tenant.as_str().to_owned(), id.to_owned());
        if rows.contains_key(&key) {
            return false;
        }
        let (mut held, mut own) = (0u32, 0u32);
        for ((of, _), row) in rows.iter() {
            if *of == key.0 && row.question.is_some() == new.question.is_some() {
                held = held.saturating_add(1);
                own = own.saturating_add(u32::from(row.subject == new.subject));
            }
        }
        if held >= caps.tenant || own >= caps.subject {
            return false;
        }
        rows.insert(key, new);
        true
    }

    pub(crate) fn ask(&self, tenant: &TenantId, id: &str, q: &Confirmation, caps: Caps) -> bool {
        let now = Instant::now();
        let row = Row {
            subject: q.subject.clone(),
            until: now.checked_add(q.left).unwrap_or(now),
            question: Some(q.clone()),
        };
        self.put(tenant, id, row, caps, now)
    }

    pub(crate) fn spend(
        &self,
        tenant: &TenantId,
        id: &str,
        subject: &Subject,
        ttl: Duration,
        caps: Caps,
    ) -> bool {
        let now = Instant::now();
        let row = Row {
            subject: subject.clone(),
            until: now.checked_add(ttl).unwrap_or(now),
            question: None,
        };
        self.put(tenant, id, row, caps, now)
    }

    pub(crate) fn find(&self, tenant: &TenantId, id: &str) -> Option<Confirmation> {
        self.rows()
            .get(&(tenant.as_str().to_owned(), id.to_owned()))?
            .shown(Instant::now())
    }

    pub(crate) fn decide(&self, tenant: &TenantId, id: &str, accept: bool) -> Option<Confirmation> {
        let now = Instant::now();
        let mut rows = self.rows();
        let row = rows
            .get_mut(&(tenant.as_str().to_owned(), id.to_owned()))
            .filter(|r| r.until > now)?;
        row.question.as_mut()?.decision = Some(accept);
        row.shown(now)
    }

    pub(crate) fn take(&self, tenant: &TenantId, id: &str, subject: &Subject) -> Taken {
        let now = Instant::now();
        let key = (tenant.as_str().to_owned(), id.to_owned());
        let mut rows = self.rows();
        let Some(question) = rows
            .get(&key)
            .filter(|r| r.until > now && r.subject == *subject)
            .and_then(|r| r.question.as_ref())
        else {
            return Taken::Unknown;
        };
        let taken = match question.decision {
            None => return Taken::Waiting,
            Some(true) => Taken::Accepted,
            Some(false) => Taken::Declined,
        };
        rows.remove(&key);
        taken
    }

    pub(crate) fn sweep(&self, tenant: &TenantId) -> u64 {
        let now = Instant::now();
        let mut rows = self.rows();
        let before = rows.len();
        rows.retain(|(of, _), r| of != tenant.as_str() || r.until > now);
        before.saturating_sub(rows.len()) as u64
    }

    pub(crate) fn purge(&self, tenant: &TenantId) {
        self.rows().retain(|(of, _), _| of != tenant.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(name: &str) -> TenantId {
        TenantId::new(name).expect("tenant")
    }

    fn who(name: &str) -> Subject {
        vec![("x-jc-subject".to_owned(), name.as_bytes().to_vec())]
    }

    fn asked(name: &str, left: Duration) -> Confirmation {
        Confirmation {
            subject: who(name),
            view: "v".into(),
            tool: "delete_entity".into(),
            message: "Allow?".into(),
            decision: None,
            left,
        }
    }

    const CAPS: Caps = Caps {
        tenant: 2,
        subject: 1,
    };

    /// The Tenant cap and the subject cap each refuse the next question; a
    /// spent nonce counts apart from the questions, and another Tenant has
    /// caps of its own.
    #[test]
    fn caps_refuse_past_the_tenant_and_the_subject_limit() {
        let c = Confirmations::default();
        let (t, ten) = (tenant("caps"), Duration::from_secs(600));
        assert!(c.ask(&t, "a", &asked("alice", ten), CAPS));
        assert!(!c.ask(&t, "a", &asked("bob", ten), CAPS), "the id is taken");
        assert!(!c.ask(&t, "b", &asked("alice", ten), CAPS), "subject cap");
        assert!(c.ask(&t, "b", &asked("bob", ten), CAPS));
        assert!(!c.ask(&t, "c", &asked("carol", ten), CAPS), "tenant cap");
        assert!(
            c.spend(&t, "n", &who("alice"), ten, CAPS),
            "nonces count apart"
        );
        assert!(c.ask(&tenant("caps_other"), "a", &asked("alice", ten), CAPS));
    }

    /// A decline is taken once; a nonce is no question, so it cannot be
    /// decided or taken.
    #[test]
    fn a_decline_is_taken_once_and_a_nonce_is_never_a_question() {
        let c = Confirmations::default();
        let (t, ten) = (tenant("decline"), Duration::from_secs(600));
        assert!(c.ask(&t, "q", &asked("alice", ten), CAPS));
        assert_eq!(c.take(&t, "q", &who("alice")), Taken::Waiting);
        let shown = c.decide(&t, "q", false).expect("decided");
        assert_eq!(shown.decision, Some(false));
        assert_eq!(
            c.take(&t, "q", &who("bob")),
            Taken::Unknown,
            "another subject"
        );
        assert_eq!(c.take(&t, "q", &who("alice")), Taken::Declined);
        assert_eq!(c.take(&t, "q", &who("alice")), Taken::Unknown, "taken once");

        assert!(c.spend(&t, "n", &who("alice"), ten, CAPS));
        assert_eq!(c.find(&t, "n"), None);
        assert_eq!(c.decide(&t, "n", true), None);
        assert_eq!(c.take(&t, "n", &who("alice")), Taken::Unknown);
        assert!(!c.spend(&t, "n", &who("alice"), ten, CAPS), "spent once");
    }

    /// The sweep removes the Tenant's expired rows only; the purge removes
    /// every row of the Tenant and leaves another Tenant's. Each put first
    /// drops every expired row, so the expired one is written last.
    #[test]
    fn sweep_takes_expired_rows_and_purge_takes_the_tenant() {
        let c = Confirmations::default();
        let (t, other) = (tenant("sweep"), tenant("sweep_other"));
        let ten = Duration::from_secs(600);
        assert!(c.ask(&t, "live", &asked("bob", ten), CAPS));
        assert!(c.ask(&other, "live", &asked("alice", ten), CAPS));
        assert!(c.ask(&t, "old", &asked("alice", Duration::ZERO), CAPS));
        assert_eq!(c.find(&t, "old"), None, "an expired question is gone");
        assert_eq!(c.decide(&t, "old", true), None);
        assert_eq!(c.take(&t, "old", &who("alice")), Taken::Unknown);
        assert_eq!(c.sweep(&other), 0, "the other Tenant's row is live");
        assert_eq!(c.sweep(&t), 1);
        assert_eq!(c.sweep(&t), 0);
        assert!(c.find(&t, "live").is_some());
        c.purge(&t);
        assert_eq!(c.find(&t, "live"), None);
        assert!(
            c.find(&other, "live").is_some(),
            "the purge left the other Tenant"
        );
        // a ttl past the clock's range expires at once instead of panicking
        assert!(c.ask(&t, "max", &asked("alice", Duration::MAX), CAPS));
        assert_eq!(c.find(&t, "max"), None);
    }
}
