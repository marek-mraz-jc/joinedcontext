//! What the Portal checks of a `kind: Portal` beyond the manifest itself (ODP-08, ODP-09): the
//! Organization it names exists, its `spec.path` is unique in that Organization and never
//! moves to another one, its `spec.host` is one the operator listed for the Organization, and
//! the deployment holds no more than `portals.max` portals. jc fills the settings from its
//! `[[organizations]]` (`hosts.portals`) and `[dataroom] portals_max`.

use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::store::{ListOptions, Mirror};
use jc_core::kinds::{is_portal_host, PortalSpec};
use std::collections::BTreeMap;

/// The deployment's word on portals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortalSettings {
    /// Each Organization's listed portal hosts (`JC_PORTAL_PORTAL_HOSTS`, `org=host` pairs
    /// separated by commas, e.g. `ba=mobility.ba.sk,ba=skoly.ba.sk`).
    pub hosts: BTreeMap<String, Vec<String>>,
    /// `portals.max` (`JC_PORTAL_PORTALS_MAX`); `None`: no cap known here.
    pub max: Option<usize>,
    /// The deployment's harvest allow-list (`JC_PORTAL_HARVEST_HOSTS`, hosts separated by
    /// commas): each `allowedHosts` entry of a source is one of these (ODP-10).
    pub harvest_hosts: Vec<String>,
}

/// `JC_PORTAL_PORTAL_HOSTS` and `JC_PORTAL_PORTALS_MAX`, checked: a plain host each, no host
/// listed twice, a positive cap.
pub fn parse(hosts: &str, max: Option<&str>, harvest: &str) -> Result<PortalSettings, String> {
    let mut out = PortalSettings::default();
    for host in harvest.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        let host = host.to_ascii_lowercase();
        if !is_portal_host(&host) {
            return Err(format!("harvest host '{host}' is not a plain host name"));
        }
        out.harvest_hosts.push(host);
    }
    let mut seen = Vec::new();
    for pair in hosts.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (org, host) = pair
            .split_once('=')
            .ok_or_else(|| format!("'{pair}' is not organization=host"))?;
        let (org, host) = (org.trim(), host.trim().to_ascii_lowercase());
        if !is_portal_host(&host) {
            return Err(format!("{org}: '{host}' is not a plain host name"));
        }
        if seen.contains(&host) {
            return Err(format!("{org}: host {host} is listed twice"));
        }
        seen.push(host.clone());
        out.hosts.entry(org.to_owned()).or_default().push(host);
    }
    out.max = match max.map(str::trim).filter(|m| !m.is_empty()) {
        None => None,
        Some(m) => Some(
            m.parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("'{m}' is not a positive number"))?,
        ),
    };
    Ok(out)
}

fn refused(field: &str, line: String) -> ApiError {
    let line = format!("{field}: {line}");
    ApiError::Unprocessable {
        detail: line.clone(),
        errors: vec![line],
    }
}

/// Every Portal manifest in `mirror` with its spec, but `except` (the one being judged).
fn others(mirror: &Mirror, except: &str) -> Vec<(String, PortalSpec)> {
    mirror
        .list(ORG_NAMESPACE, "Portal", &ListOptions::default())
        .items
        .into_iter()
        .filter(|e| e.metadata.name != except)
        .filter_map(|e| Some((e.metadata.name, serde_json::from_value(e.spec).ok()?)))
        .collect()
}

