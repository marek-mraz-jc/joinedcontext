//! Every reference app in the repository is a manifest the reconciler can actually read
//! (T-0310, AP-34, AP-37, AP-39).
//!
//! The app crates deliberately do not depend on `jc-core`: an app has to build and run
//! outside this platform. That leaves nothing checking that `apps/*/app.yaml` still parses
//! after a change to the kind, which is exactly the kind of drift that is found on a cluster
//! instead of in CI. The Portal already owns the reconciler and the contract, so the check
//! lives here.

use jc_core::kinds::{App, EndpointSlug};
use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::reconciler::{generate_slug, grants, RenderError};
use joinedcontext_portal::resource::{by_kind, repository_path};

/// Reads every `apps/*/app.yaml` beside the Portal, in path order so a failure names the same
/// app on every machine.
fn reference_apps() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps");
    let mut found: Vec<_> = std::fs::read_dir(&root)
        .expect("the apps folder exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("app.yaml"))
        .filter(|manifest| manifest.is_file())
        .map(|manifest| {
            let name = manifest
                .parent()
                .and_then(|dir| dir.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (
                name,
                std::fs::read_to_string(&manifest).expect("a readable manifest"),
            )
        })
        .collect();
    found.sort();
    found
}

#[test]
fn every_reference_app_manifest_parses_as_the_kind_the_reconciler_reads() {
    let apps = reference_apps();
    assert!(!apps.is_empty(), "no apps/*/app.yaml found");
    for (name, yaml) in apps {
        let app: App = serde_norway::from_str(&yaml)
            .unwrap_or_else(|error| panic!("apps/{name}/app.yaml does not parse: {error}"));
        assert_eq!(app.metadata.name, name, "the folder names the app");
        assert!(
            !app.spec.data_needs.is_empty(),
            "apps/{name} declares no data needs, so it has nothing to reach (AP-04)"
        );
    }
}

/// AP-124. A sample App is copied, so it spells its shape the current way: `static` and
/// `fullstack` still parse, and a copy of one carried the old name into the seed (T-4076).
#[test]
fn no_sample_app_spells_its_shape_with_a_renamed_kind() {
    for (name, yaml) in reference_apps() {
        let manifest: serde_norway::Value = serde_norway::from_str(&yaml).expect("a manifest");
        let kind = manifest["spec"]["kind"].as_str().unwrap_or_default();
        assert_eq!(
            jc_core::kinds::AppClass::renamed(kind),
            None,
            "apps/{name}/app.yaml says spec.kind {kind}"
        );
    }
}

/// AP-35. The annotations are an agent's attribution; hand-written source claiming them would
/// make the provenance trail say something untrue.
#[test]
fn a_hand_written_app_claims_no_agent_attribution() {
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        for annotation in [
            "joinedcontext.com/generated-by",
            "joinedcontext.com/prompt-digest",
        ] {
            assert!(
                !app.metadata.annotations.contains_key(annotation),
                "apps/{name} claims {annotation} but is written by hand"
            );
        }
    }
}

