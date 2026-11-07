//! A project's own namespace for its pod-backed Apps (T-2680; AP-116, AP-117, ADR-N-030 §6).
//!
//! The converger runs against a stubbed API server, so what is asserted is what leaves the
//! Portal: the first pod-backed App of a project creates `{release}-{project}-apps` with its
//! labels, Pod Security `restricted`, a default-deny policy admitting APISIX only, the pull
//! Secret and the Portal's RoleBinding, before the App's objects go into it; in the Portal's own
//! namespace it only reads the pull Secret; the last pod-backed App retired removes the namespace;
//! and a repository that holds no retired App never removes one.

use std::path::PathBuf;

use jcctl::loader::Repository;
use joinedcontext_portal::apps::converge::{Converger, Outcome};
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::apps::reconciler::Settings;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const TOKEN: &str = "the-projected-service-account-token-of-the-portal";
const NS: &str = "/api/v1/namespaces/dev-ovzdusie-apps";
const PULL: &str = "/api/v1/namespaces/joinedcontext/secrets/app-registry";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        apex: "bb.example.com".into(),
        gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
        service_url: Some("http://portal.joinedcontext.svc.cluster.local:8080".into()),
        namespace: "joinedcontext".into(),
        org_domain: "banskabystrica.sk".into(),
        apisix_namespace: "apisix".into(),
        image_repository: Some("forge.bb.example.com/joinedcontext".into()),
        pull_secret: Some("app-registry".into()),
        basemap_base: None,
        release: Some("dev".into()),
        service_account: Some("portal".into()),
    }
}

fn app(name: &str, project: &str, class: &str, lifecycle: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: {name}
  namespace: {project}
spec:
  kind: {class}
  source:
    path: ./src
  build:
    rust: \"1.90\"
    node: \"22\"
  visibility: project
  lifecycle: {lifecycle}
  dataNeeds:
    - contextSpaceRef:
        kind: ContextSpace
        name: {project}
      types: [AirQualityObserved]
      attrs: [pm10]
      operations: [queryEntity]
      representations: [ngsi-ld]
status:
  build:
    digest: \"{DIGEST}\"
    commit: 8c56954a1f0e
    sdkVersion: 0.4.1
    builtAt: \"2026-09-17T06:00:00Z\"
"
    )
}

/// A repository of `apps`, `(name, project, class, lifecycle)`, in a scratch directory.
fn scratch(tag: &str, apps: &[(&str, &str, &str, &str)]) -> (Repository, PathBuf) {
    let root = std::env::temp_dir().join(format!("jc-t2680-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, project, class, lifecycle) in apps {
        let dir = root.join(format!("projects/{project}/apps"));
        std::fs::create_dir_all(&dir).expect("a scratch repository");
        std::fs::write(
            dir.join(format!("{name}.yaml")),
            app(name, project, class, lifecycle),
        )
        .expect("an app");
    }
    std::fs::create_dir_all(&root).expect("a scratch repository");
    (Repository::load(&root).expect("the repository loads"), root)
}

/// The Deployment of `air` in its project's namespace.
const AIR: &str = "/apis/apps/v1/namespaces/dev-ovzdusie-apps/deployments/app-air";

/// Accepts every write and answers the pull Secret from the Portal's namespace; every other
/// read is a 404 (nothing deployed yet).
async fn cluster() -> MockServer {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "app-registry", "namespace": "joinedcontext", "resourceVersion": "42", "uid": "u-1" },
            "type": "kubernetes.io/dockerconfigjson",
            "data": { ".dockerconfigjson": "eyJhdXRocyI6e319" },
        })))
        .mount(&api)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(&api)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(&api)
        .await;
    api
}

fn converger(api: &MockServer, settings: Settings) -> Converger {
    Converger::new(
        KubeClient::with_token(&api.uri(), TOKEN).expect("a client"),
        settings,
    )
}

fn calls(requests: &[Request], verb: &str) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == verb)
        .map(|request| request.url.path().to_owned())
        .collect()
}

fn body(requests: &[Request], api_path: &str) -> Value {
    let request = requests
        .iter()
        .find(|request| request.method.as_str() == "PATCH" && request.url.path() == api_path)
        .unwrap_or_else(|| panic!("nothing was applied to {api_path}"));
    serde_json::from_slice(&request.body).expect("the body is the object")
}