/// The judge's check of a Portal manifest (see the module docs); `settings` `None` (a load of
/// `main` without the deployment's word) checks what the mirror alone decides.
pub fn check(
    mirror: &Mirror,
    settings: Option<&PortalSettings>,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if envelope.kind != "Portal" {
        return Ok(());
    }
    if envelope.metadata.namespace.as_deref() != Some(ORG_NAMESPACE) {
        return Err(refused(
            "metadata.namespace",
            "a Portal belongs to an Organization: namespace org (ODP-08)".into(),
        ));
    }
    let Ok(spec) = serde_json::from_value::<PortalSpec>(envelope.spec.clone()) else {
        return Ok(()); // The kind's own parse names the field.
    };
    let org = &spec.organization_ref;
    if mirror.get(ORG_NAMESPACE, "Organization", org).is_none() {
        return Err(refused(
            "spec.organizationRef",
            format!("no Organization {org}"),
        ));
    }
    let name = &envelope.metadata.name;
    if let Some(held) = mirror.get(ORG_NAMESPACE, "Portal", name) {
        let held = serde_json::from_value::<PortalSpec>(held.spec).ok();
        if held.as_ref().is_some_and(|h| &h.organization_ref != org) {
            return Err(refused(
                "spec.organizationRef",
                "a portal never moves to another Organization; make a new one there".into(),
            ));
        }
    }
    let all = others(mirror, name);
    if all
        .iter()
        .any(|(_, o)| &o.organization_ref == org && o.path == spec.path)
    {
        return Err(refused(
            "spec.path",
            format!(
                "{org} has a portal at path {} already; choose another, e.g. mobility",
                spec.path
            ),
        ));
    }
    for (i, source) in spec.sources.iter().enumerate() {
        if let Some(project) = source.projects.iter().find(|p| !project_of(mirror, p, org)) {
            return Err(refused(
                &format!("spec.sources[{i}].projects"),
                format!(
                    "{project} is no project of {org}; an endpoints source lists only this                      Organization's published Endpoints (ODP-12)"
                ),
            ));
        }
    }
    let Some(settings) = settings else {
        return Ok(());
    };
    if let Some(host) = &spec.host {
        let listed = settings.hosts.get(org).is_some_and(|h| h.contains(host));
        if !listed {
            return Err(refused(
                "spec.host",
                format!(
                    "host {host} is not listed for this Organization; ask the operator to list \
                     it, or leave host out for the address /{org}/opendata/-/{}/",
                    spec.path
                ),
            ));
        }
    }
    for (i, source) in spec.sources.iter().enumerate() {
        if let Some(host) = source.allowed_hosts.iter().find(|h| {
            !settings
                .harvest_hosts
                .iter()
                .any(|a| a.eq_ignore_ascii_case(h))
        }) {
            return Err(refused(
                &format!("spec.sources[{i}].allowedHosts"),
                format!(
                    "{host} is not in the deployment's harvest allow-list; ask the operator to \
                     add it"
                ),
            ));
        }
    }
    if let Some(max) = settings.max {
        if count_after(mirror, name, &spec) > max {
            return Err(refused(
                "metadata.name",
                format!(
                    "portals.max ({max}) reached: delete a portal you no longer need, or ask the \
                     operator to raise the limit"
                ),
            ));
        }
    }
    Ok(())
}

/// Whether `project` is a project of Organization `org`: its `Project` manifest names `org`,
/// or names no Organization (a single-Organization installation).
fn project_of(mirror: &Mirror, project: &str, org: &str) -> bool {
    mirror
        .get(ORG_NAMESPACE, "Project", project)
        .is_some_and(|p| {
            p.spec
                .get("organizationRef")
                .and_then(crate::resource::reference_name)
                .is_none_or(|o| o == org)
        })
}

/// How many portals the deployment holds with `spec` written as `name`: every Organization's
/// `default` (always there, manifest or not) and every further portal.
fn count_after(mirror: &Mirror, name: &str, spec: &PortalSpec) -> usize {
    let organizations = mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .len();
    let further = others(mirror, name)
        .iter()
        .filter(|(_, o)| !o.is_default())
        .count();
    organizations + further + usize::from(!spec.is_default())
}

