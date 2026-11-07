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
use crate::git::{Artifact, GitError, GiteaClient};

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
    gitea: &GiteaClient,
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
    let repo = gitea.for_application(repository::name(project, name));
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
    let built = newest_of(&repo, &format!("{artifact}-{commit}"), commit, None)
        .await
        .map_err(forge)?
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{at} holds no artifact {artifact}-{commit} from a run of commit {commit} (AP-104)"
            ))
        })?;
    if built.size > limit {
        return Err(ApiError::BadRequest(format!(
            "artifact {artifact}-{commit} of {at} is {} bytes, more than the {limit} a \
             {artifact} may be",
            built.size
        )));
    }
    let bytes = repo
        .download_artifact(built.id, limit)
        .await
        .map_err(forge)?;
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
    let sbom = newest_of(&repo, &format!("sbom-{commit}"), commit, Some(built.run))
        .await
        .map_err(forge)?
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{at} holds no artifact sbom-{commit} from the run that uploaded the \
                 {artifact} (AP-101, AP-104)"
            ))
        })?;
    let sbom = repo
        .download_artifact(sbom.id, MAX_SBOM_BYTES)
        .await
        .map_err(forge)?;

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

/// The App's migration files at the built commit: the `.sql` files directly in the folder its
/// `spec.storage.sql.migrations` names, in name order; none without that field.
async fn migrations_at(
    repo: &GiteaClient,
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
    repo: &GiteaClient,
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
    gitea: &GiteaClient,
    package: &str,
    version: &str,
    file: &str,
    bytes: Vec<u8>,
) -> Result<(), GitError> {
    let limit = bytes.len() as u64;
    let expected = sha256(&bytes);
    match gitea.put_generic_file(package, version, file, bytes).await {
        Err(GitError::Conflict(_)) => {
            let held = gitea
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
}
