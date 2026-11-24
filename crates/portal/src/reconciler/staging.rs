//! Staging a repository revision into a scratch tree the loader reads (CC-08; T-5340 split it
//! out of `daemon`): which forge files are manifests, the listing of a revision, the blob fetch
//! with its process-wide cache, and the scratch directory removed when the last reader drops it.
//! Encrypted secrets files are never staged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use super::daemon::SyncError;
use crate::forge::ShakoClient;

/// Identifies files that are candidates for ResourceEnvelope manifests.
///
/// The repository holds YAML that is not a manifest at all: the Portal theme, the navigation
/// tree, the locale bundles, the LinkML models and the native pipeline configurations. The
/// loader is strict on purpose, so those are never handed to it; what is handed to it are the
/// directories the manifest layout claims (CC-08).
pub(crate) fn is_candidate_manifest(path: &str) -> bool {
    let clean = path.trim_start_matches('/');
    if clean
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        // The tree comes from the forge, so it is input: nothing that could climb out of the
        // staging directory is a candidate (CC-08).
        return false;
    }
    if clean == "org.yaml" || clean == "bundle.yaml" {
        return true;
    }
    if !clean.ends_with(".yaml") && !clean.ends_with(".yml") {
        return false;
    }
    // A blueprint is an organization-level manifest with a path of its own
    // (`blueprints/{name}/blueprint.yaml`, CC-23), and the flow gallery reads it from the
    // mirror like any other resource (CC-26). Only that one name: the notes and the fixtures
    // beside it are not manifests.
    if clean.starts_with("blueprints/") {
        return clean.ends_with("/blueprint.yaml");
    }
    // Roles and their bindings live beside the projects (T-0525): the Portal reads them for
    // its own checks and compiles them for the forge (T-0527). The groups they and the Apps name
    // are org-level manifests of their own (PF-62), which the reconciler writes into the realm
    // (PF-63); left out here, no Group reached the mirror (T-3031).
    // The builder profiles are organization-level too (AG-26): `agentprofiles/{name}.yaml`.
    // Each Organization of the installation has a file of its own beside the legacy `org.yaml`
    // (PF-121, ADR-N-058). A DelegationPolicy lives in `users/delegation/` (AG-150); left out here,
    // no policy reached the mirror and every connector was held to the data-read default (T-6025).
    // The Environment overlays (`environments/{name}.yaml`, CC-73) the loader merges as
    // JC_ENVIRONMENT names one; left out here, a set JC_ENVIRONMENT stopped every sync (T-6850).
    // An Organization's open data portals are `portals/{name}.yaml` (ODP-08); left out here, no
    // Portal reached the mirror and jc's reconciler served none (T-6877).
    clean.starts_with("projects/")
        || clean.starts_with("portals/")
        || clean.starts_with("environments/")
        || clean.starts_with("organizations/")
        || clean.starts_with("users/roles/")
        || clean.starts_with("users/groups/")
        || clean.starts_with("users/assignments/")
        || clean.starts_with("users/delegation/")
        || clean.starts_with("agentprofiles/")
}

/// Whether a file is a DataModel's generated JSON Schema (DM-02): `*.schema.json` as `jcctl model
/// generate` names it, or `json-schema/*.json` as the Portal writes it, inside a project.
pub(super) fn is_model_schema(path: &str) -> bool {
    let clean = path.trim_start_matches('/');
    let segments: Vec<&str> = clean.split('/').collect();
    clean.starts_with("projects/")
        && !segments.iter().any(|s| *s == ".." || s.is_empty())
        && (clean.ends_with(".schema.json")
            || (clean.ends_with(".json") && segments.contains(&"json-schema")))
}

/// Whether a staged file is an encrypted secrets file rather than a manifest (CC-06).
///
/// The same names `jcctl` skips when it loads a repository and reads when it resolves a
/// `secretRef`, so one file is never both.
pub(super) fn is_encrypted_secrets_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(jcctl::secrets::sops::is_encrypted_file)
}

