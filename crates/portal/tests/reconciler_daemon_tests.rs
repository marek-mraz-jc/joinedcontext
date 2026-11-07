//! The embedded reconciler: the election, the loop, and what reaches the mirror
//! (T-0191, CC-03, CC-08, CC-55, MF-04, MF-05, MF-06).
//!
//! The election tests run when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test
//! may write to (locally: `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise
//! they skip with a note, like the other database tests, so the fast lane without a service
//! container stays green. Everything that does not need an election runs everywhere.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::reconciler::{Leadership, SyncError, Syncer};
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";

/// A forge serving one revision of a repository, as the daemon reads it.
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

    let tree: Vec<serde_json::Value> = files
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

    for (file_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file_path}"
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

fn client(server: &MockServer) -> Arc<GiteaClient> {
    Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("gitea client"),
    )
}

fn space(name: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: {name}\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n"
    )
}

fn database_url() -> Option<String> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

/// A lock key of this test alone, so tests sharing one database never elect each other.
fn private_key(tag: u8) -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    ((nanos as i64) & 0x0000_ffff_ffff_ff00) | i64::from(tag)
}

#[tokio::test]
async fn the_advisory_lock_admits_one_replica_at_a_time() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let key = private_key(1);

    let first = Leadership::new(pool.clone(), key);
    let second = Leadership::new(pool.clone(), key);

    assert!(first.acquire().await.expect("first election"));
    assert!(first.is_leader());
    assert!(
        !second.acquire().await.expect("second election"),
        "a second replica must not win a lock another one holds"
    );
    assert!(!second.is_leader());

    // Asking again does not lose what is already held.
    assert!(first.acquire().await.expect("re-election"));

    first.resign().await;
    assert!(!first.is_leader());
    assert!(
        second.acquire().await.expect("election after resignation"),
        "the lock must be free the moment the leader gives it up"
    );
    second.resign().await;
}

#[tokio::test]
async fn a_replica_that_lost_the_election_loads_its_own_mirror_read_only() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let key = private_key(2);

    let server = forge(&[(
        "projects/ovzdusie/spaces/mobility/space.yaml",
        &space("mobility"),
    )])
    .await;
    // Two replicas, two processes, two mirrors: nothing fills a follower's but itself.
    let leader_mirror = Arc::new(Mirror::new());
    let follower_mirror = Arc::new(Mirror::new());

    let leader = Syncer::new(client(&server), Arc::clone(&leader_mirror))
        .with_leadership(Arc::new(Leadership::new(pool.clone(), key)));
    let follower_lock = Arc::new(Leadership::new(pool.clone(), key));
    let follower = Syncer::new(client(&server), Arc::clone(&follower_mirror))
        .with_leadership(Arc::clone(&follower_lock));

    assert_eq!(leader.sync_once().await.expect("the leader reconciles"), 1);
    assert!(!follower.is_ready(), "nothing loaded yet");

    assert_eq!(
        follower
            .sync_once()
            .await
            .expect("a follower is not an error"),
        1,
        "a follower loads the repository so it can serve it (OPS-51)"
    );
    assert!(!follower.is_leader());
    assert!(!follower.status().leader);
    assert_eq!(follower.status().revision.as_deref(), Some(REVISION));
    assert!(follower.is_ready());
    assert_eq!(follower_mirror.len(), 1);

    follower_lock.resign().await;
}

