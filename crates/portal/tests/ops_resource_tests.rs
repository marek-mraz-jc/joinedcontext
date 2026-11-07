//! Every kind read, changed and removed through the operations registry, on the same path as the
//! resource routes (T-0735, AG-77, ADR-N-021, PF-50, CC-19, MF-24, AG-11).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::{self, Caller, Via};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const CSRF_TOKEN: &str = "test-csrf-token-resources";
const REPO: &str = "/api/v1/repos/test-owner/test-repo";

fn identity(username: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{username}"),
        username: username.to_string(),
        email: Some(format!("{username}@banskabystrica.sk")),
        name: Some(username.to_string()),
        roles: Vec::new(),
        groups: groups.iter().map(|group| group.to_string()).collect(),
        second_factor: false,
    }
}

fn cookie(config: &Config, who: Identity) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: who,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF_TOKEN}"));
    parts.join("; ")
}

/// The steward of the tests: the bootstrap group, which may do everything (PF-50).
fn steward() -> Identity {
    identity("demo.steward", &["portal-approver"])
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// A forge that answers every write: a file it has at `existing`, none elsewhere, and merge
/// request 9 for every branch.
async fn forge(existing: &[&str]) -> MockServer {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    for file in existing {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{file}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "sha-existing",
                "content": "YXBpVmVyc2lvbjogeW91"
            })))
            .mount(&gitea)
            .await;
    }
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.*")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&gitea)
        .await;
    // A deletion reads the tree to find the files the resource owns beside its manifest (T-0900).
    let tree: Vec<Value> = existing
        .iter()
        .map(|path| json!({ "path": path, "type": "blob" }))
        .collect();
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/git/trees/.*")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false })),
        )
        .mount(&gitea)
        .await;
    // One commit for every file of a resource goes to `POST /contents` (T-0900).
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/contents")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } })),
        )
        .mount(&gitea)
        .await;
    for verb in ["PUT", "POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!("^{REPO}/contents/.*")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "commit": { "sha": "commit-1" } })),
            )
            .mount(&gitea)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 9,
            "html_url": "https://gitea.example.sk/pulls/9",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&gitea)
        .await;
    gitea
}

fn state_with(gitea: &MockServer) -> AppState {
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client))
}

/// A request as every door makes it since T-0956 (PF-57): a proposal is checked first with the
/// same manifest, on the route (`?dryRun=All`) or through `jc_manifest_dry_run`, then sent.
async fn send(
    state: &AppState,
    who: Identity,
    http: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    if let Some((base, op)) = uri.rsplit_once("/ops/") {
        if op.ends_with("_propose") && body.get("manifest").is_some() {
            let check = json!({ "manifest": body["manifest"] });
            send_once(
                state,
                who.clone(),
                "POST",
                &format!("{base}/ops/jc_manifest_dry_run"),
                check,
            )
            .await;
        }
    } else if matches!(http, "POST" | "PUT" | "PATCH") && !uri.contains("dryRun") {
        send_once(
            state,
            who.clone(),
            http,
            &format!("{uri}?dryRun=All"),
            body.clone(),
        )
        .await;
    }
    send_once(state, who, http, uri, body).await
}

async fn send_once(
    state: &AppState,
    who: Identity,
    http: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let config = state.config.clone();
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(http)
                .uri(uri)
                .header(header::COOKIE, cookie(&config, who))
                .header(CSRF_HEADER, CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn op(
    state: &AppState,
    who: Identity,
    project: &str,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    send(
        state,
        who,
        "POST",
        &format!("/api/v1/projects/{project}/ops/{name}"),
        input,
    )
    .await
}

/// The green record check of `manifest` its workbench would keep on its own draft (T-4840).
async fn records_checked(state: &AppState, project: &str, manifest: &Value) {
    use joinedcontext_portal::ops::verdict::{digest_of, RecordsVerdict, Verdict};
    let name = manifest["metadata"]["name"].as_str().expect("a name");
    let store = state.drafts.clone();
    store
        .put(
            project,
            "Pipeline",
            name,
            manifest.clone(),
            Some(0),
            "steward",
            "person",
        )
        .await
        .expect("the draft");
    let mut verdict = Verdict::green(manifest, None);
    verdict.records = Some(Box::new(RecordsVerdict {
        ok: true,
        checked: 1,
        rejected: 0,
        spec_digest: digest_of(&manifest["spec"]),
        sample_digest: digest_of(&json!([])),
    }));
    store
        .set_verdict(project, "Pipeline", name, verdict)
        .await
        .expect("the verdict");
}

/// The file paths the forge was asked to write or remove.
async fn written(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() != "GET")
        .flat_map(|request| {
            let path = request.url.path();
            // A one-file write names its path in the URL; a batch commit (`POST /contents`)
            // names every file it touches in its body.
            if let Some(one) = path.strip_prefix(&format!("{REPO}/contents/")) {
                return vec![one.to_owned()];
            }
            if path != format!("{REPO}/contents") {
                return Vec::new();
            }
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body["files"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|file| file["path"].as_str().map(str::to_owned))
                .collect()
        })
        .collect()
}

fn endpoint() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "ovzdusie-public", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld", "ogc-features"]
        }
    })
}