/// AP-39. A write is the one thing that puts an app's publication in the red lane, so the
/// list of apps that can write is worth stating out loud rather than discovering later.
///
/// Two apps write the steward's note alone, the one attribute of their model that no pipeline
/// overwrites (AP-62, T-2434); two carry a steward's record form (T-2598, T-2617); one lets a steward
/// correct an attribute a finding names (T-3827). Any other app
/// declaring a write fails this test until somebody adds it here on purpose.
#[test]
fn only_the_note_is_ever_written_and_only_by_the_apps_named_here() {
    let writes = [
        "createEntity",
        "updateEntity",
        "updateAttrs",
        "appendAttrs",
        "deleteAttrs",
        "deleteEntity",
        "mergeEntity",
        "replaceEntity",
        "replaceAttrs",
    ];
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        let writing: Vec<_> = app
            .spec
            .data_needs
            .iter()
            .flat_map(|need| need.operations.iter())
            .map(|operation| operation.as_str())
            .filter(|operation| writes.contains(operation))
            .collect();
        match name.as_str() {
            "banskabystrica-zaznamy" | "bbsk-zaznamy" => {
                assert_eq!(writing, vec!["updateAttrs"], "{name} writes one way only");
                let attrs: Vec<_> = app
                    .spec
                    .data_needs
                    .iter()
                    .flat_map(|need| need.attrs.iter())
                    .collect();
                assert!(
                    attrs.iter().any(|attr| *attr == "stewardNote"),
                    "the note attribute has to be in the grant"
                );
                // A need's attrs are its Policy's propertyNames for every operation it names, so
                // the write sits in a need of its own on the note alone: a person never writes a
                // published figure the next pipeline run would overwrite (AP-62, T-3596).
                for need in &app.spec.data_needs {
                    if need
                        .operations
                        .iter()
                        .any(|op| writes.contains(&op.as_str()))
                    {
                        assert_eq!(need.attrs, ["stewardNote"], "{name} writes the note alone");
                    }
                }
            }
            // T-2598, T-2617: the steward's record form. Every write is granted to `steward`
            // alone and none reaches `source`; the next test holds the rest of its shape.
            "helsinki-alerts" | "air-quality" => {
                assert_eq!(
                    writing,
                    vec!["createEntity", "updateAttrs", "deleteEntity"],
                    "{name} writes through its record form only"
                );
                for need in &app.spec.data_needs {
                    let writes_here = need
                        .operations
                        .iter()
                        .any(|operation| writes.contains(&operation.as_str()));
                    if writes_here {
                        assert_eq!(
                            need.roles,
                            vec!["steward"],
                            "{name}: a write not gated to steward"
                        );
                        assert!(
                            !need.attrs.iter().any(|attr| attr == "source"),
                            "{name} writes source"
                        );
                    }
                }
            }
            // T-3827: a steward corrects one attribute of an entity a finding names; never a
            // create or a delete, never `source`, and no other role.
            "data-quality-inspector" => {
                assert_eq!(writing, vec!["updateAttrs"], "{name} only corrects");
                for need in &app.spec.data_needs {
                    if need
                        .operations
                        .iter()
                        .any(|operation| writes.contains(&operation.as_str()))
                    {
                        assert_eq!(
                            need.roles,
                            vec!["steward"],
                            "{name}: a write not gated to steward"
                        );
                    }
                }
            }
            other => assert!(
                writing.is_empty(),
                "apps/{other} declares writes {writing:?}; add it to this test on purpose"
            ),
        }
    }
}

/// T-2437. The city's records and the region's are one screen published twice: the same `ui/src`,
/// byte for byte, and two manifests. Copied rather than shared because a published App belongs to
/// one project and one space, and this repository binds one app name to one folder — so this is
/// what stops the two copies drifting apart, the way the seed holds its two copies of one model.
#[test]
fn the_two_record_grids_are_the_same_screen_published_twice() {
    let apps = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps");
    let city = apps.join("banskabystrica-zaznamy/ui/src");
    let region = apps.join("bbsk-zaznamy/ui/src");

    fn files(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("a readable directory")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let name = path
                        .strip_prefix(root)
                        .expect("inside the root")
                        .to_string_lossy()
                        .into_owned();
                    found.push((name, std::fs::read(&path).expect("a readable file")));
                }
            }
        }
        found.sort();
        found
    }

    let one = files(&city);
    let other = files(&region);
    assert!(!one.is_empty(), "the city's grid has no source");
    assert_eq!(
        one.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        other.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "the two grids hold different files"
    );
    for ((name, left), (_, right)) in one.iter().zip(other.iter()) {
        assert_eq!(
            left, right,
            "apps/*/ui/src/{name} differs between the two grids"
        );
    }
}

/// AP-01. A `static` app has no crate, and cargo's `apps/*` glob refuses a member directory
/// without a `Cargo.toml` rather than skipping it, so every one of them is excluded by name in
/// the workspace manifest. Forgetting that is a workspace that does not load at all.
#[test]
fn every_static_app_is_excluded_from_the_cargo_workspace() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("the workspace manifest");
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        let has_crate = root.join("apps").join(&name).join("Cargo.toml").is_file();
        if has_crate {
            continue;
        }
        assert!(
            manifest.contains(&format!("\"apps/{name}\"")),
            "apps/{name} has no Cargo.toml; name it in the workspace `exclude` list ({:?})",
            app.spec.class
        );
    }
}

