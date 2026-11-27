//! `jcctl migrate`: a layout 1 repository becomes the organization repository and one
//! repository per project, history kept (T-2640, CC-85, CC-86, MF-47).

use crate::common;

use common::{temp_dir, write, ENDPOINT, ENDPOINT_PATH, ORG, PROJECT, SPACE};
use jcctl::assemble::{assemble, Directories};
use jcctl::commands::migrate;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const DOPRAVA: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                       name: doprava\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.org", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A layout 1 repository with four commits: the organization, two projects, and an endpoint.
fn layout_1(test: &str) -> std::path::PathBuf {
    let dir = temp_dir(test);
    git(&dir, &["init", "-q", "-b", "main"]);
    write(&dir, "org.yaml", ORG);
    commit(&dir, "org: the organization");
    write(&dir, "projects/ovzdusie/project.yaml", PROJECT);
    write(&dir, "projects/ovzdusie/spaces/ovzdusie/space.yaml", SPACE);
    commit(&dir, "ovzdusie: the project and its space");
    write(&dir, "projects/doprava/project.yaml", DOPRAVA);
    commit(&dir, "doprava: another project");
    write(&dir, ENDPOINT_PATH, ENDPOINT);
    commit(&dir, "ovzdusie: the public endpoint");
    dir
}