/// The readiness probe waits for the mirror, and the liveness probe never does (OPS-51).
#[tokio::test]
async fn the_ready_route_answers_503_until_the_first_sync_and_200_after() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use joinedcontext_portal::config::Config;
    use joinedcontext_portal::server;
    use joinedcontext_portal::state::AppState;
    use tower::ServiceExt;

    let server = forge(&[(
        "projects/ovzdusie/spaces/mobility/space.yaml",
        &space("mobility"),
    )])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(client(&server), Arc::clone(&mirror)));
    let app = server::app(
        AppState::new(Config::for_tests(), None)
            .with_gitea(client(&server))
            .with_syncer(Arc::clone(&syncer)),
    );
    let get = |uri: &'static str| {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request")
    };

    let before = app
        .clone()
        .oneshot(get("/api/v1/ready"))
        .await
        .expect("ready");
    assert_eq!(before.status(), StatusCode::SERVICE_UNAVAILABLE);
    let health = app
        .clone()
        .oneshot(get("/api/v1/health"))
        .await
        .expect("health");
    assert_eq!(
        health.status(),
        StatusCode::OK,
        "liveness does not wait for the mirror"
    );

    assert_eq!(syncer.sync_once().await.expect("sync"), 1);

    let after = app.oneshot(get("/api/v1/ready")).await.expect("ready");
    assert_eq!(after.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(after.into_body())
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(body.as_ref(), br#"{"status":"ready"}"#);
}

#[tokio::test]
async fn the_leader_reconciles_the_repository_and_the_status_says_so() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let leadership = Arc::new(Leadership::new(pool, private_key(3)));

    let server = forge(&[
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
        ("projects/ovzdusie/spaces/air/space.yaml", &space("air")),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer =
        Syncer::new(client(&server), Arc::clone(&mirror)).with_leadership(Arc::clone(&leadership));

    assert_eq!(syncer.sync_once().await.expect("reconcile"), 2);

    let status = syncer.status();
    assert!(status.leader);
    assert_eq!(status.manifests, 2);
    assert_eq!(status.revision.as_deref(), Some(REVISION));
    assert!(status.last_sync.is_some());
    assert_eq!(status.last_error, None);

    let live = mirror
        .get("ovzdusie", "ContextSpace", "mobility")
        .expect("the space is mirrored");
    assert_eq!(
        live.status
            .as_ref()
            .and_then(|s| s.observed_revision.as_deref()),
        Some(REVISION)
    );

    leadership.resign().await;
}

#[tokio::test]
async fn without_a_database_the_only_replica_leads() {
    let server = forge(&[(
        "projects/ovzdusie/spaces/mobility/space.yaml",
        &space("mobility"),
    )])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert!(
        syncer.is_leader(),
        "a Portal with no database has no election to run and reconciles alone"
    );
    assert_eq!(syncer.sync_once().await.expect("reconcile"), 1);
    assert!(syncer.status().leader);
}

/// A merge waits for the run after it, so a run must not read the whole repository again: a
/// file whose blob id the process has read is staged from memory, and only a file the tree
/// names without an id is fetched every time (PL-61, T-4270).
#[tokio::test]
async fn a_second_run_reads_only_the_files_whose_blob_it_has_not_read() {
    let server = forge(&[]).await;
    let files = [
        (
            "projects/ovzdusie/spaces/t4270-kept.yaml",
            space("t4270-kept"),
            "t4270-blob-kept",
        ),
        (
            "projects/ovzdusie/spaces/t4270-anon.yaml",
            space("t4270-anon"),
            "",
        ),
    ];
    let tree: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, _, sha)| json!({"path": p, "type": "blob", "sha": sha}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/git/trees/{REVISION}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sha": "tree-sha", "truncated": false, "tree": tree})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    for (file_path, content, _) in &files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file_path}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha",
                "content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }
    let reads = |name: &'static str| {
        let server = &server;
        async move {
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .filter(|r| r.url.path().ends_with(name))
                .count()
        }
    };
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert_eq!(syncer.sync_once().await.expect("first run"), 2);
    assert_eq!(syncer.sync_once().await.expect("second run"), 2);

    assert_eq!(
        reads("t4270-kept.yaml").await,
        1,
        "a known blob is not fetched again"
    );
    assert_eq!(
        reads("t4270-anon.yaml").await,
        2,
        "a file without a blob id is read every run"
    );
    assert!(mirror
        .get("ovzdusie", "ContextSpace", "t4270-kept")
        .is_some());
}

#[tokio::test]
async fn a_file_of_several_documents_contributes_every_one_of_them() {
    let both = format!("{}---\n{}", space("mobility"), space("air"));
    let server = forge(&[("projects/ovzdusie/spaces/spaces.yaml", &both)]).await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert_eq!(
        syncer.sync_once().await.expect("reconcile"),
        2,
        "the loader reads multi-document YAML, so one file may declare several resources"
    );
    assert!(mirror.get("ovzdusie", "ContextSpace", "mobility").is_some());
    assert!(mirror.get("ovzdusie", "ContextSpace", "air").is_some());
}

#[tokio::test]
async fn two_files_claiming_one_identity_stop_the_run_and_keep_the_mirror() {
    let server = forge(&[
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
        (
            "projects/ovzdusie/spaces/copy/space.yaml",
            &space("mobility"),
        ),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    let err = syncer
        .sync_once()
        .await
        .expect_err("two manifests of one identity are not a repository");
    match err {
        SyncError::Load(jcctl::loader::LoadError::DuplicateIdentity { id, .. }) => {
            assert_eq!(id.name, "mobility");
        }
        other => panic!("expected a duplicate identity, got {other:?}"),
    }
    assert!(
        mirror.is_empty(),
        "a repository that does not load leaves the mirror as it was"
    );
    assert!(syncer.status().last_error.is_some());
}

#[tokio::test]
async fn a_blueprint_reaches_the_mirror_for_the_flow_gallery() {
    let blueprint = "apiVersion: joinedcontext.com/v1alpha1\nkind: Blueprint\nmetadata:\n  name: threshold-alert\n  namespace: org\nspec:\n  version: 1.2.0\n  riskClass: green\n  allowedRoles: [domain-editor]\n  parameterSchema:\n    type: object\n  templates:\n    - name: subscription\n      template: |\n        apiVersion: joinedcontext.com/v1alpha1\n";
    let server = forge(&[
        ("blueprints/threshold-alert/blueprint.yaml", blueprint),
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert_eq!(syncer.sync_once().await.expect("reconcile"), 2);
    assert!(
        mirror.get("org", "Blueprint", "threshold-alert").is_some(),
        "the gallery reads blueprints from the mirror like any other resource"
    );
}

/// T-0925, PF-32: the reader credential the run mints reaches the namespace that serves.
///
/// Both halves are mocked, because both are the point: the store's admin API accepts the three
/// calls that make the pair exist, and the API server receives the Secret the serving workloads
/// read. The Secret carries the reader and nothing that could write an artifact.
#[tokio::test]
async fn the_run_hands_the_reader_credential_to_the_namespace_that_serves() {
    const ROOT: &str = "the-root-secret-nobody-else-holds";
    let organization = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";
    let space = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
    let server = forge(&[
        ("org.yaml", organization),
        ("projects/ovzdusie/spaces/mobility/space.yaml", space),
    ])
    .await;

    let store_api = MockServer::start().await;
    for route in [
        "/rustfs/admin/v3/add-canned-policy",
        "/rustfs/admin/v3/add-user",
        "/rustfs/admin/v3/set-user-or-group-policy",
    ] {
        Mock::given(method("PUT"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200))
            .mount(&store_api)
            .await;
    }
    let store = joinedcontext_portal::artifact_store::Client::new(
        joinedcontext_portal::artifact_store::Settings {
            endpoint: store_api.uri(),
            bucket: "jc-artifacts".to_owned(),
            region: "us-east-1".to_owned(),
            root_access_key: "jc-root".to_owned(),
            root_secret_key: ROOT.to_owned(),
        },
    )
    .expect("a store client");

    let kube_api = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/namespaces/dev/secrets/artifact-store-reader-hel",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kind": "Secret"})))
        .mount(&kube_api)
        .await;
    let kube = joinedcontext_portal::apps::kube::KubeClient::with_token(&kube_api.uri(), "token")
        .expect("a kube client");

    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror))
        .with_artifact_store(Arc::new(store))
        .with_credential_secrets(Arc::new(kube), "dev");
    syncer.sync_once().await.expect("the run loads");

    let written = kube_api
        .received_requests()
        .await
        .expect("the API server was asked")
        .into_iter()
        .find(|r| r.url.path().ends_with("/secrets/artifact-store-reader-hel"))
        .expect("the reader Secret was applied");
    let body: serde_json::Value = serde_json::from_slice(&written.body).expect("a Secret");
    let reader = joinedcontext_portal::artifact_store::Credential::derive(
        ROOT,
        "hel",
        joinedcontext_portal::artifact_store::Role::Reader,
    );
    let writer = joinedcontext_portal::artifact_store::Credential::derive(
        ROOT,
        "hel",
        joinedcontext_portal::artifact_store::Role::Writer,
    );
    assert_eq!(body["stringData"]["ACCESS_KEY_ID"], "jc-hel-reader");
    assert_eq!(body["stringData"]["ACCESS_SECRET_KEY"], reader.secret_key);
    let serialised = body.to_string();
    assert!(!serialised.contains(&writer.secret_key), "the writer's key");
    assert!(!serialised.contains(ROOT), "the root secret");
}

