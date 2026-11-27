use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

mod common;

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        stepped_up_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.join("; ")
}

fn seed_demo_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());

    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ContextSpace".into(),
        metadata: ObjectMeta {
            name: "ovzdusie".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec: serde_json::json!({ "tenant": "ovzdusie" }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });

    let mut labels = BTreeMap::new();
    labels.insert("joinedcontext.com/domain".into(), "environment".into());

    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Endpoint".into(),
        metadata: ObjectMeta {
            name: "public-air".into(),
            namespace: Some("ovzdusie".into()),
            labels,
            ..Default::default()
        },
        spec: serde_json::json!({
            "audience": "public",
            "representations": ["ngsi-ld", "file.geojson"]
        }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });

    mirror
}

#[tokio::test]
async fn anonymous_call_returns_401() {
    let config = Config::for_tests();
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn label_selector_filtering() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?labelSelector=joinedcontext.com/domain=environment")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["kind"], "List");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["metadata"]["name"], "public-air");

    let no_match = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?labelSelector=joinedcontext.com/domain=mobility")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(no_match.status(), StatusCode::OK);
    let body = no_match.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn field_selector_filtering() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?fieldSelector=metadata.name=public-air")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["metadata"]["name"], "public-air");
}

#[tokio::test]
async fn two_page_pagination() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();

    for name in &["air-station-1", "air-station-2"] {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: (*name).into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        });
    }

    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let res1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?limit=2")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res1.status(), StatusCode::OK);
    let body1 = res1.into_body().collect().await.unwrap().to_bytes();
    let page1: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cont_token = page1["metadata"]["continue"]
        .as_str()
        .expect("continue token");
    assert_eq!(page1["metadata"]["remainingItemCount"], 1);

    let res2 = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/ovzdusie/endpoints?limit=2&continue={cont_token}"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res2.status(), StatusCode::OK);
    let body2 = res2.into_body().collect().await.unwrap().to_bytes();
    let page2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["metadata"].get("continue").is_none());
    assert!(page2["metadata"].get("remainingItemCount").is_none());
}

#[tokio::test]
async fn unknown_plural_returns_404_problem() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/nonexistentkind")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/resource-not-found"
    );
}

/// MF-11 (T-2375): a revision is read from the repository, so a Portal with none says so
/// instead of answering today's list as though it were the one asked for.
#[tokio::test]
async fn a_revision_on_a_portal_without_a_repository_is_unavailable() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?revision=abc1234")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

/// T-7917: the resource reads that reach past the live mirror: a manifest `main` refused is
/// listed (with its reason) only where it matches the selectors, a copy is read, and a commit
/// the forge answers oddly is the one 404 of a missing resource or the 503 of a forge down.
mod reaching_past_the_mirror {
    use super::common;
    use axum::http::StatusCode;
    use joinedcontext_portal::ops::workspaces::{Opening, Scope};
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::state::AppState;
    use serde_json::{json, Value};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAST: &str = "4f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49";
    const UNPROCESSABLE: &str = "2222222222222222222222222222222222222222";
    const DOWN: &str = "3333333333333333333333333333333333333333";

    fn reader() -> joinedcontext_portal::auth::session::Identity {
        common::person("reader@hel.fi")
    }

    fn grant(state: &AppState) {
        state.mirror.upsert(common::envelope(
            "Role",
            "reads-endpoints",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(common::envelope(
            "RoleBinding",
            "reader-ovzdusie",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reads-endpoints",
                    "scope": { "project": "ovzdusie" } }),
        ));
    }

