//! First-start provisioning `jc` does through its siblings' public HTTP APIs once they serve:
//! the shako organization repository the Portal writes manifests to, holding the installation's
//! Organization (`org.yaml`), without which the Portal opens no project (PF-66), and the
//! `steward` Role a new project's opener is bound to (PF-52). Idempotent: an existing repository
//! and existing files are left as they are.

use crate::app::StartError;
use crate::module::Module;
use crate::setup::{FORGE_OWNER, ForgeRepo, Secret, Setup, forge_repo};
use crate::siblings::{Siblings, portal_vars};
use base64::Engine as _;
use reqwest::StatusCode;
use std::time::Duration;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Shako, e)
}

/// Creates the organization repository ([`forge_repo`]) in shako (Gitea `POST /api/v1/orgs/{owner}/repos`, with
/// `auto_init`) when `jc` points the Portal at shako. Errors name the call and the status, never
/// the token.
pub async fn forge(
    siblings: &Siblings,
    setup: &Setup,
    cfg: &crate::config::Config,
) -> Result<(), StartError> {
    let vars = portal_vars(siblings, setup, crate::config::portal_env);
    let (Some(base), Some(token)) = (vars.get("JC_SHAKO_URL"), setup.value(Secret::ShakoToken))
    else {
        return Ok(());
    };
    let ForgeRepo { owner, repo: name } = forge_repo(crate::config::portal_env).map_err(err)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(err)?;
    let auth = format!("token {token}");
    let repo = format!("{base}/api/v1/repos/{owner}/{name}");
    let status = client
        .get(&repo)
        .header("authorization", &auth)
        .send()
        .await
        .map_err(|e| err(format!("GET {repo}: {e}")))?
        .status();
    if status.is_success() {
        migrate_harvest_seeds(&client, &auth, &repo, cfg).await;
        return Ok(());
    }
    if status != StatusCode::NOT_FOUND {
        return Err(err(format!("GET {repo} answered {status}")));
    }
    let create = format!("{base}/api/v1/orgs/{owner}/repos");
    let status = client
        .post(&create)
        .header("authorization", &auth)
        .json(&serde_json::json!({
            "name": name,
            "auto_init": true,
            "default_branch": "main",
            "private": true,
        }))
        .send()
        .await
        .map_err(|e| err(format!("POST {create}: {e}")))?
        .status();
    if !(status.is_success() || status == StatusCode::CONFLICT) {
        return Err(err(format!("POST {create} answered {status}")));
    }
    tracing::info!(repo = %format!("{owner}/{name}"), "shako: the Portal's repository is ready");
    let org = organization_manifest(&domain(siblings.portal_base().as_deref()));
    seed(&client, &auth, &repo, "org.yaml", &org).await?;
    seed(&client, &auth, &repo, "users/roles/steward.yaml", STEWARD).await?;
    migrate_harvest_seeds(&client, &auth, &repo, cfg).await;
    Ok(())
}

/// Moves the `[[dataroom.harvest]]` seeds into their Organization's `default` Portal manifest,
/// `portals/{organization}-default.yaml`, written once when absent (ODP-11): from then on the
/// Portal's reconciler holds the sources, and the Portal shows and runs them. A seed of the
/// `[dataroom]` catalogue belongs to the installation's Organization (`org.yaml`), and only
/// without `[[organizations]]`, whose catalogues are their own.
///
/// A convenience, never a reason not to start: until the block is refused (T-6898) the seeds keep
/// harvesting the same rows (`hs-{name}`), so a migration that fails is logged and skipped. That is
/// also why a deleted manifest comes back on the next start: it shows what the seeds still run.
async fn migrate_harvest_seeds(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    cfg: &crate::config::Config,
) {
    migrate_seeds(
        client,
        auth,
        repo,
        &cfg.dataroom,
        !cfg.organizations.is_empty(),
    )
    .await;
}