#[tokio::test]
async fn the_first_pod_backed_app_creates_its_projects_namespace_before_its_objects() {
    let api = cluster().await;
    let (repository, root) = scratch("first", &[("air", "ovzdusie", "fullstack", "published")]);
    let report = converger(&api, settings())
        .converge(&repository, &Default::default())
        .await;
    let _ = std::fs::remove_dir_all(&root);
    let outcomes: Vec<String> = report
        .iter()
        .map(|(id, outcome)| format!("{id}: {outcome:?}"))
        .collect();
    assert!(
        report
            .iter()
            .any(|(id, o)| id == "namespace ovzdusie" && matches!(o, Ok(Outcome::NamespaceReady))),
        "{outcomes:?}"
    );
    assert!(
        report
            .iter()
            .any(|(_, o)| matches!(o, Ok(Outcome::Applied))),
        "{outcomes:?}"
    );

    let requests = api.received_requests().await.expect("recorded");
    let applied = calls(&requests, "PATCH");
    let at = |p: &str| {
        applied
            .iter()
            .position(|a| a == p)
            .unwrap_or_else(|| panic!("{p} not in {applied:?}"))
    };
    // The namespace, then what makes it safe and usable, then the app. The binding comes before
    // the policy: without it the Portal may not write a NetworkPolicy there.
    let deployment = AIR;
    assert!(at(NS) < at(deployment));
    let policy =
        "/apis/networking.k8s.io/v1/namespaces/dev-ovzdusie-apps/networkpolicies/default-deny";
    let binding = "/apis/rbac.authorization.k8s.io/v1/namespaces/dev-ovzdusie-apps/rolebindings/joinedcontext-portal";
    let pull = "/api/v1/namespaces/dev-ovzdusie-apps/secrets/app-registry";
    for object in [policy, binding, pull] {
        assert!(at(object) < at(deployment), "{object} before the app");
    }
    assert!(at(NS) < at(binding) && at(binding) < at(policy) && at(binding) < at(pull));

    let namespace = body(&requests, NS);
    assert_eq!(namespace["kind"], json!("Namespace"));
    let labels = &namespace["metadata"]["labels"];
    assert_eq!(labels["joinedcontext.com/project"], json!("ovzdusie"));
    assert_eq!(labels["joinedcontext.com/release"], json!("dev"));
    assert_eq!(
        labels["joinedcontext.com/managed-by"],
        json!("joinedcontext-portal")
    );
    for mode in ["enforce", "audit", "warn"] {
        assert_eq!(
            labels[format!("pod-security.kubernetes.io/{mode}")],
            json!("restricted")
        );
    }

    let deny = body(&requests, policy);
    assert_eq!(deny["spec"]["podSelector"], json!({}));
    assert_eq!(deny["spec"]["policyTypes"], json!(["Ingress", "Egress"]));
    assert_eq!(deny["spec"]["egress"], json!([]));
    let from = &deny["spec"]["ingress"][0]["from"][0];
    assert_eq!(
        from["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
        json!("apisix")
    );
    assert_eq!(
        from["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
        json!("apisix")
    );
    assert_eq!(
        deny["spec"]["ingress"].as_array().map(Vec::len),
        Some(1),
        "APISIX only"
    );

    let rolebinding = body(&requests, binding);
    assert_eq!(
        rolebinding["roleRef"],
        json!({ "apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": "dev-portal-apps" })
    );
    assert_eq!(
        rolebinding["subjects"],
        json!([{ "kind": "ServiceAccount", "name": "portal", "namespace": "joinedcontext" }])
    );

    // The pull Secret's type and data, and nothing the API server wrote on the original.
    let copy = body(&requests, pull);
    assert_eq!(copy["type"], json!("kubernetes.io/dockerconfigjson"));
    assert_eq!(
        copy["data"],
        json!({ ".dockerconfigjson": "eyJhdXRocyI6e319" })
    );
    assert!(
        copy["metadata"].get("resourceVersion").is_none() && copy["metadata"].get("uid").is_none()
    );

    let deleted = calls(&requests, "DELETE");
    assert!(
        !deleted.iter().any(|d| d.starts_with(NS)),
        "nothing in the new namespace is removed"
    );
}

/// The Portal's own namespace holds the platform's credentials (database, Keycloak, forge), and
/// its Role there names only the pull Secret for reading (T-4798): no App, live or retired,
/// makes the converger read, write or delete anything else in it.
#[tokio::test]
async fn in_the_portals_own_namespace_only_the_pull_secret_is_read() {
    for lifecycle in ["published", "retired"] {
        let api = cluster().await;
        let (repository, root) = scratch(
            &format!("own-{lifecycle}"),
            &[
                ("air", "ovzdusie", "fullstack", lifecycle),
                ("bus", "ovzdusie", "fullstack", "published"),
            ],
        );
        let report = converger(&api, settings())
            .converge(&repository, &Default::default())
            .await;
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            report.iter().all(|(_, o)| o.is_ok()),
            "{lifecycle}: {report:?}"
        );
        let requests = api.received_requests().await.expect("recorded");
        let touched: Vec<String> = requests
            .iter()
            .filter(|r| r.url.path().contains("/namespaces/joinedcontext/"))
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect();
        assert_eq!(touched, vec![format!("GET {PULL}")], "{lifecycle}");
    }
}

#[tokio::test]
async fn the_last_pod_backed_app_retired_removes_the_namespace_and_one_left_keeps_it() {
    let api = cluster().await;
    let (repository, root) = scratch(
        "last",
        &[
            ("air", "ovzdusie", "fullstack", "retired"),
            ("bikes", "doprava", "ui-rust", "retired"),
            ("buses", "doprava", "fullstack", "published"),
        ],
    );
    let report = converger(&api, settings())
        .converge(&repository, &Default::default())
        .await;
    let _ = std::fs::remove_dir_all(&root);
    let requests = api.received_requests().await.expect("recorded");
    let deleted = calls(&requests, "DELETE");
    assert!(deleted.contains(&NS.to_owned()), "{deleted:?}");
    assert!(
        !deleted.contains(&"/api/v1/namespaces/dev-doprava-apps".to_owned()),
        "doprava still runs buses"
    );
    assert!(report
        .iter()
        .any(|(id, o)| id == "namespace ovzdusie" && matches!(o, Ok(Outcome::NamespaceDeleted))));
}

#[tokio::test]
async fn a_repository_without_a_retired_app_removes_no_namespace() {
    let api = cluster().await;
    // A short read, a project of static apps and drafts only: nothing to create, nothing to end.
    let (repository, root) = scratch(
        "short",
        &[
            ("site", "ovzdusie", "static", "published"),
            ("wip", "doprava", "fullstack", "draft"),
        ],
    );
    converger(&api, settings())
        .converge(&repository, &Default::default())
        .await;
    let (empty, empty_root) = scratch("empty", &[]);
    converger(&api, settings())
        .converge(&empty, &Default::default())
        .await;
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&empty_root);
    let requests = api.received_requests().await.expect("recorded");
    let namespaces: Vec<&str> = requests
        .iter()
        .map(|request| request.url.path())
        .filter(|p| p.starts_with("/api/v1/namespaces/") && p.ends_with("-apps"))
        .collect();
    assert!(namespaces.is_empty(), "{namespaces:?}");
}

