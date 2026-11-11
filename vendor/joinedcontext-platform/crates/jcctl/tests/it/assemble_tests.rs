//! Assembling one render from the organization repository and one repository per project
//! (T-2640, CC-85, CC-86, CC-88, PF-86).

use crate::common;

use common::{temp_dir, write, ENDPOINT, ORG, PROJECT, SPACE};
use jcctl::assemble::{assemble, assemble_reusing, Assembly, Directories, EntryStatus, Renders};
use jcctl::loader::Repository;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ENTRY: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: ovzdusie
  namespace: org
spec:
  organizationRef: banskabystrica
  repository: { name: ovzdusie }
  ref: main
"#;

/// The organization repository of layout 2: the organization, the layout, the registry.
fn organization(test: &str, entries: &[(&str, &str)]) -> PathBuf {
    let dir = temp_dir(&format!("{test}-org"));
    write(&dir, "org.yaml", ORG);
    write(&dir, ".jc/layout", "2\n");
    for (slug, entry) in entries {
        write(&dir, &format!("projects/{slug}.yaml"), entry);
    }
    dir
}

/// The project repository of the demo story: what `projects/ovzdusie/` holds in layout 1, at
/// its root.
fn project_repository(test: &str) -> PathBuf {
    let dir = temp_dir(&format!("{test}-project"));
    write(&dir, ".jc/layout", "2\n");
    write(&dir, "project.yaml", PROJECT);
    write(&dir, "spaces/ovzdusie/space.yaml", SPACE);
    write(&dir, "spaces/ovzdusie/endpoints/public-air.yaml", ENDPOINT);
    dir
}

fn checkouts(pairs: &[(&str, &Path)]) -> Directories {
    Directories(
        pairs
            .iter()
            .map(|(slug, dir)| ((*slug).to_owned(), dir.to_path_buf()))
            .collect(),
    )
}

fn resources(repository: &Repository) -> BTreeMap<String, (PathBuf, serde_json::Value)> {
    repository
        .iter()
        .map(|(id, loaded)| {
            (
                id.to_string(),
                (
                    loaded.path.clone(),
                    serde_json::to_value(&loaded.manifest).expect("a manifest serialises"),
                ),
            )
        })
        .collect()
}

/// CC-86: the demo organization renders the same from one repository (layout 1) as from the
/// organization repository and a project repository (layout 2): same ids, same paths, same
/// manifests.
#[test]
fn layout_2_renders_what_layout_1_renders() {
    let single = common::demo_repo("assemble-single");
    let one = assemble(&single, &Directories::default(), &temp_dir("unused"), None)
        .expect("layout 1 loads where it is");
    assert_eq!(one.layout, 1);

    let org = organization("same", &[("ovzdusie", ENTRY)]);
    let project = project_repository("same");
    let into = temp_dir("same-into").join("render");
    let two = assemble(&org, &checkouts(&[("ovzdusie", &project)]), &into, None)
        .expect("layout 2 assembles");
    assert_eq!(two.layout, 2);
    assert_eq!(two.entries.len(), 1);
    assert!(two.entries[0].rendered && two.entries[0].error.is_none());
    assert_eq!(resources(&two.repository), resources(&one.repository));
}

