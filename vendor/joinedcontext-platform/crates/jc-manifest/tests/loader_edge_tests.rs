//! The loader at its edges (T-7794): identifiers of an envelope without a group, a refused
//! manifest put back or dropped, preview slugs, document separators, entries that are neither
//! file nor directory, and a link that leaves the repository.

use jc_manifest::loader::{
    parse_yaml_documents, preview_slug_salted, LoadError, RawManifest, Repository, ResourceId,
};
use std::path::{Path, PathBuf};

fn manifest(yaml: &str) -> RawManifest {
    serde_norway::from_str(yaml).expect("manifest parses")
}

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "jc-manifest-loader-{test}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("parent dir");
    std::fs::write(path, body).expect("write");
}

const ORG: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Organization
metadata: { name: city, namespace: org }
spec: { domain: city.example, locales: [en], defaultLocale: en }
";

/// A manifest file that is not UTF-8 is an I/O error naming the file, not a parse error at a
/// line nobody can find.
#[test]
fn a_manifest_that_is_not_utf8_is_refused_by_its_path() {
    let dir = temp_dir("not-utf8");
    write(&dir, "org.yaml", ORG);
    std::fs::write(dir.join("broken.yaml"), [0xff, 0xfe, b'k', b':']).expect("write");
    match Repository::load_for(&dir, None) {
        Err(LoadError::Io { path, .. }) => assert_eq!(path, PathBuf::from("broken.yaml")),
        other => panic!("expected an io error, got {other:?}"),
    }
}

/// A folder the walk cannot read is an I/O error, unlike a dangling link, which is skipped.
#[test]
fn an_unreadable_folder_stops_the_walk() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable");
    write(&dir, "org.yaml", ORG);
    write(&dir, "projects/p/project.yaml", "");
    let locked = dir.join("projects/p");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock");
    let readable = std::fs::read_dir(&locked).is_ok();
    let result = Repository::load_for(&dir, None);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlock");
    if readable {
        // Root reads every folder: nothing to refuse.
        return;
    }
    assert!(matches!(result, Err(LoadError::Io { .. })), "{result:?}");
}

/// An overlay whose spec parses but fails its validation (an organization domain that is no
/// domain) refuses the load at the overlay's file.
#[test]
fn an_overlay_that_fails_validation_refuses_the_load() {
    let dir = temp_dir("overlay-invalid");
    write(&dir, "org.yaml", ORG);
    write(
        &dir,
        "environments/dev.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Environment\nmetadata: { name: dev, namespace: org }\nspec: { orgDomain: 'not a domain' }\n",
    );
    match Repository::load_for(&dir, Some("dev")) {
        Err(LoadError::Overlay { path, message }) => {
            assert_eq!(path, PathBuf::from("environments/dev.yaml"));
            assert!(message.contains("orgDomain"), "{message}");
        }
        other => panic!("expected an overlay error, got {other:?}"),
    }
}

/// A preview keeps a slug or a pinned segment that is not a string as it is: the typed check
/// refuses it later, the preview render does not invent one.
#[test]
fn a_preview_leaves_a_slug_or_segment_that_is_no_string_alone() {
    let dir = temp_dir("preview-no-string");
    write(&dir, "org.yaml", ORG);
    write(
        &dir,
        "projects/p/project.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata: { name: p, namespace: org }\nspec: { organizationRef: city }\n",
    );
    write(
        &dir,
        "projects/p/spaces/air/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata: { name: air, namespace: p }\nspec: { urnSegment: 7 }\n",
    );
    write(
        &dir,
        "projects/p/spaces/air/endpoints/e.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata: { name: e, namespace: p }\nspec: { slug: 5, contextSpaceRef: air }\n",
    );
    let repo = Repository::load_preview(&dir, None, "pv-").expect("the preview loads");
    let endpoint = repo
        .iter()
        .find(|(id, _)| id.kind == "Endpoint")
        .expect("the endpoint");
    assert_eq!(endpoint.1.manifest.spec["slug"], 5);
    let space = repo
        .iter()
        .find(|(id, _)| id.kind == "ContextSpace")
        .expect("the space");
    assert_eq!(space.1.manifest.spec["urnSegment"], 7);
}

#[test]
fn an_api_version_without_a_group_is_its_own_group_and_an_id_without_namespace_prints_bare() {
    let raw = manifest("apiVersion: v1\nkind: Organization\nmetadata: { name: city }\nspec: {}\n");
    let id = ResourceId::from_manifest(&raw);
    assert_eq!(id.group, "v1");
    assert_eq!(id.namespace, None);
    assert_eq!(id.to_string(), "Organization/city");
    let namespaced = ResourceId::from_manifest(&manifest(ORG));
    assert_eq!(namespaced.group, "joinedcontext.com");
    assert_eq!(namespaced.to_string(), "Organization/org/city");
}

