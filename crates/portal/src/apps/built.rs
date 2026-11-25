//! A build the lane proposes, checked against the forge and published by the Portal (AP-101,
//! AP-104, ADR-N-028).
//!
//! The lane's token may propose `status.build` and nothing else (AP-73). These checks bound what
//! that proposal can say: the commit is the head of the default branch of the App's own
//! repository, that repository holds an artifact `bundle-{commit}` of a run of that commit, and
//! its bytes hash to the digest. Only then does the Portal write the generic package
//! `app-{name}`, version `{commit}-{digest12}` (see `version`), with its own token: no credential that writes a package is
//! ever on the runner (AP-101).
//!
//! A `ui-rust` App's build is an image (AP-105, AP-107): the artifact `image-{commit}` is an OCI
//! image layout whose manifest hashes to the digest, every blob to the digest the manifest names,
//! and the Portal pushes those bytes unchanged as `app-{name}:{commit}` to the container registry.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::agents::repository;
use crate::error::ApiError;
use crate::forge::{Artifact, GitError, ShakoClient};

/// The largest bundle the Portal downloads and publishes.
/// ponytail: one fixed bound for every application; a setting when one needs more.
pub const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SBOM_BYTES: u64 = 16 * 1024 * 1024;
/// The largest image layout the Portal downloads and pushes: a static binary with its interface
/// embedded, held in memory for the push.
/// ponytail: in memory; stream the blobs from a file when an application needs more.
pub const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024;

const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

/// An image read out of its OCI layout, every digest checked (AP-107).
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// The manifest's bytes, as they were hashed.
    pub manifest: Vec<u8>,
    /// The config, then the layers, each under its digest.
    pub blobs: Vec<(String, Vec<u8>)>,
}

fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// The image `digest` names in an OCI image layout tar, or why the layout does not hold it: the
/// index names it, the manifest's bytes hash to it, it is an OCI image manifest, and every blob
/// it names is in the layout with that digest and size (AP-107).
pub fn image_of(layout: &[u8], digest: &str) -> Result<Image, String> {
    const MAX_ENTRIES: usize = 64;
    let mut files = std::collections::BTreeMap::new();
    let mut archive = tar::Archive::new(layout);
    let entries = archive
        .entries()
        .map_err(|e| format!("the layout is not a tar: {e}"))?;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_ENTRIES {
            return Err(format!("the layout holds more than {MAX_ENTRIES} entries"));
        }
        let mut entry = entry.map_err(|e| format!("the layout is not a tar: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| format!("a path in the layout is not readable: {e}"))?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_owned();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)
            .map_err(|e| format!("{path} of the layout is not readable: {e}"))?;
        files.insert(path, bytes);
    }
    let blob = |digest: &str| {
        files.get(&format!(
            "blobs/sha256/{}",
            digest.trim_start_matches("sha256:")
        ))
    };
    let index: Value = files
        .get("index.json")
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .ok_or("the layout has no index.json")?;
    let named = index["manifests"]
        .as_array()
        .is_some_and(|all| all.iter().any(|m| m["digest"] == digest));
    if !is_digest(digest) || !named {
        return Err(format!("the layout's index names no manifest {digest}"));
    }
    let manifest = blob(digest).ok_or(format!("the layout holds no blob {digest}"))?;
    if sha256(manifest) != digest {
        return Err(format!(
            "the manifest in the layout does not hash to {digest}"
        ));
    }
    let parsed: Value =
        serde_json::from_slice(manifest).map_err(|e| format!("the manifest is not JSON: {e}"))?;
    if parsed["mediaType"] != OCI_MANIFEST {
        return Err(format!("the manifest is not an {OCI_MANIFEST}"));
    }
    let described: Vec<&Value> = std::iter::once(&parsed["config"])
        .chain(parsed["layers"].as_array().into_iter().flatten())
        .collect();
    if described.len() < 2 {
        return Err("the manifest names no layer".to_owned());
    }
    let mut blobs = Vec::new();
    for descriptor in described {
        let named = descriptor["digest"].as_str().unwrap_or_default();
        if !is_digest(named) {
            return Err(format!(
                "the manifest names '{named}', which is not a sha256 digest"
            ));
        }
        let bytes = blob(named).ok_or(format!("the layout holds no blob {named}"))?;
        if sha256(bytes) != named || descriptor["size"].as_u64() != Some(bytes.len() as u64) {
            return Err(format!(
                "the blob {named} in the layout is not the one the manifest names"
            ));
        }
        blobs.push((named.to_owned(), bytes.clone()));
    }
    Ok(Image {
        manifest: manifest.clone(),
        blobs,
    })
}

/// The package an App's builds are published as (AP-101).
pub fn package(name: &str) -> String {
    format!("app-{name}")
}

/// The version one build is published under (AP-101, T-2671): the commit and the first 12 hex
/// digits of the digest. A rebuild of the same commit on a newer platform release makes other
/// bytes, and the registry never replaces a file, so it gets a version of its own.
pub fn version(commit: &str, digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    format!("{commit}-{}", hex.get(..12).unwrap_or(hex))
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Checks the `build` an App's lane proposes (`status.build`) and publishes it, or says which
/// check failed; nothing is published before every check has passed (AP-104).
pub async fn check_and_publish(
    shako: &ShakoClient,
    wasm: Option<&crate::apps::wasm_apps::WasmApps>,
    project: &str,
    name: &str,
    spec: &Value,
    build: &Value,
) -> Result<Option<crate::apps::wasm_apps::Published>, ApiError> {
    let field = |key: &str| build.get(key).and_then(Value::as_str).unwrap_or_default();
    let (digest, commit) = (field("digest"), field("commit"));
    if spec.pointer("/source/git").is_none_or(Value::is_null) {
        return Err(ApiError::BadRequest(format!(
            "status.build of App '{name}' is checked against the App's own repository on the \
             forge, and it names no spec.source.git (AP-100, AP-104)"
        )));
    }
    let repo = shako.for_application(repository::name(project, name));
    let at = format!("{}/{}", repo.owner, repo.repo);
    let forge = |err: GitError| {
        ApiError::Unavailable(format!(
            "the forge could not check the build of App '{name}' in {at}: {err}"
        ))
    };

    let branch = repo.default_branch().await.map_err(forge)?;
    let head = repo.branch_head(&branch).await.map_err(forge)?;
    if head != commit {
        return Err(ApiError::Conflict(format!(
            "status.build names commit {commit}, but the head of {branch} in {at} is {head}: \
             only the newest commit of the default branch is published (AP-104)"
        )));
    }

    // A `ui` App's build is a bundle, a `ui-rust` App's an image (AP-101, AP-105), the old
    // names read as the new ones (AP-124).
    let class = spec
        .get("kind")
        .and_then(Value::as_str)
        .map(jc_core::kinds::AppClass::parse);
    let ui_rust = class == Some(Ok(jc_core::kinds::AppClass::UiRust));
    let is_wasm = class == Some(Ok(jc_core::kinds::AppClass::Wasm));
    let (artifact, limit) = if ui_rust {
        ("image", MAX_IMAGE_BYTES)
    } else {
        ("bundle", MAX_BUNDLE_BYTES)
    };
    let (bytes, sbom) = match held_for(project, name, commit, digest) {
        Some(held) => held.bytes(&at, artifact, limit)?,
        None => match from_actions(&repo, &at, artifact, commit, limit).await {
            Ok(both) => both,
            Err(ActionsError::Forge(err)) => return Err(forge(err)),
            Err(ActionsError::Refused(refusal)) => return Err(refusal),
        },
    };
    let image = if ui_rust {
        let image = image_of(&bytes, digest).map_err(|reason| {
            ApiError::BadRequest(format!(
                "artifact image-{commit} of {at} is not the image status.build names: {reason} \
                 (AP-107)"
            ))
        })?;
        Some(image)
    } else {
        let hashed = sha256(&bytes);
        if hashed != digest {
            return Err(ApiError::BadRequest(format!(
                "artifact bundle-{commit} of {at} hashes to {hashed}, not the digest {digest} \
                 status.build names (AP-104)"
            )));
        }
        None
    };
    // A wasm App's server comes from the bundle just checked, and its schema is migrated before
    // anything is published: a migration that fails holds the publication (AP-149, AP-151).
    let published = if is_wasm {
        let wasm = wasm.ok_or_else(|| {
            ApiError::Unavailable(format!(
                "App '{name}' is a wasm App, and this Portal has no apps database configured to \
                 publish one (AP-149)"
            ))
        })?;
        let migrations = migrations_at(&repo, spec, commit).await.map_err(forge)?;
        let published = wasm
            .publish(project, name, &bytes, &migrations)
            .await
            .map_err(|why| ApiError::BadRequest(format!("App '{name}' is not published: {why}")))?;
        Some(published)
    } else {
        None
    };
    let package = package(name);
    let conflict = |err: GitError| match err {
        GitError::Conflict(message) => ApiError::Conflict(message),
        other => forge(other),
    };
    let files = match image {
        Some(image) => {
            let blobs: Vec<(&str, &[u8])> = image
                .blobs
                .iter()
                .map(|(digest, bytes)| (digest.as_str(), bytes.as_slice()))
                .collect();
            let stored = repo
                .push_image(&package, commit, &blobs, OCI_MANIFEST, &image.manifest)
                .await
                .map_err(forge)?;
            if stored != digest {
                return Err(ApiError::Conflict(format!(
                    "the registry stored {package}:{commit} as {stored}, not the {digest} \
                     status.build names (AP-107)"
                )));
            }
            vec![("sbom.cdx.json", sbom)]
        }
        None => vec![("bundle.tar.gz", bytes), ("sbom.cdx.json", sbom)],
    };
    let version = version(commit, digest);
    for (file, bytes) in files {
        publish_once(&repo, &package, &version, file, bytes)
            .await
            .map_err(conflict)?;
    }
    Ok(published)
}

/// A build the Portal's own lane holds (the jc build lane): the artifact and the SBOM one build
/// pod of `commit` uploaded with its build's token, frozen before the Portal hashed them. While
/// [`with_held`] runs, the lane's `status.build` of exactly this App, commit and digest is the
/// Portal's own write, checked and published from these bytes instead of the forge's artifacts
/// (AP-73, AP-104). Only Portal code sets it: no request can.
#[derive(Debug, Clone)]
pub struct Held {
    pub project: String,
    pub app: String,
    pub commit: String,
    pub digest: String,
    pub artifact: std::sync::Arc<Vec<u8>>,
    pub sbom: std::sync::Arc<Vec<u8>>,
}

tokio::task_local! {
    static HELD: Held;
}

/// Runs `work` with `held` as the build the Portal's lane writes.
pub async fn with_held<F: std::future::Future>(held: Held, work: F) -> F::Output {
    HELD.scope(held, work).await
}

/// The build the Portal's lane holds for exactly this App, commit and digest, if any.
pub fn held_for(project: &str, app: &str, commit: &str, digest: &str) -> Option<Held> {
    HELD.try_with(|held| {
        (held.project == project
            && held.app == app
            && held.commit == commit
            && held.digest == digest)
            .then(|| held.clone())
    })
    .ok()
    .flatten()
}

impl Held {
    /// The artifact and SBOM, within the artifact's limit.
    fn bytes(&self, at: &str, artifact: &str, limit: u64) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
        let size = self.artifact.len() as u64;
        if size > limit {
            return Err(ApiError::BadRequest(format!(
                "the {artifact} of the build of {} in {at} is {size} bytes, more than the {limit} a \
                 {artifact} may be",
                self.commit
            )));
        }
        Ok((self.artifact.to_vec(), self.sbom.to_vec()))
    }
}

/// The artifact and SBOM of a forge workflow run of `commit`: the newest `{artifact}-{commit}`
/// of a run of that commit within `limit`, and the `sbom-{commit}` of the same run.
async fn from_actions(
    repo: &ShakoClient,
    at: &str,
    artifact: &str,
    commit: &str,
    limit: u64,
) -> Result<(Vec<u8>, Vec<u8>), ActionsError> {
    let built = newest_of(repo, &format!("{artifact}-{commit}"), commit, None)
        .await?
        .ok_or_else(|| {
            ActionsError::Refused(ApiError::BadRequest(format!(
                "{at} holds no artifact {artifact}-{commit} from a run of commit {commit} (AP-104)"
            )))
        })?;
    if built.size > limit {
        return Err(ActionsError::Refused(ApiError::BadRequest(format!(
            "artifact {artifact}-{commit} of {at} is {} bytes, more than the {limit} a \
             {artifact} may be",
            built.size
        ))));
    }
    let bytes = repo.download_artifact(built.id, limit).await?;
    let sbom = newest_of(repo, &format!("sbom-{commit}"), commit, Some(built.run))
        .await?
        .ok_or_else(|| {
            ActionsError::Refused(ApiError::BadRequest(format!(
                "{at} holds no artifact sbom-{commit} from the run that uploaded the \
                 {artifact} (AP-101, AP-104)"
            )))
        })?;
    let sbom = repo.download_artifact(sbom.id, MAX_SBOM_BYTES).await?;
    Ok((bytes, sbom))
}

/// Why the forge's artifacts were not read: the forge, or a check of what it holds.
enum ActionsError {
    Forge(GitError),
    Refused(ApiError),
}

impl From<GitError> for ActionsError {
    fn from(err: GitError) -> Self {
        Self::Forge(err)
    }
}

/// The App's migration files at the built commit: the `.sql` files directly in the folder its
/// `spec.storage.sql.migrations` names, in name order; none without that field.
async fn migrations_at(
    repo: &ShakoClient,
    spec: &Value,
    commit: &str,
) -> Result<Vec<crate::apps::apps_db::Migration>, GitError> {
    let Some(folder) = spec
        .pointer("/storage/sql/migrations")
        .and_then(Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let tree = repo.list_tree(commit).await?;
    let mut migrations = Vec::new();
    for path in crate::apps::wasm_apps::migration_paths(&tree, folder) {
        if let Some(file) = repo.get_file(&path, commit).await? {
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            migrations.push(crate::apps::apps_db::Migration {
                file: name,
                sql: file.content,
            });
        }
    }
    Ok(migrations)
}

/// The newest artifact `name` a run of `commit` uploaded, from run `run` when one is named.
async fn newest_of(
    repo: &ShakoClient,
    name: &str,
    commit: &str,
    run: Option<u64>,
) -> Result<Option<Artifact>, GitError> {
    Ok(repo
        .artifacts_named(name)
        .await?
        .into_iter()
        .filter(|artifact| artifact.commit == commit && run.is_none_or(|run| artifact.run == run))
        .max_by_key(|artifact| artifact.id))
}

/// Writes one file of the package version. The registry never replaces a file: one already
/// there with the same bytes is the same build published again, and one with other bytes is
/// refused (AP-101); the digest in the version keeps a rebuild on a newer release apart.
async fn publish_once(
    shako: &ShakoClient,
    package: &str,
    version: &str,
    file: &str,
    bytes: Vec<u8>,
) -> Result<(), GitError> {
    let limit = bytes.len() as u64;
    let expected = sha256(&bytes);
    match shako.put_generic_file(package, version, file, bytes).await {
        Err(GitError::Conflict(_)) => {
            let held = shako
                .get_generic_file(package, version, file, limit)
                .await
                .map(|held| sha256(&held));
            match held {
                Ok(held) if held == expected => Ok(()),
                Ok(_) => Err(GitError::Conflict(format!(
                    "package {package} version {version} already holds a different {file}, and \
                     the registry never replaces a file (AP-101)"
                ))),
                Err(other) => Err(other),
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OCI layout tar the way `lane.mjs image` writes one, with `edit` applied to its files.
    fn layout(edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> (Vec<u8>, String) {
        let config = br#"{"architecture":"amd64","os":"linux"}"#.to_vec();
        let layer = b"a gzip layer".to_vec();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST,
            "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": sha256(&config), "size": config.len() },
            "layers": [{ "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": sha256(&layer), "size": layer.len() }],
        }))
        .expect("json");
        let digest = sha256(&manifest);
        let index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "manifests": [{ "mediaType": OCI_MANIFEST, "digest": digest, "size": manifest.len() }],
        }))
        .expect("json");
        let blob = |bytes: &[u8]| {
            format!(
                "blobs/sha256/{}",
                sha256(bytes).trim_start_matches("sha256:")
            )
        };
        let mut files = vec![
            (
                "oci-layout".to_owned(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            ("index.json".to_owned(), index),
            (blob(&config), config),
            (blob(&layer), layer),
            (blob(&manifest), manifest),
        ];
        edit(&mut files);
        let mut tar = tar::Builder::new(Vec::new());
        for (path, bytes) in &files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("./{path}"), bytes.as_slice())
                .expect("append");
        }
        (tar.into_inner().expect("tar"), digest)
    }

    /// AP-107: the layout's manifest is the digest, and its config and layer come back under
    /// theirs, in the order the manifest names them.
    #[test]
    fn a_layout_holding_the_digest_gives_its_manifest_and_blobs() {
        let (tar, digest) = layout(|_| {});
        let image = image_of(&tar, &digest).expect("the image");
        assert_eq!(sha256(&image.manifest), digest);
        assert_eq!(image.blobs.len(), 2);
        for (named, bytes) in &image.blobs {
            assert_eq!(&sha256(bytes), named);
        }
        assert_eq!(image.blobs[1].1, b"a gzip layer");
    }

    /// AP-107: each way a layout can be something other than the proposed image is refused,
    /// with the reason: another digest, a changed blob, a missing blob, a changed manifest, a
    /// digest that is a path, and bytes that are no tar at all.
    #[test]
    fn a_layout_that_is_not_the_proposed_image_is_refused_with_the_reason() {
        let (tar, digest) = layout(|_| {});
        let other = format!("sha256:{}", "0".repeat(64));
        assert!(image_of(&tar, &other)
            .unwrap_err()
            .contains("names no manifest"));
        assert!(image_of(&tar, "sha256:../../etc/passwd").is_err());

        let (changed, digest_of_changed) = layout(|files| {
            let layer = files
                .iter_mut()
                .find(|(_, b)| b == b"a gzip layer")
                .expect("layer");
            layer.1 = b"another layer".to_vec();
        });
        assert!(image_of(&changed, &digest_of_changed)
            .unwrap_err()
            .contains("is not the one the manifest names"));

        let (missing, digest_of_missing) =
            layout(|files| files.retain(|(_, b)| b != b"a gzip layer"));
        assert!(image_of(&missing, &digest_of_missing)
            .unwrap_err()
            .contains("holds no blob"));

        let (rewritten, digest_of_rewritten) = layout(|files| {
            let manifest = files
                .iter_mut()
                .find(|(_, b)| b.windows(8).any(|w| w == b"\"layers\""))
                .expect("manifest");
            manifest.1.push(b' ');
        });
        assert!(image_of(&rewritten, &digest_of_rewritten)
            .unwrap_err()
            .contains("does not hash to"));

        assert!(image_of(b"not a tar at all", &digest).is_err());
        assert_eq!(digest.len(), "sha256:".len() + 64);
    }

    /// A tar of `files` (path, bytes), each a regular file.
    fn tar_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, bytes.as_slice())
                .expect("append");
        }
        tar.into_inner().expect("tar")
    }

    /// A layout whose manifest is `manifest`, under its own digest in the index.
    fn layout_with_manifest(manifest: &[u8]) -> (Vec<u8>, String) {
        let digest = sha256(manifest);
        let index = serde_json::json!({ "manifests": [{ "digest": digest }] }).to_string();
        let files = vec![
            ("index.json".to_owned(), index.into_bytes()),
            (
                format!("blobs/sha256/{}", digest.trim_start_matches("sha256:")),
                manifest.to_vec(),
            ),
        ];
        (tar_of(&files), digest)
    }

    /// A manifest that is no JSON, no OCI manifest, names no layer or names a descriptor that
    /// is no digest is refused with that reason, even when it hashes to the digest.
    #[test]
    fn a_manifest_that_is_no_oci_image_is_refused_with_the_reason() {
        for (manifest, reason) in [
            (b"{not json".to_vec(), "the manifest is not JSON"),
            (
                serde_json::to_vec(&serde_json::json!({ "mediaType": "application/vnd.docker.distribution.manifest.v2+json" }))
                    .expect("json"),
                "the manifest is not an",
            ),
            (
                serde_json::to_vec(&serde_json::json!({ "mediaType": OCI_MANIFEST, "config": {} }))
                    .expect("json"),
                "the manifest names no layer",
            ),
            (
                serde_json::to_vec(&serde_json::json!({
                    "mediaType": OCI_MANIFEST,
                    "config": { "digest": "md5:abc" },
                    "layers": [{ "digest": "sha256:../../etc" }],
                }))
                .expect("json"),
                "which is not a sha256 digest",
            ),
        ] {
            let (tar, digest) = layout_with_manifest(&manifest);
            let err = image_of(&tar, &digest).expect_err(reason);
            assert!(err.contains(reason), "{err}");
        }
    }

    /// A layout of more than 64 entries is refused before it is read; a directory entry is
    /// skipped; a layout cut short is no tar; no index.json is named.
    #[test]
    fn a_layout_too_big_cut_short_or_without_an_index_is_refused() {
        let many: Vec<(String, Vec<u8>)> =
            (0..65).map(|i| (format!("f{i}"), b"x".to_vec())).collect();
        assert!(image_of(&tar_of(&many), "sha256:x")
            .unwrap_err()
            .contains("more than 64 entries"));

        let (tar, digest) = layout(|_| {});
        let mut with_dir = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        with_dir
            .append_data(&mut header, "blobs/", std::io::empty())
            .expect("a directory");
        let mut bytes = with_dir.into_inner().expect("tar");
        bytes.truncate(512);
        bytes.extend_from_slice(&tar);
        assert!(
            image_of(&bytes, &digest).is_ok(),
            "a directory entry is skipped"
        );

        let mut cut = tar_of(&[("index.json".to_owned(), vec![b' '; 1000])]);
        cut.truncate(512 + 10);
        let err = image_of(&cut, &digest).expect_err("cut short");
        // The tar reader hands over what the entry holds; the cut shows at the next header.
        assert!(err.contains("the layout is not a tar"), "{err}");

        let no_index = tar_of(&[("oci-layout".to_owned(), b"{}".to_vec())]);
        assert_eq!(
            image_of(&no_index, &digest).unwrap_err(),
            "the layout has no index.json"
        );
    }

    fn held(artifact: &[u8]) -> Held {
        Held {
            project: "roads".into(),
            app: "shop".into(),
            commit: "c0ffee".into(),
            digest: "sha256:aa".into(),
            artifact: std::sync::Arc::new(artifact.to_vec()),
            sbom: std::sync::Arc::new(b"{}".to_vec()),
        }
    }

    /// The lane's build is the held one only for exactly its App, commit and digest, and only
    /// inside its scope; an artifact over the limit is refused with its size.
    #[tokio::test]
    async fn a_held_build_counts_only_for_its_own_app_commit_and_digest() {
        assert!(held_for("roads", "shop", "c0ffee", "sha256:aa").is_none());
        with_held(held(b"bundle!"), async {
            let found = held_for("roads", "shop", "c0ffee", "sha256:aa").expect("held");
            for (project, app, commit, digest) in [
                ("other", "shop", "c0ffee", "sha256:aa"),
                ("roads", "other", "c0ffee", "sha256:aa"),
                ("roads", "shop", "beef", "sha256:aa"),
                ("roads", "shop", "c0ffee", "sha256:bb"),
            ] {
                assert!(held_for(project, app, commit, digest).is_none());
            }
            let (artifact, sbom) = found.bytes("jc/shop", "bundle", 7).expect("within");
            assert_eq!((artifact, sbom), (b"bundle!".to_vec(), b"{}".to_vec()));
            let err = found.bytes("jc/shop", "bundle", 6).expect_err("over");
            assert!(
                err.to_string()
                    .contains("is 7 bytes, more than the 6 a bundle may be"),
                "{err}"
            );
        })
        .await;
    }

    /// The registry never replaces a file: the same bytes already there are the same build,
    /// other bytes are a conflict, and a registry that cannot be read passes its error on.
    #[tokio::test]
    async fn a_file_already_published_is_kept_only_when_its_bytes_match() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for (held, outcome) in [
            (
                ResponseTemplate::new(200).set_body_bytes(b"same".to_vec()),
                "ok",
            ),
            (
                ResponseTemplate::new(200).set_body_bytes(b"diff".to_vec()),
                "conflict",
            ),
            (ResponseTemplate::new(500), "error"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path_regex("PublishPackage$"))
                .respond_with(ResponseTemplate::new(409).set_body_string("exists"))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path_regex("PackageFiles"))
                .respond_with(held)
                .mount(&server)
                .await;
            let shako = ShakoClient::new(server.uri().parse().expect("url"), "jc", "config", "t")
                .expect("client");
            let result = publish_once(
                &shako,
                "app-shop",
                "c0ffee-aa",
                "bundle.tar.gz",
                b"same".to_vec(),
            )
            .await;
            match outcome {
                "ok" => assert!(result.is_ok(), "{result:?}"),
                "conflict" => assert!(
                    matches!(&result, Err(GitError::Conflict(m)) if m.contains("never replaces a file")),
                    "{result:?}"
                ),
                _ => assert!(
                    result.is_err() && !matches!(result, Err(GitError::Conflict(_))),
                    "{result:?}"
                ),
            }
        }
    }
}