fn policy() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Policy",
        "metadata": { "name": "public-air-quality", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "assigner": "did:web:banskabystrica.sk",
            "assignee": { "kind": "role", "id": "public" },
            "operations": ["queryEntity", "retrieveEntity"],
            "information": [{ "entities": [{ "type": "AirQualityObserved" }] }]
        }
    })
}

fn pipeline() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": "smart-meter-mqtt", "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:ovzdusie-public",
            "quotas": { "maxMemoryMb": 128, "cpuMillicores": 250 }
        }
    })
}

fn role_binding() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": "ovzdusie-developers", "namespace": "org" },
        "spec": {
            "subjects": [{ "group": "air-quality-team" }],
            "role": "pipeline-developer",
            "scope": { "project": "ovzdusie" }
        }
    })
}

#[tokio::test]
async fn an_endpoint_a_pipeline_a_policy_and_a_role_binding_are_proposed_at_their_own_paths() {
    let gitea = forge(&["projects/ovzdusie/spaces/ovzdusie/endpoints/ovzdusie-public.yaml"]).await;
    let state = state_with(&gitea);
    // The endpoint exists, so its proposal is an update.
    let current = endpoint();
    state.mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie-public",
        "ovzdusie",
        current["spec"].clone(),
    ));
    let mut changed = endpoint();
    changed["spec"]["enabledRepresentations"] = json!(["ngsi-ld", "ogc-features", "csv"]);
    // The space the endpoint serves is in the project, as it has to be: a manifest naming a
    // resource that is not there is refused by the check (MF-13, T-2233).
    state
        .mirror
        .upsert(envelope("ContextSpace", "ovzdusie", "ovzdusie", json!({})));
    // A binding names a role and a group the organization has (PF-52, PF-62).
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-developer",
        "org",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "Group",
        "air-quality-team",
        "org",
        json!({ "displayName": "Air quality team" }),
    ));

    // A Pipeline proposes on a green record check of its spec (T-4840), which its workbench
    // keeps; this case is about the paths, so the check is recorded the way it would be.
    records_checked(&state, "ovzdusie", &pipeline()).await;

    for (project, manifest) in [
        ("ovzdusie", changed),
        ("ovzdusie", pipeline()),
        ("ovzdusie", policy()),
        ("org", role_binding()),
    ] {
        let kind = manifest["kind"].clone();
        let (status, change) = op(
            &state,
            steward(),
            project,
            "jc_resource_propose",
            json!({ "manifest": manifest }),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{kind}: {change}");
        assert_eq!(
            change["change"]["kind"],
            json!("Change"),
            "{kind}: {change}"
        );
    }

    let paths = written(&gitea).await;
    for expected in [
        "projects/ovzdusie/spaces/ovzdusie/endpoints/ovzdusie-public.yaml",
        "projects/ovzdusie/pipelines/smart-meter-mqtt/pipeline.yaml",
        "projects/ovzdusie/spaces/ovzdusie/policies/public-air-quality.yaml",
        "users/assignments/ovzdusie-developers.yaml",
    ] {
        assert!(
            paths.iter().any(|p| p == expected),
            "{expected} in {paths:?}"
        );
    }
}

#[tokio::test]
async fn the_route_and_the_operation_open_the_same_change() {
    let gitea = forge(&[]).await;
    let state = state_with(&gitea);

    let (rest_status, by_route) = send(
        &state,
        steward(),
        "POST",
        "/api/v1/projects/ovzdusie/policies",
        policy(),
    )
    .await;
    let (op_status, by_operation) = op(
        &state,
        steward(),
        "ovzdusie",
        "jc_resource_propose",
        json!({ "manifest": policy() }),
    )
    .await;

    assert_eq!(rest_status, StatusCode::ACCEPTED, "{by_route}");
    assert_eq!(op_status, rest_status, "{by_operation}");
    assert_eq!(
        serde_json::to_vec(&by_route).expect("json"),
        serde_json::to_vec(&by_operation["change"]).expect("json"),
        "the operation's answer carries the route's Change as it is"
    );
}

