//! context-gateway inside `jc`: every Endpoint read and write, the access document, the MCP hub
//! and the space surfaces, as on dev (auth, PolicyPdp, quotas, response scrub), in front of the
//! Antares mount.
//!
//! In slug mode the gateway answers at the paths dev's edge routes to it (`Module::root_paths`), on any
//! host, with the request path kept whole; in host mode it owns `api.{base_host}`.
//!
//! Its settings are its own `JC_GATEWAY_*` keys, read from the environment first. The keys it
//! shares with the Portal by name (`JC_OIDC_*`) are read as `JC_GATEWAY_OIDC_*` here, so one
//! environment configures both. A key the environment leaves unset comes from a sibling
//! ([`vars`]): the realm from kaisatsu (client `context-gateway`), the broker from Antares through
//! its private hop, the Portal's internal listener for revocations, connectors and usage, and the
//! manifests from shako's organization repository, read over its HTTP API into the state directory
//! every [`SYNC_EVERY`] by [`follow_forge`], as dev's git-sync and `jcctl checkouts` sidecars do
//! with git.

use crate::app::StartError;
use crate::module::Module;
use crate::modules::BuildCtx;
use crate::modules::Hop;
use crate::setup::{self, Secret, Setup};
use crate::siblings::{Siblings, reach};
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Uri};
use axum::middleware::Next;
use axum::response::Response;
use context_gateway::config::Config;
use jc_router::Mount;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the organization repository and the project checkouts are fetched (dev: 10 s).
pub const SYNC_EVERY: Duration = Duration::from_secs(10);

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Gateway, e)
}

/// The environment key the gateway's `key` is read from: `JC_OIDC_*` are the Portal's names too,
/// so the gateway's own are `JC_GATEWAY_OIDC_*`.
#[must_use]
pub fn env_key(key: &str) -> String {
    match key.strip_prefix("JC_OIDC_") {
        Some(rest) => format!("JC_GATEWAY_OIDC_{rest}"),
        None => key.to_owned(),
    }
}

/// The Portal's internal listener as the gateway reaches it: the address the Portal's listener
/// took inside this `jc` (a free loopback port by default, `modules::portal`), else an explicit
/// `JC_INTERNAL_BIND` with a port.
fn portal_internal(
    bound: Option<std::net::SocketAddr>,
    env: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let addr = match bound {
        Some(addr) => addr,
        None => env("JC_INTERNAL_BIND")?.trim().parse().ok()?,
    };
    (addr.port() != 0).then(|| format!("http://{}", reach(addr)))
}

/// Where `jc` keeps the gateway's checkouts.
#[must_use]
pub fn work_dir(setup: &Setup) -> PathBuf {
    setup.dir.join("gateway")
}

/// The gateway settings `jc` fills from its siblings, for the keys `env` (given the names of
/// [`env_key`]) leaves unset. Like the Portal's rows, a group is filled only when its first key is
/// unset and every secret it needs is held, so a generated credential never goes to an address the
/// operator chose. With `wire` off (`portal_siblings = false`) only the broker and the addresses
/// are filled: no realm and no forge.
///
/// | settings | from |
/// |---|---|
/// | `JC_GATEWAY_BROKER_URL` | Antares' hop |
/// | `JC_GATEWAY_ORG_DOMAIN`, `JC_GATEWAY_PUBLIC_URL`, `JC_GATEWAY_PREVIEWS_DIR` | the mounts, the state dir |
/// | `JC_GATEWAY_OAUTH_KEY` | the generated `gateway-oauth-key` |
/// | `JC_OIDC_ISSUER`, `_JWKS_URL`, `_CLIENT_ID`, `_CLIENT_SECRET`, `JC_GATEWAY_REVOCATIONS_URL`, `_CONNECTORS_URL`, `_USAGE_URL` | kaisatsu and the Portal |
/// | `JC_GATEWAY_REPO_DIR`, `_PROJECTS_DIR`, `_ASSEMBLY_DIR` | shako, checked out by [`follow_forge`] |
#[must_use]
pub fn vars(
    siblings: &Siblings,
    setup: &Setup,
    wire: bool,
    mount: &Mount,
    hop: Option<&Hop>,
    portal_listener: Option<std::net::SocketAddr>,
    env: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let unset = |key: &str| env(&env_key(key)).is_none_or(|v| v.trim().is_empty());
    let mut group = |values: Vec<(&str, Option<String>)>| {
        let Some((first, _)) = values.first() else {
            return;
        };
        if !unset(first) || values.iter().any(|(_, v)| v.is_none()) {
            return;
        }
        for (key, value) in values {
            if unset(key) {
                out.insert(key.to_owned(), value.unwrap_or_default());
            }
        }
    };
    let secret = |s: Secret| setup.value(s).map(str::to_owned);
    let work = work_dir(setup);
    let path = |rel: &str| Some(work.join(rel).display().to_string());

    group(vec![(
        "JC_GATEWAY_BROKER_URL",
        hop.map(|h| format!("http://{}", h.addr)),
    )]);
    let portal = siblings.portal_base();
    group(vec![(
        "JC_GATEWAY_ORG_DOMAIN",
        Some(crate::provision::domain(
            portal.as_deref().or(Some(&mount.public_base())),
        )),
    )]);
    group(vec![("JC_GATEWAY_PUBLIC_URL", Some(mount.public_base()))]);
    group(vec![("JC_GATEWAY_PREVIEWS_DIR", path("previews"))]);
    if !wire {
        return out;
    }
    // The anonymous OAuth server prod runs (ADR-N-052): its issuer is the public URL above.
    group(vec![(
        "JC_GATEWAY_OAUTH_KEY",
        secret(Secret::GatewayOauthKey),
    )]);
    if let (Some(id), Some(_), Some(internal)) = (
        siblings.get(Module::Kaisatsu),
        siblings.get(Module::Portal),
        portal_internal(portal_listener, &|k: &str| env(k)),
    ) {
        // No `JC_OIDC_TOKEN_URL`: the gateway's own token comes from the issuer's public
        // endpoint, as the Portal's sign-in does, because kaisatsu names the issuer by the host a
        // request reached it on and the Portal's internal listener checks it. The keys are read
        // on the listener (`JC_OIDC_JWKS_URL`).
        let realm = format!("/realms/{}", setup.realm);
        group(vec![
            ("JC_OIDC_ISSUER", Some(format!("{}{realm}", id.public))),
            (
                "JC_OIDC_JWKS_URL",
                Some(format!(
                    "{}{realm}/protocol/openid-connect/certs",
                    id.internal
                )),
            ),
            ("JC_OIDC_CLIENT_ID", Some(setup::GATEWAY_CLIENT.to_owned())),
            ("JC_OIDC_CLIENT_SECRET", secret(Secret::GatewayClientSecret)),
            (
                "JC_GATEWAY_REVOCATIONS_URL",
                Some(format!("{internal}/internal/revocations")),
            ),
            (
                "JC_GATEWAY_CONNECTORS_URL",
                Some(format!("{internal}/internal/connectors")),
            ),
            (
                "JC_GATEWAY_USAGE_URL",
                Some(format!("{internal}/internal/usage")),
            ),
        ]);
    }
    if siblings.get(Module::Shako).is_some() && setup.value(Secret::ShakoToken).is_some() {
        group(vec![
            ("JC_GATEWAY_REPO_DIR", path("repo/current")),
            ("JC_GATEWAY_PROJECTS_DIR", path("projects")),
            ("JC_GATEWAY_ASSEMBLY_DIR", path("assembly/current")),
        ]);
    }
    out
}

