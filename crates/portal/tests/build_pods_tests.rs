//! A build pod per App (AP-130, AP-131, T-2794).
//!
//! One pass runs against a stub that answers as both the forge and the API server, so what is
//! asserted is what leaves the Portal: a registration token of the App's own repository, a Job
//! and a claim named after the App, the token in a Secret its Job owns, nothing for a job that
//! is not an App's build, and no cache left behind by an App that is gone.

use joinedcontext_portal::apps::build_pods::{dispatch_once, Settings};
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const NS: &str = "dev";
const REGISTRATION: &str = "repository-scoped-registration-token";

fn settings() -> Settings {
    Settings {
        namespace: NS.into(),
        forge: "http://gitea-http.dev.svc.cluster.local:3000".into(),
        node_image: "r.example.org/builder@sha256:aa".into(),
        rust_image: "r.example.org/builder-rust@sha256:bb".into(),
        cache_size: "1Gi".into(),
    }
}

fn mirror(apps: &[(&str, &str)]) -> Mirror {
    let mirror = Mirror::new();
    for (project, app) in apps {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "App".into(),
            metadata: ObjectMeta::new(*app, *project),
            spec: json!({ "kind": "static" }),
            status: None,
        });
    }
    mirror
}

fn clients(server: &MockServer) -> (KubeClient, GiteaClient) {
    let kube = KubeClient::with_token(&server.uri(), "portal-sa-token").expect("a kube client");
    let forge = GiteaClient::new(
        server.uri().parse().expect("a url"),
        "joinedcontext",
        "configuration",
        "forge-token",
    )
    .expect("a forge client");
    (kube, forge)
}

fn queued(jobs: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "total_count": 1, "jobs": jobs }))
}

fn job(id: u64, repo: &str, label: &str) -> Value {
    json!({
        "id": id,
        "labels": [label],
        "status": "queued",
        "url": format!("http://gitea-http.dev.svc.cluster.local:3000/api/v1/repos/joinedcontext/{repo}/actions/jobs/{id}"),
    })
}

async fn no_claims(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NS}/persistentvolumeclaims"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .mount(server)
        .await;
}

/// The build Jobs the cluster holds when the pass lists them.
async fn build_jobs(server: &MockServer, items: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/apis/batch/v1/namespaces/{NS}/jobs")))
        .and(query_param(
            "labelSelector",
            "app.kubernetes.io/component=build-pod,joinedcontext.com/build-app",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
        .mount(server)
        .await;
}

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("a JSON body")
}

#[tokio::test]
async fn a_queued_app_build_gets_one_pod_its_own_cache_and_its_repositorys_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .and(query_param("status", "queued"))
        .respond_with(queued(json!([job(42, "helsinki_bikes", "app-build-node")])))
        .mount(&server)
        .await;
    // The first read finds no Job; the one after the apply finds it with its uid.
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/batch/v1/namespaces/{NS}/jobs/build-bikes-42"
        )))
        .respond_with(ResponseTemplate::new(404))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/batch/v1/namespaces/{NS}/jobs/build-bikes-42"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "metadata": { "name": "build-bikes-42", "uid": "0f6c-uid" } }),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_bikes/actions/runners/registration-token",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "token": REGISTRATION })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    no_claims(&server).await;
    build_jobs(&server, json!([])).await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(
        &kube,
        &forge,
        &mirror(&[("helsinki", "bikes")]),
        &settings(),
    )
    .await
    .expect("the pass");
    assert_eq!(pass.started, ["build-bikes-42"]);

    let requests = server.received_requests().await.expect("recorded");
    let applied: Vec<(String, Value)> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| (r.url.path().to_owned(), body(r)))
        .collect();
    let paths: Vec<&str> = applied.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/api/v1/namespaces/dev/persistentvolumeclaims/build-cache-bikes",
            "/apis/batch/v1/namespaces/dev/jobs/build-bikes-42",
            "/api/v1/namespaces/dev/secrets/build-bikes-42",
            "/apis/batch/v1/namespaces/dev/jobs/build-bikes-42",
        ],
        "the claim and the Job first, the token owned by the Job, then the Job marked"
    );
    assert!(applied[1].1["metadata"].get("annotations").is_none());
    assert_eq!(
        applied[3].1["metadata"]["annotations"]["joinedcontext.com/build-token-written"], "true",
        "the mark comes only after the token"
    );
    assert_eq!(applied[3].1["spec"], applied[1].1["spec"], "the same Job");
    let pod = &applied[1].1["spec"]["template"]["spec"];
    assert_eq!(
        pod["containers"][0]["image"],
        "r.example.org/builder@sha256:aa"
    );
    assert_eq!(
        pod["volumes"][4]["persistentVolumeClaim"]["claimName"],
        "build-cache-bikes"
    );
    assert!(
        !applied[1].1.to_string().contains(REGISTRATION),
        "the Job carries no token, only the name of its Secret"
    );
    let secret = &applied[2].1;
    assert_eq!(secret["stringData"]["token"], REGISTRATION);
    assert_eq!(secret["metadata"]["ownerReferences"][0]["uid"], "0f6c-uid");
}

