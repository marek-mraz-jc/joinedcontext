//! Each `[[organizations]]` entry's `Organization.spec.appearance` as the Portal of this process
//! mirrors it (UIX-16, T-6777): the open data portal and the sign-in pages follow it, jc's config
//! staying their default for every member the Organization leaves out.
//!
//! ponytail: polled every [`FOLLOW_EVERY`] from the one Portal of this process (as
//! [`super::PORTAL_INTERNAL`]); a dataroom or kaisatsu in a container without the Portal keeps
//! jc's config. Upgrade path: the Portal answers the appearance on its internal listener.

use jc_core::kinds::OrganizationAppearance;
use joinedcontext_portal::store::Mirror;
#[cfg(feature = "dataroom")]
use serde_json::Value;
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
    let look: OrganizationAppearance = match serde_json::from_value(
        envelope.spec.get("appearance")?.clone(),
    ) {
        Ok(look) => look,
        Err(e) => {
            tracing::warn!(organization = name, error = %e, "appearance unreadable; jc's config shows");
            return None;
        }
    };
    match look.validate() {
        Ok(()) => Some(look).filter(|a| !a.is_unset()),
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

/// The public Dashboards `organization`'s open data portal shows, as its catalogue home draws
/// them in `lang` (T-6780): each one's title, its pages' titles and the Endpoints it reads.
/// The Portal re-checks the Organization and `visibility: public` on each call.
#[cfg(feature = "dataroom")]
pub(crate) fn dashboard_cards(
    mirror: &Mirror,
    organization: &str,
    lang: &str,
) -> Vec<dataroom_api::DashboardCard> {
    joinedcontext_portal::api::organization_widgets::public_dashboards(mirror, organization)
        .into_iter()
        .map(|(project, dashboard)| {
            let spec = &dashboard.spec;
            let pages = spec.get("pages").and_then(Value::as_array);
            let pages = pages.map(Vec::as_slice).unwrap_or_default();
            let mut sources: Vec<String> = Vec::new();
            for page in pages {
                let layers = page.get("layers").and_then(Value::as_array);
                let from_layers = layers.into_iter().flatten().filter_map(|layer| {
                    let layer = mirror.get(&project, "Layer", layer.as_str()?)?;
                    Some(layer.spec.get("sourceEndpointRef")?.as_str()?.to_owned())
                });
                let widgets = page.get("widgets").and_then(Value::as_array);
                let from_widgets = widgets.into_iter().flatten().filter_map(|widget| {
                    Some(widget.get("endpointRef")?.as_str()?.to_owned())
                });
                for source in from_layers.chain(from_widgets) {
                    if !sources.contains(&source) {
                        sources.push(source);
                    }
                }
            }
            dataroom_api::DashboardCard {
                title: in_language(spec.get("title"), lang)
                    .unwrap_or_else(|| dashboard.metadata.name.clone()),
                pages: pages
                    .iter()
                    .filter_map(|page| page.get("title")?.as_str().map(str::to_owned))
                    .collect(),
                sources,
            }
        })
        .collect()
}

/// A title as one string, or the one of `lang` in a map per locale, else English, else any.
#[cfg(feature = "dataroom")]
fn in_language(text: Option<&Value>, lang: &str) -> Option<String> {
    match text? {
        Value::String(one) => Some(one.clone()),
        Value::Object(by) => [lang, "en"]
            .iter()
            .find_map(|l| by.get(*l))
            .or_else(|| by.values().next())
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
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

    /// T-6780: a catalogue home draws only its own Organization's public Dashboards, with the
    /// title in the page's language and the Endpoints its layers and widgets read.
    #[cfg(feature = "dataroom")]
    #[test]
    fn a_catalogue_home_draws_only_its_own_organizations_public_dashboards() {
        use joinedcontext_portal::permissions::ORG_NAMESPACE;
        use joinedcontext_portal::resource::ResourceEnvelope;
        let envelope = |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| {
            ResourceEnvelope {
                api_version: jc_core::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: jc_core::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            }
        };
        let mirror = Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            "bbsk",
            ORG_NAMESPACE,
            serde_json::json!({ "domain": "bbsk.sk",
                "appearance": { "opendataWidgets": ["doprava/verejny", "doprava/interny"] } }),
        ));
        mirror.upsert(envelope(
            "Project",
            "doprava",
            ORG_NAMESPACE,
            serde_json::json!({ "organizationRef": "bbsk" }),
        ));
        mirror.upsert(envelope(
            "Layer",
            "zastavky",
            "doprava",
            serde_json::json!({ "sourceEndpointRef": "mhd-public" }),
        ));
        mirror.upsert(envelope(
            "Dashboard",
            "verejny",
            "doprava",
            serde_json::json!({ "title": { "en": "Transport", "sk": "Doprava" },
                "visibility": "public",
                "pages": [
                    { "title": "Mapa", "layers": ["zastavky"] },
                    { "widgets": [{ "type": "bar-chart", "endpointRef": "nehody-public" },
                                  { "type": "grid", "endpointRef": "mhd-public" }] },
                ] }),
        ));
        mirror.upsert(envelope(
            "Dashboard",
            "interny",
            "doprava",
            serde_json::json!({ "title": "Interný", "visibility": "organization", "pages": [] }),
        ));
        assert_eq!(
            dashboard_cards(&mirror, "bbsk", "sk"),
            [dataroom_api::DashboardCard {
                title: "Doprava".into(),
                pages: vec!["Mapa".into()],
                sources: vec!["mhd-public".into(), "nehody-public".into()],
            }]
        );
        assert_eq!(dashboard_cards(&mirror, "bbsk", "hu")[0].title, "Transport");
        assert!(dashboard_cards(&mirror, "praha", "sk").is_empty());
    }
}