/// The gateway's configuration: the environment (under [`env_key`] names) first, then `filled`.
pub fn config(
    env: impl Fn(&str) -> Option<String>,
    filled: &BTreeMap<String, String>,
) -> Result<Config, StartError> {
    Config::from_vars(|key| env(&env_key(key)).or_else(|| filled.get(key).cloned())).map_err(err)
}

/// The largest archive one checkout downloads, and the ceilings its unpacking keeps: the
/// Portal's sync-source limits (`crates/portal/src/sync/remote.rs`).
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
/// The largest single file inside an archive.
const MAX_ENTRY: u64 = 16 * 1024 * 1024;
/// How many entries an archive may hold.
const MAX_ENTRIES: usize = 10_000;
/// The bytes one unpacked tree may occupy.
const MAX_TREE: u64 = 4 * MAX_ARCHIVE;
/// The largest branch answer read.
const MAX_ANSWER: u64 = 64 * 1024;

/// Repositories of shako read over its HTTP API, never with a `git` binary (the image has none):
/// the commit of a ref from `branches/{ref}` (or the ref itself when it is a commit id), then
/// `archive/{commit}.zip` once per commit, unpacked as `{cache}/{slug}.tree-{commit}`. Requests
/// go to the forge's own address only, with its credential as a Basic header, and no redirect is
/// followed. An entry naming an outside `url` (CC-89) is refused: jc reads its own forge only.
pub struct ForgeArchives {
    http: reqwest::blocking::Client,
    /// The forge's base, `{forge}/api/v1/repos/...` its API.
    forge: String,
    owner: String,
    cache: PathBuf,
    authorization: HeaderValue,
}

/// A name of an owner or repository that may go into a URL path: `[A-Za-z0-9._-]`, never
/// starting with a dot.
fn path_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A branch name that goes into the URL path unescaped: git's rules for a ref name over the
/// characters `[A-Za-z0-9._/-]`.
fn branch_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("/.")
}

/// A full commit id (SHA-1 or SHA-256), lower case.
fn commit_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl ForgeArchives {
    /// Reads the repositories of `owner` on the forge at `forge` (`http` or `https`) with `token`,
    /// keeping the trees under `cache`.
    pub fn new(
        forge: &str,
        owner: &str,
        token: &jc_secret::Secret,
        cache: PathBuf,
    ) -> Result<Self, String> {
        use base64::Engine as _;
        if !(forge.starts_with("http://") || forge.starts_with("https://")) {
            return Err("the forge's address is not http or https".to_owned());
        }
        if !path_name(owner) {
            return Err(format!("`{owner}` is not an owner name"));
        }
        let pair = format!("jc:{}", token.expose());
        let mut authorization = HeaderValue::from_str(&format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(pair)
        ))
        .map_err(|_| "the forge's token is not a header value".to_owned())?;
        authorization.set_sensitive(true);
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("the forge's client: {e}"))?;
        Ok(Self {
            http,
            forge: forge.trim_end_matches('/').to_owned(),
            owner: owner.to_owned(),
            cache,
            authorization,
        })
    }

    /// The body of `GET {forge}/api/v1/repos/{owner}/{path}`, at most `ceiling` bytes, or `None`
    /// for a 404. A redirect is an error: the credential goes nowhere else.
    fn get(&self, path: &str, ceiling: u64) -> Result<Option<Vec<u8>>, String> {
        use std::io::Read as _;
        let shown = format!("{}/{path}", self.owner);
        let answer = self
            .http
            .get(format!("{}/api/v1/repos/{shown}", self.forge))
            .header(
                axum::http::header::AUTHORIZATION,
                self.authorization.clone(),
            )
            .send()
            .map_err(|e| format!("{shown}: the forge does not answer: {}", e.without_url()))?;
        let status = answer.status();
        if status.is_redirection() {
            return Err(format!(
                "{shown}: the forge answered a redirect ({status}), which jc never follows"
            ));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!("{shown}: the forge answered {status}"));
        }
        let mut body = Vec::new();
        answer
            .take(ceiling + 1)
            .read_to_end(&mut body)
            .map_err(|e| format!("{shown}: {}", e))?;
        if body.len() as u64 > ceiling {
            return Err(format!(
                "{shown} is larger than the {ceiling} bytes jc reads"
            ));
        }
        Ok(Some(body))
    }

    /// The commit `git_ref` of `repo` names: itself when it is a commit id, else a branch's head,
    /// else (no such branch) the commit a tag names, read from the commit list at `git_ref`.
    fn commit(&self, repo: &str, git_ref: &str) -> Result<String, String> {
        if commit_id(git_ref) {
            return Ok(git_ref.to_owned());
        }
        if !branch_name(git_ref) {
            return Err(format!("`{git_ref}` is not a branch or tag name jc reads"));
        }
        #[derive(serde::Deserialize)]
        struct Branch {
            commit: Head,
        }
        #[derive(serde::Deserialize)]
        struct Head {
            id: String,
        }
        #[derive(serde::Deserialize)]
        struct Commit {
            sha: String,
        }
        let unread = |e: serde_json::Error| format!("{repo}: `{git_ref}` does not read: {e}");
        let id = match self.get(&format!("{repo}/branches/{git_ref}"), MAX_ANSWER)? {
            Some(body) => {
                serde_json::from_slice::<Branch>(&body)
                    .map_err(unread)?
                    .commit
                    .id
            }
            None => {
                let body = self
                    .get(&format!("{repo}/commits?sha={git_ref}&limit=1"), MAX_ANSWER)?
                    .ok_or_else(|| format!("{repo}: `{git_ref}` names no branch or tag"))?;
                let list: Vec<Commit> = serde_json::from_slice(&body).map_err(unread)?;
                list.into_iter()
                    .next()
                    .map(|c| c.sha)
                    .ok_or_else(|| format!("{repo}: `{git_ref}` names no commit"))?
            }
        };
        if !commit_id(&id) {
            return Err(format!("{repo}: `{git_ref}` names no commit id"));
        }
        Ok(id)
    }

    /// Removes the trees of `slug` other than `keep`, once the link points at `keep`.
    pub fn prune(&self, slug: &str, keep: &Path) {
        let prefix = format!("{slug}.tree-");
        for item in std::fs::read_dir(&self.cache)
            .into_iter()
            .flatten()
            .flatten()
        {
            if item.file_name().to_string_lossy().starts_with(&prefix) && item.path() != keep {
                let _ = std::fs::remove_dir_all(item.path());
            }
        }
    }
}

