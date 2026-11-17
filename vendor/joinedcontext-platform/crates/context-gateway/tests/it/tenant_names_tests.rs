//! The tenant the gateway pins for a space (PF-130, EP-22, EP-188, ARCH/TEN-10):
//! `JC_TENANT_NAMES=segment` keeps the segment, `project-key` pins
//! `{OrgId}__{ProjectId}__{space}` from the Portal's project registry, and a project whose
//! Organization does not resolve, or which has no id yet, gets no tenant.

use context_gateway::store::{self, TenantNames};
use std::path::{Path, PathBuf};

const HEADER: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind:";

fn organization(name: &str, domain: &str) -> String {
    format!(
        "{HEADER} Organization\nmetadata:\n  name: {name}\n  namespace: org\nspec:\n  domain: {domain}\n  \
         locales: [en]\n  defaultLocale: en\n"
    )
}

fn project(organization: &str) -> String {
    format!(
        "{HEADER} Project\nmetadata:\n  name: city\n  namespace: org\nspec:\n  organizationRef: {organization}\n"
    )
}

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
     name: air\n  namespace: city\nspec:\n  isSandbox: false\n";

const ENDPOINT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  \
     name: air-open\n  namespace: city\nspec:\n  contextSpaceRef: air\n  \
     slug: aaaaaaaaaaaaaaaaaaaaaaaaaa\n  audience: public\n  enabledRepresentations: [\"ngsi-ld\"]\n";

fn repo(name: &str, files: &[(&str, String)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gw-tenant-names-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a repository");
    for (file, body) in files {
        std::fs::write(dir.join(file), body).expect("a manifest");
    }
    dir
}

/// The tenant of the Endpoint and of the space's own surface, as `names` pins them.
fn tenants(dir: &Path, names: TenantNames) -> (Option<String>, Option<String>) {
    let (endpoints, spaces, ..) = store::load_named(dir, names).expect("the repository loads");
    let of = |tenant: Option<&context_gateway::middleware::tenancy::Tenant>| {
        tenant.map(|tenant| tenant.as_str().to_owned())
    };
    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.slug == "aaaaaaaaaaaaaaaaaaaaaaaaaa")
        .expect("the endpoint is in the table");
    let space = spaces
        .iter()
        .find(|space| space.endpoint.space == "city-air")
        .expect("the space is in the table");
    (
        of(endpoint.tenant.as_ref()),
        of(space.endpoint.tenant.as_ref()),
    )
}

/// ARCH/TEN-10: two Organizations whose repositories hold the same project and space names pin
/// two tenants under `project-key`, each built from its own project id, and the one segment
/// both share under `segment`.
#[test]
fn two_organizations_with_the_same_project_and_space_get_two_tenants() {
    let mut seen = Vec::new();
    for (name, domain) in [("bbsk", "bbsk.sk"), ("hel", "hel.fi")] {
        let dir = repo(
            name,
            &[
                ("org.yaml", organization(name, domain)),
                ("project.yaml", project(name)),
                ("space.yaml", SPACE.to_owned()),
                ("endpoint.yaml", ENDPOINT.to_owned()),
            ],
        );
        let segment = Some("city-air".to_owned());
        assert_eq!(
            tenants(&dir, TenantNames::Segment),
            (segment.clone(), segment)
        );
        let key = jc_types::ProjectKey::new(jc_types::OrgId::mint(), jc_types::ProjectId::mint());
        store::publish_project_keys([("city".to_owned(), key)].into());
        let (endpoint, space) = tenants(&dir, TenantNames::ProjectKey);
        assert_eq!(endpoint, space, "{name}");
        assert_eq!(
            endpoint.as_deref(),
            Some(
                jc_core::names::TenantName::of_space(key, "air")
                    .expect("a space name")
                    .as_str()
            )
        );
        seen.push(endpoint.expect("a tenant"));
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert_ne!(seen[0], seen[1]);
    for tenant in &seen {
        assert!(tenant.ends_with("__air"), "{tenant}");
        assert!(jc_core::names::TenantName::parse(tenant).is_ok(), "{tenant}");
    }
}

/// Design 10.4: a project naming an Organization the repository lacks, or none at all, or one
/// the Portal has no id for, gets no tenant under `project-key` (its Endpoints answer 503),
/// never the installation's and never a name built from text.
#[test]
fn a_project_whose_organization_or_key_does_not_resolve_gets_no_tenant() {
    for (name, files) in [
        (
            "unknown",
            vec![
                ("org.yaml", organization("hel", "hel.fi")),
                ("project.yaml", project("vantaa")),
            ],
        ),
        (
            "no-project",
            vec![("org.yaml", organization("hel", "hel.fi"))],
        ),
    ] {
        let mut files = files;
        files.push(("space.yaml", SPACE.to_owned()));
        files.push(("endpoint.yaml", ENDPOINT.to_owned()));
        let dir = repo(name, &files);
        assert_eq!(
            tenants(&dir, TenantNames::ProjectKey),
            (None, None),
            "{name}"
        );
        let segment = Some("city-air".to_owned());
        assert_eq!(
            tenants(&dir, TenantNames::Segment),
            (segment.clone(), segment),
            "{name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    let dir = repo(
        "no-key",
        &[
            ("org.yaml", organization("hel", "hel.fi")),
            ("project.yaml", project("hel")),
            ("space.yaml", SPACE.to_owned()),
            ("endpoint.yaml", ENDPOINT.to_owned()),
        ],
    );
    let (endpoints, ..) = store::load_named(&dir, TenantNames::ProjectKey).expect("loads");
    let keyed = store::PROJECT_KEYS.read().expect("keys").contains_key("city");
    let endpoint = endpoints.iter().find(|e| e.slug == "aaaaaaaaaaaaaaaaaaaaaaaaaa");
    // Another test of this binary may have published `city`; without it there is no tenant.
    if !keyed {
        assert_eq!(endpoint.and_then(|e| e.tenant.as_ref()), None);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `JC_TENANT_NAMES` takes `segment` (also unset or empty) or `project-key`; the replaced
/// `organization` and anything else are refused naming the variable.
#[test]
fn the_tenant_names_switch_reads_two_values() {
    for (value, names) in [
        (None, TenantNames::Segment),
        (Some(""), TenantNames::Segment),
        (Some("segment"), TenantNames::Segment),
        (Some(" project-key "), TenantNames::ProjectKey),
    ] {
        assert_eq!(TenantNames::parse(value), Ok(names), "{value:?}");
    }
    for bad in ["org", "organization", "project_key"] {
        let refused = TenantNames::parse(Some(bad)).expect_err("not a mode");
        assert!(refused.contains("JC_TENANT_NAMES"), "{refused}");
    }
}
