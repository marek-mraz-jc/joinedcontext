//! What one builder run's workspace looks like on a cluster (T-0538, AG-33, AG-34, AG-35, AG-40).
//!
//! The scheduler runs against a stubbed API server, so what is asserted is the three objects
//! that actually leave the Portal. Three properties cannot be seen from inside the process: the
//! pod mounts no token and no Secret, its NetworkPolicy names the peers it may reach rather
//! than leaving a rule open, and the only credential the pod reads is the run's own ticket,
//! from the run's Secret through a `secretKeyRef` (T-5702).

use joinedcontext_portal::agents::kube::{schedule_workspace_job, AGENT_SERVER_PORT, PROXY_PORT};
use joinedcontext_portal::agents::profile::Profile;
use joinedcontext_portal::agents::run::{AgentRun, AgentRunStatus};
use joinedcontext_portal::apps::kube::{KubeClient, FIELD_MANAGER};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const NAMESPACE: &str = "agents";
/// Where the Portal itself runs: a different namespace from the workspaces, which is the only
/// shape in which the ingress rule's namespace can be wrong (AG-39).
const PORTAL_NAMESPACE: &str = "joinedcontext";
const TOKEN: &str = "the-projected-service-account-token-of-the-portal";
const TICKET: &str = "gv3oq7lk2xhbm5yz4rtwpc6sdfae8nij";
const RUN_ID: &str = "e3b0c442-98fc-1c14-9afb-4c7b2756a120";
const PROXY_BASE: &str = "http://jc-agent-proxy.agents.svc.cluster.local:8080";
const TTL_SECS: i64 = 1_200;
const IMAGE_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn job_path() -> String {
    format!("/apis/batch/v1/namespaces/{NAMESPACE}/jobs/agent-run-{RUN_ID}")
}

fn service_account_path() -> String {
    format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts/agent-run-{RUN_ID}")
}

fn secret_path() -> String {
    format!("/api/v1/namespaces/{NAMESPACE}/secrets/agent-run-{RUN_ID}")
}

const JOB_UID: &str = "7c1d2e3f-0000-4000-8000-000000000001";

fn policy_path() -> String {
    format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies/agent-run-{RUN_ID}")
}

fn profile() -> Profile {
    Profile {
        name: "app-builder".into(),
        role: "builder".into(),
        image: format!("ghcr.io/all-hands-ai/agent-server:v1.4.0@{IMAGE_DIGEST}"),
        model_name: "claude-sonnet-5".into(),
        model_provider: "anthropic".into(),
        reasoning_effort: None,
        routing: None,
        max_tokens_per_run: 400_000,
        steps_per_run: 120,
        requests_per_minute: 60,
        max_response_bytes: 2_097_152,
        max_egress_bytes_per_run: jc_core::DEFAULT_EGRESS_BYTES_PER_RUN,
        allowed_hosts: vec!["registry.npmjs.org".into()],
        cpu: "1".into(),
        memory: "2Gi".into(),
        ephemeral_storage: "4Gi".into(),
        access: Default::default(),
    }
}

fn run() -> AgentRun {
    AgentRun {
        id: RUN_ID.into(),
        project: "helsinki".into(),
        app_name: "city-bikes-overview".into(),
        title: None,
        endpoint_name: "helsinki-bikes".into(),
        endpoint_slug: "si6epqkx364lprho5uaigutk274r5grb".into(),
        endpoints: serde_json::json!([]),
        profile: "app-builder".into(),
        kind: "application".into(),
        unattended: false,
        continues: None,
        app_class: "ui".into(),
        visibility: "project".into(),
        prompt: "a live bike availability dashboard".into(),
        prompt_digest: "sha256:abc".into(),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-city-bikes-overview/{RUN_ID}"),
        path_prefix: "projects/helsinki/apps/city-bikes-overview/".into(),
        status: AgentRunStatus::Queued.as_str().into(),
        ticket_hash: "$argon2id$stub".into(),
        workspace: None,
        merge_request: None,
        change_id: None,
        source_url: None,
        mirror_url: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 0,
        tokens_used: 0,
        created_by: "demo.steward@hel.fi".into(),
        origin: "person".to_owned(),
        starter: serde_json::Value::Null,
        created_at: "2026-09-12T10:15:30Z".into(),
        started_at: None,
        finished_at: None,
        expires_at: "2026-09-12T10:35:30Z".into(),
        error: None,
    }
}

