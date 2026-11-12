//! A pipeline runner in the Portal's own process (`jc`, ekiben beside the Portal) takes the
//! pipeline credentials through `pipeline_secrets::hand_runner_secrets_to`, in place of the
//! Kubernetes Secret, and each project's `secretRef` resolves only from that project's SOPS
//! files (T-6227). A project made after jc started gets its pipeline client from the workload
//! wave and its secret on the runner in the same sync (T-6832).
//!
//! Its own binary: the sink is process-wide, and `reconciler_daemon_tests` checks that a Portal
//! without a cluster or a host runner refuses a resolved credential.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use age::secrecy::ExposeSecret as _;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::pipeline_secrets::{
    hand_runner_secrets_to, Backend, Resolver, RunnerSecrets,
};
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const VALUE: &str = "the-golemio-key-only-praha-has";
const ORGANIZATION: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";

async fn forge(files: &[(String, String)]) -> MockServer {
    const REPO: &str = "/odata/Repositories(Owner='test-owner',Name='test-repo')";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"DefaultBranch": "main"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('main')")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Name": "main", "HeadSha": REVISION})),
        )
        .mount(&server)
        .await;
    let tree: Vec<Value> = files
        .iter()
        .map(|(p, _)| json!({"Path": p, "Type": "blob"}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", format!("'{REVISION}'")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Sha": "tree-sha", "Truncated": false, "Tree": tree})),
        )
        .mount(&server)
        .await;
    for (file_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file_path}'")))
            .and(query_param("@r", format!("'{REVISION}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "blob-sha",
                "Content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }
    server
}

/// One project's space, a DataSource naming the secret `golemio` as `GOLEMIO_KEY`, and the
/// pipeline reading it.
fn project(name: &str) -> Vec<(String, String)> {
    vec![
        // The Endpoint the pipeline writes through, so the Portal's judge passes it (PL-20).
        (
            format!("projects/{name}/spaces/mobility/endpoints/vehicles-in.yaml"),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: vehicles-in\n  namespace: {name}\nspec:\n  contextSpaceRef: mobility\n  slug: {}\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n", name.chars().filter(char::is_ascii_lowercase).chain(std::iter::repeat('a')).take(26).collect::<String>()),
        ),
        (
            format!("projects/{name}/spaces/mobility/space.yaml"),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: {name}\nspec:\n  isSandbox: true\n"),
        ),
        (
            format!("projects/{name}/datasources/feed.yaml"),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: feed\n  namespace: {name}\nspec:\n  type: nats\n  input:\n    urls: [\"nats://broker.example.fi:4222\"]\n    subject: vehicles\n    auth:\n      user_jwt: \"${{GOLEMIO_KEY}}\"\n  secrets:\n    - {{ name: golemio, key: password, envVar: GOLEMIO_KEY }}\n"),
        ),
        (
            format!("projects/{name}/pipelines/vehicles/pipeline.yaml"),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: vehicles\n  namespace: {name}\nspec:\n  class: resident\n  enabled: true\n  targetEndpoint: \"urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-in\"\n  quotas: {{ maxMemoryMb: 128, cpuMillicores: 250 }}\n  source:\n    dataSourceRef: {{ kind: DataSource, name: feed }}\n  compute:\n    kind: bloblang\n    bloblang: \"root = this\"\n"),
        ),
    ]
}

/// The sink is process-wide: one test hands it its runner at a time.
static SINK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The runner's environment as the last sync handed it.
#[derive(Default)]
struct Recorded(Mutex<Vec<BTreeMap<String, String>>>);

impl RunnerSecrets for Recorded {
    fn replace(&self, values: BTreeMap<String, String>) {
        self.0.lock().unwrap().push(values);
    }
}

fn condition(mirror: &Mirror, project: &str) -> (String, String) {
    let pipeline = mirror
        .get(project, "Pipeline", "vehicles")
        .expect("the pipeline is in the mirror");
    let status = serde_json::to_value(pipeline.status).expect("a status");
    let first = status["conditions"]
        .as_array()
        .and_then(|all| all.first())
        .cloned()
        .unwrap_or(Value::Null);
    (
        first["reason"].as_str().unwrap_or_default().to_owned(),
        first["message"].as_str().unwrap_or_default().to_owned(),
    )
}

#[tokio::test]
async fn a_host_runner_gets_a_projects_own_secret_and_never_another_projects() {
    let _sink = SINK.lock().await;
    let identity = age::x25519::Identity::generate();
    let encrypted = joinedcontext_portal::secret_files::encrypt(
        "golemio",
        &BTreeMap::from([(
            "password".to_owned(),
            zeroize::Zeroizing::new(VALUE.to_owned()),
        )]),
        &[identity.to_public().to_string()],
        "2026-11-08T00:00:00Z",
    )
    .expect("encrypted");
    // praha holds the secret; brno names one of the same name and holds none.
    let mut files = vec![("org.yaml".to_owned(), ORGANIZATION.to_owned())];
    files.extend(project("praha"));
    files.extend(project("brno"));
    files.push((
        "projects/praha/secrets/golemio.enc.yaml".to_owned(),
        encrypted,
    ));
    let forge = forge(&files).await;
    let key = std::env::temp_dir().join(format!("jc-host-runner-age-{}", std::process::id()));
    std::fs::write(&key, identity.to_string().expose_secret()).expect("key");

    let recorded = Arc::new(Recorded::default());
    hand_runner_secrets_to(recorded.clone());
    let mirror = Arc::new(Mirror::new());
    let client = Arc::new(
        ShakoClient::new(
            forge.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("shako client"),
    );
    Syncer::new(client, Arc::clone(&mirror))
        .with_pipeline_secrets(Resolver::new(Backend::Sops {
            age_key_file: key.clone(),
        }))
        .sync_once()
        .await
        .expect("the run loads");
    std::fs::remove_file(&key).ok();

    let handed = recorded.0.lock().unwrap().last().cloned().expect("handed");
    assert_eq!(handed.get("GOLEMIO_KEY").map(String::as_str), Some(VALUE));
    assert_eq!(handed.len(), 1, "{:?}", handed.keys());

    let (reason, message) = condition(&mirror, "brno");
    assert_eq!(reason, "SecretUnresolved", "{message}");
    assert!(message.contains("golemio"), "{message}");
    assert!(!message.contains(VALUE), "{message}");
    let (reason, message) = condition(&mirror, "praha");
    assert_ne!(reason, "SecretUnresolved", "{message}");
    assert!(!message.contains(VALUE), "{message}");
}

/// A realm whose admin API holds, for each `{project}-pipelines` asked for, the client the
/// workload wave derives (T-6832), with the secret `generated-{project}`.
async fn realm() -> MockServer {
    use joinedcontext_portal::reconciler::workload_clients::{desired_external, ClientCredential};
    const REALM: &str = "/admin/realms/hel";
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/hel/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(|request: &wiremock::Request| {
            let asked = request
                .url
                .query_pairs()
                .find(|(k, _)| k == "clientId")
                .map(|(_, v)| v.into_owned());
            let found = asked
                .and_then(|id| id.strip_suffix("-pipelines").map(str::to_owned))
                .map(|project| {
                    let mut client =
                        desired_external(&project, "pipelines", &ClientCredential::Secret);
                    client["id"] = json!(format!("uuid-{project}"));
                    json!([client])
                })
                .unwrap_or_else(|| json!([]));
            ResponseTemplate::new(200).set_body_json(found)
        })
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "^{REALM}/clients/uuid-[a-z]+/client-secret$"
        )))
        .respond_with(|request: &wiremock::Request| {
            let project = request
                .url
                .path()
                .split("/uuid-")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default()
                .to_owned();
            ResponseTemplate::new(200)
                .set_body_json(json!({ "type": "secret", "value": format!("generated-{project}") }))
        })
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "^{REALM}/clients/[^/]+/default-client-scopes$"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "id": "s-basic", "name": "basic" }])),
        )
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "^{REALM}/clients/[^/]+/(optional-client-scopes|protocol-mappers/models)$"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(path_regex(format!("^{REALM}/clients/[^/]+(/.*)?$")))
        .respond_with(ResponseTemplate::new(204))
        .with_priority(10)
        .mount(&keycloak)
        .await;
    keycloak
}