fn log(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// CC-85: each project repository holds, byte for byte, what `projects/{slug}/` held, and the
/// commits that touched it, with their messages; the organization repository keeps its whole
/// history and holds the registry entry where the project was.
#[test]
fn migrate_moves_each_project_with_its_history() {
    let source = layout_1("migrate-history");
    let out = temp_dir("migrate-history-out");
    let migration = migrate::run(&source, &out).expect("the migration runs");

    let slugs: Vec<&str> = migration
        .projects
        .iter()
        .map(|(s, _, _)| s.as_str())
        .collect();
    assert_eq!(slugs, ["doprava", "ovzdusie"]);

    let air = out.join("projects/ovzdusie");
    assert_eq!(
        log(&air),
        [
            "jcctl migrate: the project repository of layout 2 (CC-85)",
            "ovzdusie: the public endpoint",
            "ovzdusie: the project and its space",
        ]
    );
    for rel in [
        "project.yaml",
        "spaces/ovzdusie/space.yaml",
        "spaces/ovzdusie/endpoints/public-air.yaml",
    ] {
        assert_eq!(
            std::fs::read(air.join(rel)).expect("moved"),
            std::fs::read(source.join("projects/ovzdusie").join(rel)).expect("original"),
            "{rel}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(air.join(".jc/layout")).expect("layout"),
        "2\n"
    );
    assert_eq!(
        migration.projects[1].2,
        git(&air, &["rev-parse", "HEAD"]),
        "the migration reports the head it wrote"
    );

    let org = &migration.organization;
    assert_eq!(log(org).len(), 5);
    assert!(!org.join("projects/ovzdusie").exists());
    let entry = std::fs::read_to_string(org.join("projects/ovzdusie.yaml")).expect("the entry");
    let entry = jc_core::kinds::Project::from_yaml(&entry).expect("the entry parses");
    entry.validate().expect("the entry is valid");
    assert_eq!(entry.spec.git_ref.as_deref(), Some("main"));
    assert_eq!(
        std::fs::read_to_string(org.join(".jc/layout")).expect("layout"),
        "2\n"
    );
    assert!(
        log(&source).len() == 4 && git(&source, &["branch", "--format=%(refname:short)"]) == "main"
    );
}

/// CC-86: the migrated organization assembles to the render the layout 1 repository loads.
#[test]
fn the_migrated_organization_renders_what_it_rendered_before() {
    let source = layout_1("migrate-render");
    let out = temp_dir("migrate-render-out");
    let migration = migrate::run(&source, &out).expect("the migration runs");

    let before = assemble(
        &source,
        &Directories::default(),
        &temp_dir("unused-render"),
        None,
    )
    .expect("layout 1 loads");
    let checkouts = Directories(
        migration
            .projects
            .iter()
            .map(|(slug, path, _)| (slug.clone(), path.clone()))
            .collect::<BTreeMap<_, _>>(),
    );
    let after = assemble(
        &migration.organization,
        &checkouts,
        &temp_dir("migrate-render-into").join("render"),
        None,
    )
    .expect("layout 2 assembles");

    let render = |repository: &jcctl::loader::Repository| {
        repository
            .iter()
            .map(|(id, loaded)| {
                (
                    id.to_string(),
                    loaded.path.clone(),
                    serde_json::to_value(&loaded.manifest).expect("serialises"),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(render(&after.repository), render(&before.repository));
}

/// CC-85: a migration runs once, into an empty directory, from what is committed.
#[test]
fn migrate_refuses_to_run_twice_or_over_anything() {
    let source = layout_1("migrate-twice");
    let out = temp_dir("migrate-twice-out");
    let migration = migrate::run(&source, &out).expect("the first migration");

    let again = migrate::run(&migration.organization, &temp_dir("migrate-twice-again"))
        .expect_err("layout 2 is not migrated again");
    assert!(again.to_string().contains("already layout 2"), "{again}");

    let over = migrate::run(&source, &out).expect_err("the output holds the first migration");
    assert!(over.to_string().contains("not empty"), "{over}");

    write(
        &source,
        "projects/ovzdusie/project.yaml",
        "kind: half-typed\n",
    );
    let dirty = migrate::run(&source, &temp_dir("migrate-dirty")).expect_err("uncommitted");
    assert!(dirty.to_string().contains("uncommitted"), "{dirty}");

    git(&source, &["checkout", "-q", "--", "."]);
    write(&source, "projects/stray/notes.md", "not a project\n");
    commit(&source, "a directory that is no project");
    let stray = migrate::run(&source, &temp_dir("migrate-stray")).expect_err("no project.yaml");
    assert!(stray.to_string().contains("projects/stray/"), "{stray}");
}

/// An organization without projects migrates to the organization alone; a file beside the
/// project folders is not a project; `projects` as a file, a layout that does not read and a
/// project.yaml that is not YAML are refused with the path.
#[test]
fn migrate_reads_only_project_folders_and_names_what_it_cannot_read() {
    let bare = temp_dir("migrate-bare");
    git(&bare, &["init", "-q", "-b", "main"]);
    write(&bare, "org.yaml", ORG);
    commit(&bare, "org: the organization");
    let migration = migrate::run(&bare, &temp_dir("migrate-bare-out")).expect("no projects");
    assert!(migration.projects.is_empty());

    let file = temp_dir("migrate-file");
    git(&file, &["init", "-q", "-b", "main"]);
    write(&file, "org.yaml", ORG);
    write(&file, "projects", "not a folder\n");
    commit(&file, "projects as a file");
    let refused = migrate::run(&file, &temp_dir("migrate-file-out")).expect_err("a file");
    assert!(refused.to_string().contains("projects"), "{refused}");

    let source = layout_1("migrate-readme");
    write(&source, "projects/README.md", "the projects\n");
    commit(&source, "a readme beside the projects");
    let out = temp_dir("migrate-readme-out");
    let migration = migrate::run(&source, &out).expect("the readme is no project");
    assert_eq!(migration.projects.len(), 2);

    let broken = temp_dir("migrate-layout");
    write(&broken, "org.yaml", ORG);
    write(&broken, ".jc/layout", "seven\n");
    let refused = migrate::run(&broken, &temp_dir("migrate-layout-out")).expect_err("layout");
    assert!(refused.to_string().contains(".jc/layout"), "{refused}");

    let yaml = layout_1("migrate-yaml");
    write(&yaml, "projects/doprava/project.yaml", "kind: [unclosed\n");
    commit(&yaml, "doprava: broken");
    let refused = migrate::run(&yaml, &temp_dir("migrate-yaml-out")).expect_err("not YAML");
    assert!(refused.to_string().contains("doprava"), "{refused}");
}

/// The command prints the organization repository and each project with its head, and a
/// refused migration exits 1 with the reason.
#[test]
fn the_migrate_command_lists_what_it_moved() {
    let source = layout_1("migrate-cli");
    let out = temp_dir("migrate-cli-out");
    let run = |out: &Path| {
        Command::new(env!("CARGO_BIN_EXE_jcctl"))
            .args(["migrate", "--repo-dir"])
            .arg(&source)
            .arg("--out-dir")
            .arg(out)
            .output()
            .expect("jcctl runs")
    };
    let moved = run(&out);
    assert_eq!(moved.status.code(), Some(0), "{moved:?}");
    let said = String::from_utf8_lossy(&moved.stdout);
    assert!(said.starts_with("organization repository: "), "{said}");
    assert!(said.contains("project ovzdusie: "), "{said}");
    let again = run(&out);
    assert_eq!(again.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&again.stderr).contains("not empty"));
}

/// A migration without git, into a place no directory can be made, or over a project file it
/// cannot read stops naming what failed.
#[test]
fn migrate_names_a_missing_git_an_unmakeable_target_and_an_unreadable_project() {
    let source = layout_1("migrate-no-git");
    let empty = temp_dir("migrate-no-git-path");
    let out = Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(["migrate", "--repo-dir"])
        .arg(&source)
        .arg("--out-dir")
        .arg(temp_dir("migrate-no-git-out"))
        .env("PATH", &empty)
        .output()
        .expect("jcctl runs");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("status --porcelain"), "{said}");

    let blocker = temp_dir("migrate-blocked-out").join("a-file");
    std::fs::write(&blocker, "not a directory\n").expect("a file");
    let refused = migrate::run(&source, &blocker.join("out")).expect_err("no directory");
    assert!(
        matches!(refused, migrate::MigrateError::Io { .. }),
        "{refused}"
    );

    let locked = layout_1("migrate-locked");
    let project = locked.join("projects/doprava/project.yaml");
    // The status stays clean: git is told the file is unchanged, as it is in what is committed.
    git(
        &locked,
        &[
            "update-index",
            "--assume-unchanged",
            "projects/doprava/project.yaml",
        ],
    );
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o000)).expect("locked");
    }
    let refused = migrate::run(&locked, &temp_dir("migrate-locked-out")).expect_err("unreadable");
    assert!(refused.to_string().contains("project.yaml"), "{refused}");
}
