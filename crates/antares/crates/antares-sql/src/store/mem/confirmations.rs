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