/// Edge cases of one pass: the leader order, the pipelines' secrets and what reaches the mirror
/// (T-2504, PL-15, MF-04, MF-44, CC-03).
mod pass_edges {
    use super::*;
    use joinedcontext_portal::apps::kube::KubeClient;
    use joinedcontext_portal::pipeline_secrets::{Backend, Resolver};
    use joinedcontext_portal::reconciler::streams::StreamDeployer;
    use joinedcontext_portal::sync::webhook_secrets::Accepted;
    use serde_json::Value;

    const PASSWORD: &str = "the-mqtt-password-nobody-else-has";
    const ORGANIZATION: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";

    fn space_of(project: &str) -> String {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: {project}\nspec:\n  isSandbox: true\n")
    }

    /// A DataSource that names `secret` for its password, or no secret at all.
    fn data_source(project: &str, name: &str, secret: Option<&str>) -> String {
        let secrets = match secret {
            Some(secret) => format!(
                "  secrets:\n    - {{ name: {secret}, key: password, envVar: {} }}\n",
                name.to_uppercase().replace('-', "_")
            ),
            None => String::new(),
        };
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: {name}\n  namespace: {project}\nspec:\n  type: mqtt\n  input:\n    urls: [\"tcp://broker.example.fi:1883\"]\n    topics: [\"vehicles/#\"]\n{secrets}"
        )
    }

