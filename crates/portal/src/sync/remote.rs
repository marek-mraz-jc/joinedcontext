//! The three origins a `SyncSource` may name, over HTTP and nothing else (MF-27, MF-32).
//!
//! `jcctl::sync` decides when a run is due, what the import gates make of the checkout and
//! what the proposal is. This is the hop it deliberately does not have, and it answers two
//! questions per origin: where is the source now, and what is in it. The cheap question runs
//! on every tick and the expensive one only when the answer changed, which is why they are two
//! calls — a source that has not moved must not cost a download.
//!
//! Everything read here is input from outside the platform:
//!
//! - `https` only, so a source cannot be rewritten in transit;
//! - a redirect is followed only to another `https` address, and only a few times;
//! - a ceiling on every body and on every file inside an archive, because an origin that
//!   streams without end is an origin that fills the Portal's disk;
//! - an archive entry that climbs out of the checkout directory is refused, not written.
//!
//! **No credential goes out.** A `secretRef` names a Secret the reconciler resolves, and the
//! Portal has no secret store to resolve it through yet, so an origin that declares one is
//! refused with that sentence rather than fetched anonymously — which would look to an
//! operator like a private source that quietly syncs nothing (MF-31).

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jc_core::kinds::SyncOrigin;
use jcctl::sync::{RemoteError, SyncRemote};

use super::guard::{https_redirects, internal_host};
use jc_core::outbound::PublicOnly;

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The largest body a run downloads: a configuration subtree, not a data set.
const MAX_BODY: u64 = 64 * 1024 * 1024;

/// The largest single file inside an archive.
const MAX_ENTRY: u64 = 16 * 1024 * 1024;

/// How many entries an archive may hold, the ceiling the import wizard already applies.
const MAX_ENTRIES: usize = 10_000;

/// The bytes a whole checkout may occupy on disk once unpacked.
///
/// [`MAX_BODY`] bounds the download and [`MAX_ENTRY`] one file of it, and neither bounds their
/// product: ten thousand entries of sixteen megabytes is a hundred and sixty gigabytes written
/// to the node from an archive that arrived well inside both. Four times the largest download
/// is what text compresses to and far past any configuration subtree (MF-32, T-1705).
const MAX_CHECKOUT: u64 = 4 * MAX_BODY;

/// How many `https` redirects an origin may hide behind.
const MAX_REDIRECTS: usize = 3;

/// The origins of MF-27, read over HTTP.
pub struct HttpRemote {
    http: reqwest::blocking::Client,
}

impl HttpRemote {
    /// A client that refuses to leave `https`, times out and follows a few redirects.
    ///
    /// Blocking, because [`SyncRemote`] is a synchronous trait: the loop above it is a
    /// decision and not a process, and the caller runs the whole of it on a blocking task
    /// rather than holding a runtime worker while a repository is downloaded.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(https_redirects(MAX_REDIRECTS))
                .dns_resolver(Arc::new(PublicOnly))
                .timeout(TIMEOUT)
                .build()?,
        })
    }

    /// One body, with the ceiling applied whether or not the origin declares a length.
    fn body(&self, url: &str) -> Result<Vec<u8>, RemoteError> {
        secure(url)?;
        let response = self
            .http
            .get(url)
            .send()
            .map_err(|err| RemoteError::Unavailable(reason(err)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(RemoteError::Refused(format!(
                "{url} answered {}",
                status.as_u16()
            )));
        }
        bounded(response, MAX_BODY, url)
    }

    /// The commit a ref points at, from the advertisement every Git HTTP server serves.
    ///
    /// `GET {url}/info/refs?service=git-upload-pack` is the first half of a clone and a plain
    /// GET, which is what makes the cheap question cheap: no pack is negotiated and nothing is
    /// downloaded but a list of names.
    fn git_revision(&self, url: &str, git_ref: &str) -> Result<String, RemoteError> {
        // A ref that is already a commit is its own answer, and it is how a pinned source is
        // written. Answered before the request, because a pinned source needs no round trip.
        if is_commit(git_ref) {
            return Ok(git_ref.to_owned());
        }
        let base = url.trim_end_matches('/').trim_end_matches(".git");
        let body = self.body(&format!("{base}.git/info/refs?service=git-upload-pack"))?;
        let text = String::from_utf8_lossy(&body);
        advertised(&text, git_ref).ok_or_else(|| {
            RemoteError::Refused(format!("{base} advertises no ref named `{git_ref}`"))
        })
    }
}

