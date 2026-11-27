//! `jcctl validate` over layout 2: an assembled organization, and one project repository
//! alone, the gate its own CI runs (T-2640, CC-86, CC-88, CC-90).

use crate::common;

use common::{temp_dir, write, ENDPOINT, ORG, PROJECT, SPACE};
use jc_manifest::assemble::Directories;
use jc_manifest::validate::{run, run_assembled, run_project};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ENTRY: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                     name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n  \
                     repository: { name: ovzdusie }\n  ref: main\n";

fn project_repository(test: &str) -> PathBuf {
    let dir = temp_dir(&format!("{test}-project"));
    write(&dir, ".jc/layout", "2\n");
    write(&dir, "project.yaml", PROJECT);
    write(&dir, "spaces/ovzdusie/space.yaml", SPACE);
    write(&dir, "spaces/ovzdusie/endpoints/public-air.yaml", ENDPOINT);
    dir
}

fn organization(test: &str) -> PathBuf {
    let dir = temp_dir(&format!("{test}-org"));
    write(&dir, "org.yaml", ORG);
    write(&dir, ".jc/layout", "2\n");
    write(&dir, "projects/ovzdusie.yaml", ENTRY);
    dir
}

fn with(slug: &str, dir: &Path) -> Directories {
    Directories(BTreeMap::from([(slug.to_owned(), dir.to_path_buf())]))
}

fn messages(report: &jc_manifest::validate::Report) -> Vec<String> {
    report.findings.iter().map(ToString::to_string).collect()
}

/// CC-86: an assembled organization validates like the one repository it replaces.
#[test]
fn an_assembled_organization_validates_as_its_layout_1_twin() {
    let single = run(&common::demo_repo("validate-single"));
    let project = project_repository("validate-assembled");
    let assembled = run_assembled(
        &organization("validate-assembled"),
        &with("ovzdusie", &project),
    );
    assert!(assembled.is_valid(), "{:?}", messages(&assembled));
    assert_eq!(assembled.checked, single.checked);
}

/// CC-86: a project whose checkout is missing is a finding on its registry entry.
#[test]
fn a_project_without_its_checkout_is_a_finding() {
    let report = run_assembled(&organization("validate-missing"), &Directories::default());
    let found = messages(&report);
    assert!(
        found
            .iter()
            .any(|m| m.starts_with("projects/ovzdusie.yaml")
                && m.contains("could not be checked out")),
        "{found:?}"
    );
}

/// CC-88: a literal where a parameter exists and a mapping reading an undeclared one are findings
/// of their project's files, in an organization and in a project checked alone.
#[test]
fn a_literal_where_a_parameter_exists_is_a_finding_in_both_entry_points() {
    let project = project_repository("validate-literal");
    write(
        &project,
        "project.yaml",
        &(PROJECT.to_owned()
            + "  parameters:\n    audience: { type: string, default: public, enum: [public, organization] }\n"),
    );
    write(
        &project,
        "pipelines/air/bento.yaml",
        "pipeline:\n  processors:\n    - mapping: root.host = env(\"JC_PARAM_FEED_HOTS\")\n",
    );
    let alone = run_project(&project, None, "example.org", &BTreeMap::new());
    let organization = run_assembled(
        &organization("validate-literal"),
        &with("ovzdusie", &project),
    );
    for (what, report) in [("project", alone), ("organization", organization)] {
        let found = messages(&report);
        assert!(
            found.iter().any(|m| m.starts_with(
                "spaces/ovzdusie/endpoints/public-air.yaml:1 (document 1): project `ovzdusie`: spec.audience writes the value of a parameter; write `{param:audience}`"
            )),
            "{what}: {found:?}"
        );
        assert!(
            found
                .iter()
                .any(|m| m.starts_with("pipelines/air/bento.yaml:1")
                    && m.contains("project `ovzdusie`: reads env(\"JC_PARAM_FEED_HOTS\")")),
            "{what}: {found:?}"
        );
    }
}