impl jc_manifest::assemble::Resolver for ForgeArchives {
    fn checkout(&self, slug: &str, entry: &jc_core::kinds::ProjectSpec) -> Result<PathBuf, String> {
        let repository = entry
            .repository
            .as_ref()
            .ok_or("a registry entry names a repository")?;
        let repo = match (&repository.name, &repository.url) {
            (Some(name), None) => name,
            // Decided (jc-image-forge, owner may override): an outside repository is not read.
            (None, Some(_)) => {
                return Err(format!(
                    "the project `{slug}` names a repository outside jc's forge (`url`), and jc \
                     reads its own forge's repositories only"
                ));
            }
            _ => return Err("a repository names either a forge repository or a url".into()),
        };
        if !path_name(repo) || !path_name(slug) {
            return Err(format!("`{slug}`: `{repo}` is not a repository name"));
        }
        let git_ref = entry
            .git_ref
            .as_deref()
            .ok_or("a registry entry pins a ref")?;
        let commit = self.commit(repo, git_ref)?;
        let tree = self.cache.join(format!("{slug}.tree-{commit}"));
        if tree.is_dir() {
            return Ok(tree);
        }
        let archive = self
            .get(&format!("{repo}/archive/{commit}.zip"), MAX_ARCHIVE)?
            .ok_or_else(|| format!("{}/{repo}: no archive of {commit}", self.owner))?;
        std::fs::create_dir_all(&self.cache).map_err(|e| e.to_string())?;
        let partial = self.cache.join(format!(".{slug}.tree-{commit}.partial"));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir(&partial).map_err(|e| e.to_string())?;
        if let Err(why) = unpack(&archive, &partial) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(format!("{}/{repo} at {commit}: {why}", self.owner));
        }
        std::fs::rename(&partial, &tree).map_err(|e| e.to_string())?;
        Ok(tree)
    }
}

/// Unpacks a forge archive (`{repo}-{ref}/` around every file) into the empty directory `into`:
/// no entry outside it, no symlink created or followed (each file is created new), at most
/// [`MAX_ENTRIES`] entries of [`MAX_ENTRY`] bytes and [`MAX_TREE`] in all.
fn unpack(archive: &[u8], into: &Path) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    use std::path::Component;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| format!("the archive did not open: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err(format!(
            "the archive holds {} entries, past the {MAX_ENTRIES} entry limit",
            zip.len()
        ));
    }
    let mut budget = MAX_TREE;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("archive entry {index}: {e}"))?;
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        if entry.is_symlink() {
            return Err(format!("the archive carries the symlink `{name}`"));
        }
        if entry.is_dir() {
            continue;
        }
        let absolute = matches!(name.as_bytes().first(), Some(b'/' | b'\\'))
            || name.as_bytes().get(1) == Some(&b':');
        let Some(path) = entry.enclosed_name().filter(|_| !absolute) else {
            return Err(format!(
                "the archive carries `{name}`, outside the checkout"
            ));
        };
        // Drop the directory the forge wraps the tree in.
        let relative: PathBuf = path.components().skip(1).collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "the archive carries `{name}`, outside the checkout"
            ));
        }
        let ceiling = MAX_ENTRY.min(budget);
        if entry.size() > ceiling {
            return Err(format!("`{name}` is past the archive's size limits"));
        }
        let mut body = Vec::new();
        entry
            .by_ref()
            .take(ceiling + 1)
            .read_to_end(&mut body)
            .map_err(|e| format!("`{name}`: {e}"))?;
        if body.len() as u64 > ceiling {
            return Err(format!("`{name}` is past the archive's size limits"));
        }
        budget -= body.len() as u64;
        let target = into.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("`{name}`: {e}"))?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .and_then(|mut file| file.write_all(&body))
            .map_err(|e| format!("`{name}`: {e}"))?;
    }
    Ok(())
}