impl SyncRemote for HttpRemote {
    fn revision(&self, origin: &SyncOrigin) -> Result<String, RemoteError> {
        credential_free(origin)?;
        if let Some(git) = &origin.git {
            return self.git_revision(&git.url, &git.git_ref);
        }
        if let Some(bundle) = &origin.bundle {
            // The bundle's own bytes are its revision. A published bundle is a file, and a
            // file that has not changed hashes the same, whatever its server says about it.
            // An `ETag` would be cheaper and no static host is obliged to send one; this is
            // one download of something an unchanged source then does nothing with.
            return Ok(digest(&self.body(&bundle.url)?));
        }
        if let Some(api) = &origin.platform_api {
            let url = format!(
                "{}/api/v1/projects/{}/revisions?limit=1",
                api.base_url.trim_end_matches('/'),
                api.project
            );
            let body = self.body(&url)?;
            let list: serde_json::Value = serde_json::from_slice(&body).map_err(|err| {
                RemoteError::Refused(format!("{url} is not a revision list: {err}"))
            })?;
            return latest(&list).map(str::to_owned).ok_or_else(|| {
                RemoteError::Refused(format!(
                    "{url} lists no revision, so project `{}` has nothing to sync from",
                    api.project
                ))
            });
        }
        Err(RemoteError::Refused(
            "the source names no origin".to_owned(),
        ))
    }

    fn checkout(
        &self,
        origin: &SyncOrigin,
        revision: &str,
        into: &Path,
    ) -> Result<(), RemoteError> {
        credential_free(origin)?;
        if let Some(git) = &origin.git {
            let archive = self.body(&archive_url(&git.url, revision))?;
            return unpack(&archive, into, git.path.as_deref(), true);
        }
        if let Some(bundle) = &origin.bundle {
            let body = self.body(&bundle.url)?;
            // A bundle URL is an archive or a multi-document YAML, and the first bytes of a
            // zip say which (MF-16, MF-17).
            if body.starts_with(b"PK\x03\x04") {
                return unpack(&body, into, None, false);
            }
            return write(&into.join("bundle.yaml"), &body);
        }
        if let Some(api) = &origin.platform_api {
            // The partner's own export of the project at that revision: its public resource
            // API and nothing else (MF-32). It is the same archive the import wizard reads, so
            // a sync from an instance and an import of its download are one code path.
            let url = format!(
                "{}/api/v1/projects/{}/export?format=zip&revision={revision}",
                api.base_url.trim_end_matches('/'),
                api.project
            );
            let archive = self.body(&url)?;
            if !archive.starts_with(b"PK\x03\x04") {
                return Err(RemoteError::Refused(format!(
                    "{url} did not answer with an archive"
                )));
            }
            return unpack(&archive, into, None, false);
        }
        Err(RemoteError::Refused(
            "the source names no origin".to_owned(),
        ))
    }
}

/// Refuses an origin whose credential this instance cannot resolve (MF-31).
fn credential_free(origin: &SyncOrigin) -> Result<(), RemoteError> {
    match origin.secret_ref() {
        None => Ok(()),
        Some(secret) => Err(RemoteError::Refused(format!(
            "the origin authenticates with secretRef `{}` and the Portal has no secret store to \
             resolve it through; a private source is refused rather than fetched anonymously \
             (MF-31)",
            secret.name
        ))),
    }
}

/// The object name a `git-upload-pack` advertisement gives one ref, branches before tags.
///
/// Each pkt-line is `<4 hex length><sha> <name>[\0capabilities]`, and the two fields this needs
/// survive a split on whitespace.
fn advertised(text: &str, git_ref: &str) -> Option<String> {
    let wanted = [
        format!("refs/heads/{git_ref}"),
        format!("refs/tags/{git_ref}"),
        format!("refs/tags/{git_ref}^{{}}"),
    ];
    let mut found: Option<(usize, String)> = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(first), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        let name = name.split('\0').next().unwrap_or(name);
        let Some(rank) = wanted.iter().position(|candidate| candidate == name) else {
            continue;
        };
        let Some(sha) = object_name(first) else {
            continue;
        };
        // A peeled tag (`^{}`) is the commit an annotated tag names, which is what a checkout
        // wants; otherwise the first match wins.
        if rank == 2 || found.as_ref().is_none_or(|(seen, _)| rank < *seen) {
            found = Some((rank, sha));
        }
    }
    found.map(|(_, sha)| sha)
}

