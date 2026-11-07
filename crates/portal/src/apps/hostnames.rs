//! An App on names of its Organization's own domain (AP-172, ADR-N-037 §7).
//!
//! `spec.hostnames` lists them; jc-core checks their shape. What jc-core cannot know is checked
//! here, against the mirror: the name lies in the Organization's `spec.domain`, that domain is
//! `verified` (PF-41), it is not the installation's own zone, and no other App holds it. The
//! write doors refuse a name that fails; the reconciler routes only the names that pass, so a
//! domain that lapses loses its App routes at the next pass.

use serde_json::Value;

use crate::domain_verification::State;
use crate::store::Mirror;

/// The `spec.domain` of the Organization that owns `project`, while it is verified (PF-41): an
/// App names hosts of its own Organization's domain, never of another one's (ADR-N-058).
pub fn verified_domain(mirror: &Mirror, project: &str) -> Option<String> {
    let org = crate::permissions::organization_manifest_of(mirror, project)?;
    let verified = org
        .status
        .as_ref()
        .and_then(|status| status.domain_verification.as_ref())
        .is_some_and(|verification| verification.state == State::Verified);
    if !verified {
        return None;
    }
    org.spec
        .get("domain")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// `name` is `zone` or a name under it.
fn within(name: &str, zone: &str) -> bool {
    name == zone
        || name
            .strip_suffix(zone)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// Why `hostname` may not be one of App `project/app`'s, or `None` when it may. `apex` is the
/// zone the installation's own hosts sit under, when the Portal serves Apps.
pub fn refusal(
    mirror: &Mirror,
    project: &str,
    app: &str,
    hostname: &str,
    apex: Option<&str>,
) -> Option<String> {
    let Some(domain) = verified_domain(mirror, project) else {
        return Some(format!(
            "hostname '{hostname}' needs the organization's domain verified first: publish the \
             TXT record Organization settings shows, then list it again (AP-172, PF-41)"
        ));
    };
    if !within(hostname, &domain) {
        return Some(format!(
            "hostname '{hostname}' is not in the organization's domain {domain}: an App answers \
             only on {domain} or a name under it (AP-172)"
        ));
    }
    if apex.is_some_and(|apex| within(hostname, apex)) {
        return Some(format!(
            "hostname '{hostname}' is in the installation's own zone; the App already answers on \
             its host there (AP-133, AP-172)"
        ));
    }
    let held = mirror
        .matching(|envelope| envelope.kind == "App")
        .into_iter()
        .any(|other| {
            (
                other.metadata.namespace.as_deref(),
                other.metadata.name.as_str(),
            ) != (Some(project), app)
                && listed(&other.spec).any(|name| name == hostname)
        });
    // The other App is not named: it may live in a project the author cannot read (PF-59).
    held.then(|| format!("hostname '{hostname}' is held by another App (AP-172)"))
}

/// The names an App spec lists, as written.
fn listed(spec: &Value) -> impl Iterator<Item = &str> {
    spec.get("hostnames")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The names of App `project/app` the edge serves now: those [`refusal`] lets pass.
pub fn routed(mirror: &Mirror, project: &str, app: &str, spec: &Value, apex: &str) -> Vec<String> {
    listed(spec)
        .filter(|hostname| refusal(mirror, project, app, hostname, Some(apex)).is_none())
        .map(str::to_owned)
        .collect()
}

/// The first refusal among the names `spec` lists, for a write door.
pub fn check(
    mirror: &Mirror,
    project: &str,
    app: &str,
    spec: &Value,
    apex: Option<&str>,
) -> Result<(), String> {
    match listed(spec).find_map(|hostname| refusal(mirror, project, app, hostname, apex)) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::{ObjectMeta, ResourceEnvelope, Status};
    use serde_json::json;

    fn organization(state: &str) -> ResourceEnvelope {
        let status: Status = serde_json::from_value(json!({
            "domainVerification": { "state": state, "challenge": "c", "record": "r" }
        }))
        .expect("a status");
        ResourceEnvelope {
            api_version: "joinedcontext.com/v1alpha1".into(),
            kind: "Organization".into(),
            metadata: ObjectMeta {
                name: "hel".into(),
                namespace: Some(ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec: json!({ "domain": "hel.fi" }),
            status: Some(status),
        }
    }

    fn app(project: &str, name: &str, hostnames: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: "joinedcontext.com/v1alpha1".into(),
            kind: "App".into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(project.into()),
                ..Default::default()
            },
            spec: json!({ "hostnames": hostnames }),
            status: None,
        }
    }

    fn mirror(state: &str) -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(organization(state));
        mirror.upsert(app("helsinki", "bikes", json!(["bikes.hel.fi"])));
        mirror
    }

    /// F-R2POR3-003: with two Organizations an App names hosts of its own Organization's
    /// verified domain, never of the other's.
    #[test]
    fn an_app_of_another_organization_never_claims_this_ones_domain() {
        let mirror = mirror("verified");
        let mut other = organization("pending");
        other.metadata.name = "oulu".into();
        other.spec = json!({ "domain": "ouka.fi" });
        mirror.upsert(other);
        for (project, organization) in [("helsinki", "hel"), ("ouluproj", "oulu")] {
            mirror.upsert(ResourceEnvelope {
                api_version: "joinedcontext.com/v1alpha1".into(),
                kind: "Project".into(),
                metadata: ObjectMeta {
                    name: project.into(),
                    namespace: Some(ORG_NAMESPACE.into()),
                    ..Default::default()
                },
                spec: json!({ "organizationRef": organization }),
                status: None,
            });
        }
        assert_eq!(
            refusal(&mirror, "helsinki", "events", "events.hel.fi", None),
            None
        );
        assert!(refusal(&mirror, "ouluproj", "events", "events.hel.fi", None).is_some());
    }

    #[test]
    fn a_name_of_the_verified_domain_that_no_other_app_holds_passes() {
        let mirror = mirror("verified");
        for name in ["hel.fi", "events.hel.fi", "a.b.hel.fi"] {
            assert_eq!(
                refusal(&mirror, "helsinki", "events", name, Some("dev.example")),
                None,
                "{name}"
            );
        }
        // The App that holds a name keeps it on its next write.
        assert_eq!(
            refusal(&mirror, "helsinki", "bikes", "bikes.hel.fi", None),
            None
        );
    }

    #[test]
    fn a_name_outside_the_domain_or_in_the_installations_zone_is_refused() {
        let mirror = mirror("verified");
        for name in ["evil.example", "xhel.fi", "hel.fi.evil.example"] {
            let why = refusal(&mirror, "helsinki", "events", name, None).expect(name);
            assert!(
                why.contains("not in the organization's domain hel.fi"),
                "{why}"
            );
        }
        let why = refusal(
            &mirror,
            "helsinki",
            "events",
            "x.apps.hel.fi",
            Some("hel.fi"),
        )
        .expect("the installation's zone");
        assert!(why.contains("installation's own zone"), "{why}");
    }

    #[test]
    fn a_name_another_app_holds_is_refused_without_naming_it() {
        let mirror = mirror("verified");
        for (project, name) in [("helsinki", "events"), ("espoo", "bikes")] {
            let why = refusal(&mirror, project, name, "bikes.hel.fi", None).expect("held");
            assert!(why.contains("held by another App"), "{why}");
            assert!(!why.contains("helsinki/bikes"), "{why}");
        }
    }

    #[test]
    fn an_unverified_domain_refuses_every_name_and_routes_none() {
        for state in ["pending", "failed"] {
            let mirror = mirror(state);
            let why = refusal(&mirror, "helsinki", "events", "events.hel.fi", None).expect(state);
            assert!(why.contains("verified first"), "{why}");
            let spec = json!({ "hostnames": ["bikes.hel.fi"] });
            assert!(routed(&mirror, "helsinki", "bikes", &spec, "dev.example").is_empty());
        }
        let spec = json!({ "hostnames": ["bikes.hel.fi", "evil.example"] });
        assert_eq!(
            routed(
                &mirror("verified"),
                "helsinki",
                "bikes",
                &spec,
                "dev.example"
            ),
            ["bikes.hel.fi"]
        );
        assert!(check(&mirror("verified"), "helsinki", "bikes", &spec, None)
            .expect_err("one refused")
            .contains("evil.example"));
    }
}
