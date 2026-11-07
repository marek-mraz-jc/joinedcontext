//! A pipeline's `secretRef`s reach the runner's environment, or the pipeline does not run
//! (T-0927, PL-15…PL-17).
//!
//! The whole path, with the three things it talks to mocked: the forge that serves the
//! repository, the secret backend that holds the value, and the API server the Secret is written
//! to. OpenBao is the backend here because it speaks HTTP and a test can answer it; the SOPS
//! backend is the same resolution over a decrypted file and is covered where the store is
//! (`jcctl`'s `sops_tests`).

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::pipeline_secrets::{Backend, Resolver};
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const PASSWORD: &str = "the-mqtt-password-nobody-else-has";

const ORGANIZATION: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: helsinki\nspec:\n  isSandbox: true\n";

/// A DataSource whose password is an interpolation naming its own `spec.secrets` (PL-50).
const DATA_SOURCE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: DataSource
metadata:
  name: mesto-mqtt
  namespace: helsinki
spec:
  contextSpaceRef: { kind: ContextSpace, name: mobility }
  class: resident
  type: mqtt
  input:
    urls: ["tcp://broker.example.fi:1883"]
    topics: ["vehicles/#"]
    password: "${MQTT_PASSWORD}"
  secrets:
    - { name: mqtt-mesto, key: password, envVar: MQTT_PASSWORD }
"#;

const PIPELINE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: vehicles
  namespace: helsinki
spec:
  class: resident
  enabled: true
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-in"
  quotas: { maxMemoryMb: 128, cpuMillicores: 250 }
  source:
    dataSourceRef: { kind: DataSource, name: mesto-mqtt }
  compute:
    kind: bloblang
    mapping: "root = this"
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
        ("projects/helsinki/datasources/mesto-mqtt.yaml", DATA_SOURCE),
        (
            "projects/helsinki/pipelines/vehicles/pipeline.yaml",
            PIPELINE,
        ),
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

/// An OpenBao that answers the login and one KV v2 read.
async fn openbao() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": "a-session-token", "lease_duration": 3600, "renewable": true }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/mqtt-mesto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "data": { "password": PASSWORD }, "metadata": { "version": 1 } }
        })))
        .mount(&server)
        .await;
    server
}

/// An API server that accepts every apply and remembers what it was sent.
async fn cluster() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kind": "Secret"})))
        .mount(&server)
        .await;
    server
}

fn jwt_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("jc-portal-{name}-token"));
    std::fs::write(&path, "a.service.account.token").expect("write the token");
    path
}

async fn applied(cluster: &MockServer, ends_with: &str) -> Option<Value> {
    cluster
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|request| request.url.path().ends_with(ends_with))
        .map(|request| serde_json::from_slice(&request.body).expect("a json body"))
}

#[tokio::test]
async fn the_runner_gets_the_value_and_the_pipeline_is_not_refused() {
    let forge = forge(&repository()).await;
    let bao = openbao().await;
    let cluster = cluster().await;

    let syncer = Syncer::new(client(&forge), Arc::new(Mirror::new()))
        .with_pipeline_secrets(Resolver::new(Backend::OpenBao {
            address: bao.uri(),
            role: "portal".to_owned(),
            jwt_path: jwt_file("runner-gets-the-value"),
        }))
        .with_credential_secrets(
            Arc::new(KubeClient::with_token(&cluster.uri(), "token").expect("a kube client")),
            "dev",
        );
    syncer.sync_once().await.expect("the run loads");

    let secret = applied(&cluster, "/secrets/pipeline-secrets")
        .await
        .expect("the runner's Secret was written");
    assert_eq!(secret["stringData"]["MQTT_PASSWORD"], PASSWORD);
    assert_eq!(secret["kind"], "Secret");

    // The value is read once, when the pod starts, so the runner is rolled when it changes.
    let rollout = applied(&cluster, "/deployments/pipeline-runner")
        .await
        .expect("the runner was rolled");
    let annotation = rollout["spec"]["template"]["metadata"]["annotations"]
        ["joinedcontext.com/pipeline-secrets"]
        .as_str()
        .expect("the fingerprint is stamped");
    assert_eq!(annotation.len(), 64, "a sha-256 of the values");
    assert!(!annotation.contains(PASSWORD));
}

#[tokio::test]
async fn without_a_backend_the_pipeline_is_refused_and_nothing_is_written() {
    let forge = forge(&repository()).await;
    let cluster = cluster().await;
    let mirror = Arc::new(Mirror::new());

    let syncer = Syncer::new(client(&forge), Arc::clone(&mirror)).with_credential_secrets(
        Arc::new(KubeClient::with_token(&cluster.uri(), "token").expect("a kube client")),
        "dev",
    );
    syncer.sync_once().await.expect("the run loads");

    assert!(
        applied(&cluster, "/secrets/pipeline-secrets")
            .await
            .is_none(),
        "a Portal with no backend wrote a Secret"
    );
    let pipeline = mirror
        .get("helsinki", "Pipeline", "vehicles")
        .expect("the pipeline is in the mirror");
    let condition = serde_json::to_string(&pipeline.status).expect("a status");
    assert!(
        condition.contains("mqtt-mesto"),
        "the refusal names the secret: {condition}"
    );
    assert!(
        !condition.contains(PASSWORD),
        "a value reached the status: {condition}"
    );
}

