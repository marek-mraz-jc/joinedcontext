//! A `SyncSource`'s own webhook secret reaches the route that authorises its runs, or the source
//! is not reachable through the webhook at all (MF-44, T-2297).
//!
//! The whole path with the two things it talks to mocked: the forge that serves the repository and
//! the secret backend that holds the value. OpenBao is the backend here because it speaks HTTP and
//! a test can answer it; the SOPS backend is the same resolution over a decrypted file and is
//! covered where the store is (`jcctl`'s `sops_tests`), exactly as `pipeline_secrets_tests` does.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::pipeline_secrets::{Backend, Resolver};
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use joinedcontext_portal::sync::webhook_secrets::Accepted;
use serde_json::{json, Value};
use sha2::Sha256;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const HOOK_SECRET: &str = "the-origins-own-hook-secret";
const RETIRING: &str = "the-hook-secret-being-retired";

const ORGANIZATION: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: helsinki\nspec:\n  isSandbox: true\n";

/// A webhook-driven source: its origin pushes, so it carries the secret that push is signed with.
const DRIVEN: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: SyncSource
metadata:
  name: regional
  namespace: helsinki
spec:
  source:
    git: { url: https://git.region.sk/udp/models.git, ref: main }
  schedule: { webhook: true }
  webhook:
    secretRef: { name: sync-hook, key: secret }
    previousSecretRef: { name: sync-hook-old, key: secret }
  mode: mirror
  conflictPolicy: replace
"#;

/// A polled source beside it: nothing is held for it, so the webhook route refuses it whatever
/// the caller signs with.
const POLLED: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: SyncSource
metadata:
  name: nightly
  namespace: helsinki
spec:
  source:
    git: { url: https://git.region.sk/udp/other.git, ref: main }
  schedule: { interval: 30m }
  mode: mirror
  conflictPolicy: replace
"#;

async fn forge(files: &[(&str, &str)]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_branch": "main"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "main", "commit": {"id": REVISION}})),
        )
        .mount(&server)
        .await;
    let tree: Vec<Value> = files
        .iter()
        .map(|(p, _)| json!({"path": p, "type": "blob"}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/git/trees/{REVISION}"
        )))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sha": "tree-sha", "truncated": false, "tree": tree})),
        )
        .mount(&server)
        .await;
    for (file, content) in files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file}"
            )))
            .and(query_param("ref", REVISION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha",
                "content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }
    server
}

fn repository() -> Vec<(&'static str, &'static str)> {
    vec![
        ("org.yaml", ORGANIZATION),
        ("projects/helsinki/spaces/mobility/space.yaml", SPACE),
        ("projects/helsinki/sync/regional.yaml", DRIVEN),
        ("projects/helsinki/sync/nightly.yaml", POLLED),
    ]
}

fn client(server: &MockServer) -> Arc<GiteaClient> {
    Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("a forge client"),
    )
}

/// An OpenBao that answers the login and holds the two hook secrets.
async fn openbao(holds: &[(&str, &str)]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": "a-session-token", "lease_duration": 3600, "renewable": true }
        })))
        .mount(&server)
        .await;
    for (name, value) in holds {
        Mock::given(method("GET"))
            .and(path(format!("/v1/secret/data/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "data": { "secret": value }, "metadata": { "version": 1 } }
            })))
            .mount(&server)
            .await;
    }
    server
}

fn jwt_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("jc-portal-{name}-token"));
    std::fs::write(&path, "a.service.account.token").expect("write the token");
    path
}

/// The hex HMAC-SHA256 an origin presents, as Gitea computes it.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One pass over `repository`, with `holds` in the backend, and what it left for the route.
async fn pass(holds: &[(&str, &str)], case: &str) -> Arc<Accepted> {
    let forge = forge(&repository()).await;
    let bao = openbao(holds).await;
    let accepted = Arc::new(Accepted::new());
    let syncer = Syncer::new(client(&forge), Arc::new(Mirror::new()))
        .with_webhook_secrets(Arc::clone(&accepted))
        .with_pipeline_secrets(Resolver::new(Backend::OpenBao {
            address: bao.uri(),
            role: "portal".to_owned(),
            jwt_path: jwt_file(case),
        }));
    syncer.sync_once().await.expect("the run loads");
    accepted
}

#[tokio::test]
async fn the_source_that_declares_a_hook_secret_is_the_only_one_the_route_opens() {
    let accepted = pass(
        &[("sync-hook", HOOK_SECRET), ("sync-hook-old", RETIRING)],
        "declares",
    )
    .await;
    let body = br#"{"pushed":true}"#;

    // The value came out of the backend and through the reference, both of them.
    for secret in [HOOK_SECRET, RETIRING] {
        assert!(
            accepted.verifies("helsinki", "regional", body, &sign(secret, body)),
            "{secret} did not reach the route",
        );
    }
    // And nothing else opens it, including the other source's name.
    assert!(!accepted.verifies("helsinki", "regional", body, &sign("guessed", body)));
    assert!(!accepted.verifies("helsinki", "nightly", body, &sign(HOOK_SECRET, body)));
    assert!(!accepted.verifies("doprava", "regional", body, &sign(HOOK_SECRET, body)));

    // One source of the two: the polled one is not reachable through the webhook at all.
    assert_eq!(accepted.len(), 1, "a polled source was made reachable");
}

#[tokio::test]
async fn a_reference_the_backend_does_not_hold_leaves_the_source_shut() {
    // The backend answers the login and holds neither name: the pass runs, the source loads, and
    // the route has nothing to accept — which is the same `401` as a wrong signature, so a caller
    // cannot tell a misconfigured source from one that is not there (PF-59).
    let accepted = pass(&[], "unresolved").await;
    let body = b"{}";

    assert!(accepted.is_empty(), "something was accepted from nowhere");
    assert!(!accepted.verifies("helsinki", "regional", body, &sign(HOOK_SECRET, body)));
}

#[tokio::test]
async fn a_pass_with_no_secret_backend_opens_nothing() {
    // A Portal that configured no backend resolves nothing, so a webhook-driven source is shut
    // rather than open to everybody: fail closed (CC-06).
    let forge = forge(&repository()).await;
    let accepted = Arc::new(Accepted::new());
    let syncer = Syncer::new(client(&forge), Arc::new(Mirror::new()))
        .with_webhook_secrets(Arc::clone(&accepted));
    syncer.sync_once().await.expect("the run loads");

    let body = b"{}";
    assert!(accepted.is_empty());
    assert!(!accepted.verifies("helsinki", "regional", body, &sign(HOOK_SECRET, body)));
}