/// Prepares one fetched file for the loader, or leaves it out (MF-04, MF-05).
///
/// Two judgements are made here and nowhere else, because the loader is right to refuse both
/// and the Portal is right to survive them. A `status:` block somebody committed is dropped,
/// all but what a publish writes back: `status.build` (AP-13a) and a wasm App's `status.shard`
/// (AP-149). The rest of status is computed by the server and never read from Git, so a
/// manifest carrying one is sanitised rather than refused. A document of a kind the Portal does not serve is left out:
/// one unknown kind in the repository must not cost every other resource its place in the
/// mirror. Everything past this point is the loader's judgement, including which two files
/// claim one identity and which path a kind belongs at.
///
/// `Ok(None)` means the file held nothing to load.
pub(super) fn stageable(content: &str) -> Result<Option<String>, serde_norway::Error> {
    let mut kept: Vec<String> = Vec::new();

    for document in serde_norway::Deserializer::from_str(content) {
        let mut value = serde_norway::Value::deserialize(document)?;
        let Some(mapping) = value.as_mapping_mut() else {
            continue;
        };
        // Dropping `status.build` too left every published App without a build to serve
        // (T-2633), and dropping `status.shard` left every wasm App on no shard (T-3603).
        if let Some(mut committed) = mapping.remove("status") {
            let mut status = serde_norway::Mapping::new();
            if let Some(committed) = committed.as_mapping_mut() {
                for member in ["build", "shard"] {
                    if let Some(value) = committed.remove(member) {
                        status.insert(member.into(), value);
                    }
                }
            }
            if !status.is_empty() {
                mapping.insert("status".into(), status.into());
            }
        }
        // A document without a `kind` is not a manifest (a LinkML source beside its DataModel,
        // a note): nothing for an operator to act on. A kind the catalogue lacks is (OPS-27).
        let Some(kind) = mapping.get("kind").and_then(serde_norway::Value::as_str) else {
            tracing::debug!("a document without a kind, skipped");
            continue;
        };
        if crate::resource::by_kind(kind).is_none() {
            tracing::warn!(kind = %kind, "manifest of an unknown kind, skipped");
            continue;
        }
        kept.push(serde_norway::to_string(&value)?);
    }

    if kept.is_empty() {
        return Ok(None);
    }
    Ok(Some(kept.join("---\n")))
}

/// The project a manifest without a namespace belongs to, taken from where it lies (CC-08).
pub(crate) fn namespace_of(path: &str) -> String {
    let clean = path.trim_start_matches('/');
    match clean.split('/').collect::<Vec<_>>().as_slice() {
        ["projects", project, ..] if !project.is_empty() => (*project).to_owned(),
        _ => "org".to_owned(),
    }
}

/// The directory one run stages the fetched manifests in.
///
/// `jcctl` loads a repository from a path, and the Portal reads its repository over the
/// forge API, so the two meet on disk. The directory belongs to the run that staged it and to
/// the later runs that found the same blobs, and is removed when the last of them lets it go: no
/// file outlives the listing it was staged for (T-4481).
/// The repository at one revision, staged as files the loader can read (CC-08).
///
/// Its own function because two schedules need it: the mirror the Portal serves from, and the
/// foreign-model mirror that proposes a peer's schema. A second copy of this loop would be a
/// second answer to "which files in the repository are manifests".
pub(crate) async fn stage(shako: &ShakoClient, revision: &str) -> Result<Scratch, SyncError> {
    stage_repository(shako, revision, false).await
}

/// One project repository of layout 2 at `revision`, staged as its own root (CC-85): the files
/// that would be manifests under `projects/{slug}/` of layout 1, and its `.jc/layout`.
pub(crate) async fn stage_project(
    shako: &ShakoClient,
    revision: &str,
) -> Result<Scratch, SyncError> {
    stage_repository(shako, revision, true).await
}