/// The 40-character object name at the head of an advertisement line, past its pkt-line length.
fn object_name(field: &str) -> Option<String> {
    let candidate = if field.len() > 40 {
        &field[field.len() - 40..]
    } else {
        field
    };
    (candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| candidate.to_owned())
}

/// Whether `text` is a commit id: 7 to 64 hex digits (abbreviated, SHA-1 or SHA-256).
fn is_commit(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The newest revision a platform origin lists. The origin is another party: its answer goes
/// into the export URL and the branch name, so anything but a commit id is no revision.
fn latest(list: &serde_json::Value) -> Option<&str> {
    list.pointer("/items/0/sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| is_commit(sha))
}

/// Where a forge serves the tree of one commit as a zip.
///
/// Gitea and GitHub both answer `{repository}/archive/{revision}.zip`, which covers the
/// platform's own forge and the public one a shared blueprint library lives on. An origin that
/// answers something else is refused by name rather than half-read: the alternative is a Git
/// pack negotiation, and a reconciler is not the place for a second Git implementation.
fn archive_url(url: &str, revision: &str) -> String {
    let base = url.trim_end_matches('/').trim_end_matches(".git");
    format!("{base}/archive/{revision}.zip")
}

/// Unpacks an archive into the checkout, keeping only what the source declares.
///
/// `strip_root` drops the single directory a forge archive wraps everything in
/// (`{repo}-{revision}/`), so `path` means what it means in the repository.
fn unpack(
    archive: &[u8],
    into: &Path,
    subtree: Option<&str>,
    strip_root: bool,
) -> Result<(), RemoteError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|err| RemoteError::Refused(format!("the archive did not open: {err}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(RemoteError::Refused(format!(
            "the archive holds {} entries, past the {MAX_ENTRIES} entry limit",
            zip.len()
        )));
    }

    let prefix = subtree.map(|path| path.trim_matches('/').to_owned());
    let mut written = 0usize;
    // What is left of the checkout ceiling, counted across entries: a per-file limit alone
    // lets ten thousand files of it fill the node (T-1705).
    let mut budget = MAX_CHECKOUT;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|err| RemoteError::Refused(format!("archive entry {index}: {err}")))?;
        if entry.is_dir() {
            continue;
        }
        if entry.size() > MAX_ENTRY {
            return Err(RemoteError::Refused(format!(
                "{} is larger than the {MAX_ENTRY} bytes one file of a source may have",
                // zip 9: a name that is not UTF-8 is an error, not a reason to say nothing
                entry
                    .name()
                    .unwrap_or_else(|_| "(a name that is not text)".into())
            )));
        }
        // `enclosed_path` is the zip-slip guard: `None` for a path that is absolute or climbs out
        // of the archive root.
        let Some(name) = crate::api::import::enclosed_path(&entry) else {
            return Err(RemoteError::Refused(format!(
                "the archive carries `{}`, which does not stay inside the checkout",
                // zip 9: a name that is not UTF-8 is an error, not a reason to say nothing
                entry
                    .name()
                    .unwrap_or_else(|_| "(a name that is not text)".into())
            )));
        };
        let relative = if strip_root {
            let mut parts = name.components();
            parts.next();
            parts.as_path().to_path_buf()
        } else {
            name
        };
        let relative = match &prefix {
            Some(prefix) => match relative.strip_prefix(prefix) {
                Ok(inside) => inside.to_path_buf(),
                // Outside the subtree the source declares, so not part of this sync (MF-27).
                Err(_) => continue,
            },
            None => relative,
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        budget = write_entry(into, &relative, &mut entry, budget)?;
        written += 1;
    }
    if written == 0 {
        return Err(RemoteError::Refused(match &prefix {
            Some(prefix) => format!("the source carries nothing under `{prefix}`"),
            None => "the source carries no files".to_owned(),
        }));
    }
    Ok(())
}