#[tokio::test]
async fn a_job_that_is_not_an_apps_build_gets_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(json!([
            // The propose job stays on the shared runner.
            job(1, "helsinki_bikes", "node-22"),
            // A repository that looks like an App's and is none.
            job(2, "helsinki_ghost", "app-build-node"),
            // Another organization's repository.
            json!({ "id": 3, "labels": ["app-build-node"], "url": "http://f/api/v1/repos/other/helsinki_bikes/actions/jobs/3" }),
        ])))
        .mount(&server)
        .await;
    no_claims(&server).await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(
        &kube,
        &forge,
        &mirror(&[("helsinki", "bikes")]),
        &settings(),
    )
    .await
    .expect("the pass");
    assert!(pass.started.is_empty());
    let requests = server.received_requests().await.expect("recorded");
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "no token minted, nothing applied: {:?}",
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_job_left_without_its_token_is_removed_with_its_pods_for_the_next_pass() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(json!([job(42, "helsinki_bikes", "app-build-rust")])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/batch/v1/namespaces/{NS}/jobs/build-bikes-42"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "metadata": { "uid": "u" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/apis/batch/v1/namespaces/{NS}/jobs/build-bikes-42"
        )))
        .and(query_param("propagationPolicy", "Background"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    no_claims(&server).await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(
        &kube,
        &forge,
        &mirror(&[("helsinki", "bikes")]),
        &settings(),
    )
    .await
    .expect("the pass");
    assert!(
        pass.started.is_empty(),
        "started on the next pass, not this one"
    );
    no_secret_read(&server).await;
}

/// The Portal reads no Secret in the runner's namespace (T-4798): a Job marked as holding its
/// token is left to run, judged by the Job alone.
#[tokio::test]
async fn a_job_marked_with_its_token_is_left_alone_and_no_secret_is_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(json!([job(42, "helsinki_bikes", "app-build-rust")])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/batch/v1/namespaces/{NS}/jobs/build-bikes-42"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "metadata": {
            "uid": "u",
            "annotations": { "joinedcontext.com/build-token-written": "true" },
        } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(0)
        .mount(&server)
        .await;
    no_claims(&server).await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(
        &kube,
        &forge,
        &mirror(&[("helsinki", "bikes")]),
        &settings(),
    )
    .await
    .expect("the pass");
    assert!(pass.started.is_empty());
    no_secret_read(&server).await;
}

async fn no_secret_read(server: &MockServer) {
    let requests = server.received_requests().await.expect("recorded");
    let read: Vec<&str> = requests
        .iter()
        .filter(|r| r.method.as_str() != "PATCH" && r.url.path().contains("/secrets/"))
        .map(|r| r.url.path())
        .collect();
    assert!(read.is_empty(), "{read:?}");
}

