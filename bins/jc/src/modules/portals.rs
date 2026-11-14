//! The reconciler of `kind: Portal` (ODP-08, ODP-09, ODP-11): each Portal manifest of an
//! `[[organizations]]` entry, as the Portal of this process mirrors `main`, becomes a dataroom
//! catalogue through the in-process [`CatalogueWriter`], made, changed and deleted while the
//! dataroom serves.
//!
//! Every Organization has the portal `default` whether or not a manifest names it: its
//! catalogue is the `[[organizations]]` entry's `catalogue_id` at the open data host, as before,
//! so no dataset URL or IRI moves; a `default` manifest only describes it. Further portals get a
//! catalogue id derived from the Organization and the portal's path
//! ([`CatalogueId::of_portal`]) and answer at `{opendata}/-/{path}/` or on a host the operator
//! listed in `hosts.portals`.
//!
//! ponytail: polled every [`super::appearance::FOLLOW_EVERY`] from the one Portal of this
//! process, as the appearance is; upgrade path: the Portal's reconciler calls the writer.

use dataroom_core::{CatalogueId, OrgId, Publisher, RecordState};
use dataroom_server::{CatalogueSpec, CatalogueWriter, PortalRoute};
use jc_core::kinds::PortalSpec;
use joinedcontext_portal::store::{ListOptions, Mirror};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// What jc's configuration says of one Organization's portals.
#[derive(Debug, Clone)]
pub(crate) struct OrganizationPortals {
    pub name: String,
    pub org_id: OrgId,
    /// The `default` portal's catalogue: the entry's `catalogue_id`.
    pub catalogue_id: CatalogueId,
    pub scheme: String,
    /// The open data host, without a port when it has none.
    pub opendata: String,
    /// `hosts.portals`: the only hosts a further portal may take.
    pub listed: Vec<String>,
}

/// Each Portal manifest of `mirror` that passes jc-core's check, by `metadata.name`.
pub(crate) fn in_mirror(mirror: &Mirror) -> BTreeMap<String, PortalSpec> {
    let items = mirror
        .list(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Portal",
            &ListOptions::default(),
        )
        .items;
    let mut out = BTreeMap::new();
    for envelope in items {
        let name = envelope.metadata.name.clone();
        let checked = serde_json::from_value::<PortalSpec>(envelope.spec)
            .map_err(|e| e.to_string())
            .and_then(|spec| spec.validate().map(|()| spec).map_err(|e| e.to_string()));
        match checked {
            Ok(spec) => {
                out.insert(name, spec);
            }
            Err(e) => tracing::warn!(portal = %name, error = %e, "Portal refused; not served"),
        }
    }
    out
}

/// The catalogue `portal` asks for, or why it is not served: an Organization this jc does not
/// serve, or a host the operator did not list for it (the Portal's judge refuses both first).
pub(crate) fn catalogue_of(
    portal: &PortalSpec,
    orgs: &[OrganizationPortals],
) -> Result<CatalogueSpec, String> {
    let org = orgs
        .iter()
        .find(|o| o.name == portal.organization_ref)
        .ok_or_else(|| format!("no [[organizations]] {}", portal.organization_ref))?;
    let at = |host: &str| format!("{}://{host}", org.scheme);
    let (id, host, base, route) = if portal.is_default() {
        (org.catalogue_id, org.opendata.clone(), at(&org.opendata), None)
    } else {
        let id = CatalogueId::of_portal(org.org_id, &portal.path);
        let route = PortalRoute {
            organization: org.name.clone(),
            path: portal.path.clone(),
            host: portal.host.clone(),
            redirect: portal.slug == jc_core::kinds::SlugAddress::Redirect,
        };
        match &portal.host {
            Some(host) if org.listed.contains(host) => (id, host.clone(), at(host), Some(route)),
            Some(host) => {
                return Err(format!(
                    "spec.host {host} is not listed for the Organization {}",
                    org.name
                ));
            }
            None => (
                id,
                dataroom_api::synthetic_host(&org.name, &portal.path),
                format!("{}/-/{}", at(&org.opendata), portal.path),
                Some(route),
            ),
        }
    };
    let now = chrono::Utc::now();
    Ok(CatalogueSpec {
        org_id: org.org_id,
        id,
        name: portal.path.clone(),
        host,
        base: Some(base),
        organization: portal.is_default().then(|| org.name.clone()),
        publisher: Publisher {
            org_id: org.org_id,
            catalogue_id: id,
            id: String::new(),
            name: org.name.clone(),
            title: BTreeMap::from([("en".to_owned(), org.name.clone())]),
            description: BTreeMap::new(),
            uri: format!("{}/publisher/{}", at(&org.opendata), org.name),
            publisher_type: None,
            identifier: None,
            email: None,
            homepage: None,
            logo_url: None,
            contact_point: None,
            state: RecordState::Active,
            merged_into: None,
            editor_grants: Vec::new(),
            badge_initials: None,
            badge_colour: None,
            declared_by_instance: None,
            created: now,
            modified: now,
        },
        // The Portal publishes Endpoints into an Organization's `default` with the token jc
        // bound at start; a further portal's datasets come from its sources (ODP-10).
        publish_token: None,
        portal: route,
    })
}