/// One entry written under the checkout, and what is left of the checkout's byte ceiling.
fn write_entry(
    into: &Path,
    relative: &Path,
    entry: &mut impl Read,
    budget: u64,
) -> Result<u64, RemoteError> {
    // `enclosed_name` already refused an escaping path; this is the same rule read a second
    // time, kept because it is the one guarding the join that actually writes.
    if relative
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
    {
        return Err(RemoteError::Refused(format!(
            "the archive carries `{}`, which does not stay inside the checkout",
            relative.display()
        )));
    }
    let ceiling = MAX_ENTRY.min(budget);
    let mut body = Vec::new();
    entry
        .take(ceiling + 1)
        .read_to_end(&mut body)
        .map_err(|err| RemoteError::Unavailable(format!("{}: {err}", relative.display())))?;
    if body.len() as u64 > ceiling {
        return Err(RemoteError::Refused(if ceiling == MAX_ENTRY {
            format!(
                "{} unpacks to more than the {MAX_ENTRY} bytes one file of a source may have",
                relative.display()
            )
        } else {
            format!(
                "the source unpacks to more than the {MAX_CHECKOUT} bytes a checkout may occupy"
            )
        }));
    }
    write(&into.join(relative), &body)?;
    Ok(budget - body.len() as u64)
}

/// Everything a reader has, up to a ceiling, and a refusal past it.
///
/// One byte past the ceiling rather than trusting `Content-Length`: a chunked response declares
/// no length at all, and that is the shape a slow drip arrives in. The timeout on the client is
/// the other half — this one bounds the size, that one bounds the wait (MF-32, T-1705).
fn bounded(reader: impl Read, ceiling: u64, url: &str) -> Result<Vec<u8>, RemoteError> {
    let mut body = Vec::new();
    reader
        .take(ceiling + 1)
        .read_to_end(&mut body)
        .map_err(|err| RemoteError::Unavailable(format!("{url}: {err}")))?;
    if body.len() as u64 > ceiling {
        return Err(RemoteError::Refused(format!(
            "{url} is larger than the {ceiling} bytes a sync source may carry"
        )));
    }
    Ok(body)
}

fn write(path: &PathBuf, body: &[u8]) -> Result<(), RemoteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| RemoteError::Unavailable(format!("{}: {err}", parent.display())))?;
    }
    std::fs::write(path, body)
        .map_err(|err| RemoteError::Unavailable(format!("{}: {err}", path.display())))
}

/// A source is read over TLS or it is not read.
///
/// `jc-core` also accepts `ssh://` and `git@` for a Git origin, because `jcctl` on a laptop can
/// use an agent. The Portal has no key and no `git` binary, so those are refused here by name
/// rather than failing later as a URL nothing can parse. The address rule itself lives in
/// [`super::guard`], because the peer schema mirror obeys the same one (T-1705).
fn secure(url: &str) -> Result<(), RemoteError> {
    if url.starts_with("https://") {
        if let Some(ip) = internal_host(url) {
            return Err(RemoteError::Refused(format!(
                "{url} names {ip}, which is not a public address; a sync source is read from \
                 public hosts only"
            )));
        }
        return Ok(());
    }
    if url.starts_with("ssh://") || url.starts_with("git@") {
        return Err(RemoteError::Refused(format!(
            "{url} is an ssh remote, and the Portal reads a sync source over https only"
        )));
    }
    Err(RemoteError::Refused(format!(
        "{url} is not https, and a sync source is not read in the clear"
    )))
}