/// AP-75, AP-80, AP-87 (T-2596). An application kept in its own repository names that repository
/// and not a folder, and its tree is what the build lane builds: `package.json` and `index.html`
/// at the root for a Vite project, no `ui/` below it.
#[test]
fn an_app_in_its_own_repository_names_it_and_keeps_the_shape_the_lane_builds() {
    let apps = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps");
    let mut seen = 0;
    for (name, yaml) in reference_apps() {
        let dir = apps.join(&name);
        if !dir.join("package.json").is_file() {
            continue;
        }
        seen += 1;
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        let namespace = app.metadata.namespace.clone().unwrap_or_default();
        let git = app.spec.source.git.as_ref().unwrap_or_else(|| {
            panic!("apps/{name} sits at its repository's root but names no spec.source.git")
        });
        assert!(
            app.spec.source.path.is_none(),
            "apps/{name} names a folder as well (AP-87)"
        );
        assert!(
            git.url.starts_with("https://")
                && git.url.ends_with(&format!("/{namespace}_{name}.git")),
            "apps/{name} names {} instead of its own repository {namespace}_{name} (AP-75)",
            git.url
        );
        assert!(
            git.path.is_none(),
            "apps/{name} is the whole repository, not a folder of it"
        );
        assert!(
            dir.join("index.html").is_file(),
            "apps/{name} has no index.html at its root"
        );
        assert!(
            !dir.join("ui").exists(),
            "apps/{name} keeps the old ui/ folder beside its root"
        );
    }
    assert!(
        seen > 0,
        "no application in its own repository layout under apps/"
    );
}

/// AP-87. `joinedcontext.com/shipped-with: portal` lets a published static App name no
/// repository because the Portal image carries its bundle; the mark is true exactly for the
/// apps the Dockerfile builds into `/srv/apps`, so a mark without a bundle or a bundle without
/// its mark is found here, not as a 404 or a refused proposal on a cluster.
#[test]
fn the_shipped_mark_is_on_exactly_the_bundles_the_image_builds() {
    use jc_core::kinds::app::SHIPPED_WITH_ANNOTATION;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dockerfile = std::fs::read_to_string(root.join("Dockerfile")).expect("the Dockerfile");
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        let marked = app
            .metadata
            .annotations
            .get(SHIPPED_WITH_ANNOTATION)
            .is_some_and(|value| value == "portal");
        let built = dockerfile.contains(&format!("/srv/apps/{name} "))
            || dockerfile.contains(&format!("/srv/apps/{name}\n"));
        assert_eq!(
            marked, built,
            "apps/{name}: the shipped mark says {marked}, the Dockerfile builds it: {built}"
        );
    }
}

/// T-4987. `tikv-jemalloc-sys` (locked through `context-gateway` since T-4782) builds jemalloc
/// with `make` in its build script, and the slim Rust image has no `make`: the image build died
/// on "failed to execute command: No such file or directory" while every cargo lane stayed
/// green. The build stage installs it whenever the lock carries that crate.
#[test]
fn the_image_build_stage_has_make_when_the_lock_builds_jemalloc() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("the Cargo.lock");
    if !lock.contains("name = \"tikv-jemalloc-sys\"") {
        return;
    }
    let dockerfile = std::fs::read_to_string(root.join("Dockerfile")).expect("the Dockerfile");
    let build = dockerfile
        .split("\nFROM ")
        .find(|stage| stage.contains(" AS build\n"))
        .expect("the Dockerfile's build stage");
    let installs_make = build.lines().any(|line| {
        line.contains("apt-get install") && line.split_whitespace().any(|word| word == "make")
    });
    assert!(
        installs_make,
        "the build stage must apt-get install make: tikv-jemalloc-sys runs it to build jemalloc"
    );
}