/// T-6832, the jc-dev failure of T-6829: projects made after the realm import (praha and brno,
/// two Organizations' alike, neither declaring a `pipelines` account) had no client, so every
/// sample, mapping test and stream of theirs met `JC_CLIENT_SECRET_<PROJECT> was not defined`.
/// One sync now hands the runner each project's own client secret beside the pipelines'
/// credentials, without a restart; no project's variable holds another's secret.
#[tokio::test]
async fn a_project_made_after_start_has_its_pipeline_client_secret_on_the_runner() {
    let _sink = SINK.lock().await;
    use joinedcontext_portal::config::ClientAuth;
    use joinedcontext_portal::reconciler::workload_clients::WorkloadClientSync;
    let mut files = vec![("org.yaml".to_owned(), ORGANIZATION.to_owned())];
    files.extend(project("praha"));
    files.extend(project("brno"));
    let forge = forge(&files).await;
    let keycloak = realm().await;
    let clients = WorkloadClientSync::new(
        &format!("{}/realms/hel", keycloak.uri()),
        "portal-api".to_owned(),
        ClientAuth::Secret("portal-api-credential".into()),
    )
    .expect("a realm issuer")
    .with_host_runner(true);

    let recorded = Arc::new(Recorded::default());
    hand_runner_secrets_to(recorded.clone());
    let client = Arc::new(
        ShakoClient::new(
            forge.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("shako client"),
    );
    Syncer::new(client, Arc::new(Mirror::new()))
        .with_workload_clients(Arc::new(clients))
        .sync_once()
        .await
        .expect("the run loads");

    let handed = recorded.0.lock().unwrap().last().cloned().expect("handed");
    assert_eq!(
        handed.get("JC_CLIENT_SECRET_PRAHA").map(String::as_str),
        Some("generated-praha")
    );
    assert_eq!(
        handed.get("JC_CLIENT_SECRET_BRNO").map(String::as_str),
        Some("generated-brno")
    );
    assert_eq!(handed.len(), 2, "{:?}", handed.keys());
}
