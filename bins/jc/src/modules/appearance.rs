//! Each `[[organizations]]` entry's `Organization.spec.appearance` as the Portal of this process
//! mirrors it (UIX-16, T-6777): the open data portal and the sign-in pages follow it, jc's config
//! staying their default for every member the Organization leaves out.
//!
//! ponytail: polled every [`FOLLOW_EVERY`] from the one Portal of this process (as
//! [`super::PORTAL_INTERNAL`]); a dataroom or kaisatsu in a container without the Portal keeps
//! jc's config. Upgrade path: the Portal answers the appearance on its internal listener.

use jc_core::kinds::{OrganizationAppearance, OrganizationSpec};
use joinedcontext_portal::store::Mirror;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The Portal's mirror once its state is built (`portal::build`).
pub(crate) static PORTAL_MIRROR: OnceLock<Arc<Mirror>> = OnceLock::new();

/// How soon a saved appearance reaches the open data portal and the sign-in pages.
pub(crate) const FOLLOW_EVERY: Duration = Duration::from_secs(30);

/// The appearance the Organization `name` carries in `mirror`: `None` when it is not mirrored,
/// sets nothing, or fails jc-core's check (a manifest pushed past the Portal's judge).
pub(crate) fn in_mirror(mirror: &Mirror, name: &str) -> Option<OrganizationAppearance> {
    let envelope = mirror.get(
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        "Organization",
        name,
    )?;
    let spec: OrganizationSpec = serde_json::from_value(envelope.spec).ok()?;
    match spec.appearance.validate() {
        Ok(()) => Some(spec.appearance).filter(|a| !a.is_unset()),
        Err(e) => {
            tracing::warn!(organization = name, error = %e, "appearance refused; jc's config shows");
            None
        }
    }
}

/// Calls `apply` with each of `names` and its appearance at the first look and whenever it
/// changes, until `shutdown`.
pub(crate) fn follow<F, Fut>(
    names: Vec<String>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
    mut apply: F,
) where
    F: FnMut(String, Option<OrganizationAppearance>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    if names.is_empty() {
        return;
    }
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut seen: BTreeMap<String, Option<OrganizationAppearance>> = BTreeMap::new();
        loop {
            for name in &names {
                let now = PORTAL_MIRROR.get().and_then(|m| in_mirror(m, name));
                if seen.get(name) != Some(&now) {
                    apply(name.clone(), now.clone()).await;
                    seen.insert(name.clone(), now);
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(FOLLOW_EVERY) => {}
            }
        }
    });
}

/// `look`'s names as each page shows them: a short name the Organization leaves out is its
/// full name, as the Portal's branding takes it.
pub(crate) fn short_name(look: &OrganizationAppearance) -> Option<&String> {
    look.short_name.as_ref().or(look.instance_name.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jc_core::kinds::AppearanceColours;

    fn organization(
        appearance: serde_json::Value,
    ) -> joinedcontext_portal::resource::ResourceEnvelope {
        joinedcontext_portal::resource::ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: "Organization".to_owned(),
            metadata: jc_core::ObjectMeta::new(
                "bbsk",
                joinedcontext_portal::permissions::ORG_NAMESPACE,
            ),
            spec: serde_json::json!({ "domain": "bbsk.sk", "appearance": appearance }),
            status: None,
        }
    }

    #[test]
    fn a_mirrored_appearance_is_read_and_one_that_fails_the_check_is_not() {
        let mirror = Mirror::new();
        assert_eq!(in_mirror(&mirror, "bbsk"), None, "not mirrored");
        mirror.upsert(organization(serde_json::json!({
            "instanceName": "Banskobystrický kraj",
            "colours": { "primary": "#0b5394" },
        })));
        let look = in_mirror(&mirror, "bbsk").expect("its appearance");
        assert_eq!(look.instance_name.as_deref(), Some("Banskobystrický kraj"));
        assert_eq!(
            look.colours,
            AppearanceColours {
                primary: Some("#0b5394".into()),
                ..AppearanceColours::default()
            }
        );
        assert_eq!(
            short_name(&look).map(String::as_str),
            Some("Banskobystrický kraj")
        );
        mirror.upsert(organization(
            serde_json::json!({ "colours": { "primary": "red;}" } }),
        ));
        assert_eq!(
            in_mirror(&mirror, "bbsk"),
            None,
            "a CSS colour never reaches a page"
        );
        mirror.upsert(organization(serde_json::json!({})));
        assert_eq!(
            in_mirror(&mirror, "bbsk"),
            None,
            "an empty appearance is jc's config"
        );
    }
}