async fn stage_repository(
    shako: &ShakoClient,
    revision: &str,
    project: bool,
) -> Result<Scratch, SyncError> {
    stage_listed(shako, revision, &listing(shako, revision, project).await?).await
}

/// A repository's candidate files with their blob shas, by path.
pub(super) type Listing = Vec<(String, String)>;

/// The files of the repository at `revision` a stage writes, with their blobs: one tree call,
/// and the key a run reuses the last stage by (T-4481).
pub(super) async fn listing(
    shako: &ShakoClient,
    revision: &str,
    project: bool,
) -> Result<Listing, SyncError> {
    let tree_blobs = shako
        .list_tree_blobs(revision)
        .await
        .map_err(SyncError::naming(|| {
            format!("tree at {revision} in {}/{}", shako.owner, shako.repo)
        }))?;
    let candidates: Vec<(String, String)> = tree_blobs
        .into_iter()
        .filter(|(p, _)| {
            p == jc_core::project::LAYOUT_FILE
                || if project {
                    // The project's own tree is what lay under `projects/{slug}/`; the slug
                    // is the registry's, so any one stands in for it here.
                    let p = format!("projects/_/{}", p.trim_start_matches('/'));
                    is_candidate_manifest(&p)
                        || is_model_schema(&p)
                        || crate::grafana_builder::is_dashboard_file(&p)
                } else {
                    is_candidate_manifest(p)
                        || is_model_schema(p)
                        || crate::grafana_builder::is_dashboard_file(p)
                }
        })
        .collect();

    if candidates.is_empty() {
        return Err(SyncError::Empty(
            "no manifests found in repository".to_string(),
        ));
    }

    Ok(candidates)
}

/// The `candidates` of the repository at `revision`, written into a staging directory of their
/// own.
pub(super) async fn stage_listed(
    shako: &ShakoClient,
    revision: &str,
    candidates: &Listing,
) -> Result<Scratch, SyncError> {
    // The staging directory is removed when the last run that reads it ends, whichever way it
    // ends: the run that staged it, or a later one that found the same files (T-4481).
    let scratch = Scratch::new(revision)?;
    let mut staged = 0usize;
    for (path, file) in fetch_blobs(shako, revision, candidates).await? {
        let Some(file) = file else {
            // The tree listed it and the contents call does not have it: a race with a
            // force-push, not a broken manifest. The next run reads a consistent tree.
            tracing::warn!(path = %path, "candidate manifest listed in git tree not found");
            continue;
        };
        let path = &path;
        if path == jc_core::project::LAYOUT_FILE {
            // Which layout the repository follows (CC-85): read by the assembly, not a manifest.
            scratch.write(path, &file)?;
            continue;
        }
        if path.ends_with("/bento.yaml") {
            // Not a manifest: the author's Bento mapping beside a Pipeline (PL-03), staged as
            // written so the streams can render it; the loader skips it by name.
            scratch.write(path, &file)?;
            continue;
        }
        if path.ends_with(".linkml.yaml") {
            // Not a manifest: a DataModel's LinkML source (`spec.linkml` names only such a file,
            // DM-76), which the view writer compiles into the space's broker schema. Dropped
            // here, no space on the forge ever had its schema written (T-3781).
            scratch.write(path, &file)?;
            continue;
        }
        if path.ends_with(".json") {
            // Not a manifest: a DataModel's generated JSON Schema, which the runner's validation
            // stage compiles beside its DataModel (PL-60), or a builder dashboard, which the
            // dashboards step provisions (`crate::grafana_builder::DashboardSync`). Left out, every forge model held its
            // records to id and type alone (T-5081). `listing` admits a `.json` file only by
            // `is_model_schema`, in a project repository on its `projects/_/` form, so the path
            // here is not tested again: unprefixed, it failed that test and was dropped (T-6429).
            scratch.write(path, &file)?;
            continue;
        }
        if is_encrypted_secrets_file(path) {
            // Not a manifest either: the repository's own `*.enc.yaml`, which the SOPS backend
            // decrypts a Pipeline's `secretRef` from (CC-06, T-0935). It is staged as written
            // because the resolver reads this tree and nothing else; dropping it here is how
            // `demo-feed` came back as "not declared in any encrypted secrets file" while the
            // file sat in the repository. The loader skips it by name.
            scratch.write(path, &file)?;
            continue;
        }
        match stageable(&file) {
            Ok(Some(text)) => {
                scratch.write(path, &text)?;
                staged += 1;
            }
            // Each unknown kind has said so above; a file of none is not a manifest.
            Ok(None) => {
                tracing::debug!(path = %path, "no document of a kind the Portal serves, skipped")
            }
            Err(err) => {
                tracing::warn!(path = %path, error = %err, "not YAML, skipped")
            }
        }
    }

    if staged == 0 {
        return Err(SyncError::Empty(format!(
            "none of the {} candidate files holds a manifest",
            candidates.len()
        )));
    }
    Ok(scratch)
}

