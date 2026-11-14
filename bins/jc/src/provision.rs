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
        return migrate_harvest_seeds(&client, &auth, &repo, cfg).await;
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
    migrate_harvest_seeds(&client, &auth, &repo, cfg).await
}

/// Moves the `[[dataroom.harvest]]` seeds into their Organization's `default` Portal manifest,
/// `portals/{organization}-default.yaml`, written once when absent (ODP-11): from then on the
/// Portal's reconciler holds the sources, and the Portal shows and runs them. A seed of the
/// `[dataroom]` catalogue belongs to the installation's Organization (`org.yaml`), and only
/// without `[[organizations]]`, whose catalogues are their own.
#[cfg(feature = "portal")]
async fn migrate_harvest_seeds(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
    cfg: &crate::config::Config,
) -> Result<(), StartError> {
    let seeds = &cfg.dataroom.harvest;
    if seeds.is_empty() {
        return Ok(());
    }
    let mut by_organization: std::collections::BTreeMap<String, Vec<_>> = Default::default();
    for seed in seeds {
        let organization = match &seed.organization {
            Some(name) => name.clone(),
            None if cfg.organizations.is_empty() => installation_name(client, auth, repo).await?,
            None => {
                tracing::warn!(
                    source = %seed.name,
                    "[[dataroom.harvest]] of the [dataroom] catalogue beside [[organizations]]: \
                     no Portal holds that catalogue, so it stays a seed"
                );
                continue;
            }
        };
        by_organization.entry(organization).or_default().push(seed);
    }
    for (organization, seeds) in by_organization {
        let spec = crate::modules::portals::default_portal_of(
            &organization,
            &seeds,
            cfg.dataroom.config_dir.as_deref(),
        )
        .map_err(err)?;
        let name = format!("{organization}-default");
        let manifest = jc_core::envelope::ResourceEnvelope::new(
            jc_core::envelope::ObjectMeta::new(&name, "org"),
            spec,
        )
        .to_yaml()
        .map_err(err)?;
        seed(
            client,
            auth,
            repo,
            &format!("portals/{name}.yaml"),
            &manifest,
        )
        .await?;
        tracing::info!(portal = %name, sources = seeds.len(), "[[dataroom.harvest]] moved into its Portal manifest (ODP-11)");
    }
    Ok(())
}

#[cfg(not(feature = "portal"))]
async fn migrate_harvest_seeds(
    _: &reqwest::Client,
    _: &str,
    _: &str,
    _: &crate::config::Config,
) -> Result<(), StartError> {
    Ok(())
}

/// The installation Organization's name, `metadata.name` of the repository's `org.yaml`.
#[cfg(feature = "portal")]
async fn installation_name(
    client: &reqwest::Client,
    auth: &str,
    repo: &str,
) -> Result<String, StartError> {
    let file = format!("{repo}/contents/org.yaml?ref=main");
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
    let text = body["content"]
        .as_str()
        .map(|b64| b64.split_whitespace().collect::<String>())
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| err(format!("GET {file}: no base64 content")))?;
    jc_core::envelope::ResourceEnvelope::<jc_core::kinds::OrganizationSpec>::from_yaml(&text)
        .map(|org| org.metadata.name)
        .map_err(|e| err(format!("org.yaml: {e}")))
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
}
