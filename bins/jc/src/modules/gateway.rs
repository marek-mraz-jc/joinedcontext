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

/// The organization repository `{forge}/{owner}/{repo}.git` checked out under `work`: the tree of
/// `main` behind the link `repo/current`, swapped in one rename, then every registered project of
/// a layout 2 organization under `projects/` (`jcctl checkouts`). Blocking (git).
pub fn sync_forge(
    forge: &str,
    repo: &setup::ForgeRepo,
    token: &jc_secret::Secret,
    work: &Path,
) -> Result<PathBuf, String> {
    use jc_manifest::assemble::{GitCheckouts, Resolver};
    let owner = format!("{}/{}", forge.trim_end_matches('/'), repo.owner);
    let resolver = GitCheckouts {
        cache: work.join("org-cache"),
        forge: owner.clone(),
        forge_credential: Some((
            "jc".to_owned(),
            jc_core::Secret::new(token.expose().to_owned()),
        )),
        secrets: &|_| Err("jc names no Secret for an outside repository".to_owned()),
    };
    let entry: jc_core::kinds::ProjectSpec = serde_json::from_value(serde_json::json!({
        "organizationRef": repo.owner,
        "repository": { "name": repo.repo },
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
            token_file: None,
            token: Some(jc_core::Secret::new(token.expose().to_owned())),
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

/// The router for the root paths of `Module::root_paths`: `router` behind [`whole_path`].
pub fn at_root(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(whole_path))
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
        tokio::task::spawn_blocking(move || sync_forge(&f, &r, &t, &w))
            .await
            .map_err(err)?
            .map_err(|why| err(format!("the forge checkout: {why}")))?;
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
    Ok(built.app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config as JcConfig, FileConfig};

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
        assert_eq!(named["JC_OIDC_ISSUER"], "https://city.example/id/realms/dev");
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
}