async fn accepts_apply(api: &MockServer, api_path: &str) {
    Mock::given(method("PATCH"))
        .and(path(api_path.to_owned()))
        .and(query_param("fieldManager", FIELD_MANAGER))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(api)
        .await;
}

/// Every body applied to `api_path`, in order.
fn applied_all(requests: &[Request], api_path: &str) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.url.path() == api_path && request.method.as_str() == "PATCH")
        .map(|request| serde_json::from_slice(&request.body).expect("the body is the object"))
        .collect()
}

fn applied(requests: &[Request], api_path: &str) -> Value {
    let request = requests
        .iter()
        .find(|request| request.url.path() == api_path && request.method.as_str() == "PATCH")
        .unwrap_or_else(|| panic!("nothing was applied to {api_path}"));
    serde_json::from_slice(&request.body).expect("the body is the object")
}

async fn schedule() -> Vec<Request> {
    let api = MockServer::start().await;
    for api_path in [
        service_account_path(),
        policy_path(),
        secret_path(),
        job_path(),
    ] {
        accepts_apply(&api, &api_path).await;
    }
    Mock::given(method("GET"))
        .and(path(job_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": { "name": format!("agent-run-{RUN_ID}"), "uid": JOB_UID },
        })))
        .mount(&api)
        .await;
    let kube = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");
    let scheduled = schedule_workspace_job(
        Some(&kube),
        NAMESPACE,
        PORTAL_NAMESPACE,
        &run(),
        TICKET,
        PROXY_BASE,
        &profile(),
        TTL_SECS,
    )
    .await
    .expect("the four objects are applied");
    assert!(scheduled, "a cluster was given, so the workspace is real");
    api.received_requests().await.unwrap_or_default()
}

#[tokio::test]
async fn a_portal_without_a_cluster_schedules_nothing_and_says_so() {
    let scheduled = schedule_workspace_job(
        None,
        NAMESPACE,
        PORTAL_NAMESPACE,
        &run(),
        TICKET,
        PROXY_BASE,
        &profile(),
        TTL_SECS,
    )
    .await
    .expect("no cluster is not an error");
    assert!(
        !scheduled,
        "the caller has to learn that it drives the workspace itself"
    );
}