/// AP-83, AP-01, AP-75. The SDK's plain-HTML example (a published App is React since AP-138,
/// T-2923): a static App with no build step, whose folder is the bundle (`index.html` at its
/// root, no toolchain file), and whose manifest passes the validation `jcctl validate` runs.
#[test]
fn the_plain_html_example_is_its_own_bundle_and_a_valid_app() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/examples/plain-html-events");
    let yaml = std::fs::read_to_string(root.join("app.yaml"))
        .expect("sdk/examples/plain-html-events/app.yaml");
    match jc_core::registry::validate_yaml("App", &yaml) {
        Some(Ok(_)) => {}
        other => panic!("jcctl validate refuses sdk/examples/plain-html-events: {other:?}"),
    }
    let app: App = serde_norway::from_str(&yaml).expect("a manifest");
    assert_eq!(app.spec.class, jc_core::AppClass::Ui);
    assert!(app.spec.build.0.is_empty(), "no build step (AP-83)");
    assert!(
        app.spec.source.git.is_some(),
        "published from its own repository (AP-87)"
    );
    assert!(root.join("index.html").is_file(), "index.html at the root");
    for toolchain in [
        "package.json",
        "Cargo.toml",
        "vite.config.ts",
        "vitest.config.mjs",
    ] {
        assert!(
            !root.join(toolchain).exists(),
            "{toolchain} in a folder that is served as it is"
        );
    }
}

/// AP-01, AP-09, AP-40, AP-92, AP-96. The functions sample is what T-2599 pushes to its own
/// repository unchanged: a manifest `jcctl validate` accepts, opened only by a person holding one
/// of its two roles, its functions beside the pages, and its steward able to remove only a record
/// with no `source`, which the gateway checks against the stored entity (R45).
#[test]
fn the_functions_sample_has_two_roles_and_a_steward_gated_record_form() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps/helsinki-alerts");
    let (_, yaml) = reference_apps()
        .into_iter()
        .find(|(name, _)| name == "helsinki-alerts")
        .expect("apps/helsinki-alerts/app.yaml");
    match jc_core::registry::validate_yaml("App", &yaml) {
        Some(Ok(_)) => {}
        other => panic!("jcctl validate refuses apps/helsinki-alerts: {other:?}"),
    }
    let app: App = serde_norway::from_str(&yaml).expect("a manifest");
    assert_eq!(app.spec.class, jc_core::AppClass::Ui);
    assert_eq!(app.spec.visibility, jc_core::kinds::AppVisibility::Roles);
    let roles: Vec<_> = app
        .spec
        .roles
        .iter()
        .map(|role| role.name.as_str())
        .collect();
    assert_eq!(roles, ["viewer", "steward"]);
    let delete = app
        .spec
        .data_needs
        .iter()
        .find(|need| {
            need.operations
                .iter()
                .any(|op| op.as_str() == "deleteEntity")
        })
        .expect("the steward's delete item");
    assert_eq!(
        delete.operations.len(),
        1,
        "delete stands alone, with its own filter"
    );
    assert_eq!(delete.q.as_deref(), Some("!source"));
    for file in [
        "index.html",
        "functions/summary.ts",
        "functions/expiring.ts",
    ] {
        assert!(
            root.join(file).is_file(),
            "apps/helsinki-alerts has no {file}"
        );
    }
}

/// AP-100. A reference application that lives in its own repository builds itself there, with the
/// workflow the Portal writes into every generated one: the same bytes as the template's, so the
/// sample on dev and a generated application build the same way (T-2609).
#[test]
fn every_reference_app_in_its_own_repository_carries_the_templates_workflow() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let template = std::fs::read_to_string(root.join("sdk/template/.gitea/workflows/build.yml"))
        .expect("the template's workflow");
    // AP-105: a fullstack reference app carries the fullstack template's, wherever its source
    // lives now, so the repository it is seeded into builds on its first push.
    let fullstack =
        std::fs::read_to_string(root.join("sdk/template-fullstack/.gitea/workflows/build.yml"))
            .expect("the fullstack template's workflow");
    // AP-142: a ui app that compiles Rust to WebAssembly carries the SDK's React + Rust example's,
    // which builds on the app-build-rust runner.
    let wasm = std::fs::read_to_string(
        root.join("sdk/examples/react-rust-wasm/.gitea/workflows/build.yml"),
    )
    .expect("the WebAssembly example's workflow");
    let mut seen = 0;
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        let workflow = root
            .join("apps")
            .join(&name)
            .join(".gitea/workflows/build.yml");
        if app.spec.class == jc_core::kinds::AppClass::UiRust {
            assert_eq!(
                std::fs::read_to_string(&workflow).ok().as_deref(),
                Some(fullstack.as_str()),
                "apps/{name} is fullstack, so it carries the fullstack template's build.yml"
            );
            continue;
        }
        if app.spec.source.git.is_none() {
            continue;
        }
        seen += 1;
        let wants_wasm = app.spec.build.0.contains_key("rust");
        assert_eq!(
            std::fs::read_to_string(&workflow).ok().as_deref(),
            Some(if wants_wasm {
                wasm.as_str()
            } else {
                template.as_str()
            }),
            "apps/{name} names spec.source.git, so it carries the template's build.yml"
        );
    }
    assert!(seen > 0, "no reference app lives in its own repository");
}

