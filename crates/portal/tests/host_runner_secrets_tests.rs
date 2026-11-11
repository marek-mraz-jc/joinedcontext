//! A pipeline runner in the Portal's own process (`jc`, ekiben beside the Portal) takes the
//! pipeline credentials through `pipeline_secrets::hand_runner_secrets_to`, in place of the
//! Kubernetes Secret, and each project's `secretRef` resolves only from that project's SOPS
//! files (T-6227).
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
use wiremock::matchers::{method, path, query_param};
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
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: feed\n  namespace: {name}\nspec:\n  type: kafka\n  input:\n    addresses: [\"broker.example.fi:9092\"]\n    topics: [\"vehicles\"]\n    sasl:\n      password: \"${{GOLEMIO_KEY}}\"\n  secrets:\n    - {{ name: golemio, key: password, envVar: GOLEMIO_KEY }}\n"),
        ),
        (
            format!("projects/{name}/pipelines/vehicles/pipeline.yaml"),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: vehicles\n  namespace: {name}\nspec:\n  class: resident\n  enabled: true\n  targetEndpoint: \"urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-in\"\n  quotas: {{ maxMemoryMb: 128, cpuMillicores: 250 }}\n  source:\n    dataSourceRef: {{ kind: DataSource, name: feed }}\n  compute:\n    kind: bloblang\n    bloblang: \"root = this\"\n"),
        ),
    ]
}

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
