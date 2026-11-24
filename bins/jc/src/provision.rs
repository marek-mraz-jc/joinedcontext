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
pub async fn forge(siblings: &Siblings, setup: &Setup) -> Result<(), StartError> {
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
    seed(&client, &auth, &repo, "users/roles/steward.yaml", STEWARD).await
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

#[cfg(test)]
mod forge_tests {
    use super::*;
    use axum::extract::Request;
    use axum::response::IntoResponse;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    type Script = &'static [(&'static str, u16)];

    /// A fake shako: answers a call with the status of the first `script` entry whose words all
    /// occur in `METHOD uri` (404 when none does), and records each call with whether it
    /// carried the `token` credential and, for a write, the branch `main`.
    async fn shako(script: Script) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let router = axum::Router::new().fallback(move |req: Request| {
            let seen = Arc::clone(&seen);
            async move {
                let call = format!("{} {}", req.method(), req.uri());
                let authed = req
                    .headers()
                    .get("authorization")
                    .is_some_and(|v| v.as_bytes().starts_with(b"token "));
                let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let status = script
                    .iter()
                    .find(|(words, _)| words.split(' ').all(|w| call.contains(w)))
                    .map_or(404, |(_, s)| *s);
                let main = String::from_utf8_lossy(&body).contains("\"branch\":\"main\"")
                    || String::from_utf8_lossy(&body).contains("\"default_branch\":\"main\"");
                seen.lock().unwrap().push(format!("{call} {authed} {main}"));
                // 0: the connection drops without an answer (the task ends mid-request).
                assert_ne!(status, 0, "shako drops the connection");
                axum::http::StatusCode::from_u16(status)
                    .unwrap()
                    .into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (addr, calls)
    }

    fn fixture(
        shako: Option<std::net::SocketAddr>,
        roles: &[Module],
    ) -> (Siblings, Setup, tempfile::TempDir) {
        let plans = crate::siblings::tests::host_mode_plans();
        let mut siblings = Siblings::new(&plans, "127.0.0.1:8080".parse().unwrap());
        match shako {
            Some(addr) => siblings.insert_loopback(Module::Shako, addr),
            None => siblings.remove(Module::Shako),
        }
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), roles, &BTreeMap::new()).unwrap();
        (siblings, setup, dir)
    }

    fn repo() -> (String, String) {
        let r = forge_repo(crate::config::portal_env).unwrap();
        (r.owner, r.repo)
    }

    #[tokio::test]
    async fn a_missing_repository_is_created_and_seeded_with_the_token() {
        let (addr, calls) = shako(&[("POST /orgs/", 201), ("PUT", 201)]).await;
        let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
        forge(&siblings, &setup).await.unwrap();
        let (owner, name) = repo();
        let repo = format!("/api/v1/repos/{owner}/{name}");
        assert_eq!(
            *calls.lock().unwrap(),
            [
                format!("GET {repo} true false"),
                format!("POST /api/v1/orgs/{owner}/repos true true"),
                format!("GET {repo}/contents/org.yaml?ref=main true false"),
                format!("PUT {repo}/contents/org.yaml true true"),
                format!("GET {repo}/contents/users/roles/steward.yaml?ref=main true false"),
                format!("PUT {repo}/contents/users/roles/steward.yaml true true"),
            ]
        );
    }

    #[tokio::test]
    async fn an_existing_repository_is_left_alone() {
        let (addr, calls) = shako(&[("GET /repos/", 200)]).await;
        let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
        forge(&siblings, &setup).await.unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1, "only the repository GET");
    }

    #[tokio::test]
    async fn existing_files_are_not_written_again() {
        let (addr, calls) = shako(&[("GET contents", 200), ("POST", 201)]).await;
        let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
        forge(&siblings, &setup).await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert!(!calls.iter().any(|c| c.starts_with("PUT")));
    }

    #[tokio::test]
    async fn a_repository_or_file_made_in_parallel_counts_as_made() {
        let (addr, calls) = shako(&[("POST", 409), ("PUT", 409)]).await;
        let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
        forge(&siblings, &setup).await.unwrap();
        let puts = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("PUT"))
            .count();
        assert_eq!(puts, 2);
    }

    #[tokio::test]
    async fn each_refusal_names_the_call_and_status_and_never_the_token() {
        let cases: [(Script, &str, &str); 4] = [
            (&[("GET /repos/", 403)], "GET ", "403 Forbidden"),
            (&[("POST", 500)], "POST ", "500 Internal Server Error"),
            (
                &[("POST", 201), ("GET contents", 401)],
                "GET ",
                "401 Unauthorized",
            ),
            (
                &[("POST", 201), ("PUT", 422)],
                "PUT ",
                "422 Unprocessable Entity",
            ),
        ];
        for (script, call, status) in cases {
            let (addr, _calls) = shako(script).await;
            let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
            let e = forge(&siblings, &setup).await.unwrap_err().to_string();
            assert!(e.contains(call) && e.contains(status), "{e}");
            let token = setup.value(Secret::ShakoToken).unwrap();
            assert!(!e.contains(token), "{e}");
        }
    }

    #[tokio::test]
    async fn a_connection_dropped_mid_provisioning_names_the_call() {
        let cases: [(Script, &str); 3] = [
            (&[("POST", 0)], "POST "),
            (&[("POST", 201), ("GET contents", 0)], "GET "),
            (&[("POST", 201), ("PUT", 0)], "PUT "),
        ];
        for (script, call) in cases {
            let (addr, _calls) = shako(script).await;
            let (siblings, setup, _dir) = fixture(Some(addr), &Module::ALL);
            let e = forge(&siblings, &setup).await.unwrap_err().to_string();
            assert!(e.contains(&format!("{call}http://")), "{e}");
            assert!(!e.contains("answered"), "{e}");
        }
    }

    #[tokio::test]
    async fn an_unreachable_shako_names_the_call() {
        // Bound and never listening: every connection is refused.
        let gone = tokio::net::TcpSocket::new_v4().unwrap();
        gone.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let (siblings, setup, _dir) = fixture(Some(gone.local_addr().unwrap()), &Module::ALL);
        let e = forge(&siblings, &setup).await.unwrap_err().to_string();
        assert!(e.contains("GET http://127.0.0.1:"), "{e}");
    }

    #[tokio::test]
    async fn without_shako_or_its_token_nothing_is_called() {
        let (siblings, setup, _dir) = fixture(None, &Module::ALL);
        forge(&siblings, &setup).await.unwrap();
        let (addr, calls) = shako(&[]).await;
        let (siblings, setup, _dir) = fixture(Some(addr), &[Module::Portal]);
        forge(&siblings, &setup).await.unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }
}