/// AP-94, AP-109, AP-110 (T-2617). The two fullstack samples as they are seeded on `dev`: each a
/// manifest `jcctl validate` accepts, built from its own forge repository; `hsl-transport` public,
/// `air-quality` open to its project with the roles viewer and steward, and its record form never
/// reaching a measured value, the pipeline's `dateObserved` or `source`.
#[test]
fn the_fullstack_samples_build_from_their_own_repositories_with_their_visibility() {
    use jc_core::kinds::AppVisibility;
    for (name, visibility) in [
        ("hsl-transport", AppVisibility::Public),
        ("air-quality", AppVisibility::Project),
    ] {
        let (_, yaml) = reference_apps()
            .into_iter()
            .find(|(found, _)| found == name)
            .unwrap_or_else(|| panic!("apps/{name}/app.yaml"));
        match jc_core::registry::validate_yaml("App", &yaml) {
            Some(Ok(_)) => {}
            other => panic!("jcctl validate refuses apps/{name}: {other:?}"),
        }
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        assert_eq!(app.spec.class, jc_core::AppClass::UiRust, "{name}");
        assert_eq!(app.spec.visibility, visibility, "{name}");
        let source = serde_json::to_value(&app.spec.source).expect("a source");
        assert_eq!(
            source["git"]["url"],
            serde_json::json!(format!(
                "https://dev.joinedcontext.com/git/joinedcontext/helsinki_{name}.git"
            )),
            "{name} builds from its own repository"
        );
    }

    let (_, yaml) = reference_apps()
        .into_iter()
        .find(|(found, _)| found == "air-quality")
        .expect("apps/air-quality/app.yaml");
    let app: App = serde_norway::from_str(&yaml).expect("a manifest");
    let roles: Vec<_> = app
        .spec
        .roles
        .iter()
        .map(|role| role.name.as_str())
        .collect();
    assert_eq!(roles, ["viewer", "steward"]);
    for need in &app.spec.data_needs {
        if need.roles.is_empty() {
            continue;
        }
        for measured in ["pm10", "pm25", "airQualityIndex", "dateObserved", "source"] {
            assert!(
                !need.attrs.iter().any(|attr| attr == measured),
                "the steward's form may write {measured}"
            );
        }
    }
}

/// The `(name, version)` of every `[[package]]` of a Cargo.lock.
fn locked_packages(lock: &str) -> std::collections::BTreeSet<(String, String)> {
    let mut found = std::collections::BTreeSet::new();
    let mut name = None;
    for line in lock.lines() {
        let value = |key: &str| {
            line.strip_prefix(key)
                .map(|rest| rest.trim().trim_matches('"').to_owned())
        };
        if line == "[[package]]" {
            name = None;
        } else if let Some(found_name) = value("name = ") {
            name = Some(found_name);
        } else if let (Some(version), Some(package)) = (value("version = "), name.take()) {
            found.insert((package, version));
        }
    }
    found
}

