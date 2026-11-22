// SPDX-License-Identifier: EUPL-1.2
//! The store of a surface's write confirmations (T-7507), reached without
//! the broker's state: every call runs in the one Tenant its handle names.

use antares_api::confirmations::{Caps, Confirmation, Subject, Taken};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use std::time::Duration;

/// The confirmations of every Tenant, for the admin routes that find a
/// question by the Tenant its id names.
#[derive(Clone)]
pub struct Confirmations {
    state: AppState,
}

impl Confirmations {
    /// The confirmations in `state`'s store.
    #[must_use]
    pub fn new(state: &AppState) -> Self {
        Self {
            state: state.clone(),
        }
    }

    /// The confirmations of `tenant`.
    #[must_use]
    pub fn of(&self, tenant: TenantId) -> TenantConfirmations {
        TenantConfirmations {
            state: self.state.clone(),
            tenant,
        }
    }
}

/// The confirmations of one Tenant.
#[derive(Clone)]
pub struct TenantConfirmations {
    state: AppState,
    tenant: TenantId,
}

impl TenantConfirmations {
    /// The Tenant every call runs in.
    #[must_use]
    pub fn tenant(&self) -> &str {
        self.tenant.as_str()
    }

    /// Open `question` as `id`; `false` when it is taken or `caps` are full.
    ///
    /// # Errors
    /// The store's.
    pub async fn ask(
        &self,
        id: &str,
        question: &Confirmation,
        caps: Caps,
    ) -> Result<bool, NgsiError> {
        self.state
            .store
            .confirmation_ask(&self.tenant, id, question, caps)
            .await
    }

    /// The open question `id`.
    ///
    /// # Errors
    /// The store's.
    pub async fn find(&self, id: &str) -> Result<Option<Confirmation>, NgsiError> {
        self.state.store.confirmation_find(&self.tenant, id).await
    }

    /// Record the decision on the open question `id`.
    ///
    /// # Errors
    /// The store's.
    pub async fn decide(&self, id: &str, accept: bool) -> Result<Option<Confirmation>, NgsiError> {
        self.state
            .store
            .confirmation_decide(&self.tenant, id, accept)
            .await
    }

    /// What `subject`'s retry finds; a decided question is gone with it.
    ///
    /// # Errors
    /// The store's.
    pub async fn take(&self, id: &str, subject: &Subject) -> Result<Taken, NgsiError> {
        self.state
            .store
            .confirmation_take(&self.tenant, id, subject)
            .await
    }

    /// Spend `id` once; `false` when it was spent or `caps` are full.
    ///
    /// # Errors
    /// The store's.
    pub async fn spend(
        &self,
        id: &str,
        subject: &Subject,
        ttl: Duration,
        caps: Caps,
    ) -> Result<bool, NgsiError> {
        self.state
            .store
            .confirmation_spend(&self.tenant, id, subject, ttl, caps)
            .await
    }
}