fn digest(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(body)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// What went wrong, without the URL — there is no credential in one today, and this keeps the
/// log line right if one is ever added.
fn reason(err: reqwest::Error) -> String {
    if err.is_timeout() {
        return "the origin did not answer in time".to_owned();
    }
    if err.is_redirect() {
        return "the origin redirected somewhere a sync does not follow".to_owned();
    }
    if err.is_connect() {
        return "the origin could not be reached".to_owned();
    }
    err.without_url().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_platform_origin_lists_a_commit_or_no_revision() {
        let list = |sha: &str| serde_json::json!({ "items": [{ "sha": sha }] });
        assert_eq!(latest(&list("0a1b2c3d4e5f")), Some("0a1b2c3d4e5f"));
        for hostile in ["x&y/../z", "main", "0a1b2c3?format=tar", ""] {
            assert_eq!(latest(&list(hostile)), None, "{hostile}");
        }
        assert_eq!(latest(&serde_json::json!({ "items": [] })), None);
    }
    use jc_core::envelope::SecretRef;
    use jc_core::kinds::GitOrigin;

    fn git_origin(secret: Option<SecretRef>) -> SyncOrigin {
        SyncOrigin {
            git: Some(GitOrigin {
                url: "https://git.example/udp/models.git".to_owned(),
                git_ref: "main".to_owned(),
                path: None,
                secret_ref: secret,
            }),
            bundle: None,
            platform_api: None,
        }
    }

    fn zipped(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = zip::write::SimpleFileOptions::default();
            for (name, body) in entries {
                zip.start_file(*name, options).expect("an entry");
                std::io::Write::write_all(&mut zip, body.as_bytes()).expect("the body");
            }
            zip.finish().expect("the archive");
        }
        buffer
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jc-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_origin_that_needs_a_credential_is_refused_rather_than_fetched_anonymously() {
        let origin = git_origin(Some(SecretRef {
            name: "region-git-ro".to_owned(),
            key: None,
            env_var: None,
        }));
        let error = credential_free(&origin).unwrap_err();
        assert!(
            matches!(error, RemoteError::Refused(ref why) if why.contains("secret store")),
            "{error}"
        );
        assert!(credential_free(&git_origin(None)).is_ok());
    }

    /// MF-28, DM-49 (T-1705): a sync source is read over TLS or not at all, whichever scheme
    /// the origin reaches for.
    #[test]
    fn an_origin_that_is_not_https_is_refused_before_anything_is_requested() {
        for url in [
            "http://git.example/models.git",
            "file:///etc/passwd",
            "gopher://git.example/1/models",
            "ftp://git.example/models.zip",
            "data:text/yaml;base64,a2luZDogRW5kcG9pbnQK",
        ] {
            assert!(
                matches!(secure(url), Err(RemoteError::Refused(_))),
                "{url} was not refused"
            );
        }
        assert!(secure("git@git.example:udp/models.git").is_err());
        assert!(secure("ssh://git@git.example/udp/models.git").is_err());
        assert!(secure("https://git.example/models.git").is_ok());
    }

    /// MF-28, MF-32 (T-1705): an origin that streams without end is cut at the ceiling rather
    /// than read to the end of whatever it feels like sending.
    #[test]
    fn a_body_that_never_ends_is_cut_at_the_ceiling_and_named() {
        let endless = std::io::repeat(b'a');
        let error = bounded(endless, 4 * 1024, "https://git.example/models.zip").unwrap_err();
        assert!(
            matches!(error, RemoteError::Refused(ref why) if why.contains("is larger than")),
            "{error}"
        );
        let short = bounded(
            &b"kind: DataModel\n"[..],
            MAX_BODY,
            "https://git.example/one.yaml",
        )
        .expect("a body inside the ceiling is read whole");
        assert_eq!(short, b"kind: DataModel\n");
    }

    /// MF-32 (T-1705): a per-file ceiling alone lets many files fill the node, so the checkout
    /// has one of its own and the refusal says which it was.
    #[test]
    fn an_archive_whose_files_add_up_past_the_checkout_ceiling_is_refused() {
        let each = "x".repeat(8 * 1024 * 1024);
        let names: Vec<String> = (0..40)
            .map(|index| format!("models-c0ffee/models/m{index}.yaml"))
            .collect();
        let entries: Vec<(&str, &str)> = names
            .iter()
            .map(|name| (name.as_str(), each.as_str()))
            .collect();
        let into = scratch("checkout-ceiling");
        let error = unpack(&zipped(&entries), &into, None, true).unwrap_err();
        assert!(
            matches!(error, RemoteError::Refused(ref why) if why.contains("a checkout may occupy")),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn an_origin_naming_an_address_inside_the_cluster_is_refused_before_a_socket_is_opened() {
        // The Portal fetches a sync source from inside the cluster, so an origin that names the
        // node, the pod network or the link is a probe, not a source (T-0802, MF-31).
        for url in [
            "https://10.0.0.1/bundle.zip",
            "https://192.168.1.1/bundle.zip",
            "https://172.16.0.1/bundle.zip",
            "https://127.0.0.1/bundle.zip",
            "https://169.254.169.254/latest/meta-data/",
            "https://100.64.0.1/bundle.zip",
            "https://[::1]/bundle.zip",
            "https://[fd00::1]/bundle.zip",
            "https://[fe80::1]/bundle.zip",
        ] {
            assert!(
                matches!(secure(url), Err(RemoteError::Refused(_))),
                "{url} was not refused"
            );
        }
        // A public address, and a name, still pass: a name is judged by what it resolves to.
        assert!(secure("https://93.184.216.34/bundle.zip").is_ok());
        assert!(secure("https://git.example/udp/models.git").is_ok());
    }

    #[test]
    fn the_archive_url_is_the_one_a_forge_serves() {
        assert_eq!(
            archive_url("https://git.example/udp/models.git", "c0ffee1"),
            "https://git.example/udp/models/archive/c0ffee1.zip"
        );
        assert_eq!(
            archive_url("https://git.example/udp/models/", "c0ffee1"),
            "https://git.example/udp/models/archive/c0ffee1.zip"
        );
    }

    #[test]
    fn a_ref_advertisement_yields_the_commit_of_the_branch_and_not_of_another_ref() {
        let advertisement = concat!(
            "001e# service=git-upload-pack\n",
            "0000",
            "00550123456789abcdef0123456789abcdef01234567 HEAD\0symref=HEAD:refs/heads/main\n",
            "003f0123456789abcdef0123456789abcdef01234567 refs/heads/main\n",
            "003faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa refs/heads/topic\n",
            "0000",
        );
        assert_eq!(
            advertised(advertisement, "main").as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            advertised(advertisement, "topic").as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(advertised(advertisement, "release-1"), None);
    }

    #[test]
    fn a_tag_is_read_at_the_commit_it_peels_to() {
        let advertisement = concat!(
            "003fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb refs/tags/v1\n",
            "0042cccccccccccccccccccccccccccccccccccccccc refs/tags/v1^{}\n",
        );
        assert_eq!(
            advertised(advertisement, "v1").as_deref(),
            Some("cccccccccccccccccccccccccccccccccccccccc"),
            "an annotated tag object is not a tree to check out"
        );
    }

    #[test]
    fn an_archive_entry_that_climbs_out_is_refused() {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = zip::write::SimpleFileOptions::default();
            // Written raw, the way an archive built to escape carries it: `start_file` would
            // normalise the name away.
            zip.start_file("root/../../escape.yaml", options)
                .expect("an entry");
            std::io::Write::write_all(&mut zip, b"kind: Endpoint\n").expect("the body");
            zip.finish().expect("the archive");
        }
        let into = scratch("escape");
        let error = unpack(&buffer, &into, None, true).unwrap_err();
        assert!(
            matches!(error, RemoteError::Refused(ref why) if why.contains("checkout")),
            "{error}"
        );
        assert!(!into
            .parent()
            .expect("a parent")
            .join("escape.yaml")
            .exists());
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn an_archive_is_unpacked_without_the_directory_the_forge_wraps_it_in() {
        let archive = zipped(&[
            (
                "models-c0ffee/models/transport/bus.yaml",
                "kind: DataModel\n",
            ),
            ("models-c0ffee/README.md", "# not a manifest\n"),
        ]);
        let into = scratch("unpack");

        unpack(&archive, &into, Some("models/transport"), true).expect("the archive unpacks");
        assert_eq!(
            std::fs::read_to_string(into.join("bus.yaml")).expect("the manifest"),
            "kind: DataModel\n",
            "the subtree the source names is the root of the checkout"
        );
        assert!(
            !into.join("README.md").exists(),
            "what the source's `path` excludes is not written"
        );
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn a_source_whose_subtree_is_empty_says_so_instead_of_importing_nothing() {
        let archive = zipped(&[("models-c0ffee/README.md", "# not a manifest\n")]);
        let into = scratch("empty");
        let error = unpack(&archive, &into, Some("models/transport"), true).unwrap_err();
        assert!(
            matches!(error, RemoteError::Refused(ref why) if why.contains("models/transport")),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&into);
    }
}