/// AP-106 (T-2617). A fullstack sample builds in its own forge repository with `cargo test
/// --offline --locked` against the crate store the rust-1.90 runner fetched from this
/// repository's Cargo.lock, so its own lock names no crate the store lacks. When a bump here
/// breaks this, regenerate the app's lock from the workspace's: copy `Cargo.lock` into a copy
/// of `apps/<name>/` and run `cargo tree --offline` there, which keeps the locked versions and
/// drops the rest.
#[test]
fn a_fullstack_samples_lock_names_only_crates_the_runners_store_carries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let store = locked_packages(
        &std::fs::read_to_string(root.join("Cargo.lock")).expect("the workspace lock"),
    );
    assert!(store.len() > 100, "the workspace lock did not parse");
    let mut seen = 0;
    for (name, yaml) in reference_apps() {
        let app: App = serde_norway::from_str(&yaml).expect("a manifest");
        if app.spec.class != jc_core::kinds::AppClass::UiRust {
            continue;
        }
        seen += 1;
        let lock = std::fs::read_to_string(root.join("apps").join(&name).join("Cargo.lock"))
            .unwrap_or_else(|_| {
                panic!("apps/{name} has no Cargo.lock, so --locked fails on the forge")
            });
        let packages = locked_packages(&lock);
        assert!(
            packages.iter().any(|(package, _)| package == &name),
            "apps/{name}/Cargo.lock does not lock the app itself"
        );
        let missing: Vec<_> = packages
            .iter()
            .filter(|(package, _)| package != &name)
            .filter(|package| !store.contains(*package))
            .collect();
        assert!(
            missing.is_empty(),
            "apps/{name}/Cargo.lock names crates outside the store: {missing:?}"
        );
    }
    assert_eq!(
        seen, 2,
        "hsl-transport and air-quality are the fullstack samples"
    );
}

/// The publisher each seeded project names in its `spec.domain` (PF-108; the deployment's
/// `components/context-gateway/seed/*` Projects), else the dev seed's one Organization, hel.fi.
fn seed_publisher(project: &str) -> &'static str {
    match project {
        "zilina" => "zilina.sk",
        "banskabystrica" => "banskabystrica.sk",
        "bbsk" => "bbsk.sk",
        "praha" => "praha.eu",
        _ => "hel.fi",
    }
}

/// The Endpoint and Policies each sample app compiles to, as `(repository path, yaml)` in the
/// layout-1 tree (`projects/{project}/…`), with `slug` for its Endpoint. `None` for an app that
/// runs nowhere (a draft or a retired one grants nothing).
fn compiled_grants(yaml: &str, slug: &EndpointSlug) -> Option<Vec<(String, String)>> {
    let manifest: RawManifest = serde_norway::from_str(yaml).expect("a manifest");
    let publisher = seed_publisher(manifest.metadata.namespace.as_deref().unwrap_or_default());
    let (endpoint, policies) = match grants(&manifest, slug, publisher) {
        Ok(compiled) => compiled,
        Err(RenderError::NotDeployable { .. }) => return None,
        Err(error) => panic!("{}: {error}", manifest.metadata.name),
    };
    let project = manifest
        .metadata
        .namespace
        .clone()
        .expect("an app names its project");
    Some(
        std::iter::once(endpoint)
            .chain(policies)
            .map(|raw| {
                // Where the Portal's door files it (mutate.rs `resolve_repo_path`): an Endpoint
                // under the space it serves, a Policy under the project's own.
                let space = match raw.kind.as_str() {
                    "Endpoint" => raw.spec["contextSpaceRef"]["name"]
                        .as_str()
                        .map(str::to_owned),
                    _ => None,
                }
                .unwrap_or_else(|| project.clone());
                let info = by_kind(&raw.kind).expect("a catalogued kind");
                let path = repository_path(info, &project, Some(&space), &raw.metadata.name)
                    .expect("a repository path");
                (path, serde_norway::to_string(&raw).expect("yaml"))
            })
            .collect(),
    )
}

/// `apps/{name}/grants/`, one file per manifest at its repository path below it: the forge
/// bootstrap commits each beside the app's manifest (T-2667).
fn grants_dir(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("apps")
        .join(name)
        .join("grants")
}

/// Every file below `dir`, as `(path relative to it, text)`, in path order.
fn files_below(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).expect("below the dir");
                found.push((
                    relative.to_string_lossy().into_owned(),
                    std::fs::read_to_string(&path).expect("a readable grant"),
                ));
            }
        }
    }
    found.sort();
    found
}