    fn names(text: &str) -> Vec<String> {
        let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        body["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i["metadata"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// T-7917: a refused manifest is listed where the label and the field selector match it,
    /// and left out where either does not.
    #[tokio::test]
    async fn a_refused_manifest_is_listed_only_where_the_selectors_match_it() {
        let state = AppState::new(joinedcontext_portal::config::Config::for_tests(), None);
        grant(&state);
        let mut refused = common::envelope(
            "Endpoint",
            "refused-air",
            "ovzdusie",
            json!({ "contextSpaceRef": "air", "audience": "internal" }),
        );
        refused
            .metadata
            .labels
            .insert("team".to_owned(), "air".to_owned());
        state.mirror.refuse(refused);
        let list = |query: &str| {
            let uri = format!("/api/v1/projects/ovzdusie/endpoints?{query}");
            let state = state.clone();
            async move { common::send(&state, reader(), "GET", &uri, None).await }
        };
        let plain = list("").await;
        assert_eq!(names(&plain.text), ["refused-air"], "{}", plain.text);
        let both = list("labelSelector=team%3Dair&fieldSelector=metadata.name%3Drefused-air").await;
        assert_eq!(names(&both.text), ["refused-air"], "{}", both.text);
        let wrong_label = list("labelSelector=team%3Dwater").await;
        assert_eq!(wrong_label.status, StatusCode::OK, "{}", wrong_label.text);
        assert!(names(&wrong_label.text).is_empty(), "{}", wrong_label.text);
        let wrong_field =
            list("labelSelector=team%3Dair&fieldSelector=metadata.name%3Dsomething-else").await;
        assert_eq!(wrong_field.status, StatusCode::OK, "{}", wrong_field.text);
        assert!(names(&wrong_field.text).is_empty(), "{}", wrong_field.text);
    }

    /// T-7917: a copy of the project is read through its owner: the manifest answered is the
    /// project's, as the copy holds it.
    #[tokio::test]
    async fn a_manifest_is_read_inside_a_copy_the_caller_owns() {
        let state = common::state_on_forge().await;
        grant(&state);
        state.mirror.upsert(common::envelope(
            "Endpoint",
            "air",
            "ovzdusie",
            json!({ "contextSpaceRef": "air", "audience": "internal" }),
        ));
        state
            .workspaces
            .create(Opening {
                name: "air-v2",
                title: None,
                project: "ovzdusie",
                owner: "reader@hel.fi",
                base_revision: "base1",
                scope: Scope::Project {},
                ttl_hours: 24,
            })
            .await
            .expect("opened");
        let read = common::send(
            &state,
            reader(),
            "GET",
            "/api/v1/projects/ovzdusie/endpoints/air?workspace=air-v2",
            None,
        )
        .await;
        assert_eq!(read.status, StatusCode::OK, "{}", read.text);
        let manifest: Value = serde_json::from_str(&read.text).expect("a manifest");
        assert_eq!(manifest["metadata"]["name"], "air");
    }

    /// A commit's tree answering `status`, and for `PAST` a tree of five files: one the forge
    /// no longer has, one that is no manifest, one naming no namespace, one naming another project's,
    /// and one that is the project's own.
    async fn forge_at_odd_commits() -> MockServer {
        let forge = common::forge().await;
        let tree_path = format!(
            "{}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
            common::REPO
        );
        for (revision, status) in [(UNPROCESSABLE, 422), (DOWN, 503)] {
            Mock::given(method("GET"))
                .and(path(tree_path.clone()))
                .and(query_param("@r", format!("'{revision}'")))
                .respond_with(
                    ResponseTemplate::new(status).set_body_json(json!({ "message": "no" })),
                )
                .with_priority(1)
                .mount(&forge)
                .await;
        }
        let manifest = |name: &str, namespace: Option<&str>| {
            let namespace = namespace.map_or_else(String::new, |ns| format!("  namespace: {ns}\n"));
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: {name}\n{namespace}spec:\n  contextSpaceRef: air\n  audience: internal\n"
            )
        };
        let files = [
            ("projects/ovzdusie/endpoints/gone.yaml", None),
            (
                "projects/ovzdusie/endpoints/notes.yaml",
                Some("just: some notes\n".to_owned()),
            ),
            (
                "projects/ovzdusie/endpoints/bare.yaml",
                Some(manifest("bare", None)),
            ),
            (
                "projects/ovzdusie/endpoints/strayed.yaml",
                Some(manifest("strayed", Some("other"))),
            ),
            (
                "projects/ovzdusie/endpoints/kept.yaml",
                Some(manifest("kept", Some("ovzdusie"))),
            ),
        ];
        let tree: Vec<Value> = files
            .iter()
            .map(|(p, _)| json!({ "Path": p, "Type": "blob", "Sha": format!("blob-{p}") }))
            .collect();
        Mock::given(method("GET"))
            .and(path(tree_path))
            .and(query_param("@r", format!("'{PAST}'")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": PAST, "Truncated": false, "Tree": tree })),
            )
            .with_priority(1)
            .mount(&forge)
            .await;
        for (file, content) in files {
            let answer = match content {
                Some(content) => ResponseTemplate::new(200).set_body_json(json!({
                    "Sha": format!("blob-{file}"), "Content": common::encode(&content)
                })),
                None => ResponseTemplate::new(404).set_body_json(json!({ "message": "gone" })),
            };
            Mock::given(method("GET"))
                .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
                .and(query_param("@p", format!("'{file}'")))
                .and(query_param("@r", format!("'{PAST}'")))
                .respond_with(answer)
                .with_priority(1)
                .mount(&forge)
                .await;
        }
        forge
    }

    /// T-7917: at a past commit a file the forge lost or that is no manifest is skipped, a
    /// manifest with no namespace is the project's, a client error of the forge is the
    /// missing-resource 404 and a forge down is the 503.
    #[tokio::test]
    async fn a_past_commit_skips_what_is_no_manifest_and_names_a_forge_down() {
        let forge = forge_at_odd_commits().await;
        let state = common::state_on(&forge);
        grant(&state);
        let base = "/api/v1/projects/ovzdusie/endpoints";
        let list = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}?revision={PAST}"),
            None,
        )
        .await;
        assert_eq!(list.status, StatusCode::OK, "{}", list.text);
        assert_eq!(names(&list.text), ["kept"], "{}", list.text);
        let unprocessable = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}?revision={UNPROCESSABLE}"),
            None,
        )
        .await;
        assert_eq!(
            unprocessable.status,
            StatusCode::NOT_FOUND,
            "{}",
            unprocessable.text
        );
        let down = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}?revision={DOWN}"),
            None,
        )
        .await;
        assert_eq!(
            down.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            down.text
        );
    }
}