/// CC-86, PF-86: one repository under two registry slugs is two projects, each rendering its
/// own namespace; a slug unique in the organization, claimed by both, is refused by name.
#[test]
fn two_projects_on_one_repository_and_a_slug_they_both_claim() {
    let shared = temp_dir("two-slugs-project");
    write(&shared, ".jc/layout", "2\n");
    write(
        &shared,
        "spaces/ovzdusie/space.yaml",
        &SPACE
            .replace("namespace: ovzdusie", "namespace: \"{project}\"")
            .replace(
                "isSandbox: false",
                "isSandbox: false\n  urnSegment: \"{param:segment}\"",
            ),
    );
    write(
        &shared,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT
            .replace("namespace: ovzdusie", "namespace: \"{project}\"")
            .replace("slug: zt4qm7ge2xdv6ksb3ncf5arw2y", "slug: \"{param:slug}\""),
    );
    write(
        &shared,
        "project.yaml",
        &(PROJECT.replace("name: ovzdusie", "name: \"{project}\"")
            + "  parameters:\n    slug: { type: string }\n    segment: { type: string }\n"),
    );
    let entry = |slug: &str, endpoint: &str, segment: &str| {
        ENTRY.replace(
            "name: ovzdusie\n  namespace",
            &format!("name: {slug}\n  namespace"),
        ) + &format!("  parameters: {{ slug: {endpoint}, segment: {segment} }}\n")
    };
    let air = entry("air", "zt4qm7ge2xdv6ksb3ncf5arw2y", "air");
    let staging = entry("air-staging", "q7w6e5r4t3y2uaiopazsxdcfgh", "air-staging");
    let org = organization("two-slugs", &[("air", &air), ("air-staging", &staging)]);
    let resolver = checkouts(&[("air", &shared), ("air-staging", &shared)]);

    let into = temp_dir("two-slugs-into").join("r");
    let assembly =
        assemble(&org, &resolver, &into, None).expect("two projects on one repository assemble");
    let namespaces: Vec<String> = assembly
        .repository
        .iter()
        .filter(|(id, _)| id.kind == "Endpoint")
        .map(|(id, _)| id.namespace.clone().unwrap_or_default())
        .collect();
    assert_eq!(namespaces, ["air", "air-staging"]);

    // T-6709: air-staging claims the slug air serves. Only the project that changed is refused,
    // it keeps its last render, and air keeps serving the slug.
    let clash = entry("air-staging", "zt4qm7ge2xdv6ksb3ncf5arw2y", "air-staging");
    let clashing = organization("slug-clash", &[("air", &air), ("air-staging", &clash)]);
    let assembly = assemble(&clashing, &resolver, &into, None)
        .expect("a slug claimed twice refuses the project that claimed it, not the organization");
    let slugs: Vec<(String, String)> = assembly
        .repository
        .iter()
        .filter(|(id, _)| id.kind == "Endpoint")
        .map(|(id, loaded)| {
            (
                id.namespace.clone().unwrap_or_default(),
                loaded.manifest.spec["slug"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(
        slugs,
        [
            ("air".to_owned(), "zt4qm7ge2xdv6ksb3ncf5arw2y".to_owned()),
            (
                "air-staging".to_owned(),
                "q7w6e5r4t3y2uaiopazsxdcfgh".to_owned()
            ),
        ]
    );
    assert_eq!(assembly.entries[0].refused, None, "air changed nothing");
    let refusal = assembly.entries[1]
        .refused
        .as_ref()
        .expect("air-staging is refused");
    assert!(
        refusal.reason.contains("zt4qm7ge2xdv6ksb3ncf5arw2y")
            && refusal.reason.contains("air-staging"),
        "{refusal:?}"
    );
    assert!(assembly.entries[1].rendered, "its last render serves");

    // With no last render (a first assembly) neither project can be told apart: both are
    // refused, and neither serves the slug.
    let first = assemble(
        &clashing,
        &resolver,
        &temp_dir("slug-clash-into").join("r"),
        None,
    )
    .expect("the organization assembles");
    assert!(first
        .entries
        .iter()
        .all(|entry| entry.refused.is_some() && !entry.rendered));
    assert!(first
        .repository
        .iter()
        .all(|(id, _)| id.kind == "Organization"));
}

/// CC-86: a ref that cannot be fetched renders the project as the last assembly left it and
/// says so on its entry; a project never assembled is left out, with the same report.
#[test]
fn an_unfetchable_ref_keeps_the_last_checkout_and_reports_it() {
    let org = organization("unfetchable", &[("ovzdusie", ENTRY)]);
    let project = project_repository("unfetchable");
    let into = temp_dir("unfetchable-into").join("render");
    let first = assemble(&org, &checkouts(&[("ovzdusie", &project)]), &into, None)
        .expect("the first assembly");
    let before = resources(&first.repository);

    let again = assemble(&org, &Directories::default(), &into, None)
        .expect("an unfetchable project does not stop the organization");
    assert!(again.entries[0].rendered);
    assert!(again.entries[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("ovzdusie")));
    assert_eq!(resources(&again.repository), before);
    assert_eq!(
        again.repository,
        Repository::load_for(&into, None).expect("the last render loads")
    );

    let fresh = assemble(
        &org,
        &Directories::default(),
        &temp_dir("unfetchable-fresh").join("render"),
        None,
    )
    .expect("the organization assembles without the project");
    assert!(!fresh.entries[0].rendered);
    assert!(fresh
        .repository
        .iter()
        .all(|(id, _)| id.kind == "Organization"));
}

/// T-6709 (P-3): a project whose manifests do not load is refused alone, with the file and the
/// reason on its entry, and keeps its last render; the other project's new Endpoint is served
/// in the same assembly. Without a last render the broken project is left out.
#[test]
fn a_broken_project_keeps_its_last_render_and_the_others_converge() {
    let doprava_entry = ENTRY.replace("ovzdusie", "doprava");
    let org = organization(
        "isolated",
        &[("doprava", &doprava_entry), ("ovzdusie", ENTRY)],
    );
    let ovzdusie = project_repository("isolated-ovzdusie");
    let doprava = temp_dir("isolated-doprava");
    write(&doprava, ".jc/layout", "2\n");
    write(
        &doprava,
        "project.yaml",
        &PROJECT.replace("ovzdusie", "doprava"),
    );
    write(
        &doprava,
        "spaces/doprava/space.yaml",
        &SPACE.replace("ovzdusie", "doprava"),
    );
    let endpoint = |name: &str, slug: &str| {
        ENDPOINT
            .replace("ovzdusie", "doprava")
            .replace("public-air", name)
            .replace("zt4qm7ge2xdv6ksb3ncf5arw2y", slug)
    };
    let resolver = checkouts(&[("doprava", &doprava), ("ovzdusie", &ovzdusie)]);
    let into = temp_dir("isolated-into").join("r");
    assemble(&org, &resolver, &into, None).expect("both projects assemble");
    let endpoints = |assembly: &Assembly| -> Vec<String> {
        assembly
            .repository
            .iter()
            .filter(|(id, _)| id.kind == "Endpoint")
            .map(|(id, _)| id.to_string())
            .collect()
    };

    // A file that does not parse.
    write(
        &ovzdusie,
        "spaces/ovzdusie/endpoints/broken.yaml",
        "kind: [\n",
    );
    write(
        &doprava,
        "spaces/doprava/endpoints/buses.yaml",
        &endpoint("buses", "b2c3d4e5f6g7h2i3j4k5l6m7n2"),
    );
    let assembly = assemble(&org, &resolver, &into, None).expect("one broken project");
    assert_eq!(
        endpoints(&assembly),
        ["Endpoint/doprava/buses", "Endpoint/ovzdusie/public-air"]
    );
    let entry = &assembly.entries[1];
    assert!(entry.rendered && entry.error.is_none());
    let refusal = entry.refused.as_ref().expect("ovzdusie is refused");
    assert_eq!(
        refusal.path.as_deref(),
        Some(Path::new("spaces/ovzdusie/endpoints/broken.yaml"))
    );
    assert!(
        refusal.reason.contains("did not find expected node"),
        "{refusal:?}"
    );
    assert_eq!(assembly.entries[0].refused, None);

    // A duplicate id: the same Endpoint declared in a second file.
    std::fs::remove_file(ovzdusie.join("spaces/ovzdusie/endpoints/broken.yaml")).expect("fix");
    write(&ovzdusie, "spaces/ovzdusie/endpoints/twice.yaml", ENDPOINT);
    write(
        &doprava,
        "spaces/doprava/endpoints/trams.yaml",
        &endpoint("trams", "c3d4e5f6g7h2i3j4k5l6m7n2o3"),
    );
    let assembly = assemble(&org, &resolver, &into, None).expect("a duplicate in one project");
    assert_eq!(
        endpoints(&assembly),
        [
            "Endpoint/doprava/buses",
            "Endpoint/doprava/trams",
            "Endpoint/ovzdusie/public-air"
        ]
    );
    let refusal = assembly.entries[1].refused.as_ref().expect("refused");
    assert!(
        refusal.reason.contains("duplicate resource identity"),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.path.as_deref(),
        Some(Path::new("spaces/ovzdusie/endpoints/twice.yaml"))
    );
    assert_eq!(
        assembly.repository,
        Repository::load_for(&into, None).expect("the written tree loads")
    );

    // After a restart there is no last render: the broken project is left out, doprava serves.
    let restarted =
        assemble(&org, &resolver, &temp_dir("isolated-fresh").join("r"), None).expect("assembles");
    assert!(!restarted.entries[1].rendered && restarted.entries[1].refused.is_some());
    assert_eq!(
        endpoints(&restarted),
        ["Endpoint/doprava/buses", "Endpoint/doprava/trams"]
    );

    // Fixed, it converges and the refusal clears.
    std::fs::remove_file(ovzdusie.join("spaces/ovzdusie/endpoints/twice.yaml")).expect("fix");
    let fixed = assemble(&org, &resolver, &into, None).expect("assembles");
    assert!(fixed.entries.iter().all(|entry| entry.refused.is_none()));
}

/// CC-88: the entry's parameter values are rendered into the project's manifests, a literal
/// where a parameter exists and a mapping reading an undeclared one are reported.
#[test]
fn parameters_render_and_literals_are_reported() {
    let project = project_repository("parameters");
    write(
        &project,
        "project.yaml",
        &(PROJECT.to_owned()
            + "  parameters:\n    audience: { type: string, default: public, enum: [public, organization] }\n    \
               feedHost: { type: string, default: data.banskabystrica.sk }\n"),
    );
    write(
        &project,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace("audience: public", "audience: \"{param:audience}\""),
    );
    write(
        &project,
        "pipelines/air/pipeline.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: air\n  \
         namespace: ovzdusie\nspec:\n  class: stream\n  description: data.banskabystrica.sk\n",
    );
    write(
        &project,
        "pipelines/air/bento.yaml",
        "pipeline:\n  processors:\n    - mapping: root.host = env(\"JC_PARAM_FEED_HOTS\")\n",
    );
    let entry = ENTRY.to_owned() + "  parameters: { audience: organization }\n";
    let org = organization("parameters", &[("ovzdusie", &entry)]);
    let assembly = assemble(
        &org,
        &checkouts(&[("ovzdusie", &project)]),
        &temp_dir("parameters-into").join("render"),
        None,
    )
    .expect("assembles");
    let endpoint = assembly
        .repository
        .iter()
        .find(|(id, _)| id.kind == "Endpoint")
        .map(|(_, loaded)| loaded.manifest.spec["audience"].clone())
        .expect("the endpoint is rendered");
    assert_eq!(endpoint, "organization");

    let messages: Vec<String> = assembly
        .findings
        .iter()
        .map(|finding| format!("{}: {}", finding.path.display(), finding.message))
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("pipelines/air/pipeline.yaml")
                && m.contains("spec.description")
                && m.contains("{param:feedHost}")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("pipelines/air/bento.yaml") && m.contains("JC_PARAM_FEED_HOTS")),
        "{messages:?}"
    );
}

/// CC-85, PF-86: a layout 2 organization repository holding a project directory, a registry
/// entry whose file and name disagree, and a project repository without its layout are refused;
/// a refused assembly leaves the last one where it was.
#[test]
fn a_misplaced_project_is_refused_and_the_last_render_stays() {
    let org = organization("misplaced", &[("ovzdusie", ENTRY)]);
    let project = project_repository("misplaced");
    let into = temp_dir("misplaced-into").join("render");
    assemble(&org, &checkouts(&[("ovzdusie", &project)]), &into, None).expect("assembles");

    std::fs::remove_file(project.join(".jc/layout")).expect("remove the layout");
    let refused = assemble(&org, &checkouts(&[("ovzdusie", &project)]), &into, None)
        .expect("a project repository without .jc/layout refuses the project alone (T-6709)");
    let refusal = refused.entries[0].refused.as_ref().expect("refused");
    assert!(refusal.reason.contains(".jc/layout"), "{refusal:?}");
    assert_eq!(refusal.path.as_deref(), Some(Path::new(".jc/layout")));
    assert!(refused.entries[0].rendered, "the last render stays");
    Repository::load(&into).expect("the last render still loads");

    let with_directory = organization("with-directory", &[("ovzdusie", ENTRY)]);
    write(&with_directory, "projects/transport/project.yaml", PROJECT);
    let refused = assemble(
        &with_directory,
        &Directories::default(),
        &temp_dir("with-directory-into").join("r"),
        None,
    )
    .expect_err("a project directory in layout 2");
    assert!(
        refused.to_string().contains("projects/transport"),
        "{refused}"
    );

    let misnamed = organization("misnamed", &[("air", ENTRY)]);
    let refused = assemble(
        &misnamed,
        &Directories::default(),
        &temp_dir("misnamed-into").join("r"),
        None,
    )
    .expect_err("an entry named apart from its file");
    assert!(
        refused.to_string().contains("projects/air.yaml"),
        "{refused}"
    );

    let unknown = organization("unknown-layout", &[]);
    write(&unknown, ".jc/layout", "3\n");
    assert!(assemble(
        &unknown,
        &Directories::default(),
        &temp_dir("u").join("r"),
        None
    )
    .is_err());
}

/// T-4443: only a link is resolved on the way, so the check that no file of a checkout leads out
/// of it must still hold for a link to a file outside, and for a link reached through a linked
/// directory inside; a linked directory that stays inside is copied.
#[cfg(unix)]
#[test]
fn a_checkout_that_links_out_is_refused_and_one_that_links_inside_is_copied() {
    use std::os::unix::fs::symlink;
    let org = organization("links", &[("ovzdusie", ENTRY)]);
    let outside = temp_dir("links-outside");
    write(&outside, "secret.yaml", "not: for the render\n");

    let inside = project_repository("links-inside");
    write(&inside, "docs/README.md", "notes\n");
    symlink(inside.join("docs"), inside.join("notes")).expect("a link");
    let into = temp_dir("links-into").join("render");
    assemble(&org, &checkouts(&[("ovzdusie", &inside)]), &into, None)
        .expect("a link that stays inside is followed");
    assert!(into.join("projects/ovzdusie/notes/README.md").is_file());

    let out = project_repository("links-out");
    symlink(
        outside.join("secret.yaml"),
        out.join("spaces/ovzdusie/leak.yaml"),
    )
    .expect("a link");
    let refused = assemble(&org, &checkouts(&[("ovzdusie", &out)]), &into, None)
        .expect("a link out is refused with its project (T-6709)");
    let refusal = refused.entries[0].refused.as_ref().expect("refused");
    assert!(
        refusal.reason.contains("links out of the repository"),
        "{refusal:?}"
    );
    assert!(!into
        .join("projects/ovzdusie/spaces/ovzdusie/leak.yaml")
        .exists());
    assert!(
        into.join("projects/ovzdusie/notes/README.md").is_file(),
        "the last render serves"
    );

    let nested = project_repository("links-nested");
    std::fs::create_dir_all(nested.join("extra")).expect("a directory");
    symlink(outside.join("secret.yaml"), nested.join("extra/leak.yaml")).expect("a link");
    symlink(nested.join("extra"), nested.join("spaces/more")).expect("a link");
    let refused = assemble(&org, &checkouts(&[("ovzdusie", &nested)]), &into, None)
        .expect("a link out behind a linked directory is refused with its project (T-6709)");
    let refusal = refused.entries[0].refused.as_ref().expect("refused");
    assert!(
        refusal.reason.contains("links out of the repository"),
        "{refusal:?}"
    );
    assert!(!into
        .join("projects/ovzdusie/spaces/more/leak.yaml")
        .exists());
    assert!(
        into.join("projects/ovzdusie/notes/README.md").is_file(),
        "the last render serves"
    );
}

/// T-4457: the assembly keeps each manifest as it rendered it instead of reading the tree back,
/// and what it keeps is what reading the tree back gives: same paths, documents, lines,
/// manifests, rendered parameters and `{orgDomain}`.
#[test]
fn the_assembly_in_memory_is_what_the_written_render_loads() {
    let project = project_repository("in-memory");
    write(
        &project,
        "project.yaml",
        &(PROJECT.to_owned()
            + "  parameters:\n    audience: { type: string, default: public, enum: [public, organization] }\n"),
    );
    write(
        &project,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace("audience: public", "audience: \"{param:audience}\""),
    );
    // A leading `---`, an empty document and a comment: each moves the index or the line of
    // the documents after it.
    write(
        &project,
        "spaces/ovzdusie/more.yaml",
        &format!(
            "---\n{}---\n# nothing here\n---\n{}",
            SPACE.replace("name: ovzdusie\n  namespace", "name: second\n  namespace"),
            ENDPOINT
                .replace("public-air", "second-air")
                .replace("contextSpaceRef: ovzdusie", "contextSpaceRef: second")
                .replace("zt4qm7ge2xdv6ksb3ncf5arw2y", "qp3mx7kd2wbv5nsc4jgf6rta2e")
                .replace("public\n", "\"{orgDomain}\"\n")
        ),
    );
    let entry = ENTRY.to_owned() + "  parameters: { audience: organization }\n";
    let org = organization("in-memory", &[("ovzdusie", &entry)]);
    let into = temp_dir("in-memory-into").join("render");
    let assembly =
        assemble(&org, &checkouts(&[("ovzdusie", &project)]), &into, None).expect("assembles");
    let read_back = Repository::load_for(&into, None).expect("the written render loads");
    assert_eq!(assembly.repository, read_back);
    let second = assembly
        .repository
        .iter()
        .find(|(id, _)| id.name == "second-air")
        .map(|(_, loaded)| loaded)
        .expect("the third document");
    assert_eq!((second.document, second.line), (2, 8));
}

/// `slug` under `links` pointed at `tree`, as `jcctl checkouts` leaves a checkout: a link to a
/// tree named by its commit, swapped by one rename.
fn link(links: &Path, slug: &str, tree: &Path) -> Directories {
    std::fs::create_dir_all(links).expect("the links");
    let at = links.join(slug);
    let next = links.join(format!(".{slug}.next"));
    std::os::unix::fs::symlink(tree, &next).expect("linked");
    std::fs::rename(&next, &at).expect("swapped in");
    checkouts(&[(slug, &at)])
}

fn representations(assembly: &Assembly) -> serde_json::Value {
    let (_, endpoint) = assembly
        .repository
        .iter()
        .find(|(id, _)| id.kind == "Endpoint")
        .expect("the Endpoint");
    endpoint.manifest.spec["enabledRepresentations"].clone()
}

const ONE: &str = r#"enabledRepresentations: ["ngsi-ld"]"#;
const BOTH: &str = r#"enabledRepresentations: ["ngsi-ld", "csv"]"#;

/// T-4460: a linked project whose tree did not move is taken from the last assembly, not
/// rendered again, and stays on disk; relinked to another tree, it renders that tree.
#[test]
fn a_linked_project_renders_again_only_when_its_link_moves() {
    let org = organization("reuse", &[("ovzdusie", ENTRY)]);
    let c1 = project_repository("reuse-c1");
    let links = temp_dir("reuse-links");
    let into = temp_dir("reuse-into").join("r");
    let mut renders = Renders::default();
    let first = assemble_reusing(
        &org,
        &link(&links, "ovzdusie", &c1),
        &into,
        None,
        &mut renders,
    )
    .expect("assembles");
    assert_eq!(
        representations(&first),
        serde_json::json!(["ngsi-ld", "csv"])
    );
    assert!(!first.entries[0].reused, "nothing was kept yet");

    // A tree is written once; one rewritten in place is not read again while the link holds.
    let endpoint = c1.join("spaces/ovzdusie/endpoints/public-air.yaml");
    write(
        &c1,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace(BOTH, ONE),
    );
    let again = assemble_reusing(
        &org,
        &link(&links, "ovzdusie", &c1),
        &into,
        None,
        &mut renders,
    )
    .expect("assembles");
    assert_eq!(
        representations(&again),
        serde_json::json!(["ngsi-ld", "csv"])
    );
    assert!(again.entries[0].reused);
    assert!(
        into.join("projects/ovzdusie/spaces/ovzdusie/endpoints/public-air.yaml")
            .is_file(),
        "the reused render is in the new tree"
    );

    let c2 = temp_dir("reuse-c2");
    write(&c2, ".jc/layout", "2\n");
    write(&c2, "project.yaml", PROJECT);
    write(&c2, "spaces/ovzdusie/space.yaml", SPACE);
    std::fs::create_dir_all(c2.join("spaces/ovzdusie/endpoints")).expect("dir");
    std::fs::copy(
        &endpoint,
        c2.join("spaces/ovzdusie/endpoints/public-air.yaml"),
    )
    .expect("copy");
    let moved = assemble_reusing(
        &org,
        &link(&links, "ovzdusie", &c2),
        &into,
        None,
        &mut renders,
    )
    .expect("assembles");
    assert_eq!(representations(&moved), serde_json::json!(["ngsi-ld"]));
    assert!(!moved.entries[0].reused, "the link moved");
}

/// T-4460: a checkout that is not a link can change in place, so it is rendered every time and
/// nothing is kept for it.
#[test]
fn a_checkout_that_is_no_link_renders_every_time() {
    let org = organization("plain", &[("ovzdusie", ENTRY)]);
    let dir = project_repository("plain");
    let into = temp_dir("plain-into").join("r");
    let mut renders = Renders::default();
    assemble_reusing(
        &org,
        &checkouts(&[("ovzdusie", &dir)]),
        &into,
        None,
        &mut renders,
    )
    .expect("assembles");
    write(
        &dir,
        "spaces/ovzdusie/endpoints/public-air.yaml",
        &ENDPOINT.replace(BOTH, ONE),
    );
    let again = assemble_reusing(
        &org,
        &checkouts(&[("ovzdusie", &dir)]),
        &into,
        None,
        &mut renders,
    )
    .expect("assembles");
    assert_eq!(representations(&again), serde_json::json!(["ngsi-ld"]));
    assert!(!again.entries[0].reused);
}

/// T-4460: a refused assembly puts every reused render back where it was and keeps the renders
/// it had, so the last tree still serves a project whose checkout goes missing and the next
/// assembly still reuses.
#[test]
fn a_refused_assembly_puts_the_reused_renders_back() {
    let entry = |slug: &str| ENTRY.replace("ovzdusie", slug);
    let org = organization("putback", &[("ovzdusie", ENTRY)]);
    let tree = project_repository("putback");
    let links = temp_dir("putback-links");
    let into = temp_dir("putback-into").join("r");
    let mut renders = Renders::default();
    let linked = link(&links, "ovzdusie", &tree);
    assemble_reusing(&org, &linked, &into, None, &mut renders).expect("assembles");

    // doprava claims ovzdusie's Endpoint slug: the assembly is refused after the reuse.
    write(&org, "projects/doprava.yaml", &entry("doprava"));
    let clash = temp_dir("putback-doprava");
    write(&clash, ".jc/layout", "2\n");
    write(
        &clash,
        "project.yaml",
        &PROJECT.replace("ovzdusie", "doprava"),
    );
    write(
        &clash,
        "spaces/doprava/space.yaml",
        &SPACE.replace("ovzdusie", "doprava"),
    );
    write(
        &clash,
        "spaces/doprava/endpoints/public-air.yaml",
        &ENDPOINT.replace("ovzdusie", "doprava"),
    );
    let mut both = linked.0.clone();
    both.insert("doprava".to_owned(), clash);
    // T-6709: the claim refuses doprava alone; ovzdusie's reused render serves on.
    let claimed = assemble_reusing(&org, &Directories(both), &into, None, &mut renders)
        .expect("one slug, two projects: the claimant is refused");
    assert!(claimed.entries[1].reused && claimed.entries[1].refused.is_none());
    assert!(matches!(
        &claimed.entries[0],
        EntryStatus { slug, rendered: false, refused: Some(_), .. } if slug == "doprava"
    ));
    std::fs::remove_file(org.join("projects/doprava.yaml")).expect("unregister");

    // A broken organization file refuses the whole assembly after the reuse.
    write(&org, "broken.yaml", "kind: [\n");
    let refused = assemble_reusing(&org, &linked, &into, None, &mut renders)
        .expect_err("the organization's own file does not parse");
    assert!(refused.to_string().contains("broken.yaml"), "{refused}");
    assert!(
        into.join("projects/ovzdusie/spaces/ovzdusie/endpoints/public-air.yaml")
            .is_file(),
        "the reused render is back in the last tree"
    );
    std::fs::remove_file(org.join("broken.yaml")).expect("fix");
    let next = assemble_reusing(&org, &linked, &into, None, &mut renders).expect("assembles");
    assert!(next.entries[0].reused, "the renders it had are kept");
    let fallback = assemble(&org, &Directories::default(), &into, None).expect("assembles");
    assert!(
        fallback.entries[0].rendered,
        "the last tree still serves ovzdusie"
    );
}

/// F-R2PLT3-002: the `-` join of project and name is not one-to-one, so the ServiceAccount
/// `vendorx-parking-push` of `ovzdusie` and `parking-push` of `ovzdusie-vendorx` derive one
/// Keycloak client, `ovzdusie-vendorx-parking-push`; the second is refused by name.
#[test]
fn two_projects_deriving_one_client_id_are_refused() {
    const ACCOUNT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ServiceAccount\nmetadata:\n  name: NAME\n  namespace: PROJECT\nspec:\n  owner: { user: \"jana.k@bb\" }\n  purpose: \"pushes ParkingSpot updates\"\n  roles:\n    - role: space-writer\n      scope: { contextSpace: ovzdusie }\n      types: [ParkingSpot]\n      operations: [createEntity]\n  credentials:\n    - kind: oauth-client\n      name: main\n";
    let account =
        |name: &str, project: &str| ACCOUNT.replace("NAME", name).replace("PROJECT", project);
    let first = project_repository("derived-client-a");
    write(
        &first,
        "access/serviceaccounts/vendorx-parking-push.yaml",
        &account("vendorx-parking-push", "ovzdusie"),
    );
    let second = temp_dir("derived-client-b-project");
    write(&second, ".jc/layout", "2\n");
    write(
        &second,
        "project.yaml",
        &PROJECT.replace("name: ovzdusie", "name: ovzdusie-vendorx"),
    );
    let entry = ENTRY.replace("name: ovzdusie\n", "name: ovzdusie-vendorx\n");
    let org = organization(
        "derived-client",
        &[("ovzdusie", ENTRY), ("ovzdusie-vendorx", &entry)],
    );
    let into = temp_dir("derived-client-into").join("r");
    let alone = assemble(
        &org,
        &checkouts(&[("ovzdusie", &first), ("ovzdusie-vendorx", &second)]),
        &into,
        None,
    );
    assert!(alone.is_ok(), "{:?}", alone.err().map(|e| e.to_string()));

    write(
        &second,
        "access/serviceaccounts/parking-push.yaml",
        &account("parking-push", "ovzdusie-vendorx"),
    );
    let refused = assemble(
        &org,
        &checkouts(&[("ovzdusie", &first), ("ovzdusie-vendorx", &second)]),
        &into,
        None,
    )
    .expect("one client id derived twice refuses the project that changed (T-6709)");
    assert_eq!(refused.entries[0].refused, None);
    let refusal = refused.entries[1]
        .refused
        .as_ref()
        .expect("ovzdusie-vendorx is refused");
    assert!(
        refusal.reason.contains("ovzdusie-vendorx-parking-push"),
        "{refusal:?}"
    );
    let accounts: Vec<String> = refused
        .repository
        .iter()
        .filter(|(id, _)| id.kind == "ServiceAccount")
        .map(|(id, _)| id.to_string())
        .collect();
    assert_eq!(accounts, ["ServiceAccount/ovzdusie/vendorx-parking-push"]);
}