/// The organization repository `{owner}/{repo}` of the forge checked out under `work`: the tree
/// of `main` behind the link `repo/current`, swapped in one rename, then every registered
/// project of a layout 2 organization under `projects/` (`jcctl checkouts`), all read over the
/// forge's HTTP API ([`ForgeArchives`]). Blocking.
pub fn sync_forge(
    forge: &str,
    repo: &setup::ForgeRepo,
    token: &jc_secret::Secret,
    work: &Path,
) -> Result<PathBuf, String> {
    use jc_manifest::assemble::Resolver;
    let resolver = ForgeArchives::new(forge, &repo.owner, token, work.join("org-cache"))?;
    let entry: jc_core::kinds::ProjectSpec = serde_json::from_value(serde_json::json!({
        "organizationRef": repo.owner,
        "repository": { "name": repo.repo },
        "ref": "main",
    }))
    .map_err(|e| e.to_string())?;
    let tree = resolver.checkout("org", &entry)?;
    let current = work.join("repo");
    std::fs::create_dir_all(&current).map_err(|e| e.to_string())?;
    let next = current.join(".current.next");
    let _ = std::fs::remove_file(&next);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&tree, &next).map_err(|e| e.to_string())?;
    std::fs::rename(&next, current.join("current")).map_err(|e| e.to_string())?;
    resolver.prune("org", &tree);
    let layout = std::fs::read_to_string(tree.join(".jc/layout")).unwrap_or_default();
    if layout.trim() == "2" {
        let projects = work.join("projects");
        let options = jcctl::commands::checkouts::Options {
            forge: forge.to_owned(),
            token_file: None,
            token: None,
            secrets_dir: None,
            only: None,
        };
        let archives = ForgeArchives::new(forge, &repo.owner, token, projects.join(".cache"))?;
        let failed: BTreeSet<String> =
            jcctl::commands::checkouts::sync_with(&tree, &projects, &options, &archives)?
                .into_iter()
                .filter_map(|linked| Some(format!("{}: {}", linked.slug, linked.result.err()?)))
                .collect();
        for failure in newly_failed(failed) {
            tracing::warn!(%failure, "gateway: a project checkout failed");
        }
    }
    Ok(tree)
}

/// The project checkouts the last pass found failing: a failure is logged when it first appears,
/// not on every pass ([`SYNC_EVERY`]), which repeated each one per pass and hid other lines.
static CHECKOUTS_FAILING: std::sync::Mutex<BTreeSet<String>> =
    std::sync::Mutex::new(BTreeSet::new());

/// Of this pass's `failed` checkouts, the ones the last pass did not have.
fn newly_failed(failed: BTreeSet<String>) -> Vec<String> {
    let mut last = CHECKOUTS_FAILING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let new = failed.difference(&last).cloned().collect();
    *last = failed;
    new
}

/// Fetches the forge every [`SYNC_EVERY`]; a failed pass keeps the last tree and says why.
async fn follow_forge(
    forge: String,
    repo: setup::ForgeRepo,
    token: jc_secret::Secret,
    work: PathBuf,
) {
    loop {
        tokio::time::sleep(SYNC_EVERY).await;
        let (forge, repo, token, work) = (forge.clone(), repo.clone(), token.clone(), work.clone());
        match tokio::task::spawn_blocking(move || sync_forge(&forge, &repo, &token, &work)).await {
            Ok(Ok(_)) => {}
            Ok(Err(why)) => tracing::warn!(%why, "gateway: the forge checkout failed"),
            Err(e) => tracing::warn!(error = %e, "gateway: the forge checkout stopped"),
        }
    }
}

/// Puts the mount's prefix back in front of the path `jc-router` stripped: the gateway routes on
/// the whole path (`/api/endpoint/{slug}/...`) at the paths of `Module::root_paths`.
async fn whole_path(mut req: Request, next: Next) -> Response {
    let prefix = req
        .extensions()
        .get::<Mount>()
        .map(|m| m.path_prefix.clone())
        .unwrap_or_default();
    if !prefix.is_empty() {
        let path = match req.uri().path() {
            "/" => "",
            path => path,
        };
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        if let Ok(uri) = Uri::try_from(format!("{prefix}{path}{query}")) {
            *req.uri_mut() = uri;
        }
    }
    next.run(req).await
}

/// Endpoint reads from a sandboxed page (`Origin: null`): jc's viz module serves the catalogue's
/// explorer in an opaque origin when it shares its host with the Portal (`modules::viz`), so its
/// reads of the gateway on that same host are cross-origin. Such a read goes on anonymously (no
/// cookie, no token: the page holds neither) and its answer carries CORS for `null` alone, no
/// credentials, with the headers dev's edge exposes. Any other origin, method or path is untouched.
async fn opaque_reads(mut req: Request, next: Next) -> Response {
    use axum::http::{Method, StatusCode, header};
    let opaque = req
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|o| o == "null")
        && req.uri().path().starts_with("/api/endpoint/");
    if !opaque {
        return next.run(req).await;
    }
    let read = |m: &Method| m == Method::GET || m == Method::HEAD;
    let preflight = req.method() == Method::OPTIONS
        && req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|m| Method::from_bytes(m.as_bytes()).ok())
            .is_some_and(|m| read(&m));
    if !preflight && !read(req.method()) {
        return next.run(req).await;
    }
    let mut res = if preflight {
        let mut res = Response::new(axum::body::Body::empty());
        *res.status_mut() = StatusCode::NO_CONTENT;
        let h = res.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Accept, Content-Type"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        res
    } else {
        req.headers_mut().remove(header::COOKIE);
        req.headers_mut().remove(header::AUTHORIZATION);
        let mut res = next.run(req).await;
        res.headers_mut().insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("NGSILD-Results-Count, Content-Range, Link"),
        );
        res
    };
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("null"),
    );
    h.remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    res
}

/// The router for the root paths of `Module::root_paths`: `router` behind [`whole_path`]. The
/// rewrite runs ahead of `router`'s own routing: axum matches a router's routes before its
/// layers run, so `router` sits in an outer router's fallback.
pub fn at_root(router: Router) -> Router {
    Router::new()
        .fallback_service(router)
        .layer(axum::middleware::from_fn(whole_path))
}

/// `router` with the Organization of each request's host in its extensions
/// (`context_gateway::app::HostOrganization`), from the host `jc-router` resolved (its
/// `Mount`), for the `[[organizations]]` API, MCP and Portal hosts: there the gateway lists that
/// Organization's Endpoints alone.
///
/// Beside it goes the host's link base (`context_gateway::app::HostBase`): an API or MCP host
/// links itself, the Portal host its Organization's API host, so an answer on one
/// Organization's host links no address of another's or of the deployment's.
pub fn by_organization(
    router: Router,
    orgs: &[crate::config::OrganizationSection],
    scheme: &str,
) -> Router {
    let hosts: std::sync::Arc<BTreeMap<String, (String, String)>> = std::sync::Arc::new(
        orgs.iter()
            .flat_map(|org| {
                let base = |h: &str| format!("{scheme}://{h}");
                [
                    (&org.hosts.api, base(&org.hosts.api)),
                    (&org.hosts.mcp, base(&org.hosts.mcp)),
                    (&org.hosts.portal, base(&org.hosts.api)),
                ]
                .map(|(h, link)| (bare(h).to_owned(), (org.name.clone(), link)))
            })
            .collect(),
    );
    router.layer(axum::middleware::from_fn(
        move |mut req: Request, next: Next| {
            let hosts = hosts.clone();
            async move {
                // Only jc places it: whatever a request carried is dropped first.
                req.extensions_mut()
                    .remove::<context_gateway::app::HostOrganization>();
                req.extensions_mut()
                    .remove::<context_gateway::app::HostBase>();
                let org = req
                    .extensions()
                    .get::<Mount>()
                    .and_then(|m| hosts.get(bare(&m.host)).cloned());
                if let Some((org, link)) = org {
                    req.extensions_mut()
                        .insert(context_gateway::app::HostOrganization(org));
                    req.extensions_mut()
                        .insert(context_gateway::app::HostBase(link));
                }
                next.run(req).await
            }
        },
    ))
}