/// The catalogue a further portal holds, whatever its host: what a delete removes.
fn further_catalogue(portal: &PortalSpec, orgs: &[OrganizationPortals]) -> Option<(OrgId, CatalogueId)> {
    let org = orgs.iter().find(|o| o.name == portal.organization_ref)?;
    (!portal.is_default()).then(|| (org.org_id, CatalogueId::of_portal(org.org_id, &portal.path)))
}

/// Follows the Portal manifests until `shutdown`: a new or changed further portal is written,
/// one that left `main` is deleted (its addresses answer 410). `default` is never deleted.
pub(crate) fn follow(
    writer: CatalogueWriter,
    orgs: Vec<OrganizationPortals>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    if orgs.is_empty() {
        return;
    }
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut served: BTreeMap<String, PortalSpec> = BTreeMap::new();
        loop {
            if let Some(state) = super::PORTAL_STATE.get() {
                apply(&writer, &orgs, &mut served, in_mirror(&state.mirror)).await;
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(super::appearance::FOLLOW_EVERY) => {}
            }
        }
    });
}

/// One round of [`follow`]: `served` is what the last round wrote.
pub(crate) async fn apply(
    writer: &CatalogueWriter,
    orgs: &[OrganizationPortals],
    served: &mut BTreeMap<String, PortalSpec>,
    now: BTreeMap<String, PortalSpec>,
) {
    for (name, gone) in served.clone() {
        if now.contains_key(&name) || gone.is_default() {
            continue;
        }
        if let Some((org, id)) = further_catalogue(&gone, orgs) {
            match writer.delete(org, id).await {
                Ok(_) => tracing::info!(portal = %name, "portal deleted"),
                Err(e) => {
                    tracing::warn!(portal = %name, error = %e, "portal not deleted; next round");
                    continue;
                }
            }
        }
        served.remove(&name);
    }
    for (name, portal) in now {
        if served.get(&name) == Some(&portal) {
            continue;
        }
        // `default` keeps the catalogue, grants and publish token jc bound at start.
        if portal.is_default() {
            served.insert(name, portal);
            continue;
        }
        // A portal moved to another path leaves its old catalogue.
        if let Some(old) = served.get(&name).filter(|old| old.path != portal.path)
            && let Some((org, id)) = further_catalogue(old, orgs)
            && let Err(e) = writer.delete(org, id).await
        {
            tracing::warn!(portal = %name, error = %e, "old path not deleted; next round");
            continue;
        }
        let written = match catalogue_of(&portal, orgs) {
            Ok(spec) => writer.upsert(spec).await.map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        match written {
            Ok(_) => {
                tracing::info!(portal = %name, path = %portal.path, "portal served");
                served.insert(name, portal);
            }
            Err(e) => tracing::warn!(portal = %name, error = %e, "portal not served"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city() -> Vec<OrganizationPortals> {
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        vec![OrganizationPortals {
            name: "city".into(),
            org_id: OrgId::of_organization(installation, "city"),
            catalogue_id: CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c1")
                .unwrap(),
            scheme: "https".into(),
            opendata: "data.city.test".into(),
            listed: vec!["mobility.city.test".into()],
        }]
    }

    fn portal(path: &str, host: Option<&str>) -> PortalSpec {
        serde_json::from_value(serde_json::json!({
            "organizationRef": "city", "path": path, "host": host,
        }))
        .unwrap()
    }

    #[test]
    fn the_default_portal_keeps_its_catalogue_id() {
        let orgs = city();
        let spec = catalogue_of(&portal("default", None), &orgs).unwrap();
        assert_eq!(spec.id, orgs[0].catalogue_id, "the configured catalogue_id");
        assert_eq!(spec.host, "data.city.test");
        assert_eq!(spec.base.as_deref(), Some("https://data.city.test"));
        assert_eq!(spec.organization.as_deref(), Some("city"));
        assert!(spec.portal.is_none() && spec.publish_token.is_none());
    }

    #[test]
    fn a_further_portal_gets_a_stable_catalogue_and_its_slug_address() {
        let orgs = city();
        let spec = catalogue_of(&portal("mobility", None), &orgs).unwrap();
        assert_eq!(spec.id, CatalogueId::of_portal(orgs[0].org_id, "mobility"));
        assert_ne!(spec.id, orgs[0].catalogue_id);
        assert_eq!(spec.host, "mobility.city.portal.invalid");
        assert_eq!(spec.base.as_deref(), Some("https://data.city.test/-/mobility"));
        assert!(spec.organization.is_none(), "never the Organization's own catalogue");
        let own = catalogue_of(&portal("mobility", Some("mobility.city.test")), &orgs).unwrap();
        assert_eq!(own.id, spec.id, "a new address keeps the catalogue");
        assert_eq!(own.base.as_deref(), Some("https://mobility.city.test"));
    }

    #[test]
    fn a_host_not_listed_for_the_organization_is_refused() {
        let orgs = city();
        let err = catalogue_of(&portal("mobility", Some("data.town.test")), &orgs).unwrap_err();
        assert!(err.contains("not listed"), "{err}");
        let mut other = portal("mobility", None);
        other.organization_ref = "town".into();
        assert!(catalogue_of(&other, &orgs).is_err(), "an Organization jc does not serve");
    }
}
