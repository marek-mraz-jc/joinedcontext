//! Staging a repository revision into a scratch tree the loader reads (CC-08; T-5340 split it
//! out of `daemon`): which forge files are manifests, the listing of a revision, the blob fetch
//! with its process-wide cache, and the scratch directory removed when the last reader drops it.
//! Encrypted secrets files are never staged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use super::daemon::SyncError;
use crate::git::GiteaClient;

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
    clean.starts_with("projects/")
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
pub(super) fn namespace_of(path: &str) -> String {
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
pub(crate) async fn stage(gitea: &GiteaClient, revision: &str) -> Result<Scratch, SyncError> {
    stage_repository(gitea, revision, false).await
}

/// One project repository of layout 2 at `revision`, staged as its own root (CC-85): the files
/// that would be manifests under `projects/{slug}/` of layout 1, and its `.jc/layout`.
pub(crate) async fn stage_project(
    gitea: &GiteaClient,
    revision: &str,
) -> Result<Scratch, SyncError> {
    stage_repository(gitea, revision, true).await
}

async fn stage_repository(
    gitea: &GiteaClient,
    revision: &str,
    project: bool,
) -> Result<Scratch, SyncError> {
    stage_listed(gitea, revision, &listing(gitea, revision, project).await?).await
}

/// A repository's candidate files with their blob shas, by path.
pub(super) type Listing = Vec<(String, String)>;

/// The files of the repository at `revision` a stage writes, with their blobs: one tree call,
/// and the key a run reuses the last stage by (T-4481).
pub(super) async fn listing(
    gitea: &GiteaClient,
    revision: &str,
    project: bool,
) -> Result<Listing, SyncError> {
    let tree_blobs = gitea.list_tree_blobs(revision).await?;
    let candidates: Vec<(String, String)> = tree_blobs
        .into_iter()
        .filter(|(p, _)| {
            p == jc_core::project::LAYOUT_FILE
                || if project {
                    // The project's own tree is what lay under `projects/{slug}/`; the slug
                    // is the registry's, so any one stands in for it here.
                    let p = format!("projects/_/{}", p.trim_start_matches('/'));
                    is_candidate_manifest(&p) || is_model_schema(&p)
                } else {
                    is_candidate_manifest(p) || is_model_schema(p)
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
    gitea: &GiteaClient,
    revision: &str,
    candidates: &Listing,
) -> Result<Scratch, SyncError> {
    // The staging directory is removed when the last run that reads it ends, whichever way it
    // ends: the run that staged it, or a later one that found the same files (T-4481).
    let scratch = Scratch::new(revision)?;
    let mut staged = 0usize;
    for (path, file) in fetch_blobs(gitea, revision, candidates).await? {
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
            // stage compiles beside its DataModel (PL-60). Left out, every forge model held its
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
    gitea: &GiteaClient,
    revision: &str,
    candidates: &[(String, String)],
) -> Result<Vec<(String, Option<Arc<str>>)>, SyncError> {
    use futures_util::{StreamExt, TryStreamExt};
    let fetches = candidates.iter().cloned().map(|(path, sha)| {
        let gitea = gitea.clone();
        let revision = revision.to_owned();
        async move {
            if let Some(content) = known_blob(&sha) {
                return Ok::<_, SyncError>((path, Some(content)));
            }
            let content: Option<Arc<str>> = gitea
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
        // One directory per run, never per revision: two runs staging into one directory would
        // read each other's files, and the first to finish would delete the other's.
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
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
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SyncError::Scratch {
                path: parent.to_path_buf(),
                source,
            })?;
        }
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