#[tokio::test]
async fn without_a_release_or_a_pull_secret_no_app_is_deployed_and_each_says_why() {
    for (tag, settings, reason) in [
        (
            "norelease",
            Settings {
                release: None,
                ..settings()
            },
            "JC_PORTAL_RELEASE",
        ),
        (
            "nopull",
            Settings {
                pull_secret: Some("absent".into()),
                ..settings()
            },
            "absent",
        ),
    ] {
        let api = cluster().await;
        let (repository, root) = scratch(tag, &[("air", "ovzdusie", "fullstack", "published")]);
        let report = converger(&api, settings)
            .converge(&repository, &Default::default())
            .await;
        let _ = std::fs::remove_dir_all(&root);
        let namespace = report
            .iter()
            .find(|(id, _)| id == "namespace ovzdusie")
            .expect("a namespace line");
        let said = match &namespace.1 {
            Err(err) => err.to_string(),
            other => panic!("{other:?}"),
        };
        assert!(said.contains(reason), "{said}");
        assert!(
            report
                .iter()
                .any(|(_, o)| matches!(o, Ok(Outcome::Skipped(why)) if why.contains("not ready"))),
            "the app waits for its namespace"
        );
        let requests = api.received_requests().await.expect("recorded");
        assert!(
            !calls(&requests, "PATCH")
                .iter()
                .any(|p| p.contains("/deployments/")),
            "no app object without its namespace"
        );
    }
}

#[tokio::test]
async fn a_namespace_name_longer_than_a_label_is_refused_before_any_call() {
    let api = cluster().await;
    let project = "a-project-name-that-is-long-enough-to-overflow-the-label-limit";
    let (repository, root) = scratch("long", &[("air", project, "fullstack", "published")]);
    let report = converger(&api, settings())
        .converge(&repository, &Default::default())
        .await;
    let _ = std::fs::remove_dir_all(&root);
    let line = report
        .iter()
        .find(|(id, _)| id.starts_with("namespace "))
        .expect("a namespace line");
    let said = line
        .1
        .as_ref()
        .map(|o| format!("{o:?}"))
        .unwrap_or_else(|err| err.to_string());
    assert!(said.contains("is not a namespace name"), "{said}");
    let requests = api.received_requests().await.expect("recorded");
    assert!(calls(&requests, "PATCH").is_empty());
}
