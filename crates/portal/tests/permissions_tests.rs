//! T-0526: roles as code, enforced (PF-50). Who may propose, delete and approve comes from the
//! `Role` and `RoleBinding` manifests in the mirror, never from the token.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use jc_core::kinds::Verb;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::{effective, ORG_NAMESPACE};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "test-csrf-token-12345";

fn identity(email: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        second_factor: false,
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

/// A resource of the project the proposals in this file are about.
///
/// The manifests below name a space and a data source; since T-2233 the dry run resolves what a
/// manifest names, so the project has to really hold them — a proposal that could never be applied
/// is no test of who may propose it.
fn in_project(kind: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, "ovzdusie"),
        spec: json!({}),
        status: None,
    }
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// The docs example: a developer proposes pipelines and non-public endpoints (Architecture/12 §2a).
fn developer_role() -> ResourceEnvelope {
    org(
        "Role",
        "pipeline-developer",
        json!({ "rules": [
            { "kinds": ["Pipeline", "DataSource", "Mapping"], "verbs": ["propose"] },
            { "kinds": ["Endpoint"], "verbs": ["propose"], "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
        ]}),
    )
}

fn binding(
    name: &str,
    role: &str,
    subjects: Value,
    scope: Value,
    validity: Option<Value>,
) -> ResourceEnvelope {
    let mut spec = json!({ "subjects": subjects, "role": role, "scope": scope });
    if let Some(v) = validity {
        spec["validity"] = v;
    }
    org("RoleBinding", name, spec)
}

fn mirror_with(envelopes: Vec<ResourceEnvelope>) -> Mirror {
    let mirror = Mirror::new();
    for env in envelopes {
        mirror.upsert(env);
    }
    mirror
}

fn pipeline(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
        }
    })
}

fn endpoint(audience: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "air", "namespace": "ovzdusie" },
        "spec": { "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": audience, "enabledRepresentations": ["ngsi-ld"] }
    })
}

