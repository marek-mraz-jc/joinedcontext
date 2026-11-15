//! The tenant the gateway pins for a space (PF-130, EP-22, EP-188): `JC_TENANT_NAMES=segment`
//! keeps the segment, `organization` prefixes it with the project's Organization id from the
//! repository alone, and a project whose Organization does not resolve gets no tenant.

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

/// PF-130: two Organizations whose repositories hold the same project and space names pin two
/// tenants under `organization`, and the one segment both share under `segment`.
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
        let (endpoint, space) = tenants(&dir, TenantNames::Organization);
        assert_eq!(endpoint, space, "{name}");
        seen.push(endpoint.expect("a tenant"));
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert_eq!(seen, ["sk_bbsk__city-air", "fi_hel__city-air"]);
}

/// Design 10.4: a project naming an Organization the repository lacks, or none at all, gets no
/// tenant under `organization` (its Endpoints answer 503), never the installation's.
#[test]
fn a_project_whose_organization_does_not_resolve_gets_no_tenant() {
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
            tenants(&dir, TenantNames::Organization),
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
}

/// `JC_TENANT_NAMES` takes `segment` (also unset or empty) or `organization`; anything else is
/// refused naming the variable.
#[test]
fn the_tenant_names_switch_reads_two_values() {
    for (value, names) in [
        (None, TenantNames::Segment),
        (Some(""), TenantNames::Segment),
        (Some("segment"), TenantNames::Segment),
        (Some(" organization "), TenantNames::Organization),
    ] {
        assert_eq!(TenantNames::parse(value), Ok(names), "{value:?}");
    }
    let refused = TenantNames::parse(Some("org")).expect_err("not a mode");
    assert!(refused.contains("JC_TENANT_NAMES"), "{refused}");
}
