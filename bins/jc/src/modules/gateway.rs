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
//! manifests from shako's organization repository, checked out under the state directory every
//! [`SYNC_EVERY`] by [`follow_forge`], as dev's git-sync and `jcctl checkouts` sidecars do.

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
use std::collections::BTreeMap;
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

/// The Portal's internal listener as the gateway reaches it.
fn portal_internal(env: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let bind = env("JC_INTERNAL_BIND").unwrap_or_else(|| "0.0.0.0:9090".to_owned());
    let addr: std::net::SocketAddr = bind.trim().parse().ok()?;
    Some(format!("http://{}", reach(addr)))
}

/// Where `jc` keeps the gateway's checkouts.
#[must_use]
pub fn work_dir(setup: &Setup) -> PathBuf {
    setup.dir.join("gateway")
}

/// The gateway settings `jc` fills from its siblings, for the keys `env` (given the names of
/// [`env_key`]) leaves unset. Like the Portal's rows, a group is filled only when its first key is
/// unset and every secret it needs is held, so a generated credential never goes to an address the
/// operator chose.
///
/// | settings | from |
/// |---|---|
/// | `JC_GATEWAY_BROKER_URL` | Antares' hop |
/// | `JC_GATEWAY_ORG_DOMAIN`, `JC_GATEWAY_PUBLIC_URL`, `JC_GATEWAY_PREVIEWS_DIR` | the mounts, the state dir |
/// | `JC_OIDC_ISSUER`, `_JWKS_URL`, `_TOKEN_URL`, `_CLIENT_ID`, `_CLIENT_SECRET`, `JC_GATEWAY_REVOCATIONS_URL`, `_CONNECTORS_URL`, `_USAGE_URL` | kaisatsu and the Portal |
/// | `JC_GATEWAY_REPO_DIR`, `_PROJECTS_DIR`, `_ASSEMBLY_DIR` | shako, checked out by [`follow_forge`] |
#[must_use]
pub fn vars(
    siblings: &Siblings,
    setup: &Setup,
    mount: &Mount,
    hop: Option<&Hop>,
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
    if let (Some(id), Some(_), Some(internal)) = (
        siblings.get(Module::Kaisatsu),
        siblings.get(Module::Portal),
        portal_internal(&|k: &str| env(k)),
    ) {
        let realm = format!("/realms/{}", setup::PORTAL_REALM);
        group(vec![
            ("JC_OIDC_ISSUER", Some(format!("{}{realm}", id.public))),
            (
                "JC_OIDC_JWKS_URL",
                Some(format!(
                    "{}{realm}/protocol/openid-connect/certs",
                    id.internal
                )),
            ),
            (
                "JC_OIDC_TOKEN_URL",
                Some(format!(
                    "{}{realm}/protocol/openid-connect/token",
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

/// The organization repository `{forge}/{owner}/{repo}.git` checked out under `work`: the tree of
/// `main` behind the link `repo/current`, swapped in one rename, then every registered project of
/// a layout 2 organization under `projects/` (`jcctl checkouts`). Blocking (git).
pub fn sync_forge(forge: &str, token_file: &Path, work: &Path) -> Result<PathBuf, String> {
    use jc_manifest::assemble::{GitCheckouts, Resolver};
    let token = crate::runtime::read_secret_file(token_file)?;
    let owner = format!("{}/{}", forge.trim_end_matches('/'), setup::FORGE_OWNER);
    let resolver = GitCheckouts {
        cache: work.join("org-cache"),
        forge: owner.clone(),
        forge_credential: Some(("jc".to_owned(), jc_core::Secret::new(token))),
        secrets: &|_| Err("jc names no Secret for an outside repository".to_owned()),
    };
    let entry: jc_core::kinds::ProjectSpec = serde_json::from_value(serde_json::json!({
        "organizationRef": setup::FORGE_OWNER,
        "repository": { "name": setup::FORGE_REPO },
        "ref": "main",
    }))
    .map_err(|e| e.to_string())?;
    let tree = resolver.checkout("org", &entry)?;
    let repo = work.join("repo");
    std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
    let next = repo.join(".current.next");
    let _ = std::fs::remove_file(&next);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&tree, &next).map_err(|e| e.to_string())?;
    std::fs::rename(&next, repo.join("current")).map_err(|e| e.to_string())?;
    resolver.prune("org", &tree);
    let layout = std::fs::read_to_string(tree.join(".jc/layout")).unwrap_or_default();
    if layout.trim() == "2" {
        let options = jcctl::commands::checkouts::Options {
            forge: owner,
            token_file: Some(token_file.to_path_buf()),
            secrets_dir: None,
            only: None,
        };
        for linked in jcctl::commands::checkouts::sync(&tree, &work.join("projects"), &options)? {
            if let Err(why) = linked.result {
                tracing::warn!(project = %linked.slug, %why, "gateway: a project checkout failed");
            }
        }
    }
    Ok(tree)
}

/// Fetches the forge every [`SYNC_EVERY`]; a failed pass keeps the last tree and says why.
async fn follow_forge(forge: String, token_file: PathBuf, work: PathBuf) {
    loop {
        tokio::time::sleep(SYNC_EVERY).await;
        let (forge, token_file, work) = (forge.clone(), token_file.clone(), work.clone());
        match tokio::task::spawn_blocking(move || sync_forge(&forge, &token_file, &work)).await {
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

/// The router for the root paths of `Module::root_paths`: `router` behind [`whole_path`].
pub fn at_root(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(whole_path))
}

pub async fn build(
    ctx: BuildCtx<'_>,
    mount: Mount,
    hop: Option<Hop>,
) -> Result<Router, StartError> {
    let env = |key: &str| std::env::var(key).ok();
    let filled = vars(ctx.siblings, ctx.setup, &mount, hop.as_ref(), env);
    tracing::info!(
        keys = ?filled.keys().collect::<Vec<_>>(),
        "gateway: settings filled from sibling modules"
    );
    let config = config(env, &filled)?;
    if filled.contains_key("JC_GATEWAY_REPO_DIR") {
        let forge = ctx
            .siblings
            .get(Module::Shako)
            .map(|s| s.internal.clone())
            .ok_or_else(|| err("no shako"))?;
        let token = ctx
            .setup
            .path(Secret::ShakoToken)
            .ok_or_else(|| err("no shako token"))?
            .to_path_buf();
        let work = work_dir(ctx.setup);
        crate::setup::private_dir(&work)?;
        let (f, t, w) = (forge.clone(), token.clone(), work.clone());
        tokio::task::spawn_blocking(move || sync_forge(&f, &t, &w))
            .await
            .map_err(err)?
            .map_err(|why| err(format!("the forge checkout: {why}")))?;
        tokio::spawn(follow_forge(forge, token, work));
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
    Ok(built.app)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