    fn pipeline(project: &str, name: &str, source: &str) -> String {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: {name}\n  namespace: {project}\nspec:\n  class: resident\n  enabled: true\n  targetEndpoint: \"urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-in\"\n  quotas: {{ maxMemoryMb: 128, cpuMillicores: 250 }}\n  source:\n    dataSourceRef: {{ kind: DataSource, name: {source} }}\n  compute:\n    kind: bloblang\n    bloblang: \"root = this\"\n"
        )
    }

    /// One project's space, and a pipeline per `(pipeline, secret)` reading a DataSource of its
    /// own that names that secret.
    fn project(name: &str, pipelines: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
        let mut files = vec![(
            format!("projects/{name}/spaces/mobility/space.yaml"),
            space_of(name),
        )];
        for (pipe, secret) in pipelines {
            let source = format!("{pipe}-source");
            files.push((
                format!("projects/{name}/datasources/{source}.yaml"),
                data_source(name, &source, *secret),
            ));
            files.push((
                format!("projects/{name}/pipelines/{pipe}/pipeline.yaml"),
                pipeline(name, pipe, &source),
            ));
        }
        files
    }

    async fn serve(projects: &[Vec<(String, String)>]) -> MockServer {
        let mut files = vec![("org.yaml".to_owned(), ORGANIZATION.to_owned())];
        files.extend(projects.iter().flatten().cloned());
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        forge(&borrowed).await
    }

    /// An OpenBao that logs in and holds `mqtt-mesto` and `sync-hook`; any other name is absent.
    async fn openbao() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/auth/kubernetes/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "auth": { "client_token": "a-session", "lease_duration": 3600, "renewable": true }
            })))
            .mount(&server)
            .await;
        for (name, key, value) in [
            ("mqtt-mesto", "password", PASSWORD),
            ("sync-hook", "secret", "the-hook-secret"),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/v1/secret/data/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "data": { key: value }, "metadata": { "version": 1 } }
                })))
                .mount(&server)
                .await;
        }
        server
    }

    fn resolver(bao: &MockServer) -> Resolver {
        // One file per mock server: the tests run in parallel, and a shared file read while
        // another test's write had truncated it is empty, which the resolver refuses (T-3004).
        let jwt =
            std::env::temp_dir().join(format!("jc-portal-t2504-token-{}", bao.address().port()));
        std::fs::write(&jwt, "a.service.account.token").expect("write the token");
        Resolver::new(Backend::OpenBao {
            address: bao.uri(),
            role: "portal".to_owned(),
            jwt_path: jwt,
        })
    }

    /// An API server answering every apply with `status`.
    async fn cluster(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"kind": "Status"})))
            .mount(&server)
            .await;
        server
    }

    /// A runner that accepts every stream.
    async fn runner() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        for verb in ["PUT", "POST", "DELETE"] {
            Mock::given(method(verb))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .mount(&server)
                .await;
        }
        server
    }

    fn kube(cluster: &MockServer) -> Arc<KubeClient> {
        Arc::new(KubeClient::with_token(&cluster.uri(), "token").expect("a kube client"))
    }

    fn deployer(runner: &MockServer) -> Arc<StreamDeployer> {
        Arc::new(StreamDeployer::new(format!("{}/{{project}}", runner.uri())))
    }

    async fn calls(server: &MockServer, verb: &str, ends_with: &str) -> usize {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == verb && r.url.path().ends_with(ends_with))
            .count()
    }

    /// The `StreamDeployed` condition of a pipeline: `(reason, message)`.
    fn condition(mirror: &Mirror, project: &str, name: &str) -> (String, String) {
        let pipeline = mirror
            .get(project, "Pipeline", name)
            .expect("the pipeline is in the mirror");
        let status = serde_json::to_value(pipeline.status).expect("a status");
        let condition = status["conditions"]
            .as_array()
            .and_then(|all| all.first())
            .cloned()
            .unwrap_or(Value::Null);
        (
            condition["reason"].as_str().unwrap_or_default().to_owned(),
            condition["message"].as_str().unwrap_or_default().to_owned(),
        )
    }

    /// T-2891, AP-112: a runner that holds a stream PUT open (Bento waits for a stream that
    /// cannot stop) does not hold the edge file back: the edge step reads its base while that
    /// PUT is still unanswered, in the same pass.
    #[tokio::test]
    async fn the_edge_file_converges_while_a_stream_put_is_still_unanswered() {
        use joinedcontext_portal::apps::reconciler::Settings;
        use joinedcontext_portal::reconciler::edge_file::EdgeFile;
        use std::time::{Duration, Instant};

        let mut files = project("helsinki", &[("vehicles", None)]);
        files.push((
            "projects/helsinki/endpoints/vehicles-in.yaml".to_owned(),
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: vehicles-in\n  namespace: helsinki\nspec:\n  contextSpaceRef: mobility\n  slug: mluyob4nz52lok3ssk7pgn5vwt\n  audience: internal\n  enabledRepresentations:\n    - ngsi-ld\n".to_owned(),
        ));
        let forge = serve(&[files]).await;
        let runner = MockServer::start().await;
        // Each project's write budget, declared before its streams (T-5803).
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex("/resources/rate_limit/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&runner)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&runner)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(20)))
            .mount(&runner)
            .await;
        let cluster = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/namespaces/apisix/configmaps/apisix-standalone-base",
            ))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"kind": "Status"})))
            .mount(&cluster)
            .await;
        let edge = Arc::new(EdgeFile::new(
            KubeClient::with_token(&cluster.uri(), "token").expect("a kube client"),
            "apisix".to_owned(),
        ));
        let settings = Settings {
            host: "hel.example".into(),
            apex: "hel.example".into(),
            gateway_url: None,
            service_url: None,
            namespace: "jc".into(),
            org_domain: "hel.fi".into(),
            apisix_namespace: "apisix".into(),
            image_repository: None,
            pull_secret: None,
            basemap_base: None,
            release: None,
            service_account: None,
        };
        let syncer = Syncer::new(client(&forge), Arc::new(Mirror::new()))
            .with_streams(deployer(&runner))
            .with_edge_file(edge, settings);
        let run = tokio::spawn(async move { syncer.sync_once().await });

        let started = Instant::now();
        while calls(&cluster, "GET", "/configmaps/apisix-standalone-base").await == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the edge step waited for the stream PUT"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !run.is_finished(),
            "the pass ended before the runner answered the PUT, so this proves nothing"
        );
        run.abort();
    }

    /// Case 3: a reference the store does not hold refuses its pipeline by name, and the runner
    /// is never asked to start it.
    #[tokio::test]
    async fn a_pipeline_with_an_unresolvable_reference_is_refused_and_named_not_deployed() {
        let forge = serve(&[project("helsinki", &[("vehicles", Some("no-such"))])]).await;
        let (bao, cluster, runner) = (openbao().await, cluster(200).await, runner().await);
        let mirror = Arc::new(Mirror::new());
        Syncer::new(client(&forge), Arc::clone(&mirror))
            .with_pipeline_secrets(resolver(&bao))
            .with_credential_secrets(kube(&cluster), "dev")
            .with_streams(deployer(&runner))
            .sync_once()
            .await
            .expect("the run loads");

        let (reason, message) = condition(&mirror, "helsinki", "vehicles");
        assert_eq!(reason, "SecretUnresolved", "{message}");
        assert!(message.contains("no-such"), "{message}");
        assert_eq!(calls(&runner, "PUT", "/streams/vehicles").await, 0);
        assert_eq!(calls(&runner, "POST", "/streams/vehicles").await, 0);
        assert_eq!(
            calls(&cluster, "PATCH", "/secrets/pipeline-secrets").await,
            0,
            "nothing resolved, so no Secret is written"
        );
    }

    /// Case 4: two projects with a pipeline of the same name; one reference fails, and only that
    /// project's pipeline carries the refusal.
    #[tokio::test]
    async fn two_pipelines_of_different_namespaces_never_share_a_refused_reason() {
        let forge = serve(&[
            project("helsinki", &[("vehicles", Some("mqtt-mesto"))]),
            project("espoo", &[("vehicles", Some("no-such"))]),
        ])
        .await;
        let (bao, cluster) = (openbao().await, cluster(200).await);
        let mirror = Arc::new(Mirror::new());
        Syncer::new(client(&forge), Arc::clone(&mirror))
            .with_pipeline_secrets(resolver(&bao))
            .with_credential_secrets(kube(&cluster), "dev")
            .sync_once()
            .await
            .expect("the run loads");

        let (espoo, said) = condition(&mirror, "espoo", "vehicles");
        assert_eq!(espoo, "SecretUnresolved");
        assert!(said.contains("no-such"), "{said}");
        let (helsinki, said) = condition(&mirror, "helsinki", "vehicles");
        assert_ne!(helsinki, "SecretUnresolved", "{said}");
        assert!(!said.contains("no-such"), "{said}");
    }

    /// Case 5: two pipelines' credentials go into one Secret, written once per pass.
    #[tokio::test]
    async fn resolve_pipeline_secrets_writes_the_runner_secret_only_once_per_pass() {
        let forge = serve(&[project(
            "helsinki",
            &[
                ("vehicles", Some("mqtt-mesto")),
                ("buses", Some("mqtt-mesto")),
            ],
        )])
        .await;
        let (bao, cluster) = (openbao().await, cluster(200).await);
        Syncer::new(client(&forge), Arc::new(Mirror::new()))
            .with_pipeline_secrets(resolver(&bao))
            .with_credential_secrets(kube(&cluster), "dev")
            .sync_once()
            .await
            .expect("the run loads");

        assert_eq!(
            calls(&cluster, "PATCH", "/secrets/pipeline-secrets").await,
            1
        );
        let secret: Value = cluster
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .find(|r| r.url.path().ends_with("/secrets/pipeline-secrets"))
            .map(|r| serde_json::from_slice(&r.body).expect("a json body"))
            .expect("the Secret");
        assert_eq!(secret["stringData"]["VEHICLES_SOURCE"], PASSWORD);
        assert_eq!(secret["stringData"]["BUSES_SOURCE"], PASSWORD);
    }

    /// Case 6: with no backend, only the pipeline that declares a reference is refused.
    #[tokio::test]
    async fn no_pipeline_secrets_backend_configured_refuses_only_pipelines_that_declare_a_reference(
    ) {
        let forge = serve(&[project(
            "helsinki",
            &[("vehicles", Some("mqtt-mesto")), ("counts", None)],
        )])
        .await;
        let mirror = Arc::new(Mirror::new());
        Syncer::new(client(&forge), Arc::clone(&mirror))
            .sync_once()
            .await
            .expect("the run loads");

        let (reason, message) = condition(&mirror, "helsinki", "vehicles");
        assert_eq!(reason, "SecretUnresolved");
        assert!(message.contains("no secret backend"), "{message}");
        let (reason, message) = condition(&mirror, "helsinki", "counts");
        assert_ne!(reason, "SecretUnresolved", "{message}");
    }

    /// Case 7 (T-2522): a credential that resolved but never reached the runner's Secret, because
    /// the Portal has no cluster or the API server refused the write, refuses the pipeline. A
    /// stream started without it fails in the runner's log, where nobody looks.
    #[tokio::test]
    async fn a_credential_that_never_reached_the_runner_refuses_its_pipeline() {
        for refused_write in [None, Some(422u16)] {
            let mut files = project("helsinki", &[("vehicles", Some("mqtt-mesto"))]);
            files.push((
                "projects/helsinki/endpoints/vehicles-in.yaml".to_owned(),
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: vehicles-in\n  namespace: helsinki\nspec:\n  contextSpaceRef: mobility\n  slug: mluyob4nz52lok3ssk7pgn5vwt\n  audience: internal\n  enabledRepresentations:\n    - ngsi-ld\n".to_owned(),
            ));
            let forge = serve(&[files]).await;
            let (bao, runner) = (openbao().await, runner().await);
            let mirror = Arc::new(Mirror::new());
            let mut syncer = Syncer::new(client(&forge), Arc::clone(&mirror))
                .with_pipeline_secrets(resolver(&bao))
                .with_streams(deployer(&runner));
            let cluster = match refused_write {
                Some(status) => Some(cluster(status).await),
                None => None,
            };
            if let Some(cluster) = cluster.as_ref() {
                syncer = syncer.with_credential_secrets(kube(cluster), "dev");
            }
            syncer
                .sync_once()
                .await
                .expect("the run loads, and is not an error");

            let (reason, message) = condition(&mirror, "helsinki", "vehicles");
            assert_eq!(reason, "SecretUnresolved", "{refused_write:?}: {message}");
            assert!(!message.contains(PASSWORD), "{message}");
            assert_eq!(calls(&runner, "PUT", "/streams/vehicles").await, 0);
            assert_eq!(calls(&runner, "POST", "/streams/vehicles").await, 0);
        }
    }

    /// Cases 8 and 9: a manifest the Portal has no view for does not cost the rest, and one
    /// without a namespace takes its project from its path.
    #[tokio::test]
    async fn an_unknown_kind_costs_nothing_and_a_manifest_without_a_namespace_takes_its_path() {
        let unnamespaced = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: parking\nspec:\n  isSandbox: true\n";
        let forge = serve(&[vec![
            (
                "projects/helsinki/spaces/mobility/space.yaml".to_owned(),
                space_of("helsinki"),
            ),
            (
                "projects/helsinki/spaces/parking/space.yaml".to_owned(),
                unnamespaced.to_owned(),
            ),
            (
                "projects/helsinki/gizmos/one.yaml".to_owned(),
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Gizmo\nmetadata:\n  name: one\n  namespace: helsinki\nspec: {}\n".to_owned(),
            ),
        ]])
        .await;
        let mirror = Arc::new(Mirror::new());
        Syncer::new(client(&forge), Arc::clone(&mirror))
            .sync_once()
            .await
            .expect("the run loads");
        assert!(mirror.get("helsinki", "ContextSpace", "parking").is_some());
        assert!(mirror.get("helsinki", "ContextSpace", "mobility").is_some());
    }

    /// Case 10: a repository with nothing of a known kind is a named error, and the mirror a
    /// reader sees stays what it was.
    #[tokio::test]
    async fn loaded_zero_resources_is_a_named_error_not_an_empty_success() {
        let forge = forge(&[("README.md", "# nothing here yet\n")]).await;
        let mirror = Arc::new(Mirror::new());
        let err = Syncer::new(client(&forge), Arc::clone(&mirror))
            .sync_once()
            .await
            .expect_err("an empty repository is not a success");
        assert!(matches!(err, SyncError::Empty(_)), "{err}");
        assert!(mirror.is_empty());
    }

    /// Case 11: every resource of a pass carries the commit the pass staged.
    #[tokio::test]
    async fn the_observed_revision_on_every_resource_matches_the_staged_commit() {
        let forge = serve(&[
            project("helsinki", &[("counts", None)]),
            project("espoo", &[]),
        ])
        .await;
        let mirror = Arc::new(Mirror::new());
        Syncer::new(client(&forge), Arc::clone(&mirror))
            .sync_once()
            .await
            .expect("the run loads");
        let all = mirror.matching(|_| true);
        assert!(all.len() >= 5, "{}", all.len());
        for resource in all {
            let revision = resource
                .status
                .as_ref()
                .and_then(|s| s.observed_revision.clone());
            assert_eq!(
                revision.as_deref(),
                Some(REVISION),
                "{}",
                resource.metadata.name
            );
        }
    }

    /// T-4599: a replica that served as follower and then takes the lock drops the follower
    /// stamp at the start of its stream wave, not when the wave ends: while the runner still
    /// holds the PUT, the mirror already serves this run's revision and the pipeline says the
    /// leader is deploying it; once the runner answers, it is Live.
    #[tokio::test]
    async fn a_replica_that_takes_the_lock_replaces_the_follower_stamp_before_its_streams_answer() {
        use std::time::{Duration, Instant};
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = joinedcontext_portal::db::connect(&url)
            .await
            .expect("connect and migrate");
        let key = private_key(4);
        let holder = Leadership::new(pool.clone(), key);
        assert!(holder.acquire().await.expect("the old pod leads"));

        let mut files = project("helsinki", &[("vehicles", None)]);
        files.push((
            "projects/helsinki/endpoints/vehicles-in.yaml".to_owned(),
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: vehicles-in\n  namespace: helsinki\nspec:\n  contextSpaceRef: mobility\n  slug: mluyob4nz52lok3ssk7pgn5vwt\n  audience: internal\n  enabledRepresentations:\n    - ngsi-ld\n".to_owned(),
        ));
        let forge = serve(&[files]).await;
        let runner = MockServer::start().await;
        // Each project's write budget, declared before its streams (T-5803).
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex("/resources/rate_limit/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&runner)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&runner)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&runner)
            .await;
        let mirror = Arc::new(Mirror::new());
        let lock = Arc::new(Leadership::new(pool.clone(), key));
        let syncer = Arc::new(
            Syncer::new(client(&forge), Arc::clone(&mirror))
                .with_streams(deployer(&runner))
                .with_leadership(Arc::clone(&lock)),
        );

        syncer.sync_once().await.expect("a follower's run");
        assert!(!lock.is_leader());
        assert_eq!(condition(&mirror, "helsinki", "vehicles").0, "Follower");

        holder.resign().await;
        let leading = Arc::clone(&syncer);
        let run = tokio::spawn(async move { leading.sync_once().await });
        let started = Instant::now();
        while calls(&runner, "PUT", "").await == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the leader never sent the stream"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !run.is_finished(),
            "the runner answered before the mirror was read, so this proves nothing"
        );
        assert!(lock.is_leader());
        assert_eq!(condition(&mirror, "helsinki", "vehicles").0, "Deploying");

        run.await.expect("the run ends").expect("a leader's run");
        let vehicles = mirror
            .get("helsinki", "Pipeline", "vehicles")
            .and_then(|pipeline| pipeline.status)
            .expect("a status");
        assert_eq!(vehicles.phase, joinedcontext_portal::resource::Phase::Live);
        assert!(
            !vehicles
                .conditions
                .iter()
                .any(|c| c.reason.as_deref() == Some("Deploying")),
            "{:?}",
            vehicles.conditions
        );
        lock.resign().await;
    }

    /// Cases 1 and 2: a follower resolves the webhook secrets any replica may be asked to check,
    /// and never a pipeline's: no read of a pipeline credential, no Secret written.
    #[tokio::test]
    async fn a_follower_resolves_webhook_secrets_and_never_a_pipelines() {
        let Some(url) = database_url() else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let pool = joinedcontext_portal::db::connect(&url)
            .await
            .expect("connect and migrate");
        let key = private_key(3);
        let holder = Leadership::new(pool.clone(), key);
        assert!(holder.acquire().await.expect("another replica leads"));

        let driven = "apiVersion: joinedcontext.com/v1alpha1\nkind: SyncSource\nmetadata:\n  name: regional\n  namespace: helsinki\nspec:\n  source:\n    git: { url: https://git.region.sk/udp/models.git, ref: main }\n  schedule: { webhook: true }\n  webhook:\n    secretRef: { name: sync-hook, key: secret }\n  mode: mirror\n  conflictPolicy: replace\n";
        let mut files = project("helsinki", &[("vehicles", Some("mqtt-mesto"))]);
        files.push((
            "projects/helsinki/syncsources/regional.yaml".to_owned(),
            driven.to_owned(),
        ));
        let forge = serve(&[files]).await;
        let (bao, cluster) = (openbao().await, cluster(200).await);
        let accepted = Arc::new(Accepted::new());
        let follower_lock = Arc::new(Leadership::new(pool.clone(), key));
        let loaded = Syncer::new(client(&forge), Arc::new(Mirror::new()))
            .with_pipeline_secrets(resolver(&bao))
            .with_credential_secrets(kube(&cluster), "dev")
            .with_webhook_secrets(Arc::clone(&accepted))
            .with_leadership(Arc::clone(&follower_lock))
            .sync_once()
            .await
            .expect("a follower is not an error");
        assert!(loaded > 0);
        assert!(!follower_lock.is_leader());

        assert_eq!(
            accepted.len(),
            1,
            "the follower holds the source's hook secret"
        );
        assert_eq!(calls(&bao, "GET", "/sync-hook").await, 1);
        assert_eq!(
            calls(&bao, "GET", "/mqtt-mesto").await,
            0,
            "a follower read a pipeline's credential"
        );
        assert!(
            cluster
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "a follower wrote to the cluster"
        );
        holder.resign().await;
    }
}