fn bare(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

pub async fn build(
    ctx: BuildCtx<'_>,
    wire: bool,
    mount: Mount,
    hop: Option<Hop>,
    portal: Option<std::net::SocketAddr>,
) -> Result<Router, StartError> {
    let env = crate::config::portal_env;
    let filled = vars(
        ctx.siblings,
        ctx.setup,
        wire,
        &mount,
        hop.as_ref(),
        portal,
        env,
    );
    tracing::info!(
        keys = ?filled.keys().collect::<Vec<_>>(),
        "gateway: settings filled from sibling modules"
    );
    if wire
        && ctx.siblings.get(Module::Kaisatsu).is_some()
        && !filled.contains_key("JC_OIDC_ISSUER")
        && env(&env_key("JC_OIDC_ISSUER")).is_none()
    {
        tracing::warn!(
            "gateway: no realm, so every token is refused: the Portal's internal listener \
             (revocations) has no known address"
        );
    }
    let config = config(env, &filled)?;
    if filled.contains_key("JC_GATEWAY_REPO_DIR") {
        let forge = ctx
            .siblings
            .get(Module::Shako)
            .map(|s| s.internal.clone())
            .ok_or_else(|| err("no shako"))?;
        // The shako token is held in memory (per boot, or the operator's file read at start).
        let token = ctx
            .setup
            .get(Secret::ShakoToken)
            .ok_or_else(|| err("no shako token"))?
            .value
            .clone();
        // The repository the Portal writes, read the same way (JC_SHAKO_OWNER, JC_SHAKO_REPO).
        let repo = setup::forge_repo(env).map_err(err)?;
        let work = work_dir(ctx.setup);
        crate::setup::private_dir(&work)?;
        let (f, r, t, w) = (forge.clone(), repo.clone(), token.clone(), work.clone());
        let tree = tokio::task::spawn_blocking(move || sync_forge(&f, &r, &t, &w))
            .await
            .map_err(err)?
            .map_err(|why| err(format!("the forge checkout: {why}")))?;
        tracing::info!(tree = %tree.display(), "gateway: the forge checkout read");
        tokio::spawn(follow_forge(forge, repo, token, work));
    }
    let mut broker = context_gateway::serve::broker_of(&config).map_err(err)?;
    if let (Some(hop), true) = (&hop, filled.contains_key("JC_GATEWAY_BROKER_URL")) {
        let value = HeaderValue::from_str(&hop.secret).map_err(err)?;
        broker = broker.with_hop_header(
            HeaderName::from_static(crate::modules::GATEWAY_HOP_HEADER),
            value,
        );
    }
    let built = context_gateway::serve::build(&config, broker)
        .await
        .map_err(err)?;
    Ok(built.app.layer(axum::middleware::from_fn(opaque_reads)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_checkout_is_logged_when_it_first_fails() {
        let set = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
        assert_eq!(
            newly_failed(set(&["a: gone", "b: gone"])),
            ["a: gone", "b: gone"]
        );
        assert!(newly_failed(set(&["b: gone", "a: gone"])).is_empty());
        assert!(newly_failed(set(&["b: gone"])).is_empty());
        assert_eq!(newly_failed(set(&["a: gone", "b: gone"])), ["a: gone"]);
        assert!(newly_failed(BTreeSet::new()).is_empty());
    }
    use crate::config::{Cli, Config as JcConfig, FileConfig};

    /// A router with routes of its own (the assistant's `/d/...`) answers at a root path: the
    /// prefix `jc-router` strips is back before it routes.
    #[tokio::test]
    async fn a_router_with_its_own_routes_answers_at_its_root_path() {
        use tower::ServiceExt;
        let inner = Router::new().route("/d/{id}/widget", axum::routing::get(|| async { "w" }));
        let spec = jc_router::MountSpec {
            module: "assistant".into(),
            host: None,
            path_prefix: Some("/d".into()),
        };
        let app = jc_router::JcRouterBuilder::new("https", "city.example")
            .mount(spec, at_root(inner))
            .build()
            .unwrap()
            .into_axum_router();
        let req = axum::http::Request::get("/d/data/widget")
            .header("host", "city.example")
            .body(axum::body::Body::empty())
            .unwrap();
        let answer = app.oneshot(req).await.unwrap();
        assert_eq!(answer.status(), axum::http::StatusCode::OK);
    }

    /// A read from a sandboxed page reaches the gateway anonymously and gets CORS for `null`; a
    /// read from any other origin, a write, and a path outside the Endpoints get none.
    #[tokio::test]
    async fn a_sandboxed_page_reads_endpoints_anonymously_and_nothing_else_gets_cors() {
        use axum::http::{Request as HttpRequest, StatusCode, header};
        use tower::ServiceExt;
        // The inner router answers what credentials reached it.
        let inner = Router::new().fallback(|req: Request| async move {
            let seen = |name: header::HeaderName| req.headers().contains_key(name);
            format!(
                "cookie={} auth={}",
                seen(header::COOKIE),
                seen(header::AUTHORIZATION)
            )
        });
        let app = inner.layer(axum::middleware::from_fn(opaque_reads));
        let send = |method: &str, path: &str, origin: &str| {
            let mut req = HttpRequest::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, "jc_session=s")
                .header(header::AUTHORIZATION, "Bearer t")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET");
            if !origin.is_empty() {
                req = req.header(header::ORIGIN, origin);
            }
            app.clone()
                .oneshot(req.body(axum::body::Body::empty()).unwrap())
        };
        let text = |res: Response| async move {
            String::from_utf8_lossy(
                &axum::body::to_bytes(res.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .into_owned()
        };
        let read = "/api/endpoint/abc/ngsi-ld/v1/entities?type=GtfsStop";
        let res = send("GET", read, "null").await.unwrap();
        let h = res.headers().clone();
        assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], "null");
        assert!(h.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
        assert!(
            h[header::ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .contains("NGSILD-Results-Count")
        );
        assert_eq!(h[header::VARY], "Origin");
        assert_eq!(text(res).await, "cookie=false auth=false");

        let pre = send("OPTIONS", read, "null").await.unwrap();
        assert_eq!(pre.status(), StatusCode::NO_CONTENT);
        assert_eq!(pre.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "null");
        assert_eq!(
            pre.headers()[header::ACCESS_CONTROL_ALLOW_METHODS],
            "GET, HEAD"
        );

        for (method, path, origin) in [
            ("GET", read, "https://evil.test"),
            ("GET", read, ""),
            ("POST", "/api/endpoint/abc/ngsi-ld/v1/entities", "null"),
            ("DELETE", "/api/endpoint/abc/ngsi-ld/v1/entities/x", "null"),
            ("GET", "/api/mcp", "null"),
            ("GET", "/portal/api/v1/me", "null"),
        ] {
            let res = send(method, path, origin).await.unwrap();
            assert!(
                res.headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none(),
                "{method} {path} {origin}"
            );
            assert_eq!(text(res).await, "cookie=true auth=true", "{method} {path}");
        }
    }

    /// The siblings and secrets of `--role all` in slug mode, and the gateway's mount.
    fn fixture() -> (Siblings, Setup, Mount, tempfile::TempDir) {
        let cfg = JcConfig::merge(
            Cli {
                role: Some("all".into()),
                base_host: Some("city.example".into()),
                scheme: Some("https".into()),
                ..Cli::default()
            },
            FileConfig::default(),
        )
        .unwrap();
        let plans = crate::app::plan_mounts(&cfg).unwrap();
        let mount = plans
            .iter()
            .find(|p| p.module == Module::Gateway)
            .map(|p| p.mount.clone())
            .unwrap();
        let siblings = Siblings::new(&plans, "0.0.0.0:8443".parse().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let setup =
            Setup::prepare(dir.path(), &Module::ALL, &std::collections::BTreeMap::new()).unwrap();
        (siblings, setup, mount, dir)
    }

    #[test]
    fn the_siblings_fill_the_realm_the_forge_and_the_hop_and_an_explicit_key_wins() {
        let (siblings, setup, mount, _d) = fixture();
        let hop = Hop {
            addr: "127.0.0.1:4242".parse().unwrap(),
            secret: "s".into(),
        };
        let bind = |k: &str| (k == "JC_INTERNAL_BIND").then(|| "0.0.0.0:9090".to_owned());
        let v = vars(&siblings, &setup, true, &mount, Some(&hop), None, bind);
        assert_eq!(v["JC_GATEWAY_BROKER_URL"], "http://127.0.0.1:4242");
        assert_eq!(v["JC_GATEWAY_PUBLIC_URL"], "https://city.example");
        assert_eq!(
            v["JC_GATEWAY_OAUTH_KEY"],
            setup.value(Secret::GatewayOauthKey).unwrap()
        );
        assert_eq!(v["JC_OIDC_ISSUER"], "https://city.example/id/realms/jc");
        assert_eq!(
            v["JC_OIDC_JWKS_URL"],
            "http://127.0.0.1:8443/id/realms/jc/protocol/openid-connect/certs"
        );
        assert_eq!(v["JC_OIDC_CLIENT_ID"], setup::GATEWAY_CLIENT);
        assert_eq!(
            v["JC_OIDC_CLIENT_SECRET"],
            setup.value(Secret::GatewayClientSecret).unwrap()
        );
        // A deployment naming its realm moves the issuer and the keys with it.
        let mut dev = setup.clone();
        dev.realm = setup::Realm::parse("dev").unwrap();
        let named = vars(&siblings, &dev, true, &mount, Some(&hop), None, bind);
        assert_eq!(
            named["JC_OIDC_ISSUER"],
            "https://city.example/id/realms/dev"
        );
        assert_eq!(
            named["JC_OIDC_JWKS_URL"],
            "http://127.0.0.1:8443/id/realms/dev/protocol/openid-connect/certs"
        );
        assert_eq!(
            v["JC_GATEWAY_REVOCATIONS_URL"],
            "http://127.0.0.1:9090/internal/revocations"
        );
        assert!(v["JC_GATEWAY_REPO_DIR"].ends_with("gateway/repo/current"));
        // The address the Portal's listener took (its default is a free port) wins over the
        // environment; without either there is no address and so no realm.
        let none = |_: &str| None;
        let taken = Some("127.0.0.1:40123".parse().unwrap());
        let v = vars(&siblings, &setup, true, &mount, Some(&hop), taken, none);
        assert_eq!(
            v["JC_GATEWAY_REVOCATIONS_URL"],
            "http://127.0.0.1:40123/internal/revocations"
        );
        let v = vars(&siblings, &setup, true, &mount, Some(&hop), None, none);
        assert!(!v.contains_key("JC_OIDC_ISSUER") && !v.contains_key("JC_GATEWAY_REVOCATIONS_URL"));
        let port0 = |k: &str| (k == "JC_INTERNAL_BIND").then(|| "127.0.0.1:0".to_owned());
        assert!(
            !vars(&siblings, &setup, true, &mount, Some(&hop), None, port0)
                .contains_key("JC_OIDC_ISSUER")
        );
        // Without the wiring: the broker and the addresses only, no realm and no forge.
        let bare = vars(&siblings, &setup, false, &mount, Some(&hop), None, bind);
        assert_eq!(
            bare.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "JC_GATEWAY_BROKER_URL",
                "JC_GATEWAY_ORG_DOMAIN",
                "JC_GATEWAY_PREVIEWS_DIR",
                "JC_GATEWAY_PUBLIC_URL"
            ]
        );
        // An issuer the operator set keeps the whole realm row, the generated secret included,
        // out of the fill; the hop without its secret is not offered.
        let env =
            |k: &str| (k == "JC_GATEWAY_OIDC_ISSUER").then(|| "https://idp.example/r".to_owned());
        let env = |k: &str| bind(k).or_else(|| env(k));
        let v = vars(&siblings, &setup, true, &mount, None, None, env);
        assert!(!v.contains_key("JC_OIDC_CLIENT_SECRET"));
        assert!(!v.contains_key("JC_GATEWAY_BROKER_URL"));
    }

    #[test]
    fn the_oidc_keys_are_read_under_the_gateways_own_names() {
        assert_eq!(env_key("JC_OIDC_ISSUER"), "JC_GATEWAY_OIDC_ISSUER");
        assert_eq!(env_key("JC_GATEWAY_BROKER_URL"), "JC_GATEWAY_BROKER_URL");
        let filled = BTreeMap::from([
            (
                "JC_GATEWAY_BROKER_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
            ("JC_GATEWAY_ORG_DOMAIN".to_owned(), "jc.invalid".to_owned()),
        ]);
        // The Portal's JC_OIDC_ISSUER in the environment is not the gateway's.
        let env = |k: &str| (k == "JC_OIDC_ISSUER").then(|| "https://other/realms/x".to_owned());
        let cfg = config(env, &filled).unwrap();
        assert_eq!(cfg.oidc_issuer, None);
        // An explicit gateway key wins over the sibling.
        let env =
            |k: &str| (k == "JC_GATEWAY_BROKER_URL").then(|| "https://broker.example".to_owned());
        assert_eq!(
            config(env, &filled).unwrap().broker_url,
            "https://broker.example"
        );
    }

    /// A zip of `files` under the forge's `{repo}-{ref}/` root.
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    /// `router` served on a loopback port from its own thread; its base URL.
    fn serve(router: Router) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, router).await.unwrap();
                });
        });
        format!("http://{addr}")
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn entry(spec: serde_json::Value) -> jc_core::kinds::ProjectSpec {
        let mut base = serde_json::json!({"organizationRef": "city", "ref": "main"});
        base.as_object_mut()
            .unwrap()
            .extend(spec.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn archives(forge: &str, cache: &Path) -> ForgeArchives {
        let token = jc_secret::Secret::new("forge-token-not-secret".to_owned());
        ForgeArchives::new(forge, "city", &token, cache.to_path_buf()).unwrap()
    }

    /// A forge holding `main` at [`SHA`] with `archive` as its zip; counts the archive reads and
    /// refuses a request without the jc credential.
    fn forge(archive: Vec<u8>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use axum::http::{HeaderMap, StatusCode};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let want = format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                "jc:forge-token-not-secret"
            )
        );
        let authorized = move |headers: &HeaderMap| {
            headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(want.as_str())
        };
        let check = authorized.clone();
        let router = Router::new()
            .route(
                "/api/v1/repos/city/conf/branches/main",
                axum::routing::get(move |headers: HeaderMap| async move {
                    if !check(&headers) {
                        return (StatusCode::UNAUTHORIZED, String::new());
                    }
                    let body = serde_json::json!({"name": "main", "commit": {"id": SHA}});
                    (StatusCode::OK, body.to_string())
                }),
            )
            .route(
                &format!("/api/v1/repos/city/conf/archive/{SHA}.zip"),
                axum::routing::get(move |headers: HeaderMap| async move {
                    if !authorized(&headers) {
                        return (StatusCode::UNAUTHORIZED, Vec::new());
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, archive)
                }),
            );
        (serve(router), reads)
    }

    #[test]
    fn a_checkout_reads_the_branch_and_its_archive_once_per_commit() {
        use jc_manifest::assemble::Resolver as _;
        let zip = zip_of(&[
            ("conf-main/org.yaml", b"kind: Organization\n"),
            ("conf-main/projects/a.yaml", b"a\n"),
        ]);
        let (base, reads) = forge(zip);
        let cache = tempfile::tempdir().unwrap();
        let resolver = archives(&base, cache.path());
        let spec = entry(serde_json::json!({"repository": {"name": "conf"}}));
        let tree = resolver.checkout("org", &spec).unwrap();
        assert_eq!(tree, cache.path().join(format!("org.tree-{SHA}")));
        assert_eq!(
            std::fs::read_to_string(tree.join("org.yaml")).unwrap(),
            "kind: Organization\n"
        );
        assert!(tree.join("projects/a.yaml").is_file());
        // The same commit again: no second download.
        assert_eq!(resolver.checkout("org", &spec).unwrap(), tree);
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A wrong credential is the forge's 401, named without the token.
        let token = jc_secret::Secret::new("wrong".to_owned());
        let other = ForgeArchives::new(&base, "city", &token, cache.path().join("x")).unwrap();
        let why = other.checkout("org", &spec).unwrap_err();
        assert!(why.contains("401") && !why.contains("wrong"), "{why}");
    }

    /// A layout 2 organization: its registry's forge project, pinned to a tag, is checked out
    /// under `projects/` through `jcctl checkouts`, and an entry naming an outside `url` is
    /// refused and linked nowhere.
    #[test]
    fn a_layout_2_organization_links_its_forge_projects_and_refuses_an_outside_one() {
        use axum::http::{StatusCode, Uri};
        const TAGGED: &str = "89abcdef0123456789abcdef0123456789abcdef";
        let entry = |name: &str, repository: &str| {
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata: {{ name: {name}, \
                 namespace: org }}\nspec:\n  organizationRef: city\n  repository: {repository}\n  \
                 ref: v1.0.0\n"
            )
        };
        let (p, x) = (
            entry("p", "{ name: proj }"),
            entry("x", "{ url: \"https://elsewhere.example/city/x.git\" }"),
        );
        let org = zip_of(&[
            ("conf-main/.jc/layout", b"2\n"),
            ("conf-main/projects/p.yaml", p.as_bytes()),
            ("conf-main/projects/x.yaml", x.as_bytes()),
        ]);
        let project = zip_of(&[("proj-v1.0.0/project.yaml", b"kind: Project\n")]);
        let answers: BTreeMap<String, Vec<u8>> = BTreeMap::from([
            (
                "/api/v1/repos/city/conf/branches/main".to_owned(),
                format!(r#"{{"commit":{{"id":"{SHA}"}}}}"#).into_bytes(),
            ),
            (format!("/api/v1/repos/city/conf/archive/{SHA}.zip"), org),
            (
                "/api/v1/repos/city/proj/commits?sha=v1.0.0&limit=1".to_owned(),
                format!(r#"[{{"sha":"{TAGGED}"}}]"#).into_bytes(),
            ),
            (
                format!("/api/v1/repos/city/proj/archive/{TAGGED}.zip"),
                project,
            ),
        ]);
        let base = serve(Router::new().fallback(move |uri: Uri| async move {
            let key = uri.path_and_query().map(|p| p.as_str()).unwrap_or_default();
            match answers.get(key) {
                Some(body) => (StatusCode::OK, body.clone()),
                None => (StatusCode::NOT_FOUND, Vec::new()),
            }
        }));
        let work = tempfile::tempdir().unwrap();
        let repo = setup::ForgeRepo {
            owner: "city".into(),
            repo: "conf".into(),
        };
        let token = jc_secret::Secret::new("forge-token-not-secret".to_owned());
        let tree = sync_forge(&base, &repo, &token, work.path()).unwrap();
        assert!(work.path().join("repo/current/projects/p.yaml").is_file());
        assert_eq!(tree, work.path().join(format!("org-cache/org.tree-{SHA}")));
        let linked = work.path().join("projects/p");
        assert_eq!(
            std::fs::read_to_string(linked.join("project.yaml")).unwrap(),
            "kind: Project\n"
        );
        assert!(
            std::fs::read_link(&linked)
                .unwrap()
                .ends_with(format!("p.tree-{TAGGED}"))
        );
        assert!(!work.path().join("projects/x").exists());
    }

    #[test]
    fn a_ref_that_is_no_plain_branch_name_is_refused_before_any_request() {
        use jc_manifest::assemble::Resolver as _;
        let cache = tempfile::tempdir().unwrap();
        // Port 9 answers nothing: a refusal naming the ref was decided before a request.
        let resolver = archives("http://127.0.0.1:9", cache.path());
        for bad in [
            "../../admin",
            "main/../../x",
            "-main",
            "/main",
            "main?x=1",
            "main#x",
            "main%2f..",
            "a b",
            "a//b",
            ".hidden",
            "a/.b",
            "x.lock",
            "main/",
            "",
        ] {
            let spec = entry(serde_json::json!({"repository": {"name": "conf"}, "ref": bad}));
            let why = resolver.checkout("org", &spec).unwrap_err();
            assert!(
                why.contains("is not a branch or tag name"),
                "{bad:?}: {why}"
            );
        }
        for bad in ["../conf", "conf/x", ".git", "a b"] {
            let spec = entry(serde_json::json!({"repository": {"name": bad}}));
            let why = resolver.checkout("org", &spec).unwrap_err();
            assert!(why.contains("is not a repository name"), "{bad:?}: {why}");
        }
        assert!(branch_name("main") && branch_name("workspace/air-v2"));
        assert!(commit_id(SHA) && !commit_id(&SHA.to_uppercase()));
    }

    #[test]
    fn an_outside_repository_is_refused() {
        use jc_manifest::assemble::Resolver as _;
        let cache = tempfile::tempdir().unwrap();
        let resolver = archives("http://127.0.0.1:9", cache.path());
        let spec = entry(serde_json::json!({
            "repository": {"url": "https://elsewhere.example/city/conf.git"}
        }));
        let why = resolver.checkout("p", &spec).unwrap_err();
        assert!(why.contains("outside jc's forge"), "{why}");
        assert!(!why.contains("elsewhere.example"), "{why}");
    }

    #[test]
    fn a_redirect_is_refused_and_not_followed() {
        use axum::http::StatusCode;
        use jc_manifest::assemble::Resolver as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let elsewhere = serve(Router::new().fallback(move || async move {
            seen.fetch_add(1, Ordering::SeqCst);
            "{}"
        }));
        let target = format!("{elsewhere}/api/v1/repos/city/conf/branches/main");
        let base = serve(Router::new().fallback(move || async move {
            (StatusCode::FOUND, [(axum::http::header::LOCATION, target)])
        }));
        let cache = tempfile::tempdir().unwrap();
        let spec = entry(serde_json::json!({"repository": {"name": "conf"}}));
        let why = archives(&base, cache.path())
            .checkout("org", &spec)
            .unwrap_err();
        assert!(why.contains("redirect"), "{why}");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_over_long_branch_answer_is_refused() {
        use jc_manifest::assemble::Resolver as _;
        let huge = "x".repeat(MAX_ANSWER as usize + 1);
        let base = serve(Router::new().fallback(move || async move { huge }));
        let cache = tempfile::tempdir().unwrap();
        let spec = entry(serde_json::json!({"repository": {"name": "conf"}}));
        let why = archives(&base, cache.path())
            .checkout("org", &spec)
            .unwrap_err();
        assert!(why.contains("larger than"), "{why}");
    }

    #[test]
    fn an_archive_reaching_outside_holding_a_symlink_or_past_its_caps_is_refused() {
        use std::io::Write as _;
        let refused = |archive: Vec<u8>| {
            let dir = tempfile::tempdir().unwrap();
            let into = dir.path().join("tree");
            std::fs::create_dir(&into).unwrap();
            let why = unpack(&archive, &into).unwrap_err();
            // Nothing landed beside the tree.
            let beside: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
            assert_eq!(beside.len(), 1, "{why}");
            why
        };
        for name in ["conf-main/../../evil", "../evil", "/etc/evil", "C:/evil"] {
            let why = refused(zip_of(&[(name, b"x")]));
            assert!(why.contains("outside the checkout"), "{name}: {why}");
        }
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        zip.add_symlink("conf-main/link", "/etc/passwd", opts)
            .unwrap();
        zip.finish().unwrap();
        let why = refused(out.into_inner());
        assert!(why.contains("symlink"), "{why}");
        let big = vec![0u8; MAX_ENTRY as usize + 1];
        let why = refused(zip_of(&[("conf-main/big", &big)]));
        assert!(why.contains("size limits"), "{why}");
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for i in 0..=MAX_ENTRIES {
            zip.start_file(format!("conf-main/{i}"), opts).unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();
        let why = refused(out.into_inner());
        assert!(why.contains("entry limit"), "{why}");
    }
}
