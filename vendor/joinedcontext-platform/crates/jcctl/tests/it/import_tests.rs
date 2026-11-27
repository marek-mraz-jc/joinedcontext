//! `jcctl import`: a bundle from somewhere else, made this repository's (T-0134, MF-20,
//! MF-22, MF-23, MF-24, PF-22).

use crate::common;

use common::*;
use jcctl::commands::import::{collect, write, Conflict, Options, Outcome};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A bundle as `export` writes one: a project's manifests under their own paths.
fn bundle(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    write_file(
        &dir,
        "projects/vzduch/spaces/vzduch/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: vzduch\n  namespace: vzduch\nspec:\n  isSandbox: false\n",
    );
    write_file(
        &dir,
        "projects/vzduch/spaces/vzduch/endpoints/vzduch-public.yaml",
        r#"apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: vzduch-public
  namespace: vzduch
spec:
  contextSpaceRef: vzduch
  slug: 7fmk2hqz9xtc4arv6dn3ybs8pw
  audience: public
  enabledRepresentations: ["ngsi-ld"]
  seedEntity: urn:ngsi-ld:AirQualityObserved:kosice.sk:vzduch:s1
"#,
    );
    dir
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    common::write(dir, rel, body);
}

/// A destination repository that already holds the demo project.
fn destination(name: &str) -> PathBuf {
    demo_repo(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(args)
        .output()
        .expect("jcctl runs")
}

fn options(namespace: &str) -> Options {
    Options {
        namespace: Some(namespace.to_owned()),
        ..Options::default()
    }
}

/// A write that meets a file where a folder goes, or a folder where a file goes, fails naming
/// the path instead of writing half the bundle silently.
#[test]
fn a_write_blocked_by_the_tree_names_the_path() {
    let source = bundle("import-blocked-src");
    let dest = destination("import-blocked-dest");
    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    let first = report.imported[0].path.clone();
    let parent = dest.join(&first).parent().expect("a parent").to_path_buf();
    std::fs::remove_dir_all(&parent).ok();
    std::fs::create_dir_all(parent.parent().expect("a grandparent")).expect("the grandparent");
    std::fs::write(&parent, "a file where the folder goes").expect("block the folder");
    let error = write(&dest, &report).expect_err("the folder is a file");
    assert!(
        error.to_string().contains(
            &parent
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        ),
        "{error}"
    );

    std::fs::remove_file(&parent).expect("unblock");
    std::fs::create_dir_all(dest.join(&first)).expect("a folder where the file goes");
    let error = write(&dest, &report).expect_err("the file is a folder");
    assert!(
        error
            .to_string()
            .contains(&first.to_string_lossy().into_owned()),
        "{error}"
    );
}

#[test]
fn a_bundle_lands_under_the_target_project_at_the_paths_its_kinds_prescribe() {
    let source = bundle("import-namespace-src");
    let dest = destination("import-namespace-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    assert!(report.is_acceptable(), "{:?}", report.rejections);
    let paths: Vec<String> = report
        .imported
        .iter()
        .map(|i| i.path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        paths,
        vec![
            "projects/ovzdusie/spaces/vzduch/endpoints/vzduch-public.yaml".to_owned(),
            "projects/ovzdusie/spaces/vzduch/space.yaml".to_owned(),
        ]
    );
    assert!(report
        .imported
        .iter()
        .all(|i| i.manifest.metadata.namespace.as_deref() == Some("ovzdusie")));

    assert_eq!(write(&dest, &report).expect("import writes"), 2);
    assert!(dest
        .join("projects/ovzdusie/spaces/vzduch/space.yaml")
        .exists());

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn every_imported_manifest_says_where_it_came_from() {
    // MF-20: a reviewer reading the merge request has to be able to find the bundle.
    let source = bundle("import-provenance-src");
    let dest = destination("import-provenance-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    let annotations = report.imported[0].manifest.metadata.rest["annotations"]
        .as_object()
        .expect("annotations are an object");
    let from = annotations["joinedcontext.com/imported-from"]
        .as_str()
        .expect("the provenance is a string");
    assert!(from.contains("vzduch-public.yaml"), "{from}");

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn an_imported_legacy_title_map_is_written_as_one_string() {
    // UI-50: jcctl rewrites the map form on the next write of a manifest it touches.
    let source = bundle("import-title-src");
    write_file(
        &source,
        "projects/vzduch/spaces/vzduch/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: vzduch\n  namespace: vzduch\n  title: { sk: Vzduch, en: Air }\nspec:\n  isSandbox: false\n",
    );
    let dest = destination("import-title-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    let space = report
        .imported
        .iter()
        .find(|resource| resource.manifest.kind == "ContextSpace")
        .expect("the space is imported");
    assert_eq!(
        space.manifest.metadata.rest["title"],
        serde_json::json!("Air")
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn the_organisation_domain_of_every_entity_urn_is_rewritten() {
    // PF-22: a duplicated project keeps its shape and stops claiming the other city's ids.
    let source = bundle("import-urn-src");
    let dest = destination("import-urn-dest");
    let options = Options {
        namespace: Some("ovzdusie".to_owned()),
        org_domain: Some(("kosice.sk".to_owned(), "banskabystrica.sk".to_owned())),
        ..Options::default()
    };

    let report = collect(&source, &dest, &options).expect("import collects");

    let endpoint = report
        .imported
        .iter()
        .find(|i| i.manifest.kind == "Endpoint")
        .expect("the endpoint is in the bundle");
    assert_eq!(
        endpoint.manifest.spec["seedEntity"],
        "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:vzduch:s1",
        "the domain segment and nothing else"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

/// Each conflict policy has its name and its outcome: an existing space is replaced, renamed
/// or skipped as asked, and an unknown policy name is refused.
#[test]
fn each_conflict_policy_says_what_it_did() {
    assert_eq!(Conflict::parse("fail"), Some(Conflict::Fail));
    assert_eq!(Conflict::parse("overwrite"), None);
    for (name, word) in [
        ("replace", "REPLACED"),
        ("rename", "RENAMED"),
        ("skip", "SKIPPED"),
    ] {
        let source = temp_dir(&format!("import-policy-{name}"));
        write_file(
            &source,
            "projects/ovzdusie/spaces/ovzdusie/space.yaml",
            common::SPACE,
        );
        let dest = destination(&format!("import-policy-{name}-dest"));
        let options = Options {
            namespace: Some("ovzdusie".to_owned()),
            conflict: Conflict::parse(name).expect("a policy"),
            ..Options::default()
        };
        let report = collect(&source, &dest, &options).expect("import collects");
        let outcomes: Vec<&str> = report.imported.iter().map(|i| i.outcome.as_str()).collect();
        assert_eq!(outcomes, [word], "{name}");
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&dest);
    }
}

/// One file is a bundle too; its empty documents are skipped, a document that is no manifest
/// is a rejection the JSON report lists with its source, and the same resource twice is an
/// error naming it.
#[test]
fn a_single_file_bundle_with_a_broken_document_is_rejected_by_source() {
    let dir = temp_dir("import-file");
    let file = dir.join("bundle.yaml");
    std::fs::write(
        &file,
        format!("---\n---\n{}---\nkind: [unclosed\n", common::SPACE),
    )
    .expect("bundle");
    let dest = destination("import-file-dest");
    let report = collect(&file, &dest, &options("ovzdusie")).expect("import collects");
    assert!(!report.is_acceptable());
    let json = report.to_json();
    let rejections = json["rejections"].as_array().expect("a list");
    assert!(
        rejections.iter().any(|r| r["reason"]
            .as_str()
            .is_some_and(|r| r.contains("does not parse"))
            && r["source"]
                .as_str()
                .is_some_and(|s| s.ends_with("bundle.yaml:10"))),
        "{json}"
    );

    std::fs::write(&file, format!("{}---\n{}", common::SPACE, common::SPACE)).expect("twice");
    assert!(collect(&file, &dest, &options("ovzdusie")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn an_anchored_id_pattern_moves_to_the_importing_organisation() {
    // T-0826: a registration and a Policy anchor their coverage with a pattern, which is a URN
    // prefix behind a `^` with the domain's dots escaped (R33). Left alone it keeps routing at
    // the city the bundle came from.
    let source = temp_dir("import-pattern-src");
    write_file(
        &source,
        "projects/vzduch/spaces/vzduch/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: vzduch\n  namespace: vzduch\nspec:\n  isSandbox: false\n",
    );
    write_file(
        &source,
        "projects/vzduch/spaces/vzduch/csrs/kosice.yaml",
        r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSourceRegistration
metadata:
  name: kosice
  namespace: vzduch
spec:
  contextSpaceRef: vzduch
  endpoint: https://kosice.sk/ngsi-ld/v1
  information:
    - entities:
        - type: AirQualityObserved
          idPattern: "^urn:ngsi-ld:AirQualityObserved:kosice\\.sk:vzduch:.*$"
"#,
    );
    let dest = destination("import-pattern-dest");
    let options = Options {
        namespace: Some("ovzdusie".to_owned()),
        org_domain: Some(("kosice.sk".to_owned(), "banskabystrica.sk".to_owned())),
        ..Options::default()
    };

    let report = collect(&source, &dest, &options).expect("import collects");

    let registration = report
        .imported
        .iter()
        .find(|i| i.manifest.kind == "ContextSourceRegistration")
        .expect("the registration is in the bundle");
    assert_eq!(
        registration.manifest.spec["information"][0]["entities"][0]["idPattern"],
        r"^urn:ngsi-ld:AirQualityObserved:banskabystrica\.sk:vzduch:.*$",
        "the domain segment, escaped, and the space segment untouched"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn a_string_that_is_not_an_entity_urn_is_left_alone() {
    let source = temp_dir("import-urn-untouched-src");
    write_file(
        &source,
        "space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: vzduch\n  namespace: vzduch\nspec:\n  isSandbox: false\n  note: \"urn:ngsi-ld:short\"\n  home: \"https://kosice.sk/vzduch\"\n",
    );
    let dest = destination("import-urn-untouched-dest");
    let options = Options {
        namespace: Some("ovzdusie".to_owned()),
        org_domain: Some(("kosice.sk".to_owned(), "banskabystrica.sk".to_owned())),
        ..Options::default()
    };

    let report = collect(&source, &dest, &options).expect("import collects");

    let spec = &report.imported[0].manifest.spec;
    assert_eq!(
        spec["note"], "urn:ngsi-ld:short",
        "too few segments to be one"
    );
    assert_eq!(spec["home"], "https://kosice.sk/vzduch", "not a URN at all");

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn a_collision_is_refused_by_default_and_the_four_policies_each_do_their_own_thing() {
    // MF-23. The bundle is the destination's own endpoint, exported and re-imported.
    let source = temp_dir("import-conflict-src");
    write_file(&source, "endpoint.yaml", ENDPOINT);
    let dest = destination("import-conflict-dest");

    let failed = collect(&source, &dest, &Options::default()).expect("import collects");
    assert!(!failed.is_acceptable());
    assert!(failed.rejections[0]
        .reason
        .contains("already in the repository"));
    assert_eq!(write(&dest, &failed).expect("nothing is written"), 0);

    for (policy, expected) in [
        (Conflict::Skip, Outcome::Skipped),
        (Conflict::Replace, Outcome::Replaced),
        (
            Conflict::Rename,
            Outcome::Renamed("public-air-2".to_owned()),
        ),
    ] {
        let report = collect(
            &source,
            &dest,
            &Options {
                conflict: policy,
                ..Options::default()
            },
        )
        .expect("import collects");
        assert!(
            report.is_acceptable(),
            "{policy:?}: {:?}",
            report.rejections
        );
        assert_eq!(report.imported[0].outcome, expected, "{policy:?}");
    }

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn skipping_writes_nothing_and_renaming_writes_beside_the_existing_one() {
    let source = temp_dir("import-conflict-write-src");
    write_file(&source, "endpoint.yaml", ENDPOINT);
    let dest = destination("import-conflict-write-dest");
    let before = std::fs::read_to_string(dest.join(ENDPOINT_PATH)).expect("the original is there");

    let skipped = collect(
        &source,
        &dest,
        &Options {
            conflict: Conflict::Skip,
            ..Options::default()
        },
    )
    .expect("import collects");
    assert_eq!(write(&dest, &skipped).expect("import writes"), 0);
    assert_eq!(
        std::fs::read_to_string(dest.join(ENDPOINT_PATH)).unwrap(),
        before,
        "skip leaves the repository exactly as it was"
    );

    let renamed = collect(
        &source,
        &dest,
        &Options {
            conflict: Conflict::Rename,
            ..Options::default()
        },
    )
    .expect("import collects");
    assert_eq!(write(&dest, &renamed).expect("import writes"), 1);
    assert!(dest
        .join("projects/ovzdusie/spaces/ovzdusie/endpoints/public-air-2.yaml")
        .exists());
    assert_eq!(
        std::fs::read_to_string(dest.join(ENDPOINT_PATH)).unwrap(),
        before,
        "and the one it was renamed away from is untouched"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn a_literal_credential_is_refused_and_the_whole_import_stops() {
    // MF-24. The credential must not reach the repository, and neither must the half of the
    // bundle that came before it: a repository that half-validates is worse than no import.
    let source = bundle("import-secret-src");
    write_file(
        &source,
        "projects/vzduch/datasources/feed.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: feed\n  namespace: vzduch\nspec:\n  kind: mqtt\n  url: mqtts://broker.kosice.sk\n  password: hunter2\n",
    );
    let dest = destination("import-secret-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    assert!(!report.is_acceptable());
    assert!(
        report
            .rejections
            .iter()
            .any(|r| r.reason.contains("password")),
        "{:?}",
        report.rejections
    );
    assert_eq!(write(&dest, &report).expect("nothing is written"), 0);
    assert!(!dest
        .join("projects/ovzdusie/spaces/vzduch/space.yaml")
        .exists());

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn an_unknown_kind_and_an_unserved_api_version_are_both_refused() {
    let source = temp_dir("import-unknown-src");
    write_file(
        &source,
        "strange.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Telepath\nmetadata:\n  name: t\n  namespace: vzduch\nspec: {}\n",
    );
    write_file(
        &source,
        "old.yaml",
        "apiVersion: joinedcontext.com/v1\nkind: ContextSpace\nmetadata:\n  name: old\n  namespace: vzduch\nspec:\n  isSandbox: false\n",
    );
    let dest = destination("import-unknown-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    let reasons: Vec<&str> = report
        .rejections
        .iter()
        .map(|r| r.reason.as_str())
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("Telepath")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("apiVersion")),
        "{reasons:?}"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

/// MF-20, MF-22: an organization's own manifest keeps no project namespace; one without a
/// namespace lands in the target; a typed reference into the source project follows it; an
/// empty document is no resource; annotations that are no map are left for validation.
#[test]
fn namespaces_typed_references_and_odd_documents_are_rewritten_as_they_say() {
    let source = temp_dir("import-rewrites-src");
    write_file(
        &source,
        "bundle.yaml",
        "# nothing but a comment\n---\napiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: kosice\nspec:\n  domain: kosice.sk\n---\napiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: bare\n  annotations: 5\nspec:\n  isSandbox: true\n---\napiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: typed\n  namespace: vzduch\nspec:\n  contextSpaceRef: { kind: ContextSpace, name: bare, namespace: vzduch }\n  slug: 5cnx8we2rt7qh4vaj3md6ykbzp\n  audience: project\n  enabledRepresentations: [\"ngsi-ld\"]\n",
    );
    let dest = destination("import-rewrites-dest");
    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    let of = |name: &str| {
        report
            .imported
            .iter()
            .find(|i| i.manifest.metadata.name == name)
            .map(|i| i.manifest.clone())
            .expect("imported")
    };
    assert_eq!(of("kosice").metadata.namespace, None);
    assert_eq!(of("bare").metadata.namespace.as_deref(), Some("ovzdusie"));
    assert_eq!(of("bare").metadata.rest["annotations"], 5);
    assert_eq!(of("typed").spec["contextSpaceRef"]["namespace"], "ovzdusie");
    assert_eq!(report.imported.len(), 3, "{:?}", report.rejections);
}

/// MF-24: a typed reference that resolves nowhere names its kind; a file that is no text
/// stops the import naming it.
#[test]
fn a_dangling_typed_reference_and_a_file_that_is_no_text() {
    let source = temp_dir("import-typed-dangling-src");
    write_file(
        &source,
        "endpoint.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: orphan\n  namespace: vzduch\nspec:\n  contextSpaceRef: { kind: ContextSpace, name: nowhere }\n  slug: 5cnx8we2rt7qh4vaj3md6ykbzp\n  audience: project\n  enabledRepresentations: [\"ngsi-ld\"]\n",
    );
    let dest = destination("import-typed-dangling-dest");
    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    let reasons: Vec<&str> = report
        .rejections
        .iter()
        .map(|r| r.reason.as_str())
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("ContextSpace/nowhere")),
        "{reasons:?}"
    );
    std::fs::write(source.join("binary.yaml"), [0xff_u8, 0xfe, 0x00]).expect("a binary file");
    assert!(collect(&source, &dest, &options("ovzdusie")).is_err());
}

#[test]
fn a_reference_to_something_that_exists_nowhere_is_refused() {
    // MF-24: an import whose references dangle produces a repository that does not validate,
    // and the merge request would be the first place anyone found out.
    let source = temp_dir("import-dangling-src");
    write_file(
        &source,
        "endpoint.yaml",
        r#"apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: orphan
  namespace: vzduch
spec:
  contextSpaceRef: a-space-nobody-has
  slug: 5cnx8we2rt7qh4vaj3md6ykbzp
  audience: project
  enabledRepresentations: ["ngsi-ld"]
"#,
    );
    let dest = destination("import-dangling-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    assert!(!report.is_acceptable());
    assert!(
        report.rejections[0].reason.contains("a-space-nobody-has"),
        "{:?}",
        report.rejections
    );

    // The same endpoint pointing at a space the destination already has is fine.
    write_file(
        &source,
        "endpoint.yaml",
        &std::fs::read_to_string(source.join("endpoint.yaml"))
            .unwrap()
            .replace("a-space-nobody-has", "ovzdusie"),
    );
    let ok = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    assert!(ok.is_acceptable(), "{:?}", ok.rejections);

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn a_compressed_archive_is_refused_with_what_to_do_about_it() {
    let dest = destination("import-archive-dest");
    let archive = dest.join("bundle.tgz");
    std::fs::write(&archive, b"not really a tarball").unwrap();

    let err = collect(&archive, &dest, &Options::default()).expect_err("an archive is refused");
    assert!(err.to_string().contains("extract it"), "{err}");

    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn the_bundle_index_is_not_imported_as_a_resource() {
    // MF-17: `kind: Bundle` describes the bundle. Importing it would put the packing list
    // into the repository as though it were configuration.
    let source = bundle("import-index-src");
    write_file(
        &source,
        "bundle.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Bundle\nmetadata:\n  name: vzduch-export\n  namespace: vzduch\nspec:\n  resources: []\n",
    );
    let dest = destination("import-index-dest");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");

    assert!(report.is_acceptable(), "{:?}", report.rejections);
    assert!(report.imported.iter().all(|i| i.manifest.kind != "Bundle"));

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn the_command_writes_the_bundle_and_reports_what_it_did() {
    let source = bundle("import-cli-src");
    let dest = destination("import-cli-dest");

    let output = run(&[
        "import",
        source.to_str().unwrap(),
        "--repo-dir",
        dest.to_str().unwrap(),
        "--namespace",
        "ovzdusie",
        "--json",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(report["summary"]["imported"], 2);
    assert_eq!(report["summary"]["rejected"], 0);
    assert!(dest
        .join("projects/ovzdusie/spaces/vzduch/space.yaml")
        .exists());

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn the_command_exits_one_and_writes_nothing_when_the_bundle_is_refused() {
    let source = temp_dir("import-cli-bad-src");
    write_file(
        &source,
        "bad.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Telepath\nmetadata:\n  name: t\n  namespace: vzduch\nspec: {}\n",
    );
    let dest = destination("import-cli-bad-dest");

    let output = run(&[
        "import",
        source.to_str().unwrap(),
        "--repo-dir",
        dest.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Telepath"));

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&dest);
}

/// CC-08: no `jcctl` command reads a file outside the directory it was given.
///
/// `documents` walked the bundle with `WalkDir` and read every `*.yaml` it found, links included,
/// so a link planted in an unpacked bundle put a manifest from anywhere on the machine into the
/// plan. The walk is the loader's now, and the loader has refused this all along (T-1478).
#[test]
fn import_refuses_a_link_out_of_the_source() {
    let source = bundle("import-link-escape-src");
    let dest = destination("import-link-escape-dest");
    let outside = temp_dir("import-link-escape-outside");
    write_file(
        &outside,
        "elsewhere.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: smuggled\n  namespace: vzduch\nspec:\n  isSandbox: true\n",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.join("elsewhere.yaml"),
        source.join("projects/vzduch/spaces/vzduch/smuggled.yaml"),
    )
    .expect("plant the link");

    let refused = collect(&source, &dest, &options("ovzdusie"))
        .expect_err("a link out of the bundle is refused");
    let said = refused.to_string();
    assert!(
        said.contains("escapes repository root"),
        "the refusal says why: {said}"
    );
    // The path it names is the one inside the bundle; where the link pointed is not repeated back.
    assert!(
        said.contains("projects/vzduch/spaces/vzduch/smuggled.yaml"),
        "the refusal names the file: {said}"
    );
    assert!(
        !said.contains("elsewhere.yaml") && !said.contains("isSandbox"),
        "neither the target nor its contents are in the message: {said}"
    );
}

/// A link inside the bundle is still read: an unpacked archive may hold them, and the rule is
/// about leaving the tree, not about links.
#[test]
fn import_reads_a_link_that_stays_inside_the_source() {
    let source = bundle("import-link-inside-src");
    let dest = destination("import-link-inside-dest");
    // A staged copy inside the bundle, linked to from where the walk looks.
    write_file(
        &source,
        "projects/vzduch/.staged/voda.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: voda\n  namespace: vzduch\nspec:\n  isSandbox: false\n",
    );
    std::fs::create_dir_all(source.join("projects/vzduch/spaces/voda"))
        .expect("create the space directory the link lives in");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        source.join("projects/vzduch/.staged/voda.yaml"),
        source.join("projects/vzduch/spaces/voda/space.yaml"),
    )
    .expect("plant the link");

    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    let paths: Vec<String> = report
        .imported
        .iter()
        .map(|imported| imported.path.to_string_lossy().into_owned())
        .collect();
    assert!(
        paths.iter().any(|path| path.contains("voda")),
        "the linked document was read: {paths:?}"
    );
}

/// T-5702 (F-PLT3-001): a bundle from another instance names its own files. A space named
/// `../../../tmp/pwn` used to build a path outside `--repo-dir`.
#[test]
fn a_bundle_name_that_climbs_out_of_the_repository_is_refused() {
    let source = bundle("import-climb");
    write_file(
        &source,
        "projects/vzduch/spaces/vzduch/endpoints/climb.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: ../../../../climbed\n  namespace: vzduch\nspec:\n  isSandbox: false\n",
    );
    let dest = destination("import-climb-dest");
    let report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    assert!(
        report.rejections.iter().any(
            |r| r.source.contains("climb.yaml") && r.reason.contains("is not a DNS-1123 label")
        ),
        "{:?}",
        report.rejections
    );
    assert_eq!(write(&dest, &report).expect("nothing to write"), 0);
}

#[test]
fn write_refuses_a_path_outside_the_repository_whatever_collect_let_through() {
    let source = bundle("import-escape");
    let dest = destination("import-escape-dest");
    let mut report = collect(&source, &dest, &options("ovzdusie")).expect("import collects");
    report.imported[0].path = PathBuf::from("../escaped.yaml");
    let err = write(&dest, &report).expect_err("refused");
    assert!(
        err.to_string().contains("not a path inside the repository"),
        "{err}"
    );
    assert!(!dest.join("../escaped.yaml").exists());
}