/// How many contents calls one staging has in flight.
const FETCHES_AT_ONCE: usize = 8;

/// The contents of `candidates` (path, blob id) at `revision`, in their order; `None` for one
/// the forge lists and does not serve.
///
/// A blob id names its content for good, so a file this process read before is not read again:
/// a run after one merge fetches the files that merge changed, not the whole repository, and the
/// mirror catches up in seconds rather than minutes (PL-61, T-4270). The rest go
/// [`FETCHES_AT_ONCE`] at a time.
async fn fetch_blobs(
    shako: &ShakoClient,
    revision: &str,
    candidates: &[(String, String)],
) -> Result<Vec<(String, Option<Arc<str>>)>, SyncError> {
    use futures_util::{StreamExt, TryStreamExt};
    let fetches = candidates.iter().cloned().map(|(path, sha)| {
        let shako = shako.clone();
        let revision = revision.to_owned();
        async move {
            if let Some(content) = known_blob(&sha) {
                return Ok::<_, SyncError>((path, Some(content)));
            }
            let content: Option<Arc<str>> = shako
                .get_file(&path, &revision)
                .await?
                .map(|file| file.content.into());
            if let Some(content) = &content {
                keep_blob(sha, content.clone());
            }
            Ok((path, content))
        }
    });
    futures_util::stream::iter(fetches)
        .buffered(FETCHES_AT_ONCE)
        .try_collect()
        .await
}

/// The bytes the blobs this process read may hold together (T-4353).
const BLOBS_MAX_BYTES: usize = 64 * 1024 * 1024;
// ponytail: one process-wide LRU by bytes; a repository larger than the cap is read again in part
// on every sync.
static BLOBS: std::sync::LazyLock<std::sync::Mutex<crate::cache::ByteLru<String, Arc<str>>>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(crate::cache::ByteLru::new(BLOBS_MAX_BYTES, None))
    });

/// The content of blob `sha`, when this process has read it; a tree entry without an id has none.
fn known_blob(sha: &str) -> Option<Arc<str>> {
    if sha.is_empty() {
        return None;
    }
    BLOBS.lock().unwrap_or_else(|p| p.into_inner()).get(sha)
}

fn keep_blob(sha: String, content: Arc<str>) {
    if sha.is_empty() {
        return;
    }
    let bytes = sha.len() + content.len();
    BLOBS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(sha, content, bytes);
}

pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(revision: &str) -> Result<Self, SyncError> {
        Self::under(&std::env::temp_dir(), revision)
    }

    /// [`Scratch::new`] under `base` instead of the process's temporary directory.
    fn under(base: &Path, revision: &str) -> Result<Self, SyncError> {
        // One directory per run, never per revision: two runs staging into one directory would
        // read each other's files, and the first to finish would delete the other's.
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = base.join(format!(
            "jc-portal-sync-{}-{}-{run}",
            std::process::id(),
            revision.get(..12).unwrap_or(revision)
        ));
        std::fs::create_dir_all(&path).map_err(|source| SyncError::Scratch {
            path: path.clone(),
            source,
        })?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Writes one repository file under the run's directory. A path that climbs out (`..`, a
    /// root or a prefix) refuses the sync instead of writing beside it (T-4039): the tree comes
    /// from the forge, and a forge without `receive.fsckObjects` takes such a tree object.
    pub(super) fn write(&self, relative: &str, content: &str) -> Result<(), SyncError> {
        let mut target = self.0.clone();
        for component in Path::new(relative.trim_start_matches('/')).components() {
            match component {
                std::path::Component::Normal(part) => target.push(part),
                std::path::Component::CurDir => {}
                _ => {
                    return Err(SyncError::Scratch {
                        path: PathBuf::from(relative),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "the path leaves the repository",
                        ),
                    })
                }
            }
        }
        // The target lies under the run's directory, so it always has a parent.
        let parent = target.parent().unwrap_or(&self.0);
        std::fs::create_dir_all(parent).map_err(|source| SyncError::Scratch {
            path: parent.to_path_buf(),
            source,
        })?;
        std::fs::write(&target, content).map_err(|source| SyncError::Scratch {
            path: target,
            source,
        })
    }
}

