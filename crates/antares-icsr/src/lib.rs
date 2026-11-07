// SPDX-License-Identifier: EUPL-1.2
//! Internal context sources (ADR-0025, feature `internal-csr`): a Context
//! Source Registration whose endpoint is this broker is served in process,
//! as a request to the registration's Tenant through the broker's own
//! router, decided by that Tenant's engine about the caller who asked, and
//! only when that Tenant granted it.
//!
//! - [`SelfUrls`] (rule 1): the URLs this deployment answers under, listed in
//!   [`SELF_URLS_ENV`] and compared exactly. A forward to anything else goes
//!   over HTTP, as before.
//! - [`ExportGrant`] (rule 5): the target Tenant's consent, written to
//!   `/q/grants/{target}/{source}`. It is checked when a registration that
//!   names this broker is stored (12.2.2, 12.2.4: 403 before anything is
//!   stored) and on every forward; a write needs `read-write` (C6).
//! - The forward (rules 2 to 4) runs through
//!   [`antares_ops::Ops::forward_in_process`]: the subject-only caller map,
//!   the depth bound (508 or 504, never HTTP), the response caps. A refusal
//!   reaches the merge as 403, which it reports as code 299 with no text of
//!   the target's (C10).
//! - The notifications of a 5.8.1.4 Subscription copy whose endpoint is the
//!   receiver path under a self URL (rule 7) are posted to that path through
//!   the router, with no subject, after the copy's Tenant's grant for the
//!   receiving Tenant is read again (C7). The receiving Tenant is the one
//!   the inbound index maps the copy's `subscriptionId` to.
//!
//! ```
//! # async fn demo() -> Result<(), String> {
//! let broker = antares_ops::test_kit::TestBroker::new().await?
//!     .with_forward_transport(antares_icsr::transport(antares_icsr::SelfUrls::parse("http://self.example")?))?;
//! # let _ = broker;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod grant;
mod self_url;

use antares_ops::{
    AdmitFuture, BuiltForward, ForwardAnswer, ForwardFuture, ForwardTransport, NotifyFuture,
    OperationId, Ops, TenantId,
};
use std::sync::Arc;

pub use grant::{held, routes, Access, ExportGrant, MAX_GRANT_BYTES};
pub use self_url::SelfUrls;

/// The comma-separated base URLs under which this deployment is reachable
/// (ADR-0025 rule 1). Empty or unset: every forward goes over HTTP.
pub const SELF_URLS_ENV: &str = "ANTARES_SELF_URLS";

/// What the composition root hands [`antares_ops::with_forward_transport`]:
/// the transport over `selfs`, with `/q/grants` added to the admin surface.
pub fn transport(selfs: SelfUrls) -> impl FnOnce(Ops) -> Arc<dyn ForwardTransport> {
    move |ops| {
        ops.add_admin(routes(&ops));
        Arc::new(Internal { ops, selfs })
    }
}

/// Where a 5.8.1.4 Subscription copy sends its notifications under this
/// broker's public URL (ADR-0019).
const RECEIVER: &str = "/ex/v1/remote-notify";

/// The transport: serves the forwards whose target is a self URL.
struct Internal {
    ops: Ops,
    selfs: SelfUrls,
}

impl ForwardTransport for Internal {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        let target = req.target()?;
        let self_url = self.selfs.matching(&target)?;
        Some(Box::pin(self.serve(req, target, self_url)))
    }

    fn admit<'a>(
        &'a self,
        source: &'a TenantId,
        endpoint: &'a str,
        target: Option<&'a str>,
    ) -> AdmitFuture<'a> {
        Box::pin(async move {
            if self.selfs.matching(endpoint).is_none() {
                return Ok(());
            }
            let target = target.unwrap_or(TenantId::DEFAULT);
            match held(&self.ops, target, source.as_str()).await {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(format!(
                    "the registration names this broker, and tenant {target:?} holds no export grant for tenant {:?}",
                    source.as_str()
                )),
                Err(e) => {
                    tracing::error!(error = e.as_str(), "export grants: the store failed");
                    Err("the export grant could not be read".into())
                }
            }
        })
    }

    fn notifies(&self, uri: &str) -> bool {
        let Some(base) = self.selfs.matching(uri) else {
            return false;
        };
        url::Url::parse(uri).is_ok_and(|u| String::from(u).strip_prefix(base) == Some(RECEIVER))
    }

    fn notify<'a>(
        &'a self,
        tenant: &'a TenantId,
        _uri: &'a str,
        notification: &'a serde_json::Value,
    ) -> NotifyFuture<'a> {
        Box::pin(self.deliver(tenant.as_str(), notification))
    }
}