#[tokio::test]
async fn a_retired_apps_cache_goes_and_a_live_apps_stays() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(json!([])))
        .mount(&server)
        .await;
    let claim = |app: &str| {
        json!({ "metadata": {
            "name": format!("build-cache-{app}"),
            "labels": { "joinedcontext.com/build-app": app, "app.kubernetes.io/component": "build-pod" }
        }})
    };
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NS}/persistentvolumeclaims"
        )))
        .and(query_param(
            "labelSelector",
            "app.kubernetes.io/component=build-pod,joinedcontext.com/build-app",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "items": [claim("bikes"), claim("retired"), claim("old")] }),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/namespaces/{NS}/persistentvolumeclaims/build-cache-retired"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    // An App the mirror still holds as `retired` loses its cache as one that is gone does.
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/namespaces/{NS}/persistentvolumeclaims/build-cache-old"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let (kube, forge) = clients(&server);
    let apps = mirror(&[("helsinki", "bikes")]);
    apps.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "App".into(),
        metadata: ObjectMeta::new("old", "helsinki"),
        spec: json!({ "kind": "static", "lifecycle": "retired" }),
        status: None,
    });
    let pass = dispatch_once(&kube, &forge, &apps, &settings())
        .await
        .expect("the pass");
    assert_eq!(pass.removed, ["build-cache-retired", "build-cache-old"]);

    // A mirror that holds no App yet deletes nothing: a restart never empties the caches.
    let empty = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(json!([])))
        .mount(&empty)
        .await;
    let (kube, forge) = clients(&empty);
    let pass = dispatch_once(&kube, &forge, &mirror(&[]), &settings())
        .await
        .expect("the pass");
    assert!(pass.removed.is_empty());
    let requests = empty.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1, "only the queue was read");
}

/// Five queued builds of five Apps, as one seed sync leaves them (T-5815): `ids` are the forge
/// job ids, `build-{app}-{id}` the Jobs, none of which exists yet.
async fn five_queued(server: &MockServer) -> Vec<(&'static str, u64)> {
    let builds = vec![
        ("weather", 520),
        ("quality", 522),
        ("planner", 524),
        ("transport", 526),
        ("forecast", 528),
    ];
    let jobs: Vec<Value> = builds
        .iter()
        .map(|(app, id)| job(*id, &format!("helsinki_{app}"), "app-build-rust"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/joinedcontext/actions/jobs"))
        .respond_with(queued(Value::Array(jobs)))
        .mount(server)
        .await;
    for (app, id) in &builds {
        let job_path = format!("/apis/batch/v1/namespaces/{NS}/jobs/build-{app}-{id}");
        Mock::given(method("GET"))
            .and(path(job_path.clone()))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(job_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "metadata": { "name": format!("build-{app}-{id}"), "uid": format!("{app}-uid") } }),
            ))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/repos/joinedcontext/helsinki_{app}/actions/runners/registration-token"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "token": REGISTRATION })),
            )
            .mount(server)
            .await;
    }
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
    no_claims(server).await;
    builds
}

fn five_apps(builds: &[(&str, u64)]) -> Mirror {
    let apps: Vec<(&str, &str)> = builds.iter().map(|(app, _)| ("helsinki", *app)).collect();
    mirror(&apps)
}

fn minted(requests: &[Request]) -> usize {
    requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/registration-token"))
        .count()
}

#[tokio::test]
async fn five_queued_builds_start_one_job_and_the_rest_wait_in_the_queue() {
    let server = MockServer::start().await;
    let builds = five_queued(&server).await;
    // Finished and departing Jobs hold no requests and do not count.
    build_jobs(
        &server,
        json!([
            { "metadata": { "name": "build-old-1" },
              "status": { "conditions": [{ "type": "Complete", "status": "True" }] } },
            { "metadata": { "name": "build-old-2" },
              "status": { "conditions": [{ "type": "Failed", "status": "True" }] } },
            { "metadata": { "name": "build-old-3", "deletionTimestamp": "2026-10-26T19:00:00Z" } },
        ]),
    )
    .await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(&kube, &forge, &five_apps(&builds), &settings())
        .await
        .expect("the pass");
    assert_eq!(pass.started, ["build-weather-520"], "one build at a time");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(minted(&requests), 1, "no token for a build that waits");
    let lists = requests
        .iter()
        .filter(|r| r.url.path() == format!("/apis/batch/v1/namespaces/{NS}/jobs"))
        .count();
    assert_eq!(lists, 1, "the running Jobs are read once per pass");
}

#[tokio::test]
async fn a_running_build_holds_every_queued_one_back() {
    let server = MockServer::start().await;
    let builds = five_queued(&server).await;
    build_jobs(
        &server,
        json!([{ "metadata": { "name": "build-earlier-519" }, "status": { "active": 1 } }]),
    )
    .await;

    let (kube, forge) = clients(&server);
    let pass = dispatch_once(&kube, &forge, &five_apps(&builds), &settings())
        .await
        .expect("the pass");
    assert!(pass.started.is_empty(), "{:?}", pass.started);
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(minted(&requests), 0);
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "nothing applied while a build runs"
    );
}