/// A deletion the propose door refuses: an Organization's `default` portal (ODP-09).
pub fn deletion(mirror: &Mirror, kind: &str, namespace: &str, name: &str) -> Result<(), ApiError> {
    if kind != "Portal" || namespace != ORG_NAMESPACE {
        return Ok(());
    }
    let default = mirror
        .get(ORG_NAMESPACE, "Portal", name)
        .and_then(|e| serde_json::from_value::<PortalSpec>(e.spec).ok())
        .is_some_and(|s| s.is_default());
    if default {
        return Err(refused(
            "metadata.name",
            "the default portal cannot be deleted; edit it instead (ODP-09)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: jc_core::ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        }
    }

    fn portal(name: &str, org: &str, path: &str, host: Option<&str>) -> ResourceEnvelope {
        let mut spec = json!({ "organizationRef": org, "path": path });
        if let Some(host) = host {
            spec["host"] = json!(host);
        }
        envelope("Portal", name, spec)
    }

    fn mirror() -> Mirror {
        let mirror = Mirror::new();
        for org in ["ba", "ke"] {
            mirror.upsert(envelope(
                "Organization",
                org,
                json!({ "domain": format!("{org}.sk") }),
            ));
        }
        mirror
    }

    fn settings(max: usize) -> PortalSettings {
        parse(
            "ba=mobility.ba.sk",
            Some(&max.to_string()),
            "data.kosice.sk",
        )
        .expect("settings")
    }

    fn field(err: ApiError) -> String {
        match err {
            ApiError::Unprocessable { detail, .. } => detail,
            other => panic!("not a 422: {other:?}"),
        }
    }

    #[test]
    fn a_host_not_listed_for_the_organization_is_refused() {
        let mirror = mirror();
        let s = settings(10);
        check(
            &mirror,
            Some(&s),
            &portal("ba-mobility", "ba", "mobility", Some("mobility.ba.sk")),
        )
        .expect("a listed host");
        for (org, host) in [("ba", "evil.example.com"), ("ke", "mobility.ba.sk")] {
            let detail = field(
                check(&mirror, Some(&s), &portal("p", org, "mobility", Some(host))).unwrap_err(),
            );
            assert!(
                detail.starts_with("spec.host: host ") && detail.contains("not listed"),
                "{org} {host}: {detail}"
            );
        }
    }

    #[test]
    fn portals_max_refuses_the_next_portal_with_its_name() {
        let mirror = mirror();
        // Two Organizations hold their `default` already: two of three.
        let s = settings(3);
        mirror.upsert(portal("ba-mobility", "ba", "mobility", None));
        let detail =
            field(check(&mirror, Some(&s), &portal("ke-skoly", "ke", "skoly", None)).unwrap_err());
        assert!(detail.contains("portals.max (3) reached"), "{detail}");
        check(
            &mirror,
            Some(&s),
            &portal("ba-mobility", "ba", "mobility", None),
        )
        .expect("a change of a portal counts it once");
        check(
            &mirror,
            Some(&s),
            &portal("ke-default", "ke", "default", None),
        )
        .expect("a default is always there");
    }

    #[test]
    fn a_path_is_unique_in_its_organization_and_a_portal_never_moves() {
        let mirror = mirror();
        mirror.upsert(portal("ba-mobility", "ba", "mobility", None));
        let detail =
            field(check(&mirror, None, &portal("ba-two", "ba", "mobility", None)).unwrap_err());
        assert!(detail.starts_with("spec.path:"), "{detail}");
        check(
            &mirror,
            None,
            &portal("ke-mobility", "ke", "mobility", None),
        )
        .expect("another Organization's path of the same name");
        let detail = field(
            check(
                &mirror,
                None,
                &portal("ba-mobility", "ke", "mobility", None),
            )
            .unwrap_err(),
        );
        assert!(detail.starts_with("spec.organizationRef:"), "{detail}");
        let detail = field(check(&mirror, None, &portal("x", "praha", "x", None)).unwrap_err());
        assert!(detail.contains("no Organization praha"), "{detail}");
    }

    #[test]
    fn deleting_the_default_portal_is_refused() {
        let mirror = mirror();
        mirror.upsert(portal("ba-default", "ba", "default", None));
        mirror.upsert(portal("ba-mobility", "ba", "mobility", None));
        let detail = field(deletion(&mirror, "Portal", ORG_NAMESPACE, "ba-default").unwrap_err());
        assert!(
            detail.contains("the default portal cannot be deleted"),
            "{detail}"
        );
        deletion(&mirror, "Portal", ORG_NAMESPACE, "ba-mobility").expect("a further portal");
    }

    #[test]
    fn the_settings_refuse_a_url_a_twice_listed_host_and_a_zero_cap() {
        assert!(parse("ba=https://x.sk", None, "").is_err());
        assert!(parse("ba=x.sk,ke=x.sk", None, "").is_err());
        assert!(parse("", Some("0"), "").is_err());
        assert!(parse("", None, "http://x.sk").is_err());
        assert_eq!(parse(" ", None, " ").unwrap(), PortalSettings::default());
    }

    #[test]
    fn an_endpoints_source_naming_another_organizations_project_is_refused() {
        let mirror = mirror();
        for (project, org) in [("doprava", "ba"), ("kosice-air", "ke")] {
            mirror.upsert(envelope(
                "Project",
                project,
                json!({ "organizationRef": { "name": org } }),
            ));
        }
        let listing = |project: &str| {
            let mut p = portal("ba-mobility", "ba", "mobility", None);
            p.spec["sources"] =
                json!([{ "name": "own", "type": "endpoints", "projects": [project] }]);
            p
        };
        check(&mirror, None, &listing("doprava")).expect("its own project");
        for foreign in ["kosice-air", "nowhere"] {
            let detail = field(check(&mirror, None, &listing(foreign)).unwrap_err());
            assert!(
                detail.starts_with(&format!(
                    "spec.sources[0].projects: {foreign} is no project of ba"
                )),
                "{detail}"
            );
        }
    }

    #[test]
    fn allowed_hosts_outside_the_deployment_list_are_refused() {
        let mirror = mirror();
        let s = settings(10);
        let source = |host: &str| {
            let mut p = portal("ba-region", "ba", "region", None);
            p.spec["sources"] = json!([{ "name": "ke", "type": "dcat-ap",
                "url": format!("https://{host}/catalog.ttl"), "allowedHosts": [host] }]);
            p
        };
        check(&mirror, Some(&s), &source("data.kosice.sk")).expect("a listed harvest host");
        let detail = field(check(&mirror, Some(&s), &source("10.0.0.1.nip.io")).unwrap_err());
        assert!(
            detail.starts_with("spec.sources[0].allowedHosts: 10.0.0.1.nip.io is not in"),
            "{detail}"
        );
    }
}