#[test]
fn refuse_puts_the_last_good_manifest_back_or_drops_the_resource() {
    let mut repo = Repository::from_manifests(vec![manifest(ORG)], None).expect("assembles");
    let id = ResourceId::from_manifest(&manifest(ORG));
    let good = repo.get(&id).cloned().expect("the Organization");
    repo.refuse(&id, Some(good.clone()));
    assert_eq!(repo.get(&id).map(|r| &r.manifest), Some(&good.manifest));
    repo.refuse(&id, None);
    assert!(repo.get(&id).is_none());
    assert!(repo.is_empty());
}

#[test]
fn a_preview_slug_is_salted_opaque_and_stable() {
    let unsalted = preview_slug_salted("ws-", "", "zt4qm7ge2xdv6ksb3ncf5arw2y");
    let salted = preview_slug_salted("ws-", "s1", "zt4qm7ge2xdv6ksb3ncf5arw2y");
    assert_ne!(unsalted, salted);
    assert_eq!(
        unsalted,
        preview_slug_salted("ws-", "", "zt4qm7ge2xdv6ksb3ncf5arw2y")
    );
    assert_eq!(salted.len(), 32);
    assert!(salted
        .chars()
        .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)));
    assert!(!salted.contains("zt4qm7ge"));
}

#[test]
fn a_separator_followed_by_a_tab_or_a_comment_splits_and_an_indented_one_does_not() {
    let chunks = parse_yaml_documents("a: 1\n---\tx\nb: 2\n--- # c\nc: 3\n  ---\nd: 4\n---x\n");
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[1].content, "b: 2");
    assert_eq!(chunks[2].start_line, 4, "the line of its separator");
    assert!(chunks[2].content.contains("  ---") && chunks[2].content.contains("---x"));
}

#[test]
fn a_socket_and_a_non_utf8_file_name_are_skipped_not_loaded() {
    use std::os::unix::ffi::OsStrExt;
    let dir = temp_dir("odd-entries");
    write(&dir, "org.yaml", ORG);
    let _socket = std::os::unix::net::UnixListener::bind(dir.join("sock.yaml")).expect("a socket");
    std::fs::write(
        dir.join(std::ffi::OsStr::from_bytes(b"bad\xff.yaml")),
        "not: [yaml",
    )
    .expect("odd name");
    let repo = Repository::load(&dir).expect("the odd entries are skipped");
    assert_eq!(repo.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_link_out_of_the_repository_is_refused_and_a_dangling_one_skipped() {
    let outside = temp_dir("outside");
    write(&outside, "org.yaml", ORG);
    let dir = temp_dir("escape");
    write(&dir, "org.yaml", ORG);
    std::os::unix::fs::symlink(dir.join("missing.yaml"), dir.join("dangling.yaml"))
        .expect("a dangling link");
    assert_eq!(
        Repository::load(&dir)
            .expect("a dangling link is skipped")
            .len(),
        1
    );
    std::os::unix::fs::symlink(outside.join("org.yaml"), dir.join("projects.yaml"))
        .expect("a link out");
    let Err(refused) = Repository::load(&dir) else {
        panic!("a link out of the repository loads");
    };
    assert!(
        matches!(refused, LoadError::PathEscapesRepository { .. }),
        "{refused:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn a_domain_given_to_from_manifests_renders_every_kind_but_the_environment() {
    let space = manifest(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata: { name: air, namespace: p }\nspec: { description: { en: '{orgDomain} air' } }\n",
    );
    let environment = manifest(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Environment\nmetadata: { name: dev, namespace: org }\nspec: { note: '{orgDomain}' }\n",
    );
    let repo = Repository::from_manifests(
        vec![space.clone(), environment.clone()],
        Some("city.example"),
    )
    .expect("assembles");
    assert_eq!(repo.org_domain(), Some("city.example"));
    let rendered = repo
        .get(&ResourceId::from_manifest(&space))
        .expect("the space");
    assert_eq!(
        rendered.manifest.spec["description"]["en"],
        "city.example air"
    );
    let kept = repo
        .get(&ResourceId::from_manifest(&environment))
        .expect("the environment");
    assert_eq!(kept.manifest.spec["note"], "{orgDomain}");
}