/// The refusal of a forward: 403, which the merge reports as code 299 and
/// a proxied registration as 502, never with the reason (C10).
const REFUSED: ForwardAnswer = (403, serde_json::Value::Null, Vec::new());

impl Internal {
    /// Deliver `notification` of a copy in Tenant `target` to the Tenant
    /// that receives it, if `target` still grants that Tenant its data.
    async fn deliver(&self, target: &str, notification: &serde_json::Value) -> Result<(), String> {
        let copy = notification
            .get("subscriptionId")
            .and_then(serde_json::Value::as_str)
            .ok_or("a notification without subscriptionId")?;
        let source = self
            .ops
            .notification_receiver(copy)
            .await
            .ok_or("no distributed Subscription receives this notification")?;
        match held(&self.ops, target, &source).await {
            Ok(Some(g)) if g.allows(false) => {}
            Ok(_) => {
                tracing::info!(
                    subscription = copy,
                    source_tenant = source.as_str(),
                    target_tenant = target,
                    "an internal notification without a grant for it is not delivered"
                );
                return Err("no export grant".into());
            }
            Err(e) => {
                tracing::error!(error = e.as_str(), "export grants: the store failed");
                return Err("the export grant could not be read".into());
            }
        }
        tracing::info!(
            subscription = copy,
            source_tenant = source.as_str(),
            target_tenant = target,
            "internal notification"
        );
        self.ops.notify_in_process(RECEIVER, notification).await
    }

    /// Serve `req`, whose target `url` lies under `self_url`, in process if
    /// the target Tenant's grant allows it.
    async fn serve(&self, req: BuiltForward<'_>, url: String, self_url: &str) -> ForwardAnswer {
        let source = req.tenant().as_str().to_owned();
        let target = req
            .registration_tenant()
            .unwrap_or(TenantId::DEFAULT)
            .to_owned();
        let rest = url.get(self_url.len()..).unwrap_or_default();
        let (path, query) = rest
            .split_once('?')
            .map_or((rest, None), |(p, q)| (p, Some(q)));
        // The forwards of Append and Update Attributes end in `/attrs/` (the
        // form the ETSI suite's sources take): the same operation.
        let named = path.strip_suffix('/').unwrap_or(path);
        let Some(op) = OperationId::of(req.method().as_str(), named, query) else {
            tracing::warn!(
                registration = req.registration_id(),
                source_tenant = source.as_str(),
                method = req.method().as_str(),
                "an internal forward that is no NGSI-LD operation is not made"
            );
            return REFUSED;
        };
        let write = op.gate_clause().is_some_and(|c| c.starts_with("5.6."));
        let grant = match held(&self.ops, &target, &source).await {
            Ok(Some(g)) if g.allows(write) => g,
            Ok(found) => {
                tracing::info!(
                    registration = req.registration_id(),
                    source_tenant = source.as_str(),
                    target_tenant = target.as_str(),
                    operation = op.id(),
                    granted = found.is_some(),
                    "an internal forward without a grant for it is not made"
                );
                return REFUSED;
            }
            Err(e) => {
                tracing::error!(error = e.as_str(), "export grants: the store failed");
                return REFUSED;
            }
        };
        // C9: the target's log names the registration and the source, never
        // the subject.
        tracing::info!(
            registration = req.registration_id(),
            source_tenant = source.as_str(),
            target_tenant = target.as_str(),
            operation = op.id(),
            "internal context source forward"
        );
        match grant.view.as_deref() {
            None => self.ops.forward_in_process(req, self_url).await,
            Some(id) => match self.ops.view(id) {
                Some(view) if view.tenant() == target && view.allows(op) => {
                    view.forward_in_process(req, self_url).await
                }
                _ => {
                    tracing::info!(
                        registration = req.registration_id(),
                        view = id,
                        operation = op.id(),
                        "the grant's view is not loaded for its tenant or does not allow the forward"
                    );
                    REFUSED
                }
            },
        }
    }
}