impl Drop for Scratch {
    /// The staged tree goes on a thread of its own: removing 31,401 files took 1.7 s at 10,000
    /// Endpoints, every wave, after the mirror was already swapped (T-4482). The directory is
    /// this run's alone (`new`), so nothing else reads or writes it meanwhile.
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.0);
        std::thread::spawn(move || {
            if let Err(err) = std::fs::remove_dir_all(&path) {
                tracing::warn!(path = %path.display(), error = %err, "could not remove the sync staging directory");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: helsinki\nspec:\n  isSandbox: true\n";
    const TREE: &str =
        "/odata/Repositories(Owner='o',Name='r')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)";
    const FILE: &str = "/odata/Repositories(Owner='o',Name='r')/Shako.File(Path=@p,Ref=@r)";

    /// A forge listing `files` (path, blob sha, content; `None` is listed and not served) at
    /// `rev`, each content served at most `reads` times.
    async fn forge(
        rev: &str,
        files: &[(&str, &str, Option<&str>)],
        reads: u64,
    ) -> (MockServer, ShakoClient) {
        let server = MockServer::start().await;
        let tree: Vec<serde_json::Value> = files
            .iter()
            .map(|(p, sha, _)| json!({ "Path": p, "Type": "blob", "Sha": sha }))
            .chain([json!({ "Path": "projects/helsinki", "Type": "tree" })])
            .collect();
        Mock::given(method("GET"))
            .and(path(TREE))
            .and(query_param("@r", format!("'{rev}'")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": "tree-1", "Truncated": false, "Tree": tree })),
            )
            .mount(&server)
            .await;
        for (file, sha, content) in files {
            let answer = match content {
                Some(content) => ResponseTemplate::new(200).set_body_json(json!({
                    "Sha": sha,
                    "Content": base64::engine::general_purpose::STANDARD.encode(content)
                })),
                None => ResponseTemplate::new(404),
            };
            Mock::given(method("GET"))
                .and(path(FILE))
                .and(query_param("@p", format!("'{file}'")))
                .respond_with(answer)
                .up_to_n_times(reads)
                .mount(&server)
                .await;
        }
        let shako =
            ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t").expect("client");
        (server, shako)
    }

    fn read(scratch: &Scratch, file: &str) -> Option<String> {
        std::fs::read_to_string(scratch.path().join(file)).ok()
    }

    /// CC-08: what a run stages beside the manifests as written (a Bento mapping), what it
    /// sanitises (a committed `status`), and what it leaves out: a listed file the forge does
    /// not serve, a document of no kind the Portal serves, a YAML list.
    #[tokio::test]
    async fn a_stage_writes_the_manifests_and_the_files_beside_them_and_skips_the_rest() {
        let with_status =
            format!("{SPACE}status:\n  phase: Ready\n---\n- a list\n---\nkind: Unknown\n");
        let note = "kind: NotAKind\nmetadata:\n  name: x\n";
        let files = [
            (
                "projects/helsinki/spaces/mobility/space.yaml",
                "s1",
                Some(with_status.as_str()),
            ),
            (
                "projects/helsinki/pipelines/feed/bento.yaml",
                "s2",
                Some("input: {}\n"),
            ),
            ("projects/helsinki/pipelines/gone.yaml", "s3", None),
            ("projects/helsinki/notes/note.yaml", "s4", Some(note)),
            ("README.md", "s5", Some("# not listed")),
        ];
        let (_server, shako) = forge("rev-stage", &files, 1).await;
        let scratch = stage(&shako, "rev-stage").await.expect("the run stages");
        let space = read(&scratch, "projects/helsinki/spaces/mobility/space.yaml").expect("space");
        assert!(space.contains("kind: ContextSpace"), "{space}");
        assert!(
            !space.contains("status"),
            "the committed status is dropped: {space}"
        );
        assert!(!space.contains("Unknown"), "{space}");
        assert_eq!(
            read(&scratch, "projects/helsinki/pipelines/feed/bento.yaml").as_deref(),
            Some("input: {}\n")
        );
        for skipped in [
            "projects/helsinki/pipelines/gone.yaml",
            "projects/helsinki/notes/note.yaml",
            "README.md",
        ] {
            assert_eq!(read(&scratch, skipped), None, "{skipped}");
        }
    }

    /// PL-61: a blob read once is not read again by the next stage, so a second run of the same
    /// tree asks the forge for no contents; the forge's one answer per file proves it.
    #[tokio::test]
    async fn a_blob_read_once_is_served_from_the_process_cache() {
        let files = [(
            "projects/helsinki/spaces/mobility/space.yaml",
            "blob-cached-t7651",
            Some(SPACE),
        )];
        let (server, shako) = forge("rev-cache", &files, 1).await;
        let first = stage(&shako, "rev-cache")
            .await
            .expect("the first run stages");
        let second = stage(&shako, "rev-cache")
            .await
            .expect("the second run stages from the cache");
        for scratch in [&first, &second] {
            let staged = read(scratch, "projects/helsinki/spaces/mobility/space.yaml");
            assert!(staged.is_some_and(|s| s.contains("ContextSpace")));
        }
        let reads = server.received_requests().await.unwrap_or_default();
        assert_eq!(reads.iter().filter(|r| r.url.path() == FILE).count(), 1);
        assert_ne!(first.path(), second.path(), "one directory per run");
    }

    /// A tree with no candidate is an empty repository, a tree whose candidates hold no manifest
    /// is one too, and a forge that does not answer the tree names the tree and the repository.
    #[tokio::test]
    async fn a_repository_without_manifests_or_a_tree_is_refused_by_name() {
        let (_server, shako) = forge("rev-none", &[("README.md", "x", Some("# hi"))], 1).await;
        let none = stage(&shako, "rev-none").await.err().expect("no candidate");
        assert!(
            matches!(&none, SyncError::Empty(m) if m.contains("no manifests")),
            "{none}"
        );

        let note = [(
            "projects/helsinki/notes/note.yaml",
            "n1",
            Some("just: text\n"),
        )];
        let (_server, shako) = forge("rev-notes", &note, 1).await;
        let empty = stage(&shako, "rev-notes").await.err().expect("no manifest");
        assert!(
            matches!(&empty, SyncError::Empty(m) if m.contains("none of the 1 candidate")),
            "{empty}"
        );

        let (_server, shako) = forge("rev-other", &[], 1).await;
        let missing = stage(&shako, "rev-unknown").await.err().expect("no tree");
        assert!(
            missing.to_string().contains("tree at rev-unknown in o/r"),
            "{missing}"
        );
    }

    /// CC-85: a project repository is listed as its own root, its non-manifest files left out.
    #[tokio::test]
    async fn a_project_repository_lists_its_own_root() {
        let files = [
            ("spaces/mobility/space.yaml", "p1", Some(SPACE)),
            ("docs/readme.md", "p2", Some("# no")),
        ];
        let (_server, shako) = forge("rev-project", &files, 1).await;
        let listed = listing(&shako, "rev-project", true).await.expect("listed");
        assert_eq!(
            listed,
            vec![("spaces/mobility/space.yaml".to_owned(), "p1".to_owned())]
        );
        let scratch = stage_project(&shako, "rev-project").await.expect("staged");
        assert!(read(&scratch, "spaces/mobility/space.yaml").is_some());
    }

    #[test]
    fn a_manifest_without_a_namespace_belongs_to_where_it_lies() {
        assert_eq!(namespace_of("/projects/helsinki/spaces/a.yaml"), "helsinki");
        assert_eq!(namespace_of("projects//a.yaml"), "org");
        assert_eq!(namespace_of("org.yaml"), "org");
    }

    /// A committed status that is not a mapping keeps nothing; one that is keeps `build` and
    /// `shard` alone.
    #[test]
    fn a_committed_status_keeps_only_what_a_publish_writes() {
        let plain = stageable(&format!("{SPACE}status: Ready\n"))
            .expect("yaml")
            .expect("kept");
        assert!(!plain.contains("status"), "{plain}");
        let built = stageable(&format!(
            "{SPACE}status:\n  phase: Ready\n  build: b1\n  shard: s1\n"
        ))
        .expect("yaml")
        .expect("kept");
        assert!(
            built.contains("build: b1") && built.contains("shard: s1") && !built.contains("phase"),
            "{built}"
        );
    }

    /// T-4039 and the disk: a directory that cannot be made, a file where a directory must go
    /// and a directory where a file must go each refuse the sync and name the path.
    #[test]
    fn a_scratch_that_cannot_be_written_refuses_with_the_path() {
        let base = Scratch::new("rev-disk").expect("scratch");
        base.write("file", "x").expect("a file");
        let blocked = Scratch::under(&base.path().join("file"), "rev-disk")
            .err()
            .expect("not a directory");
        assert!(
            matches!(&blocked, SyncError::Scratch { path, .. } if path.starts_with(base.path().join("file"))),
            "{blocked}"
        );
        let parent = base
            .write("file/inner.yaml", "x")
            .expect_err("a file is no parent");
        assert!(
            matches!(&parent, SyncError::Scratch { path, .. } if path.ends_with("file")),
            "{parent}"
        );
        base.write("dir/inner.yaml", "x").expect("nested");
        let onto = base.write("dir", "x").expect_err("a directory is no file");
        assert!(
            matches!(&onto, SyncError::Scratch { path, .. } if path.ends_with("dir")),
            "{onto}"
        );
        let out = base.write("../escape", "x").expect_err("climbs out");
        assert!(
            matches!(&out, SyncError::Scratch { path, .. } if path == Path::new("../escape")),
            "{out}"
        );
    }

    /// A poisoned blob cache still answers: a panic elsewhere in the process costs no sync.
    #[test]
    fn a_poisoned_blob_cache_still_answers() {
        let _ = std::thread::spawn(|| {
            let _held = BLOBS.lock();
            panic!("poison the cache");
        })
        .join();
        keep_blob("poisoned-t7651".into(), Arc::from("content"));
        assert_eq!(known_blob("poisoned-t7651").as_deref(), Some("content"));
        keep_blob(String::new(), Arc::from("unnamed"));
        assert_eq!(known_blob(""), None, "a blob without an id is never kept");
    }
}