/// Edge cases of the OpenBao resolution itself (T-2503, PL-15, PL-17): every refusal names the
/// reference a person wrote, and none carries the ServiceAccount JWT, the session token or a
/// value. The role, the store's address and the KV path are deployment configuration, not
/// credentials, and PL-17 redacts values; they may appear, so a person can act on the reason.
mod openbao_edges {
    use super::*;
    use jc_core::envelope::SecretRef;
    use joinedcontext_portal::pipeline_secrets::SecretError;
    use std::path::Path;

    const JWT: &str = "eyJ.the-service-account-jwt.sig";
    const SESSION: &str = "hvs.the-session-token";

    fn reference(name: &str, key: Option<&str>, env_var: &str) -> SecretRef {
        SecretRef {
            name: name.to_owned(),
            key: key.map(str::to_owned),
            env_var: Some(env_var.to_owned()),
        }
    }

    fn token_file(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("jc-portal-t2503-{name}-token"));
        std::fs::write(&path, content).expect("write the token");
        path
    }

    fn resolver(address: &str, jwt_path: std::path::PathBuf) -> Resolver {
        Resolver::new(Backend::OpenBao {
            address: address.to_owned(),
            role: "portal".to_owned(),
            jwt_path,
        })
    }

    async fn login_ok(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v1/auth/kubernetes/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "auth": { "client_token": SESSION, "lease_duration": 3600, "renewable": true }
            })))
            .mount(server)
            .await;
    }

    async fn kv(server: &MockServer, name: &str, status: u16, body: Value) {
        Mock::given(method("GET"))
            .and(path(format!("/v1/secret/data/{name}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn refusal(resolver: &Resolver, references: &[SecretRef]) -> (String, String) {
        match resolver
            .resolve(Path::new("."), "helsinki", references)
            .await
        {
            Err(SecretError::Unresolved { name, reason })
            | Err(SecretError::Refused { name, reason }) => (name, reason),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    fn assert_no_credential(reason: &str) {
        for secret in [JWT, SESSION, PASSWORD] {
            assert!(
                !reason.contains(secret),
                "a credential reached the reason: {reason}"
            );
        }
    }

    /// Case 1: OpenBao refuses the login; the reason says so and carries neither the JWT nor a
    /// session token.
    #[tokio::test]
    async fn a_login_the_realm_refuses_carries_no_jwt_in_the_message() {
        let bao = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/auth/kubernetes/login"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"errors": ["permission denied"]})),
            )
            .mount(&bao)
            .await;
        let resolver = resolver(&bao.uri(), token_file("login-refused", JWT));
        let (name, reason) =
            refusal(&resolver, &[reference("mqtt-mesto", None, "MQTT_PASSWORD")]).await;
        assert_eq!(name, "mqtt-mesto");
        assert!(
            reason.contains("403") && reason.contains("permission denied"),
            "{reason}"
        );
        assert_no_credential(&reason);
    }

    /// Case 2: a name that is not one path segment would read outside the project's subtree; it
    /// is refused before any read, and no KV path is ever asked for.
    #[tokio::test]
    async fn a_reference_naming_a_path_outside_the_projects_subtree_is_refused() {
        for bad in ["../other-project/db", "nested/name", ".hidden", ""] {
            let bao = MockServer::start().await;
            login_ok(&bao).await;
            let resolver = resolver(&bao.uri(), token_file("outside", JWT));
            let (name, reason) = refusal(&resolver, &[reference(bad, None, "X")]).await;
            assert_eq!(name, bad);
            // jc-core's MF-35 rule (T-5702) refuses these names before the backend's own
            // path-segment check; either refusal names the reference and reads nothing.
            assert!(
                reason.contains("single path segment") || reason.contains("secretRef.name"),
                "{bad}: {reason}"
            );
            let reads = bao.received_requests().await.unwrap_or_default();
            assert!(
                reads.iter().all(|r| r.method.as_str() != "GET"),
                "{bad}: a KV read was sent: {reads:?}"
            );
        }
    }

    /// Case 3: the store cannot be reached; the reason says so and carries no credential.
    #[tokio::test]
    async fn a_transport_error_names_no_credential() {
        // A port nothing listens on: bind, read the port, drop the listener.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("a free port")
            .local_addr()
            .expect("an address")
            .port();
        let resolver = resolver(
            &format!("http://127.0.0.1:{port}"),
            token_file("unreachable", JWT),
        );
        let (name, reason) = refusal(&resolver, &[reference("mqtt-mesto", None, "M")]).await;
        assert_eq!(name, "mqtt-mesto");
        assert!(reason.contains("could not reach OpenBao"), "{reason}");
        assert_no_credential(&reason);
    }

    /// Cases 4 and 5: a token file that is missing or empty is a named refusal before any
    /// request, and never an empty login.
    #[tokio::test]
    async fn a_missing_or_empty_service_account_token_is_refused_before_any_request() {
        let bao = MockServer::start().await;
        login_ok(&bao).await;
        let missing = std::env::temp_dir().join("jc-portal-t2503-no-such-token");
        let _ = std::fs::remove_file(&missing);
        for (jwt_path, expected) in [
            (missing, "I/O error reading the ServiceAccount token"),
            (token_file("empty", " \n"), "is empty"),
        ] {
            let resolver = resolver(&bao.uri(), jwt_path);
            let (name, reason) = refusal(&resolver, &[reference("mqtt-mesto", None, "M")]).await;
            assert_eq!(name, "mqtt-mesto");
            assert!(reason.contains(expected), "{reason}");
        }
        let sent = bao.received_requests().await.unwrap_or_default();
        assert!(
            sent.is_empty(),
            "a login was sent without a token: {sent:?}"
        );
    }

    /// Cases 6 to 9, table-driven: what the store answers for the second of two references, and
    /// the words the refusal must carry. The refusal names *that* reference, not the first one
    /// the pipeline lists, so the person fixes the right secret.
    #[tokio::test]
    async fn a_secret_the_store_cannot_serve_is_refused_by_its_own_name() {
        let cases: [(&str, u16, Value, Option<&str>, &str); 5] = [
            (
                "not-found",
                404,
                json!({}),
                None,
                "does not exist, or this role has no grant",
            ),
            (
                "deleted",
                200,
                json!({"data": {"data": null, "metadata": {"version": 2}}}),
                None,
                "no live version",
            ),
            (
                "wrong-key",
                200,
                json!({"data": {"data": {"password": "v"}, "metadata": {"version": 1}}}),
                Some("username"),
                "has no key `username`",
            ),
            (
                "not-text",
                200,
                json!({"data": {"data": {"port": 1883}, "metadata": {"version": 1}}}),
                Some("port"),
                "is not a string",
            ),
            (
                "several-keys",
                200,
                json!({"data": {"data": {"a": "1", "b": "2"}, "metadata": {"version": 1}}}),
                None,
                "must name one",
            ),
        ];
        for (secret, status, body, key, expected) in cases {
            let bao = MockServer::start().await;
            login_ok(&bao).await;
            kv(
                &bao,
                "mqtt-mesto",
                200,
                json!({"data": {"data": {"password": PASSWORD}, "metadata": {"version": 1}}}),
            )
            .await;
            kv(&bao, secret, status, body).await;
            let resolver = resolver(&bao.uri(), token_file("by-name", JWT));
            let (name, reason) = refusal(
                &resolver,
                &[
                    reference("mqtt-mesto", Some("password"), "MQTT_PASSWORD"),
                    reference(secret, key, "SECOND"),
                ],
            )
            .await;
            assert_eq!(name, secret, "the refusal names another secret: {reason}");
            assert!(reason.contains(expected), "{secret}: {reason}");
            assert_no_credential(&reason);
        }
    }

    /// Case 10: two references to two secrets resolve in one call, each to its own variable.
    #[tokio::test]
    async fn two_references_to_different_secrets_both_resolve_in_one_call() {
        let bao = MockServer::start().await;
        login_ok(&bao).await;
        kv(
            &bao,
            "mqtt-mesto",
            200,
            json!({"data": {"data": {"password": PASSWORD}, "metadata": {"version": 1}}}),
        )
        .await;
        kv(
            &bao,
            "http-api",
            200,
            json!({"data": {"data": {"token": "the-http-token"}, "metadata": {"version": 3}}}),
        )
        .await;
        let resolver = resolver(&bao.uri(), token_file("two", JWT));
        let environment = resolver
            .resolve(
                Path::new("."),
                "helsinki",
                &[
                    reference("mqtt-mesto", Some("password"), "MQTT_PASSWORD"),
                    reference("http-api", None, "HTTP_TOKEN"),
                ],
            )
            .await
            .expect("both resolve");
        assert_eq!(environment.len(), 2);
        assert_eq!(environment["MQTT_PASSWORD"], PASSWORD);
        assert_eq!(environment["HTTP_TOKEN"], "the-http-token");
    }

    /// Case 11: a pipeline with no references never logs in, never reads the token file, and
    /// gets an empty environment. `environment()` returns before the backend is chosen, which is
    /// what makes the `references[0]` of `resolve_with_openbao` safe.
    #[tokio::test]
    async fn an_empty_reference_list_never_reaches_the_resolver() {
        let bao = MockServer::start().await;
        let resolver = resolver(
            &bao.uri(),
            std::env::temp_dir().join("jc-portal-t2503-never"),
        );
        let environment = resolver
            .resolve(Path::new("."), "helsinki", &[])
            .await
            .expect("nothing to resolve");
        assert!(environment.is_empty());
        let sent = bao.received_requests().await.unwrap_or_default();
        assert!(sent.is_empty(), "the store was reached: {sent:?}");
    }
}