#[tokio::test]
async fn the_workspace_mounts_no_token_and_no_secret() {
    let requests = schedule().await;

    let service_account = applied(&requests, &service_account_path());
    assert_eq!(
        service_account["automountServiceAccountToken"],
        json!(false),
        "AG-34: the workspace has no business reaching the Kubernetes API"
    );

    let job = applied(&requests, &job_path());
    let pod = &job["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], json!(false));
    let container = &pod["containers"][0];

    let volumes = pod["volumes"].as_array().expect("volumes");
    for volume in volumes {
        assert!(
            volume.get("secret").is_none() && volume.get("projected").is_none(),
            "nothing but an emptyDir is mounted: {volume}"
        );
    }
    for mount in container["volumeMounts"].as_array().expect("mounts") {
        let name = mount["name"].as_str().unwrap_or_default();
        assert!(
            volumes.iter().any(|volume| volume["name"] == name),
            "{name} is mounted from nowhere"
        );
    }
    assert!(
        container.get("envFrom").is_none(),
        "AG-34: no Secret or ConfigMap is poured into the environment"
    );
}

#[tokio::test]
async fn the_only_credential_in_the_pod_spec_is_the_runs_own_ticket() {
    let requests = schedule().await;
    let job = applied(&requests, &job_path());
    let env = job["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()
        .expect("env")
        .clone();

    let value_of = |name: &str| -> String {
        env.iter()
            .find(|entry| entry["name"] == name)
            .and_then(|entry| entry["value"].as_str())
            .unwrap_or_else(|| panic!("{name} is not a plain value in the environment"))
            .to_owned()
    };
    let from_ticket_secret = |name: &str| -> bool {
        env.iter().any(|entry| {
            entry["name"] == name
                && entry.get("value").is_none()
                && entry["valueFrom"]["secretKeyRef"]
                    == json!({ "name": format!("agent-run-{RUN_ID}"), "key": "ticket" })
        })
    };

    assert_eq!(value_of("JC_RUN_ID"), RUN_ID);
    assert!(from_ticket_secret("JC_RUN_TICKET"));
    assert_eq!(value_of("JC_PROXY_BASE"), PROXY_BASE);
    assert!(
        from_ticket_secret("OH_SESSION_API_KEYS_0"),
        "the agent server's own API key is the run ticket, so only the Portal drives it"
    );
    assert!(
        value_of("LLM_BASE_URL").starts_with(PROXY_BASE),
        "the model is reached through the proxy alone (AG-40)"
    );
    assert!(from_ticket_secret("LLM_API_KEY"));
    assert_eq!(value_of("LLM_MODEL"), "claude-sonnet-5");

    // T-5702: a person who may read the Job does not read a live ticket.
    assert!(
        !job.to_string().contains(TICKET),
        "the ticket is in the Job's spec"
    );

    // Everything in the environment is either a run fact or the ticket. A provider key, a forge
    // token or a realm secret reaching the workspace is the failure ADR-N-020 exists to prevent.
    for entry in &env {
        let name = entry["name"].as_str().unwrap_or_default();
        let value = entry["value"].as_str().unwrap_or_default();
        assert!(
            value != "sk-" && !name.contains("FORGE") && !name.contains("CLIENT_SECRET"),
            "{name} looks like a credential the workspace must not hold"
        );
    }
}

#[tokio::test]
async fn the_ticket_secret_holds_the_ticket_and_is_owned_by_the_job() {
    let requests = schedule().await;
    let secrets = applied_all(&requests, &secret_path());
    assert_eq!(
        secrets.len(),
        2,
        "applied, then applied again with its owner"
    );
    for secret in &secrets {
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["stringData"]["ticket"], TICKET);
    }
    assert_eq!(
        secrets[1]["metadata"]["ownerReferences"],
        json!([{
            "apiVersion": "batch/v1",
            "kind": "Job",
            "name": format!("agent-run-{RUN_ID}"),
            "uid": JOB_UID,
        }]),
        "the garbage collector removes the Secret with its Job"
    );
    let order: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.url.path())
        .collect();
    let secret_at = order
        .iter()
        .position(|api_path| *api_path == secret_path())
        .expect("the secret was applied");
    let job_at = order
        .iter()
        .position(|api_path| *api_path == job_path())
        .expect("the job was applied");
    assert!(
        secret_at < job_at,
        "the pod starts only once its Secret exists"
    );
}