/// The slug the app's committed Endpoint keeps, read back from its grants.
fn pinned_slug(name: &str) -> Option<EndpointSlug> {
    files_below(&grants_dir(name))
        .into_iter()
        .find_map(|(_, text)| {
            let raw: RawManifest = serde_norway::from_str(&text).ok()?;
            (raw.kind == "Endpoint")
                .then(|| {
                    raw.spec["slug"]
                        .as_str()
                        .and_then(|s| EndpointSlug::new(s).ok())
                })
                .flatten()
        })
}

/// The app's grants, its default groups aside: those are the seed's, members included, and
/// [`every_sample_app_role_goes_to_its_default_group`] holds them.
fn held_grants(name: &str) -> Vec<(String, String)> {
    files_below(&grants_dir(name))
        .into_iter()
        .filter(|(path, _)| !path.starts_with("users/"))
        .collect()
}

/// T-2667, CC-61, AP-96: a seeded app is committed by the forge bootstrap, not through the
/// Portal's door, so the Endpoint and Policies the door would commit beside it travel with it.
/// Without them the gateway knows no endpoint for the app and every read answers 404. A change to
/// an app's needs, roles or visibility that is not written back here is red: rerun
/// `cargo test --test reference_apps_tests -- --ignored write_sample_app_grants`.
#[test]
fn every_sample_app_carries_the_grants_the_reconciler_compiles_for_it() {
    for (name, yaml) in reference_apps() {
        let slug = pinned_slug(&name);
        let compiled = compiled_grants(&yaml, slug.as_ref().unwrap_or(&generate_slug()));
        match (compiled, slug) {
            (None, _) => assert!(
                held_grants(&name).is_empty(),
                "apps/{name} grants nothing, yet carries grants"
            ),
            (Some(_), None) => panic!("apps/{name} has no grants/ with its Endpoint"),
            (Some(mut compiled), Some(_)) => {
                compiled.sort();
                assert_eq!(held_grants(&name), compiled, "apps/{name}/grants is stale");
            }
        }
    }
}

/// T-2775, EP-20: a sample app is seeded and vendored into the forge, and a seed sets no rate
/// limit nobody chose, so no sample app names `spec.limits.requestsPerMinute`, and none of its
/// compiled Endpoints carries `spec.rateLimits` (the deployment's seed test holds the same rule).
#[test]
fn no_sample_app_sets_a_rate_limit() {
    for (name, yaml) in reference_apps() {
        let manifest: RawManifest = serde_norway::from_str(&yaml).expect("a manifest");
        assert!(
            manifest.spec.pointer("/limits/requestsPerMinute").is_none(),
            "apps/{name} sets spec.limits.requestsPerMinute: a seed sets no limit nobody chose"
        );
        for (path, text) in held_grants(&name) {
            assert!(
                !text.contains("rateLimits"),
                "apps/{name}/grants/{path} carries spec.rateLimits"
            );
        }
    }
}

/// Writes `apps/*/grants/`, keeping each app's slug once it has one (EP-02).
#[test]
#[ignore = "writes apps/*/grants; run after changing a sample app's needs, roles or visibility"]
fn write_sample_app_grants() {
    for (name, yaml) in reference_apps() {
        let dir = grants_dir(&name);
        let slug = pinned_slug(&name).unwrap_or_else(generate_slug);
        // The default groups under users/ are written by hand: they carry the demo people.
        let _ = std::fs::remove_dir_all(dir.join("projects"));
        let Some(compiled) = compiled_grants(&yaml, &slug) else {
            continue;
        };
        for (path, text) in compiled {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().expect("a folder")).expect("grants dir");
            std::fs::write(file, text).expect("write a grant");
        }
    }
}