#[cfg(all(feature = "portal", feature = "dataroom"))]
async fn migrate_seeds(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    dataroom: &crate::config::DataroomSection,
    has_organizations: bool,
) {
    let seeds = &dataroom.harvest;
    if seeds.is_empty() {
        return;
    }
    let mut installation = None;
    let mut by_organization: std::collections::BTreeMap<String, Vec<_>> = Default::default();
    for seed in seeds {
        let organization = match &seed.organization {
            Some(name) => name.clone(),
            None if has_organizations => {
                tracing::warn!(
                    source = %seed.name,
                    "[[dataroom.harvest]] of the [dataroom] catalogue beside [[organizations]]: \
                     no Portal holds that catalogue, so it stays a seed"
                );
                continue;
            }
            None => {
                if installation.is_none() {
                    installation = Some(installation_name(client, auth, repo).await);
                }
                match installation.as_ref() {
                    Some(Ok(name)) => name.clone(),
                    Some(Err(e)) => {
                        tracing::warn!(source = %seed.name, error = %e, "[[dataroom.harvest]] stays a seed: the installation Organization is unknown");
                        continue;
                    }
                    None => continue,
                }
            }
        };
        by_organization.entry(organization).or_default().push(seed);
    }
    for (organization, seeds) in by_organization {
        let name = format!("{organization}-default");
        match migrate_one(client, auth, repo, &organization, &name, &seeds, dataroom).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(
                portal = %name,
                error = %e,
                "[[dataroom.harvest]] not moved into its Portal manifest; the seeds keep harvesting"
            ),
        }
    }
}

#[cfg(all(feature = "portal", feature = "dataroom"))]
async fn migrate_one(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    organization: &str,
    name: &str,
    seeds: &[&crate::config::HarvestSeed],
    dataroom: &crate::config::DataroomSection,
) -> Result<(), StartError> {
    let spec = crate::modules::portals::default_portal_of(
        organization,
        seeds,
        dataroom.config_dir.as_deref(),
    )
    .map_err(err)?;
    let path = format!("portals/{name}.yaml");
    if let Some(held) = default_portal_filed(client, auth, repo, organization).await?
        && held != path
    {
        tracing::info!(portal = %name, held = %held, "the Organization's default Portal is filed already; the seeds are not moved");
        return Ok(());
    }
    let manifest = jc_core::envelope::ResourceEnvelope::new(
        jc_core::envelope::ObjectMeta::new(name, "org"),
        spec,
    )
    .to_yaml()
    .map_err(err)?;
    seed(client, auth, repo, &path, &manifest).await?;
    tracing::info!(portal = %name, sources = seeds.len(), "[[dataroom.harvest]] moved into its Portal manifest (ODP-11)");
    Ok(())
}