async fn post(
    app: axum::Router,
    config: &Config,
    who: Identity,
    uri: &str,
    body: &Value,
) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookies(config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn app_with(config: &Config, envelopes: Vec<ResourceEnvelope>) -> axum::Router {
    let state = AppState::new(config.clone(), None);
    for env in envelopes {
        state.mirror.upsert(env);
    }
    server::app(state)
}

#[tokio::test]
async fn a_user_with_no_binding_cannot_propose_and_the_403_names_the_verb() {
    let config = Config::for_tests();
    let app = app_with(&config, vec![developer_role()]);
    let (status, body) = post(
        app,
        &config,
        identity("viewer@hel.fi", &[]),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline("aq"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("propose") && body.contains("Pipeline"),
        "{body}"
    );
}

#[tokio::test]
async fn a_developer_proposes_a_pipeline_but_not_a_public_endpoint() {
    let config = Config::for_tests();
    let dev = || identity("jana@hel.fi", &["air-quality-team"]);
    let repo = || {
        vec![
            developer_role(),
            binding(
                "ovzdusie-developers",
                "pipeline-developer",
                json!([{ "group": "air-quality-team" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
            in_project("ContextSpace", "ovzdusie"),
            in_project("DataSource", "mqtt-mesto"),
        ]
    };
    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline("aq"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/endpoints?dryRun=All",
        &endpoint("organization"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/endpoints?dryRun=All",
        &endpoint("public"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("spec.audience"), "{body}");

    // The binding covers ovzdusie, not another project.
    let other = pipeline("aq");
    let mut other = other;
    other["metadata"]["namespace"] = json!("doprava");
    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/doprava/pipelines?dryRun=All",
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn deletion_needs_delete_and_the_bootstrap_group_needs_no_repository() {
    let config = Config::for_tests();
    let live = || {
        let mut env = serde_json::from_value::<ResourceEnvelope>(pipeline("aq")).expect("envelope");
        env.metadata.namespace = Some("ovzdusie".into());
        env
    };
    let dev = identity("jana@hel.fi", &["air-quality-team"]);
    let repo = vec![
        live(),
        developer_role(),
        binding(
            "ovzdusie-developers",
            "pipeline-developer",
            json!([{ "user": "jana@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
    ];
    let delete = |app: axum::Router, who: Identity, config: Config| async move {
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/projects/ovzdusie/pipelines/aq?dryRun=All")
                    .header(header::COOKIE, cookies(&config, who))
                    .header(CSRF_HEADER, CSRF)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        response.status()
    };
    assert_eq!(
        delete(app_with(&config, repo), dev, config.clone()).await,
        StatusCode::FORBIDDEN
    );

    // `portal-approver` is the bootstrap group of `Config::for_tests()`: everything, with an empty repository.
    let admin = identity("admin@hel.fi", &["portal-approver"]);
    assert_eq!(
        delete(app_with(&config, vec![live()]), admin, config.clone()).await,
        StatusCode::OK
    );
}

#[test]
fn approve_expiry_and_organization_scope_are_decided_by_the_bindings() {
    let now = Utc.with_ymd_and_hms(2026, 9, 7, 12, 0, 0).unwrap();
    let approver_role = org(
        "Role",
        "approver",
        json!({ "rules": [{ "kinds": ["Pipeline", "Endpoint"], "verbs": ["approve", "delete"] }] }),
    );
    let mirror = mirror_with(vec![
        developer_role(),
        approver_role,
        binding(
            "org-approvers",
            "approver",
            json!([{ "user": "boss@hel.fi" }]),
            json!({ "organization": "hel" }),
            None,
        ),
        binding(
            "gone",
            "approver",
            json!([{ "user": "former@hel.fi" }]),
            json!({ "organization": "hel" }),
            Some(json!({ "notAfter": "2026-01-01T00:00:00Z" })),
        ),
        binding(
            "devs",
            "pipeline-developer",
            json!([{ "group": "air-quality-team" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
    ]);
    let target = pipeline("aq");

    let boss = effective(
        &mirror,
        "platform-admins",
        &identity("boss@hel.fi", &[]),
        "any-project",
        now,
    );
    assert!(
        boss.check("Pipeline", Verb::Approve, Some(&target)).is_ok(),
        "organization scope covers every project"
    );
    assert!(
        boss.check("Pipeline", Verb::Propose, Some(&target))
            .is_err(),
        "approve is not propose"
    );

    let former = effective(
        &mirror,
        "platform-admins",
        &identity("former@hel.fi", &[]),
        "ovzdusie",
        now,
    );
    assert!(
        former.grants.is_empty(),
        "an expired binding grants nothing"
    );

    let dev = effective(
        &mirror,
        "platform-admins",
        &identity("jana@hel.fi", &["air-quality-team"]),
        "ovzdusie",
        now,
    );
    assert!(dev.check("Pipeline", Verb::Propose, Some(&target)).is_ok());
    let err = dev
        .check("Pipeline", Verb::Approve, Some(&target))
        .expect_err("a developer does not approve");
    assert!(err.to_string().contains("approve"), "{err}");
    assert_eq!(dev.grants.len(), 2, "one grant per rule of the role");
}

#[tokio::test]
async fn permissions_me_lists_the_grants_of_the_caller() {
    let config = Config::for_tests();
    let app = app_with(
        &config,
        vec![
            developer_role(),
            binding(
                "ovzdusie-developers",
                "pipeline-developer",
                json!([{ "group": "air-quality-team" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
        ],
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/permissions/me")
                .header(
                    header::COOKIE,
                    cookies(&config, identity("jana@hel.fi", &["air-quality-team"])),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let doc: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(doc["project"], "ovzdusie");
    assert_eq!(doc["bootstrap"], false);
    assert_eq!(doc["grants"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["grants"][0]["role"], "pipeline-developer");
    assert_eq!(
        doc["grants"][1]["rule"]["constraints"][0]["field"],
        "spec.audience"
    );
}

// --- PF-59: reading is a verb, per kind ------------------------------------------------------

async fn get(app: axum::Router, config: &Config, who: Identity, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookies(config, who))
                .body(Body::empty())
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A developer proposes pipelines, so they read pipelines; nobody gave them the project's
/// service accounts, and a kind they may not read is not there (PF-59, R20, T-0906).
#[tokio::test]
async fn a_list_answers_only_the_kinds_the_caller_reads() {
    let config = Config::for_tests();
    let envelopes = vec![
        developer_role(),
        binding(
            "developers",
            "pipeline-developer",
            json!([{ "user": "jana@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
    ];
    let who = identity("jana@hel.fi", &[]);

    let (status, body) = get(
        app_with(&config, envelopes.clone()),
        &config,
        who.clone(),
        "/api/v1/projects/ovzdusie/pipelines",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "propose implies read (PF-59): {body}"
    );

    let (status, body) = get(
        app_with(&config, envelopes.clone()),
        &config,
        who.clone(),
        "/api/v1/projects/ovzdusie/serviceaccounts",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body.contains("serviceaccounts") && !body.contains("secretRef"),
        "the refusal names what the caller typed and nothing else: {body}"
    );

    // The same rule on the single resource, and on the caller's own permissions document.
    let (status, _) = get(
        app_with(&config, envelopes.clone()),
        &config,
        who.clone(),
        "/api/v1/projects/ovzdusie/serviceaccounts/gateway",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(
        app_with(&config, envelopes),
        &config,
        who,
        "/api/v1/projects/ovzdusie/permissions/me",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the project itself is readable");
}

/// PF-56, PF-61: the `viewer` role the platform seeds reads every project kind and changes
/// nothing, which is what keeps every signed-in person's Portal readable (T-0906).
#[tokio::test]
async fn the_viewer_role_reads_every_kind_and_a_stranger_reads_none() {
    let config = Config::for_tests();
    let envelopes = vec![
        org(
            "Role",
            "viewer",
            json!({ "rules": [{ "kinds": ["Pipeline", "ServiceAccount", "Endpoint"], "verbs": ["read"] }] }),
        ),
        binding(
            "viewers",
            "viewer",
            json!([{ "group": "platform-readers" }]),
            json!({ "organization": "hel" }),
            None,
        ),
    ];
    for plural in ["pipelines", "serviceaccounts", "endpoints"] {
        let (status, body) = get(
            app_with(&config, envelopes.clone()),
            &config,
            identity("jana@hel.fi", &["platform-readers"]),
            &format!("/api/v1/projects/ovzdusie/{plural}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{plural}: {body}");
    }

    // Nobody put this person in the group, so the project reads like a project that is not
    // there — the one answer for "missing" and "not yours" (R20).
    for uri in [
        "/api/v1/projects/ovzdusie/pipelines",
        "/api/v1/projects/ovzdusie/permissions/me",
    ] {
        let (status, _) = get(
            app_with(&config, envelopes.clone()),
            &config,
            identity("stranger@hel.fi", &[]),
            uri,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

/// One Endpoint of `ovzdusie`, in the named context space.
fn endpoint_in(project: &str, name: &str, space: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Endpoint".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec: json!({ "contextSpaceRef": space, "audience": "internal", "slug": name }),
        status: None,
    }
}

/// The names of the endpoints an answer lists, as `project/name`.
fn listed(body: &str) -> Vec<String> {
    let doc: Value = serde_json::from_str(body).expect("json");
    doc["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            format!(
                "{}/{}",
                item["metadata"]["namespace"].as_str().unwrap_or_default(),
                item["metadata"]["name"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// PF-61, PF-03, R20, T-2877: `GET /api/v1/endpoints` is the organization-level page, an
/// administration view. An administrator bound at organization scope reads every project's
/// endpoints; a reader of Endpoints bound to the whole organization, one project or one context
/// space is not an administrator and is answered `404`, like a person no binding names.
#[tokio::test]
async fn the_organization_endpoint_list_answers_an_administrator_and_nobody_else() {
    let config = Config::for_tests();
    let reader = org(
        "Role",
        "endpoint-reader",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    );
    let administrator = org(
        "Role",
        "administrator",
        json!({ "rules": [
            { "kinds": ["RoleBinding"], "verbs": ["approve", "delete"] },
            { "kinds": ["Endpoint"], "verbs": ["read"] },
        ] }),
    );
    let world = vec![
        reader,
        administrator,
        endpoint_in("ovzdusie", "air-public", "vzduch"),
        endpoint_in("ovzdusie", "traffic-public", "doprava"),
        endpoint_in("helsinki", "bikes", "mobility"),
        // The space a space-scoped binding names has to exist for the binding to reach it.
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "ContextSpace".to_owned(),
            metadata: ObjectMeta::new("vzduch", "ovzdusie"),
            spec: json!({ "isSandbox": false }),
            status: None,
        },
    ];
    let bound = |name: &str, role: &str, scope: Value| {
        let mut envelopes = world.clone();
        envelopes.push(binding(
            name,
            role,
            json!([{ "user": "jana@hel.fi" }]),
            scope,
            None,
        ));
        envelopes
    };
    let who = identity("jana@hel.fi", &[]);
    let read_all = |envelopes: Vec<ResourceEnvelope>| {
        let config = config.clone();
        let who = who.clone();
        async move {
            get(
                app_with(&config, envelopes),
                &config,
                who,
                "/api/v1/endpoints",
            )
            .await
        }
    };

    // The administrator: every endpoint of every project, each with its project.
    let (status, body) = read_all(bound(
        "admins",
        "administrator",
        json!({ "organization": "bb" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        listed(&body),
        vec![
            "helsinki/bikes",
            "ovzdusie/air-public",
            "ovzdusie/traffic-public"
        ],
        "an administrator reads every project"
    );

    // A reader of Endpoints at any scope, and a person no binding names: `404`, nothing listed.
    for scope in [
        Some(json!({ "organization": "bb" })),
        Some(json!({ "project": "ovzdusie" })),
        Some(json!({ "contextSpace": "vzduch" })),
        None,
    ] {
        let envelopes = match &scope {
            Some(scope) => bound("readers", "endpoint-reader", scope.clone()),
            None => world.clone(),
        };
        let (status, body) = read_all(envelopes).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{scope:?}: {body}");
        assert!(listed(&body).is_empty(), "{scope:?}: {body}");
    }
}

/// PF-61: each grant says where it comes from, so the page can read "steward, inherited from the
/// organization" instead of making an inherited grant look local.
#[tokio::test]
async fn permissions_me_names_the_scope_each_grant_was_inherited_from() {
    let config = Config::for_tests();
    let envelopes = vec![
        org(
            "Role",
            "org-admin",
            json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read", "approve"] }] }),
        ),
        binding(
            "the-admins",
            "org-admin",
            json!([{ "user": "jana@hel.fi" }]),
            json!({ "organization": "bb" }),
            None,
        ),
    ];
    let (status, body) = get(
        app_with(&config, envelopes),
        &config,
        identity("jana@hel.fi", &[]),
        "/api/v1/projects/ovzdusie/permissions/me",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(doc["grants"][0]["scope"], "organization", "{body}");
    assert_eq!(doc["grants"][0]["role"], "org-admin", "{body}");
}

#[test]
fn the_janitor_approves_and_deletes_only_what_a_journey_named() {
    // T-2627: the residue sweep's own role, confined by the name a journey gives (PF-49).
    let now = Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap();
    let mirror = mirror_with(vec![
        org(
            "Role",
            "janitor",
            json!({ "rules": [{
                "kinds": ["Pipeline"],
                "verbs": ["approve", "delete"],
                "constraints": [{ "field": "metadata.name", "pattern": "t1[0-9]{3}[a-z]?-.+|.+-[0-9]{4}" }]
            }]}),
        ),
        binding(
            "janitors",
            "janitor",
            json!([{ "user": "janitor@hel.fi" }]),
            json!({ "organization": "hel" }),
            None,
        ),
    ]);
    let janitor = effective(
        &mirror,
        "platform-admins",
        &identity("janitor@hel.fi", &[]),
        "ovzdusie",
        now,
    );
    for name in ["t1588-bikes", "t1589r-bikes", "citybikes-0915"] {
        let target = pipeline(name);
        assert!(
            janitor
                .check("Pipeline", Verb::Approve, Some(&target))
                .is_ok(),
            "{name}"
        );
        assert!(
            janitor
                .check("Pipeline", Verb::Delete, Some(&target))
                .is_ok(),
            "{name}"
        );
    }
    for name in ["aq", "helsinki-t1588-bikes", "citybikes-09150"] {
        let err = janitor
            .check("Pipeline", Verb::Delete, Some(&pipeline(name)))
            .expect_err("a name nobody's journey gave");
        assert!(
            err.to_string()
                .contains("metadata.name does not satisfy role janitor (must match"),
            "{err}"
        );
    }
    assert!(
        janitor
            .check("Pipeline", Verb::Propose, Some(&pipeline("t1588-bikes")))
            .is_err(),
        "the janitor proposes nothing"
    );
    assert!(
        janitor.check("Pipeline", Verb::Delete, None).is_err(),
        "no manifest, no name to match"
    );
}

/// PF-87, T-2655: who reaches a project's repository in the forge. Read makes a reader, propose
/// a writer (and a reader); an organization binding reaches every project; a group counts through
/// its manifest; a binding to another project, to one space, or out of force reaches nobody.
#[test]
fn a_projects_readers_and_writers_come_from_the_bindings_that_reach_it() {
    use joinedcontext_portal::permissions::project_members;

    let viewer = org(
        "Role",
        "viewer",
        json!({ "rules": [{ "kinds": ["Pipeline", "Endpoint"], "verbs": ["read"] }] }),
    );
    let steward = org(
        "Role",
        "steward",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["approve"] }] }),
    );
    let mirror = mirror_with(vec![
        viewer,
        steward,
        developer_role(),
        binding(
            "review-here",
            "steward",
            json!([{ "user": "Steward@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
        binding(
            "review-one-space",
            "steward",
            json!([{ "user": "spacesteward@hel.fi" }]),
            json!({ "contextSpace": "air" }),
            None,
        ),
        org(
            "Group",
            "air-team",
            json!({ "members": [{ "user": "Eva@hel.fi" }] }),
        ),
        binding(
            "read-here",
            "viewer",
            json!([{ "user": "reader@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
        binding(
            "develop-here",
            "pipeline-developer",
            json!([{ "group": "air-team" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
        binding(
            "read-everywhere",
            "viewer",
            json!([{ "user": "auditor@hel.fi" }]),
            json!({ "organization": "hel" }),
            None,
        ),
        binding(
            "elsewhere",
            "pipeline-developer",
            json!([{ "user": "other@hel.fi" }]),
            json!({ "project": "doprava" }),
            None,
        ),
        binding(
            "one-space",
            "viewer",
            json!([{ "user": "space@hel.fi" }]),
            json!({ "contextSpace": "air" }),
            None,
        ),
        binding(
            "expired",
            "pipeline-developer",
            json!([{ "user": "gone@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            Some(json!({ "notAfter": "2026-01-01T00:00:00Z" })),
        ),
    ]);
    let now = Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap();
    let members = project_members(&mirror, "ovzdusie", now);
    let set = |people: &[&str]| people.iter().map(|p| (*p).to_owned()).collect();
    assert_eq!(
        members.readers,
        set(&["auditor@hel.fi", "eva@hel.fi", "reader@hel.fi"])
    );
    assert_eq!(members.writers, set(&["eva@hel.fi", "steward@hel.fi"]));
    // Who reviews a Change of the project, a preview's readers besides its author (CC-91): only
    // an approve grant at the project or the organization, never a proposer or a one-space one.
    assert_eq!(members.approvers, set(&["steward@hel.fi"]));
    // A project nobody is bound to has the organization's readers and nobody else.
    let other = project_members(&mirror, "nowhere", now);
    assert_eq!(other.readers, set(&["auditor@hel.fi"]));
    assert!(other.writers.is_empty());
    assert!(other.approvers.is_empty());
}

/// T-2852 (PF-59, R20, EP-83): asking for a catalogue draft is no oracle for another project's
/// Endpoint names. A caller who does not read the project gets one `404` for an Endpoint that
/// exists and one that does not, and so does a caller who reads the project but not its Endpoints.
#[tokio::test]
async fn a_catalogue_draft_tells_nobody_who_cannot_read_the_endpoints_which_names_exist() {
    let config = Config::for_tests();
    let endpoint_editor = org(
        "Role",
        "endpoint-editor",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["propose"] }] }),
    );
    let pipeline_reader = org(
        "Role",
        "pipeline-reader",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    );
    let app = app_with(
        &config,
        vec![
            endpoint_editor,
            pipeline_reader,
            binding(
                "elsewhere",
                "endpoint-editor",
                json!([{ "user": "outsider@hel.fi" }]),
                json!({ "project": "doprava" }),
                None,
            ),
            binding(
                "pipelines-only",
                "pipeline-reader",
                json!([{ "user": "pipes@hel.fi" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
            binding(
                "endpoints",
                "endpoint-editor",
                json!([{ "user": "editor@hel.fi" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
            endpoint_in("ovzdusie", "existing", "ovzdusie"),
        ],
    );
    let uri = "/api/v1/projects/ovzdusie/catalogue/drafts";
    let ask = |who: &str, name: &str| {
        let app = app.clone();
        let config = config.clone();
        let who = identity(who, &[]);
        let body = json!({ "endpoint": name });
        async move { post(app, &config, who, uri, &body).await }
    };

    for who in ["outsider@hel.fi", "pipes@hel.fi"] {
        let (existing, existing_body) = ask(who, "existing").await;
        let (missing, missing_body) = ask(who, "missing").await;
        assert_eq!(existing, StatusCode::NOT_FOUND, "{who}: {existing_body}");
        assert_eq!(missing, StatusCode::NOT_FOUND, "{who}: {missing_body}");
        assert_eq!(
            existing_body.replace("existing", "NAME"),
            missing_body.replace("missing", "NAME"),
            "{who} tells an existing Endpoint from a missing one"
        );
    }

    // Who may propose the kind here is answered as before: the name resolves, and the draft
    // stops at the project's missing catalogue, not at a permission.
    let (status, body) = ask("editor@hel.fi", "existing").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = ask("editor@hel.fi", "missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// A ServiceAccount of project `project`, with `roles` as its manifest declares them.
fn account(project: &str, name: &str, roles: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "ServiceAccount".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec: json!({
            "owner": { "user": "jana@hel.fi" },
            "purpose": "proposes pipelines from the vendor's CI",
            "roles": roles,
            "credentials": [{ "kind": "oauth-client", "name": "main" }]
        }),
        status: None,
    }
}

/// What a `client_credentials` token of Keycloak client `client` verifies to.
fn token_of(client: &str, username: &str) -> Identity {
    Identity {
        client: Some(client.to_owned()),
        subject: format!("sa:{client}"),
        username: username.to_owned(),
        email: None,
        name: None,
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn pipeline_admin_role() -> ResourceEnvelope {
    org(
        "Role",
        "pipeline-admin",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose", "approve", "delete"] }] }),
    )
}

#[test]
fn a_service_accounts_declared_roles_are_in_force_for_its_own_client_and_nobody_else() {
    let now = Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap();
    let roles = json!([
        { "role": "pipeline-admin", "scope": { "project": "ovzdusie" } },
        // A gateway template: data access through Policies, nothing in the Portal (CC-60).
        { "role": "space-reader", "scope": { "contextSpace": "ovzdusie" } }
    ]);
    let mirror = mirror_with(vec![
        pipeline_admin_role(),
        account("ovzdusie", "vendor-ci", roles.clone()),
    ]);
    let target = pipeline("aq");
    let effective_for =
        |who: &Identity, project: &str| effective(&mirror, "platform-admins", who, project, now);

    let account_token = token_of("ovzdusie-vendor-ci", "service-account-ovzdusie-vendor-ci");
    let own = effective_for(&account_token, "ovzdusie");
    assert!(own.check("Pipeline", Verb::Propose, Some(&target)).is_ok());
    assert!(own.check("Pipeline", Verb::Delete, Some(&target)).is_ok());
    assert_eq!(
        own.grants.len(),
        1,
        "one Role rule; the template grants nothing: {:?}",
        own.grants
    );
    assert_eq!(own.grants[0].binding, "serviceaccount/vendor-ci");
    assert!(
        effective_for(&account_token, "doprava").grants.is_empty(),
        "the role holds on the scope the manifest wrote beside it and nowhere else"
    );

    // A person who signed in through the account's client is not the account.
    let person = token_of("ovzdusie-vendor-ci", "jana");
    assert!(effective_for(&person, "ovzdusie").grants.is_empty());
    // A person whose user name looks like the account's, from another client or a cookie.
    let lookalike = Identity {
        client: None,
        ..token_of("x", "service-account-ovzdusie-vendor-ci")
    };
    assert!(effective_for(&lookalike, "ovzdusie").grants.is_empty());
    // A client no manifest derives.
    let unknown = token_of("ovzdusie-other", "service-account-ovzdusie-other");
    assert!(effective_for(&unknown, "ovzdusie").grants.is_empty());

    // An id two accounts derive is nobody's (T-1454): `ovzdusie` + `x-ci` and `ovzdusie-x` + `ci`.
    let twice = mirror_with(vec![
        pipeline_admin_role(),
        account("ovzdusie", "x-ci", roles.clone()),
        account("ovzdusie-x", "ci", roles),
    ]);
    let ambiguous = token_of("ovzdusie-x-ci", "service-account-ovzdusie-x-ci");
    assert!(
        effective(&twice, "platform-admins", &ambiguous, "ovzdusie", now)
            .grants
            .is_empty()
    );
}

#[test]
fn a_service_account_may_propose_and_never_approve() {
    let now = Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap();
    let approve_only = org(
        "Role",
        "pipeline-approver",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["approve"] }] }),
    );
    let mirror = mirror_with(vec![
        pipeline_admin_role(),
        approve_only,
        account(
            "ovzdusie",
            "vendor-ci",
            json!([
                { "role": "pipeline-admin", "scope": { "project": "ovzdusie" } },
                { "role": "pipeline-approver", "scope": { "organization": "hel" } }
            ]),
        ),
    ]);
    let token = token_of("ovzdusie-vendor-ci", "service-account-ovzdusie-vendor-ci");
    let held = effective(&mirror, "platform-admins", &token, "ovzdusie", now);
    let target = pipeline("aq");
    assert!(held.check("Pipeline", Verb::Propose, Some(&target)).is_ok());
    let refused = held
        .check("Pipeline", Verb::Approve, Some(&target))
        .expect_err("a workload proposes, a person approves (PF-58)");
    assert!(refused.to_string().contains("approve"), "{refused}");
    assert!(
        held.grants
            .iter()
            .all(|grant| !grant.rule.verbs.contains(&Verb::Approve)),
        "approve is taken out of every rule, and a rule of approve alone is gone: {:?}",
        held.grants
    );
    assert_eq!(held.grants.len(), 1);
}

// --- T-4589: a space binding applies in the project that holds the space ---------------------

fn space_in(project: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "ContextSpace".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec: json!({}),
        status: None,
    }
}

fn in_(project: &str, kind: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec: json!({}),
        status: None,
    }
}

/// The steward of space `air`: reads and proposes pipelines and entities, scoped to that space.
fn air_steward() -> Vec<ResourceEnvelope> {
    vec![
        org(
            "Role",
            "space-steward",
            json!({ "rules": [
                { "kinds": ["Pipeline", "Entity", "ContextSpace"], "verbs": ["read", "propose"] }
            ]}),
        ),
        binding(
            "air-steward",
            "space-steward",
            json!([{ "user": "steward@ovzdusie.sk" }]),
            json!({ "contextSpace": "air" }),
            None,
        ),
    ]
}

/// The binding names `air` and no project: it holds in `ovzdusie`, which holds `air`, and in no
/// other project, so `helsinki` reads like a project that is not there (PF-59, R20).
#[test]
fn a_space_binding_holds_only_in_the_project_that_holds_the_space() {
    let mut envelopes = air_steward();
    envelopes.push(space_in("ovzdusie", "air"));
    envelopes.push(space_in("helsinki", "bikes"));
    let mirror = mirror_with(envelopes);
    let steward = identity("steward@ovzdusie.sk", &[]);
    let now = Utc::now();

    let home = effective(&mirror, "", &steward, "ovzdusie", now);
    assert!(home.may_read_project());
    assert!(home.may_in("Entity", Verb::Propose, Some("air")));
    assert!(!home.may_in("Entity", Verb::Propose, Some("other")));

    let elsewhere = effective(&mirror, "", &steward, "helsinki", now);
    assert!(elsewhere.grants.is_empty(), "{:?}", elsewhere.grants);
    assert!(!elsewhere.may_read_project());
    assert!(!elsewhere.may("Pipeline", Verb::Propose));
}

/// A space name is unique in the organization (PF-44, PF-76). A mirror that holds `air` in two
/// projects breaks that, and the binding then grants nothing in either rather than both.
#[test]
fn a_space_name_held_by_two_projects_grants_nothing_in_either() {
    let mut envelopes = air_steward();
    envelopes.push(space_in("ovzdusie", "air"));
    envelopes.push(space_in("helsinki", "air"));
    let mirror = mirror_with(envelopes);
    let steward = identity("steward@ovzdusie.sk", &[]);
    for project in ["ovzdusie", "helsinki"] {
        let granted = effective(&mirror, "", &steward, project, Utc::now());
        assert!(granted.grants.is_empty(), "{project}: {:?}", granted.grants);
        assert!(
            !granted.may_in("Entity", Verb::Propose, Some("air")),
            "{project}"
        );
    }
}

/// The routes the review named: another project's drift, activity, pipeline templates and a
/// replay of its rejected records are `404` for the steward of `ovzdusie/air`; the steward's own
/// project still answers, except its pipeline templates, which live in no space (T-4593).
#[tokio::test]
async fn the_steward_of_one_space_finds_nothing_in_another_project() {
    let config = Config::for_tests();
    let mut envelopes = air_steward();
    envelopes.push(space_in("ovzdusie", "air"));
    envelopes.push(space_in("helsinki", "bikes"));
    envelopes.push(in_("helsinki", "Pipeline", "bikes-feed"));
    let steward = || identity("steward@ovzdusie.sk", &[]);

    for uri in [
        "/api/v1/projects/helsinki/drift",
        "/api/v1/projects/helsinki/activity",
        "/api/v1/projects/helsinki/pipelinetemplates",
    ] {
        let (status, body) = get(
            app_with(&config, envelopes.clone()),
            &config,
            steward(),
            uri,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    let (status, body) = post(
        app_with(&config, envelopes.clone()),
        &config,
        steward(),
        "/api/v1/projects/helsinki/pipelines/bikes-feed/rejected/retry",
        &json!({ "ids": [1] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = get(
        app_with(&config, envelopes.clone()),
        &config,
        steward(),
        "/api/v1/projects/ovzdusie/drift",
    )
    .await;
    assert_ne!(status, StatusCode::NOT_FOUND, "{body}");
    // T-4593: pipeline templates live in no space, so a space binding reaches them in no
    // project, its own included.
    let (status, body) = get(
        app_with(&config, envelopes),
        &config,
        steward(),
        "/api/v1/projects/ovzdusie/pipelinetemplates",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// T-4839: a Pipeline that carries a `spec.lifetime`, for the door's two refusals below.
fn pipeline_with_lifetime(lifetime: Value) -> Value {
    let mut body = pipeline("aq");
    body["spec"]["lifetime"] = lifetime;
    body
}

#[tokio::test]
async fn a_steward_without_the_pipeline_grant_cannot_set_a_lifetime() {
    let config = Config::for_tests();
    let steward = org(
        "Role",
        "space-steward",
        json!({ "rules": [{ "kinds": ["Endpoint", "Policy", "ContextSpace"], "verbs": ["propose"] }] }),
    );
    let app = app_with(
        &config,
        vec![
            steward,
            binding(
                "ovzdusie-stewards",
                "space-steward",
                json!([{ "user": "steward@hel.fi" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
            in_project("ContextSpace", "ovzdusie"),
            in_project("DataSource", "mqtt-mesto"),
        ],
    );
    let (status, body) = post(
        app,
        &config,
        identity("steward@hel.fi", &[]),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline_with_lifetime(
            json!([{ "type": "AirQualityObserved", "entity": { "rolling": "PT1H" } }]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("propose") && body.contains("Pipeline"),
        "{body}"
    );
}

#[tokio::test]
async fn a_lifetime_naming_no_slot_of_the_spaces_model_is_refused_before_a_change_exists() {
    use std::collections::HashMap;
    use std::sync::Arc;
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    for env in [
        developer_role(),
        binding(
            "ovzdusie-developers",
            "pipeline-developer",
            json!([{ "group": "air-quality-team" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
        // The proposer reads the target space's entities, as PL-94 asks of a pipeline's writer.
        org(
            "Role",
            "entity-reader",
            json!({ "rules": [{ "kinds": ["Entity"], "verbs": ["read"] }] }),
        ),
        binding(
            "ovzdusie-readers",
            "entity-reader",
            json!([{ "group": "air-quality-team" }]),
            json!({ "contextSpace": "ovzdusie" }),
            None,
        ),
        in_project("ContextSpace", "ovzdusie"),
        in_project("DataSource", "mqtt-mesto"),
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new("public-air", "ovzdusie"),
            spec: json!({ "contextSpaceRef": "ovzdusie", "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "audience": "public" }),
            status: None,
        },
    ] {
        state.mirror.upsert(env);
    }
    let schema = json!({ "definitions": { "AirQualityObserved": { "type": "object", "properties": {
        "dateObserved": { "type": "string", "format": "date-time", "x-ngsi-ld-kind": "Property" },
        "pm10": { "type": "number", "x-ngsi-ld-kind": "Property" }
    }}}});
    let model = joinedcontext_portal::pipeline_validation::ModelSchema::compile(
        "bb-air-quality",
        "1.0.0",
        &schema,
        &["AirQualityObserved".to_owned()],
        false,
    );
    state.model_schemas.replace(HashMap::from([(
        ("ovzdusie".to_owned(), "ovzdusie".to_owned()),
        Arc::new(model),
    )]));
    let dev = identity("jana@hel.fi", &["air-quality-team"]);
    let lifetime = json!([{ "type": "AirQualityObserved", "attributes": [{ "names": ["pm25"], "rolling": "PT1H" }] }]);
    let (status, body) = post(
        server::app(state.clone()),
        &config,
        dev.clone(),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline_with_lifetime(lifetime),
    )
    .await;
    // The dry run answers the door's refusal as a red verdict, the finding naming the slots.
    assert_eq!(status, StatusCode::OK, "{body}");
    let verdict: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(verdict["verdict"]["ok"], json!(false), "{body}");
    let finding = verdict["verdict"]["findings"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        finding.starts_with(
            "spec.lifetime[0].attributes[0].names: `pm25` is not a slot of `AirQualityObserved`"
        ),
        "{body}"
    );
    assert!(
        finding.ends_with("the slots of `AirQualityObserved` are dateObserved, pm10"),
        "{body}"
    );

    let fits = json!([{ "type": "AirQualityObserved", "entity": { "from": "dateObserved", "plus": "P1D" } }]);
    let (status, body) = post(
        server::app(state),
        &config,
        dev,
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline_with_lifetime(fits),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let verdict: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(verdict["valid"], json!(true), "{body}");
}

/// A ServiceAccount grant carries a window (AG-131): before `notBefore` and after `notAfter` it
/// grants nothing, at that second and before any reaper removed it, and the Access tab reads its
/// state beside the rules it would carry, `approve` never among them (PF-58).
#[test]
fn an_account_grant_outside_its_window_grants_nothing() {
    use joinedcontext_portal::permissions::{account_access, GrantState};
    let until = Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap();
    let roles = json!([
        { "role": "pipeline-admin", "scope": { "project": "ovzdusie" },
          "validity": { "notAfter": "2026-10-21T12:00:00Z" } },
        { "role": "pipeline-admin", "scope": { "project": "doprava" },
          "validity": { "notBefore": "2026-10-22T00:00:00Z" } }
    ]);
    let mirror = mirror_with(vec![
        pipeline_admin_role(),
        account("ovzdusie", "vendor-ci", roles),
    ]);
    let target = pipeline("aq");
    let token = token_of("ovzdusie-vendor-ci", "service-account-ovzdusie-vendor-ci");
    let at = |now| effective(&mirror, "platform-admins", &token, "ovzdusie", now);
    assert!(at(until)
        .check("Pipeline", Verb::Propose, Some(&target))
        .is_ok());
    let after = until + chrono::Duration::seconds(1);
    assert!(at(after)
        .check("Pipeline", Verb::Propose, Some(&target))
        .is_err());
    assert!(
        effective(&mirror, "platform-admins", &token, "doprava", after)
            .grants
            .is_empty(),
        "a grant whose window has not opened grants nothing yet"
    );

    let spec: jc_core::kinds::ServiceAccountSpec = serde_json::from_value(
        mirror
            .get("ovzdusie", "ServiceAccount", "vendor-ci")
            .unwrap()
            .spec,
    )
    .unwrap();
    let access = account_access(&mirror, &spec, after);
    assert_eq!(access[0].state, GrantState::Expired);
    assert_eq!(access[1].state, GrantState::Pending);
    assert_eq!(access[0].scope, "project:ovzdusie");
    assert!(access[0]
        .rules
        .iter()
        .all(|rule| !rule.verbs.contains(&Verb::Approve)));
    assert!(access[0].rules[0].verbs.contains(&Verb::Delete));
}

/// T-4845: a Pipeline that carries a `spec.history`, for the door's refusals below.
fn pipeline_with_history(history: Value) -> Value {
    let mut body = pipeline("aq");
    body["spec"]["history"] = history;
    body
}

#[tokio::test]
async fn a_steward_without_the_pipeline_grant_cannot_set_a_history_plan() {
    let config = Config::for_tests();
    let steward = org(
        "Role",
        "space-steward",
        json!({ "rules": [{ "kinds": ["Endpoint", "Policy", "ContextSpace"], "verbs": ["propose"] }] }),
    );
    let app = app_with(
        &config,
        vec![
            steward,
            binding(
                "ovzdusie-stewards",
                "space-steward",
                json!([{ "user": "steward@hel.fi" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
            in_project("ContextSpace", "ovzdusie"),
            in_project("DataSource", "mqtt-mesto"),
        ],
    );
    let (status, body) = post(
        app,
        &config,
        identity("steward@hel.fi", &[]),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline_with_history(json!([{ "type": "AirQualityObserved", "attributes": [
            { "names": ["pm10"], "keep": "changes", "heartbeat": "PT1H" }
        ] }])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("propose") && body.contains("Pipeline"),
        "{body}"
    );
}

/// Two Organizations (PF-121): `hel` the installation's, `zilina` owning the project `ovzdusie`;
/// each with an administrator bound at its own organization scope, and the role both hold.
fn two_organizations() -> Vec<ResourceEnvelope> {
    let admin_role = org(
        "Role",
        "org-admin",
        json!({ "rules": [
            { "kinds": ["Pipeline", "DataSource", "Endpoint", "ContextSpace"], "verbs": ["propose", "approve", "delete"] },
            { "kinds": ["RoleBinding", "Organization", "ContextBroker"], "verbs": ["propose", "approve", "delete"] }
        ]}),
    );
    vec![
        org(
            "Organization",
            "hel",
            json!({ "domain": "hel.fi", "installation": true }),
        ),
        org("Organization", "zilina", json!({ "domain": "zilina.sk" })),
        org(
            "Project",
            "ovzdusie",
            json!({ "organizationRef": { "kind": "Organization", "name": "zilina" } }),
        ),
        org(
            "Project",
            "helsinki",
            json!({ "organizationRef": { "kind": "Organization", "name": "hel" } }),
        ),
        admin_role,
        binding(
            "hel-admins",
            "org-admin",
            json!([{ "user": "jana@hel.fi" }]),
            json!({ "organization": "hel" }),
            None,
        ),
        binding(
            "zilina-admins",
            "org-admin",
            json!([{ "user": "eva@zilina.sk" }]),
            json!({ "organization": "zilina" }),
            None,
        ),
        in_project("ContextSpace", "ovzdusie"),
        in_project("DataSource", "mqtt-mesto"),
    ]
}

/// PF-123 (ADR-N-058 section 2.4): an organization-scoped binding ends at its Organization. The
/// Helsinki administrator proposes nothing in Žilina's project, the Žilina administrator proposes
/// there exactly what the Helsinki one did before the move.
#[tokio::test]
async fn pf_123_an_organization_admin_proposes_only_in_its_own_organizations_projects() {
    let config = Config::for_tests();
    let target = pipeline("aq");
    let uri = "/api/v1/projects/ovzdusie/pipelines?dryRun=All";
    let app = || app_with(&config, two_organizations());
    let (status, body) = post(app(), &config, identity("jana@hel.fi", &[]), uri, &target).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = post(app(), &config, identity("eva@zilina.sk", &[]), uri, &target).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Reading the project's private parts: the stranger organization's admin reads nothing.
    let read = |who: &str| {
        get(
            app_with(&config, two_organizations()),
            &config,
            identity(who, &[]),
            "/api/v1/projects/ovzdusie/datasources",
        )
    };
    let (status, body) = read("jana@hel.fi").await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status} {body}"
    );
    assert!(!body.contains("mqtt-mesto"), "{body}");
    let (status, body) = read("eva@zilina.sk").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("mqtt-mesto"), "{body}");
}

/// PF-123: approving follows the same line, and the installation-wide kinds (bindings, brokers,
/// every Organization) stay with the installation's administrators: a change filed in Žilina's
/// project carries none of them past its approver.
#[test]
fn pf_123_approval_and_installation_kinds_end_at_the_organization() {
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let mirror = mirror_with(two_organizations());
    let target = pipeline("aq");
    let at = |who: &str, project: &str| {
        effective(
            &mirror,
            "platform-admins",
            &identity(who, &[]),
            project,
            now,
        )
    };

    let hel_in_zilina = at("jana@hel.fi", "ovzdusie");
    assert!(hel_in_zilina.grants.is_empty());
    assert!(hel_in_zilina
        .check("Pipeline", Verb::Approve, Some(&target))
        .is_err());
    assert!(!hel_in_zilina.may_read_project());

    let zilina = at("eva@zilina.sk", "ovzdusie");
    assert!(zilina
        .check("Pipeline", Verb::Approve, Some(&target))
        .is_ok());
    assert!(zilina
        .check("Pipeline", Verb::Propose, Some(&target))
        .is_ok());
    for kind in ["RoleBinding", "Organization", "ContextBroker"] {
        assert!(
            !zilina.may(kind, Verb::Propose),
            "{kind} in a Žilina project"
        );
        assert!(
            !zilina.may(kind, Verb::Approve),
            "{kind} in a Žilina project"
        );
    }
    // At the installation's namespace only Žilina's own settings (T-5248), none of the
    // installation's projects.
    assert!(at("eva@zilina.sk", ORG_NAMESPACE)
        .grants
        .iter()
        .all(|g| g.organization.as_deref() == Some("zilina")));
    assert!(at("eva@zilina.sk", "helsinki").grants.is_empty());

    // The installation's administrator keeps the installation-wide kinds and its own projects.
    let hel = at("jana@hel.fi", ORG_NAMESPACE);
    assert!(hel.may("RoleBinding", Verb::Approve) && hel.may("Organization", Verb::Propose));
    assert!(at("jana@hel.fi", "helsinki")
        .check("Pipeline", Verb::Approve, Some(&target))
        .is_ok());
}

/// One Organization, as every installation before ADR-N-058: a binding at organization scope
/// still covers every project, whatever name it gives (PF-03).
#[test]
fn pf_123_one_organization_covers_every_project_as_before() {
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let mut envelopes = two_organizations();
    envelopes.retain(|env| !(env.kind == "Organization" && env.metadata.name == "zilina"));
    let mirror = mirror_with(envelopes);
    let hel = effective(
        &mirror,
        "platform-admins",
        &identity("jana@hel.fi", &[]),
        "ovzdusie",
        now,
    );
    assert!(hel
        .check("Pipeline", Verb::Approve, Some(&pipeline("aq")))
        .is_ok());
    assert!(hel.may("RoleBinding", Verb::Approve));
}

/// [`two_organizations`] with Groups in the administrators' role, a group a Helsinki binding
/// names and a request helper for any method (T-5248).
fn own_settings_repository() -> Vec<ResourceEnvelope> {
    let mut envelopes = two_organizations();
    envelopes.retain(|env| env.kind != "Role");
    envelopes.push(org(
        "Role",
        "org-admin",
        json!({ "rules": [
            { "kinds": ["Pipeline", "DataSource", "Endpoint", "ContextSpace"], "verbs": ["propose", "approve", "delete"] },
            { "kinds": ["RoleBinding", "Organization", "ContextBroker", "Group"], "verbs": ["propose", "approve", "delete"] }
        ]}),
    ));
    envelopes.push(binding(
        "hel-team",
        "org-admin",
        json!([{ "group": "hel-stewards" }]),
        json!({ "project": "helsinki" }),
        None,
    ));
    envelopes.push(org(
        "Group",
        "hel-stewards",
        json!({ "members": [{ "user": "pekka@hel.fi" }] }),
    ));
    // A Policy of Helsinki's grants its data to every token holding the groups it names.
    envelopes.push(ResourceEnvelope {
        spec: json!({ "contextSpaceRef": "bikes", "assignee": { "kind": "group", "id": "helsinki" },
                      "rules": [{ "assignee": { "kind": "group", "id": "zilina-bikes" } }] }),
        ..in_("helsinki", "Policy", "bikes-ops-read")
    });
    // A Helsinki App named zilina-x grants its roles to its default group by name (AP-118), and a
    // second one names a plain group in its access.
    envelopes.push(ResourceEnvelope {
        spec: json!({ "access": [
            { "role": "steward", "subjects": [{ "group": "zilina-x-steward" }] },
            { "role": "viewer", "subjects": [{ "group": "zilina-x-viewers" }] }
        ]}),
        ..in_("helsinki", "App", "zilina-x")
    });
    let mut default_group = org(
        "Group",
        "zilina-x-steward",
        json!({ "members": [{ "user": "pekka@hel.fi" }] }),
    );
    default_group.metadata.annotations.insert(
        "joinedcontext.com/app".to_owned(),
        "helsinki/zilina-x".to_owned(),
    );
    envelopes.push(default_group);
    envelopes
}

async fn send(
    config: &Config,
    method: &str,
    who: &str,
    uri: &str,
    body: Option<&Value>,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookies(config, identity(who, &[])))
        .header(CSRF_HEADER, CSRF)
        .header(header::CONTENT_TYPE, "application/json");
    let body = body.map_or_else(Body::empty, |b| {
        Body::from(serde_json::to_vec(b).expect("json"))
    });
    let response = app_with(config, own_settings_repository())
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn manifest(kind: &str, name: &str, spec: Value) -> Value {
    json!({ "apiVersion": API_VERSION, "kind": kind,
            "metadata": { "name": name, "namespace": ORG_NAMESPACE }, "spec": spec })
}

/// Peter is named by a Keycloak user id (ADR-N-061, PF-126).
fn zilina_binding(name: &str, scope: Value) -> Value {
    manifest(
        "RoleBinding",
        name,
        json!({ "subjects": [{ "user": "5e1d7c3b-9a2f-4b8e-a6d4-2c0f8e9b1a73" }], "role": "org-admin", "scope": scope }),
    )
}

/// ADR-N-058 section 2.4 (T-5248): the Žilina administrator edits Žilina's own Organization and
/// never Helsinki's nor the installation flag; binds people inside Žilina (its Organization, its
/// project) and nowhere else.
#[tokio::test]
async fn an_organization_admin_changes_its_own_organization_and_binds_inside_it() {
    let config = Config::for_tests();
    let eva = "eva@zilina.sk";
    let put = |name: &str, spec: Value| {
        let uri = format!("/api/v1/projects/org/organizations/{name}?dryRun=All");
        let body = manifest("Organization", name, spec);
        let config = config.clone();
        async move { send(&config, "PUT", eva, &uri, Some(&body)).await }
    };
    let (status, body) = put(
        "zilina",
        json!({ "domain": "zilina.sk", "locales": ["sk", "en"], "defaultLocale": "sk" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#""valid":true"#), "{body}");
    let (status, body) = put("hel", json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en", "installation": true })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = put("zilina", json!({ "domain": "zilina.sk", "locales": ["sk"], "defaultLocale": "sk", "installation": true })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("spec.installation"), "{body}");

    let bind = |scope: Value| {
        let body = zilina_binding("zilina-peter", scope);
        let config = config.clone();
        async move {
            send(
                &config,
                "POST",
                eva,
                "/api/v1/projects/org/rolebindings?dryRun=All",
                Some(&body),
            )
            .await
        }
    };
    for scope in [
        json!({ "organization": "zilina" }),
        json!({ "project": "ovzdusie" }),
    ] {
        let (status, body) = bind(scope.clone()).await;
        assert_eq!(status, StatusCode::OK, "{scope}: {body}");
        assert!(body.contains(r#""valid":true"#), "{scope}: {body}");
    }
    for scope in [
        json!({ "organization": "hel" }),
        json!({ "project": "helsinki" }),
    ] {
        let (status, body) = bind(scope.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{scope}: {body}");
    }
}

/// T-5248: what stands in the repository for another Organization is not rewritten into one's
/// own, read, listed or deleted; a group a Helsinki binding names stays Helsinki's.
#[tokio::test]
async fn an_organization_admin_reaches_no_binding_or_group_of_another_organization() {
    let config = Config::for_tests();
    let eva = "eva@zilina.sk";
    let takeover = zilina_binding("hel-admins", json!({ "organization": "zilina" }));
    let (status, body) = send(
        &config,
        "PUT",
        eva,
        "/api/v1/projects/org/rolebindings/hel-admins?dryRun=All",
        Some(&takeover),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = send(
        &config,
        "GET",
        eva,
        "/api/v1/projects/org/rolebindings",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("zilina-admins"), "{body}");
    assert!(
        !body.contains("hel-admins") && !body.contains("jana@hel.fi"),
        "{body}"
    );
    let (status, _) = send(
        &config,
        "GET",
        eva,
        "/api/v1/projects/org/rolebindings/hel-admins",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &config,
        "DELETE",
        eva,
        "/api/v1/projects/org/rolebindings/hel-admins?dryRun=All",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        &config,
        "GET",
        eva,
        "/api/v1/projects/org/organizations",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("zilina.sk") && !body.contains("hel.fi"),
        "{body}"
    );

    let group = |name: &str| {
        manifest(
            "Group",
            name,
            json!({ "members": [{ "user": "5e1d7c3b-9a2f-4b8e-a6d4-2c0f8e9b1a73" }] }),
        )
    };
    let (status, body) = send(
        &config,
        "PUT",
        eva,
        "/api/v1/projects/org/groups/hel-stewards?dryRun=All",
        Some(&group("hel-stewards")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = send(
        &config,
        "POST",
        eva,
        "/api/v1/projects/org/groups?dryRun=All",
        Some(&group("zilina-stewards")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#""valid":true"#), "{body}");
    // A group a Policy of another Organization names, or a name Žilina does not own, would put
    // Žilina's people into Helsinki's data (integrator-2 review of T-5248).
    for name in ["helsinki", "zilina-bikes", "bbsk-zaznamy-steward"] {
        let (status, body) = send(
            &config,
            "POST",
            eva,
            "/api/v1/projects/org/groups?dryRun=All",
            Some(&group(name)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body}");
        assert!(body.contains("metadata.name"), "{name}: {body}");
    }
    let mut app_group = group("zilina-app");
    app_group["metadata"]["annotations"] = json!({ "joinedcontext.com/app": "helsinki/bikes" });
    let (status, body) = send(
        &config,
        "POST",
        eva,
        "/api/v1/projects/org/groups?dryRun=All",
        Some(&app_group),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("metadata.annotations"), "{body}");
    // An App outside Žilina grants its role to these groups by name: rewriting the default group
    // without its annotation, or a group the App's access names, would hand Žilina that role
    // (integrator-2 second review of T-5248).
    for name in ["zilina-x-steward", "zilina-x-viewers"] {
        let (status, body) = send(
            &config,
            "PUT",
            eva,
            &format!("/api/v1/projects/org/groups/{name}?dryRun=All"),
            Some(&group(name)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body}");
    }
    let (status, body) = send(
        &config,
        "DELETE",
        eva,
        "/api/v1/projects/org/groups/zilina-x-steward?dryRun=All",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = send(&config, "GET", eva, "/api/v1/projects/org/groups", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("zilina-x-steward"), "{body}");
}

/// T-5248: the narrowed grants never make the Žilina administrator one of the installation's.
#[test]
fn own_settings_grants_administer_nothing_of_the_installation() {
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let mirror = mirror_with(own_settings_repository());
    let eva = effective(
        &mirror,
        "platform-admins",
        &identity("eva@zilina.sk", &[]),
        ORG_NAMESPACE,
        now,
    );
    assert!(!eva.grants.is_empty());
    assert!(eva
        .grants
        .iter()
        .all(|g| g.organization.as_deref() == Some("zilina")));
    assert!(!eva.administers_organization());
    assert!(!eva.may_read("RoleBinding") && eva.may_read_some("RoleBinding"));
    assert!(eva.may("RoleBinding", Verb::Propose));
    assert!(!eva.may("ContextBroker", Verb::Propose));
    assert!(!eva.may("Organization", Verb::Delete));
    // A scope naming two levels applies at the widest: Helsinki's.
    let mixed = zilina_binding(
        "mixed",
        json!({ "organization": "hel", "project": "ovzdusie" }),
    );
    assert!(eva
        .check("RoleBinding", Verb::Propose, Some(&mixed))
        .is_err());
    let jana = effective(
        &mirror,
        "platform-admins",
        &identity("jana@hel.fi", &[]),
        ORG_NAMESPACE,
        now,
    );
    assert!(jana.administers_organization());
}