/// AP-118, ADR-N-031 §3.5: a seeded app gets what the Portal's door gives a proposed one, so
/// every role of a sample app goes to its default group `{app}-{role}`, committed beside the app
/// under `grants/users/groups/` and annotated with it; the demo people are that group's members
/// and nobody holds a role by name. No other group rides with an app.
#[test]
fn every_sample_app_role_goes_to_its_default_group() {
    use jc_core::kinds::{AppSpec, GroupSpec};
    for (name, yaml) in reference_apps() {
        let manifest: RawManifest = serde_norway::from_str(&yaml).expect("a manifest");
        let project = manifest.metadata.namespace.clone().expect("a project");
        let spec: AppSpec = serde_json::from_value(manifest.spec.clone()).expect("an App");
        let mut expected: Vec<String> = spec
            .roles
            .iter()
            .map(|role| format!("users/groups/{name}-{}.yaml", role.name))
            .collect();
        expected.sort();
        let groups: Vec<(String, String)> = files_below(&grants_dir(&name))
            .into_iter()
            .filter(|(path, _)| path.starts_with("users/"))
            .collect();
        let paths: Vec<&String> = groups.iter().map(|(path, _)| path).collect();
        assert_eq!(paths, expected.iter().collect::<Vec<_>>(), "apps/{name}");
        for (path, text) in &groups {
            let group: RawManifest = serde_norway::from_str(text).expect("a Group");
            assert_eq!(group.kind, "Group", "{path}");
            let annotations = &group.metadata.rest["annotations"];
            assert_eq!(
                annotations["joinedcontext.com/app"].as_str(),
                Some(format!("{project}/{name}").as_str()),
                "{path}"
            );
            let members: GroupSpec =
                serde_json::from_value(group.spec.clone()).expect("a Group spec");
            members.validate().expect("valid members");
            assert!(!members.members.is_empty(), "{path}: the demo people");
        }
        for role in &spec.roles {
            let group = format!("{name}-{}", role.name);
            let entry = spec
                .access
                .iter()
                .find(|entry| entry.role == role.name)
                .unwrap_or_else(|| panic!("apps/{name}: role {} goes nowhere", role.name));
            let subjects = serde_json::to_value(&entry.subjects).expect("subjects");
            assert_eq!(
                subjects,
                serde_json::json!([{ "group": group }]),
                "apps/{name}: role {} goes to {group} alone",
                role.name
            );
        }
    }
}

/// T-3575, AP-151: a generated `wasm` App keeps the wasm template's `server/Cargo.lock`, which the
/// model never writes, so the runner's store fetches that lock and the App builds `--locked`
/// offline; the template's server crate is a cdylib on the guest SDK alone.
#[test]
fn the_wasm_templates_lock_is_in_the_runners_store() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dockerfile =
        std::fs::read_to_string(root.join("builder/Dockerfile")).expect("the builder's Dockerfile");
    assert!(
        dockerfile.contains("sdk/template-wasm/server"),
        "the crate store does not fetch sdk/template-wasm/server/Cargo.lock"
    );
    let server = root.join("sdk/template-wasm/server");
    let lock = locked_packages(
        &std::fs::read_to_string(server.join("Cargo.lock")).expect("the template's lock"),
    );
    assert!(
        lock.iter().any(|(name, _)| name == "jc-app-sdk"),
        "{lock:?}"
    );
    let manifest = std::fs::read_to_string(server.join("Cargo.toml")).expect("the manifest");
    assert!(
        manifest.contains(r#"crate-type = ["cdylib", "rlib"]"#),
        "{manifest}"
    );
}

/// T-4073, T-4074, EP-104: every sample App's Endpoint serves OGC API Features beside its own
/// representations, read only. WebSocket stays a need's own choice: a subscription is a write for
/// an App (AP-09), so an App that follows live data subscribes on its space's public Endpoint.
#[test]
fn every_sample_app_endpoint_serves_ogc_features() {
    for (name, _) in reference_apps() {
        let endpoints: Vec<RawManifest> = held_grants(&name)
            .into_iter()
            .filter_map(|(_, text)| serde_norway::from_str::<RawManifest>(&text).ok())
            .filter(|raw| raw.kind == "Endpoint")
            .collect();
        for endpoint in endpoints {
            let representations = endpoint.spec["enabledRepresentations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                representations.contains(&serde_json::json!("ogc-features")),
                "apps/{name}: its Endpoint serves no ogc-features"
            );
        }
    }
}