/// The path of a manifest under `portals/` that already holds `organization`'s default Portal,
/// under whatever name, so the migration never files it twice.
#[cfg(all(feature = "portal", feature = "dataroom"))]
async fn default_portal_filed(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    organization: &str,
) -> Result<Option<String>, StartError> {
    let dir = format!("{repo}/contents/portals?ref=main");
    let answer = client
        .get(&dir)
        .header("authorization", auth)
        .send()
        .await
        .map_err(|e| err(format!("GET {dir}: {e}")))?;
    let status = answer.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(err(format!("GET {dir} answered {status}")));
    }
    let listing: Vec<serde_json::Value> = answer
        .json()
        .await
        .map_err(|e| err(format!("GET {dir}: {e}")))?;
    for path in listing
        .iter()
        .filter(|entry| entry["type"] == "file")
        .filter_map(|entry| entry["path"].as_str())
        .filter(|path| path.ends_with(".yaml"))
    {
        let text = file_text(client, auth, repo, path).await?;
        let held =
            jc_core::envelope::ResourceEnvelope::<jc_core::kinds::PortalSpec>::from_yaml(&text)
                .is_ok_and(|p| p.spec.organization_ref == organization && p.spec.is_default());
        if held {
            return Ok(Some(path.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(not(all(feature = "portal", feature = "dataroom")))]
async fn migrate_seeds(
    _: &reqwest::Client,
    _: &str,
    _: &str,
    _: &crate::config::DataroomSection,
    _: bool,
) {
}

/// The installation Organization's name, `metadata.name` of the repository's `org.yaml`.
#[cfg(all(feature = "portal", feature = "dataroom"))]
async fn installation_name(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
) -> Result<String, StartError> {
    let text = file_text(client, auth, repo, "org.yaml").await?;
    jc_core::envelope::ResourceEnvelope::<jc_core::kinds::OrganizationSpec>::from_yaml(&text)
        .map(|org| org.metadata.name)
        .map_err(|e| err(format!("org.yaml: {e}")))
}

/// The text of the repository's file `path` on `main`.
#[cfg(all(feature = "portal", feature = "dataroom"))]
async fn file_text(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    path: &str,
) -> Result<String, StartError> {
    let file = format!("{repo}/contents/{path}?ref=main");
    let answer = client
        .get(&file)
        .header("authorization", auth)
        .send()
        .await
        .map_err(|e| err(format!("GET {file}: {e}")))?;
    let status = answer.status();
    if !status.is_success() {
        return Err(err(format!("GET {file} answered {status}")));
    }
    let body: serde_json::Value = answer
        .json()
        .await
        .map_err(|e| err(format!("GET {file}: {e}")))?;
    body["content"]
        .as_str()
        .map(|b64| b64.split_whitespace().collect::<String>())
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| err(format!("GET {file}: no base64 content")))
}

/// Writes `path` when the repository has no such file.
async fn seed(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    path: &str,
    content: &str,
) -> Result<(), StartError> {
    let file = format!("{repo}/contents/{path}");
    let status = client
        .get(format!("{file}?ref=main"))
        .header("authorization", auth)
        .send()
        .await
        .map_err(|e| err(format!("GET {file}: {e}")))?
        .status();
    if status.is_success() {
        return Ok(());
    }
    if status != StatusCode::NOT_FOUND {
        return Err(err(format!("GET {file} answered {status}")));
    }
    let status = client
        .put(&file)
        .header("authorization", auth)
        .json(&serde_json::json!({
            "content": base64::engine::general_purpose::STANDARD.encode(content),
            "message": format!("Add {path}"),
            "branch": "main",
        }))
        .send()
        .await
        .map_err(|e| err(format!("PUT {file}: {e}")))?
        .status();
    if status.is_success() || status == StatusCode::CONFLICT {
        Ok(())
    } else {
        Err(err(format!("PUT {file} answered {status}")))
    }
}

/// The Organization's DNS domain: the Portal's host when it has two labels, else `jc.invalid`
/// (a name like `localhost` is no organization domain).
pub(crate) fn domain(portal_base: Option<&str>) -> String {
    let host = portal_base
        .and_then(|base| base.split("://").nth(1))
        .and_then(|rest| rest.split(['/', ':']).next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.contains('.') && host.parse::<std::net::IpAddr>().is_err() {
        host
    } else {
        "jc.invalid".to_owned()
    }
}

/// The role a project's opener holds in it: propose and approve every project kind (the yellow
/// lane), as the platform's demonstration seed has it; deletion, roles and bindings stay with
/// the organization's administrators.
const STEWARD: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Role
metadata:
  name: steward
  namespace: org
spec:
  rules:
    - kinds: [ContextSpace, DataModel, Mapping, Policy, ScopeDefinition, Endpoint, ModelProjection,
        SharedSpaceReference, ContextSourceRegistration, ServiceAccount, Pipeline, DataSource,
        MqttBroker, App, Blueprint, DataSpaceParticipant, DataOffer, DataAgreement, SyncSource,
        Bundle, UiSchema, Subscription, Entity, Dashboard, Layer]
      verbs: [propose, approve]
";

fn organization_manifest(domain: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: {FORGE_OWNER}\n  \
         namespace: org\nspec:\n  domain: {domain}\n  installation: true\n  locales: [\"en\"]\n  \
         defaultLocale: en\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_domain_is_the_portal_host_or_a_reserved_name() {
        assert_eq!(
            domain(Some("https://portal.city.example/")),
            "portal.city.example"
        );
        assert_eq!(domain(Some("http://City.Example:8080/")), "city.example");
        assert_eq!(domain(Some("http://localhost:42081/")), "jc.invalid");
        assert_eq!(domain(Some("http://127.0.0.1:8080/")), "jc.invalid");
        assert_eq!(domain(None), "jc.invalid");
        assert!(organization_manifest("city.example").contains("domain: city.example\n"));
    }

    /// A shako holding `files` (path -> text) that records each file PUT to it.
    #[cfg(all(feature = "portal", feature = "dataroom"))]
    async fn shako(
        files: Vec<(&'static str, String)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::http::{Method, StatusCode, Uri};
        use axum::response::IntoResponse;
        let put = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let written = put.clone();
        let app = axum::Router::new().fallback(move |method: Method, uri: Uri| {
            let (files, put) = (files.clone(), put.clone());
            async move {
                let path = uri.path().trim_start_matches("/repo/contents/").to_owned();
                if method == Method::PUT {
                    put.lock().unwrap().push(path);
                    return StatusCode::CREATED.into_response();
                }
                let listing: Vec<_> = files
                    .iter()
                    .filter(|(p, _)| p.starts_with(&format!("{path}/")))
                    .map(|(p, _)| serde_json::json!({"type": "file", "path": p}))
                    .collect();
                if !listing.is_empty() {
                    return axum::Json(listing).into_response();
                }
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, text)) => axum::Json(serde_json::json!({
                        "content": base64::engine::general_purpose::STANDARD.encode(text)
                    }))
                    .into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/repo", socket.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(socket, app).await });
        (base, written)
    }

    #[cfg(all(feature = "portal", feature = "dataroom"))]
    fn prod_dataroom() -> crate::config::DataroomSection {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        crate::config::Config::read_file(&docs.join("harvest-sources.toml"))
            .unwrap()
            .dataroom
            .unwrap()
    }

    #[cfg(all(feature = "portal", feature = "dataroom"))]
    async fn migrated(
        files: Vec<(&'static str, String)>,
        dataroom: &crate::config::DataroomSection,
    ) -> Vec<String> {
        let (repo, put) = shako(files).await;
        migrate_seeds(&reqwest::Client::new(), "token t", &repo, dataroom, false).await;
        put.lock().unwrap().clone()
    }

    #[cfg(all(feature = "portal", feature = "dataroom"))]
    #[tokio::test]
    async fn the_seeds_move_into_the_installation_default_portal() {
        let org = ("org.yaml", organization_manifest("city.example"));
        let dataroom = prod_dataroom();
        assert_eq!(
            migrated(vec![org], &dataroom).await,
            [format!("portals/{FORGE_OWNER}-default.yaml")]
        );
    }

    /// A migration that cannot run never stops jc from starting: the seeds keep harvesting.
    #[cfg(all(feature = "portal", feature = "dataroom"))]
    #[tokio::test]
    async fn a_seed_migration_that_fails_is_skipped_and_jc_starts() {
        let org = ("org.yaml", organization_manifest("city.example"));
        let mut refused = prod_dataroom();
        refused.harvest[0].kind = "no-such-kind".into();
        assert!(migrated(vec![org], &refused).await.is_empty());
        assert!(migrated(Vec::new(), &prod_dataroom()).await.is_empty());
    }

    #[cfg(all(feature = "portal", feature = "dataroom"))]
    #[tokio::test]
    async fn a_default_portal_filed_under_another_name_is_not_filed_twice() {
        let org = ("org.yaml", organization_manifest("city.example"));
        let held = (
            "portals/city.yaml",
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Portal\nmetadata:\n  name: city\n  \
                 namespace: org\nspec:\n  organizationRef: {FORGE_OWNER}\n  path: default\n"
            ),
        );
        assert!(migrated(vec![org, held], &prod_dataroom()).await.is_empty());
    }
}
