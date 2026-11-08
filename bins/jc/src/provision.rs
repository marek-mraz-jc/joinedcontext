//! First-start provisioning `jc` does through its siblings' public HTTP APIs once they serve:
//! the shako organization repository the Portal writes manifests to. Idempotent: an existing
//! repository is left as it is.

use crate::app::StartError;
use crate::module::Module;
use crate::setup::{FORGE_OWNER, FORGE_REPO, Secret, Setup};
use crate::siblings::{Siblings, portal_vars};
use reqwest::StatusCode;
use std::time::Duration;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Shako, e)
}

/// Creates `{FORGE_OWNER}/{FORGE_REPO}` in shako (Gitea `POST /api/v1/orgs/{owner}/repos`, with
/// `auto_init`) when `jc` points the Portal at shako. Errors name the call and the status, never
/// the token.
pub async fn forge(siblings: &Siblings, setup: &Setup) -> Result<(), StartError> {
    let vars = portal_vars(siblings, setup, |key| std::env::var(key).ok());
    let (Some(base), Some(token)) = (vars.get("JC_GITEA_URL"), setup.value(Secret::ShakoToken))
    else {
        return Ok(());
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(err)?;
    let auth = format!("token {token}");
    let repo = format!("{base}/api/v1/repos/{FORGE_OWNER}/{FORGE_REPO}");
    let status = client
        .get(&repo)
        .header("authorization", &auth)
        .send()
        .await
        .map_err(|e| err(format!("GET {repo}: {e}")))?
        .status();
    if status.is_success() {
        return Ok(());
    }
    if status != StatusCode::NOT_FOUND {
        return Err(err(format!("GET {repo} answered {status}")));
    }
    let create = format!("{base}/api/v1/orgs/{FORGE_OWNER}/repos");
    let status = client
        .post(&create)
        .header("authorization", &auth)
        .json(&serde_json::json!({
            "name": FORGE_REPO,
            "auto_init": true,
            "default_branch": "main",
            "private": true,
        }))
        .send()
        .await
        .map_err(|e| err(format!("POST {create}: {e}")))?
        .status();
    if status.is_success() || status == StatusCode::CONFLICT {
        tracing::info!(repo = %format!("{FORGE_OWNER}/{FORGE_REPO}"), "shako: the Portal's repository is ready");
        Ok(())
    } else {
        Err(err(format!("POST {create} answered {status}")))
    }
}