#[tokio::test]
async fn a_deletion_is_red_needs_the_name_typed_back_and_names_what_still_references_it() {
    let gitea = forge(&["projects/ovzdusie/spaces/mobility/endpoints/live-traffic.yaml"]).await;
    let state = state_with(&gitea);
    state
        .mirror
        .upsert(envelope("ContextSpace", "mobility", "ovzdusie", json!({})));
    state.mirror.upsert(envelope(
        "Endpoint",
        "live-traffic",
        "ovzdusie",
        json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" }, "slug": "k7m2p9q4r6s8t3v5w7x2y4z6a8" }),
    ));
    // A project elsewhere also reads the space: counted, never named.
    state.mirror.upsert(envelope(
        "SharedSpaceReference",
        "mobility-from-ovzdusie",
        "doprava",
        json!({ "target": { "kind": "ContextSpace", "name": "mobility", "namespace": "ovzdusie" } }),
    ));

    let (status, refused) = op(
        &state,
        steward(),
        "ovzdusie",
        "jc_resource_delete",
        json!({ "kind": "ContextSpace", "name": "mobility", "confirm": "mobility" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], json!("referenced"));
    assert_eq!(
        refused["by"],
        json!([{ "kind": "Endpoint", "name": "live-traffic" }])
    );
    assert_eq!(refused["elsewhere"], json!(1));
    assert!(
        !refused.to_string().contains("mobility-from-ovzdusie"),
        "{refused}"
    );

    let (status, mistyped) = op(
        &state,
        steward(),
        "ovzdusie",
        "jc_resource_delete",
        json!({ "kind": "Endpoint", "name": "live-traffic", "confirm": "live-trafic" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{mistyped}");
    assert!(
        written(&gitea).await.is_empty(),
        "nothing is written before the name matches"
    );

    let (status, change) = op(
        &state,
        steward(),
        "ovzdusie",
        "jc_resource_delete",
        json!({ "kind": "Endpoint", "name": "live-traffic", "confirm": "live-traffic" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{change}");
    assert_eq!(change["lane"], json!("red"), "{change}");
    assert_eq!(
        change["change"]["status"]["plan"]["delete"],
        json!(1),
        "{change}"
    );
    assert_eq!(
        written(&gitea).await,
        ["projects/ovzdusie/spaces/mobility/endpoints/live-traffic.yaml"]
    );
}

#[tokio::test]
async fn a_secret_typed_into_a_manifest_is_refused_before_a_branch_exists() {
    let gitea = forge(&[]).await;
    let state = state_with(&gitea);
    let mut manifest = pipeline();
    manifest["spec"]["source"] =
        json!({ "http": { "url": "https://feed.example.sk", "token": "hunter2" } });

    let (status, problem) = op(
        &state,
        steward(),
        "ovzdusie",
        "jc_resource_propose",
        json!({ "manifest": manifest }),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert!(problem.to_string().contains("secretRef"), "{problem}");
    let branches = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path().ends_with("/branches"))
        .count();
    assert_eq!(branches, 0, "no branch for a manifest with a secret in it");
}

#[tokio::test]
async fn a_person_without_the_verb_is_refused_with_it_and_an_agent_never_decides_a_change() {
    let gitea = forge(&[]).await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-developer",
        "org",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "developers",
        "org",
        json!({ "role": "pipeline-developer", "subjects": [{ "group": "devs" }], "scope": { "project": "ovzdusie" } }),
    ));
    let developer = identity("jana.kovacova", &["devs"]);

    let (status, refused) = op(
        &state,
        developer.clone(),
        "ovzdusie",
        "jc_resource_propose",
        json!({ "manifest": policy() }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(
        refused.to_string().contains("Policy"),
        "the refusal names the kind: {refused}"
    );

    let (status, refused) = op(
        &state,
        developer,
        "ovzdusie",
        "jc_change_reject",
        json!({ "id": "chg-00000009", "reason": "not now" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    let agent = Caller::new(steward(), Via::Agent);
    for (name, input) in [
        ("jc_change_approve", json!({ "id": "chg-00000009" })),
        (
            "jc_change_reject",
            json!({ "id": "chg-00000009", "reason": "the model said so" }),
        ),
    ] {
        let operation = ops::find(name).expect("registered");
        let refused = ops::call(operation, &agent, &state, "ovzdusie", input)
            .await
            .expect_err("an agent run never decides a change");
        assert!(refused.to_string().contains("AG-11"), "{name}: {refused}");
    }
    assert!(
        gitea
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "the refusals reach no forge"
    );
}

#[tokio::test]
async fn listing_and_reading_answer_by_kind_and_space_and_an_unknown_name_gets_the_real_ones() {
    let state = AppState::new(Config::for_tests(), None);
    for (name, space) in [("helsinki-bikes", "helsinki"), ("air-public", "ilma")] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            "helsinki",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space } }),
        ));
    }

    let (status, listed) = op(
        &state,
        steward(),
        "helsinki",
        "jc_resource_list",
        json!({ "kind": "Endpoint", "space": "helsinki" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    assert_eq!(listed["items"][0]["name"], json!("helsinki-bikes"));
    assert_eq!(listed["items"][0]["space"], json!("helsinki"));

    let (status, read) = op(
        &state,
        steward(),
        "helsinki",
        "jc_resource_get",
        json!({ "kind": "Endpoint", "name": "air-public" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["spec"]["contextSpaceRef"]["name"], json!("ilma"));

    let (status, missing) = op(
        &state,
        steward(),
        "helsinki",
        "jc_resource_get",
        json!({ "kind": "Endpoint", "name": "helsinki-bike" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert!(
        missing.to_string().contains("air-public, helsinki-bikes"),
        "{missing}"
    );

    let (status, unknown) = op(
        &state,
        steward(),
        "helsinki",
        "jc_resource_list",
        json!({ "kind": "Bikes" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert!(
        unknown.to_string().contains("Endpoint"),
        "the kinds there are: {unknown}"
    );
}

/// PF-59, T-0840: an organization-level kind lives in `org`, and the operations find it there
/// without the caller naming another project.
#[tokio::test]
async fn an_organization_kind_is_read_through_the_operations_of_any_project() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Role",
        "editor",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "editors",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "steward@hel.fi" }],
            "role": "editor",
            "scope": { "organization": "hel" }
        }),
    ));
    let state = AppState::new(config, None).with_mirror(mirror);
    let caller = Caller {
        bearer: None,
        identity: steward(),
        via: Via::Mcp,
        access: None,
    };

    let list = ops::find("jc_resource_list").expect("registered");
    let answer = ops::call(list, &caller, &state, "ovzdusie", json!({ "kind": "Role" }))
        .await
        .expect("the roles of the organization");
    let names: Vec<&str> = answer["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert_eq!(names, vec!["editor"], "{answer}");

    let get = ops::find("jc_resource_get").expect("registered");
    let answer = ops::call(
        get,
        &caller,
        &state,
        "ovzdusie",
        json!({ "kind": "RoleBinding", "name": "editors" }),
    )
    .await
    .expect("the binding itself");
    assert_eq!(answer["metadata"]["namespace"], json!("org"), "{answer}");

    // A name that is not there still names the ones that are, from the same namespace.
    let missing = ops::call(
        get,
        &caller,
        &state,
        "ovzdusie",
        json!({ "kind": "Role", "name": "steward" }),
    )
    .await
    .expect_err("no such role");
    assert!(format!("{missing:?}").contains("editor"), "{missing:?}");
}

/// PF-59, R20, T-0840: the registry answers a kind the caller's bindings do not read exactly as
/// it answers one that does not exist — the REST list has said so since T-0906.
#[tokio::test]
async fn a_kind_no_binding_reads_is_not_there_through_the_operations_either() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Endpoint",
        "public-air",
        "ovzdusie",
        json!({ "slug": "publicair00000000000000000000", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }),
    ));
    mirror.upsert(envelope(
        "Role",
        "pipelines-only",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "viewer-binding",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "viewer@banskabystrica.sk" }],
            "role": "pipelines-only",
            "scope": { "project": "ovzdusie" }
        }),
    ));
    let state = AppState::new(config, None).with_mirror(mirror);
    let caller = Caller {
        bearer: None,
        identity: identity("viewer", &[]),
        via: Via::Mcp,
        access: None,
    };

    let list = ops::find("jc_resource_list").expect("registered");
    let refused = ops::call(
        list,
        &caller,
        &state,
        "ovzdusie",
        json!({ "kind": "Endpoint" }),
    )
    .await
    .expect_err("a kind this binding does not read");
    let said = format!("{refused:?}");
    assert!(said.contains("not found"), "{said}");
    assert!(!said.contains("403") && !said.contains("Denied"), "{said}");

    // The kind it does read still answers.
    let allowed = ops::call(
        list,
        &caller,
        &state,
        "ovzdusie",
        json!({ "kind": "Pipeline" }),
    )
    .await
    .expect("the kind the binding reads");
    assert_eq!(allowed["items"], json!([]));
}