#[tokio::test]
async fn the_network_policy_names_dns_the_proxy_and_the_mesh_control_plane_and_nothing_else() {
    let requests = schedule().await;
    let policy = applied(&requests, &policy_path());
    let spec = &policy["spec"];

    assert_eq!(
        spec["podSelector"]["matchLabels"]["joinedcontext.com/run-id"],
        json!(RUN_ID),
        "the policy holds this run's pod, not every pod in the namespace"
    );
    assert_eq!(spec["policyTypes"], json!(["Ingress", "Egress"]));

    let egress = spec["egress"].as_array().expect("egress rules");
    assert_eq!(
        spec["egress"],
        json!([
            {
                "to": [{
                    "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                    "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } }
                }],
                "ports": [
                    { "protocol": "UDP", "port": 53 },
                    { "protocol": "TCP", "port": 53 }
                ]
            },
            {
                "to": [{
                    "podSelector": { "matchLabels": { "app.kubernetes.io/name": "agent-runner-proxy" } }
                }],
                "ports": [
                    { "protocol": "TCP", "port": PROXY_PORT },
                    { "protocol": "TCP", "port": 4143 }
                ]
            },
            {
                "to": [{
                    "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "linkerd" } }
                }],
                "ports": [
                    { "protocol": "TCP", "port": 8080 },
                    { "protocol": "TCP", "port": 8086 },
                    { "protocol": "TCP", "port": 8090 }
                ]
            }
        ]),
        "DNS, the credential proxy (by the label its pods really carry, its port and the mesh \
         inbound port) and the Linkerd control plane (identity, destination, policy), and no \
         fourth peer"
    );
    for rule in egress {
        assert!(
            rule["to"].as_array().is_some_and(|to| !to.is_empty()),
            "AG-40: a rule with no 'to' opens the port on every address in the cluster: {rule}"
        );
    }

    let ingress = spec["ingress"].as_array().expect("ingress rules");
    assert_eq!(ingress.len(), 1, "the Portal drives the agent server");
    assert_eq!(
        ingress[0]["ports"][0]["port"],
        json!(AGENT_SERVER_PORT),
        "on the agent server's port alone"
    );
    let from = &ingress[0]["from"][0];
    assert_eq!(
        from["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
        json!("portal-portal"),
        "the label the Portal's own pods really carry"
    );
    assert_eq!(
        from["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
        json!(PORTAL_NAMESPACE),
        "AG-39: the Portal's namespace, not every namespace holding a pod with that label"
    );
    for rule in ingress.iter().chain(egress.iter()) {
        for peer in rule["from"]
            .as_array()
            .into_iter()
            .chain(rule["to"].as_array())
            .flatten()
        {
            assert!(
                peer["namespaceSelector"]
                    .as_object()
                    .is_none_or(|it| !it.is_empty()),
                "an empty namespaceSelector matches every namespace in the cluster: {peer}"
            );
        }
    }
}

#[tokio::test]
async fn the_job_is_pinned_by_digest_bounded_and_unprivileged() {
    let requests = schedule().await;
    let job = applied(&requests, &job_path());

    assert_eq!(job["spec"]["backoffLimit"], json!(0), "no replay of a run");
    assert_eq!(
        job["spec"]["activeDeadlineSeconds"],
        json!(TTL_SECS),
        "the pod and the run record agree about when the run is over"
    );

    let container = &job["spec"]["template"]["spec"]["containers"][0];
    assert!(
        container["image"]
            .as_str()
            .is_some_and(|image| image.ends_with(IMAGE_DIGEST)),
        "AG-49: the image is pinned by digest, not by tag"
    );
    assert_eq!(
        container["securityContext"]["readOnlyRootFilesystem"],
        json!(true)
    );
    assert_eq!(
        container["securityContext"]["allowPrivilegeEscalation"],
        json!(false)
    );
    assert_eq!(
        container["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(
        container["resources"]["limits"]["memory"],
        json!("2Gi"),
        "the profile's bounds are the pod's bounds (AG-41)"
    );
}

#[tokio::test]
async fn the_policy_is_applied_before_the_job_that_it_holds() {
    let requests = schedule().await;
    let order: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.url.path())
        .collect();
    let policy_at = order
        .iter()
        .position(|api_path| *api_path == policy_path())
        .expect("the policy was applied");
    let job_at = order
        .iter()
        .position(|api_path| *api_path == job_path())
        .expect("the job was applied");
    assert!(
        policy_at < job_at,
        "a pod that starts before its policy has a window with the namespace default"
    );
}

#[tokio::test]
async fn cancelling_removes_all_four_objects() {
    let api = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(&api)
        .await;
    let kube = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");

    joinedcontext_portal::agents::kube::delete_workspace_job(Some(&kube), NAMESPACE, RUN_ID).await;

    let deleted: Vec<String> = api
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .map(|request| request.url.path().to_owned())
        .collect();
    for api_path in [
        job_path(),
        policy_path(),
        service_account_path(),
        secret_path(),
    ] {
        assert!(deleted.contains(&api_path), "{api_path} was left behind");
    }
}