/// T-6709: a project whose manifest cannot be read is refused alone, and its reason is a finding
/// on the file that broke it, under the project's folder, in an organization and in a project
/// checked alone.
#[test]
fn a_project_refused_at_assembly_is_a_finding_on_the_file_that_broke_it() {
    let project = project_repository("validate-refused");
    write(
        &project,
        "spaces/ovzdusie/endpoints/broken.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: [\n",
    );
    let alone = run_project(&project, None, "example.org", &BTreeMap::new());
    let organization = run_assembled(
        &organization("validate-refused"),
        &with("ovzdusie", &project),
    );
    for (what, report) in [("project", alone), ("organization", organization)] {
        let found: Vec<_> = report
            .findings
            .iter()
            .filter(|f| {
                f.path == Path::new("projects/ovzdusie/spaces/ovzdusie/endpoints/broken.yaml")
            })
            .collect();
        assert_eq!(found.len(), 1, "{what}: {:?}", messages(&report));
        assert!(
            found[0]
                .message
                .starts_with("ovzdusie: spaces/ovzdusie/endpoints/broken.yaml"),
            "{what}: {}",
            found[0].message
        );
    }
}

/// CC-90: a project repository validates alone, without its organization, and its findings
/// name the paths of the project repository.
#[test]
fn a_project_repository_validates_alone_with_its_own_paths() {
    let project = project_repository("validate-alone");
    let report = run_project(&project, None, "example.org", &BTreeMap::new());
    assert!(report.is_valid(), "{:?}", messages(&report));
    assert!(report.checked >= 3, "{}", report.checked);

    write(
        &project,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace("audience: public", "audience: everyone"),
    );
    let report = run_project(&project, None, "example.org", &BTreeMap::new());
    let found = messages(&report);
    assert!(
        found
            .iter()
            .any(|m| m.starts_with("spaces/ovzdusie/endpoints/public-air.yaml")),
        "{found:?}"
    );
}

/// EP-168, ADR-N-067 2.3 (T-6409): `jcctl validate` takes a Policy whose rule is for every member of
/// the project's Organization, and refuses one that names an Organization, naming the field.
#[test]
fn an_organization_assignee_validates_and_one_naming_an_organization_is_refused() {
    const POLICY: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  \
                          name: staff\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: ovzdusie\n  \
                          assigner: did:web:example.org\n  rules:\n    \
                          - assignee: { kind: organization }\n      operations: [retrieveOps]\n";
    let project = project_repository("validate-organization-assignee");
    write(&project, "spaces/ovzdusie/policies/staff.yaml", POLICY);
    let report = run_project(&project, None, "example.org", &BTreeMap::new());
    assert!(report.is_valid(), "{:?}", messages(&report));

    write(
        &project,
        "spaces/ovzdusie/policies/staff.yaml",
        &POLICY.replace(
            "{ kind: organization }",
            "{ kind: organization, id: espoo }",
        ),
    );
    let report = run_project(&project, None, "example.org", &BTreeMap::new());
    let found = messages(&report);
    assert!(
        found
            .iter()
            .any(|m| m.starts_with("spaces/ovzdusie/policies/staff.yaml")
                && m.contains("spec.assignee.id")),
        "{found:?}"
    );
}

/// CC-88, CC-90: a parameter without a default needs a value to check the project with, and
/// `--param` gives it; a project named `{project}` needs the slug to check it under.
#[test]
fn a_project_is_checked_with_its_parameters_and_its_slug() {
    let project = project_repository("validate-parameters");
    write(
        &project,
        "project.yaml",
        &(PROJECT.to_owned() + "  parameters:\n    audience: { type: string }\n"),
    );
    write(
        &project,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace("audience: public", "audience: \"{param:audience}\""),
    );
    let without = run_project(&project, None, "example.org", &BTreeMap::new());
    assert!(
        messages(&without).iter().any(|m| m.contains("audience")),
        "{:?}",
        messages(&without)
    );
    let values = BTreeMap::from([("audience".to_owned(), json!("public"))]);
    let given = run_project(&project, None, "example.org", &values);
    assert!(given.is_valid(), "{:?}", messages(&given));

    write(
        &project,
        "project.yaml",
        &(PROJECT.replace("name: ovzdusie", "name: \"{project}\"")
            + "  parameters:\n    audience: { type: string, default: public }\n"),
    );
    let unnamed = run_project(&project, None, "example.org", &BTreeMap::new());
    assert!(
        messages(&unnamed).iter().any(|m| m.contains("--slug")),
        "{:?}",
        messages(&unnamed)
    );
    let named = run_project(&project, Some("ovzdusie"), "example.org", &BTreeMap::new());
    assert!(named.is_valid(), "{:?}", messages(&named));
}
