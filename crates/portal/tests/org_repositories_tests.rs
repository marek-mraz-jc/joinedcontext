//! Administration > Repositories (T-7409): an Organization's administrators list its forge
//! repositories and download one as a `.tar.gz` of its default branch's head. The forge holds
//! both Organizations' repositories under one owner, so a list or download that reached past its
//! own Organization would show it.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{envelope, person, send};
use http_body_util::BodyExt;
use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::api::org_repositories::MAX_ARCHIVE_BYTES;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOOR: &str = "/api/v1/organization/repositories";
const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
const ARCHIVE: &[u8] = b"\x1f\x8b\x08\0an archive of alfa-city";

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    envelope(kind, name, ORG_NAMESPACE, spec)
}

/// `GET R`, its `main` at [`HEAD`] and the archive of [`HEAD`] for one repository of owner `jc`.
async fn repository(forge: &MockServer, name: &str, archived: bool) {
    let base = format!("/odata/Repositories(Owner='jc',Name='{name}')");
    Mock::given(method("GET"))
        .and(path(base.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Owner": "jc", "Name": name, "DefaultBranch": "main", "Archived": archived,
            "SizeBytes": 20480, "UpdatedAt": "2026-11-18T09:30:00+00:00",
        })))
        .mount(forge)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/Branches('main')")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "Name": "main", "HeadSha": HEAD })),
        )
        .mount(forge)
        .await;
    let download = format!("/api/v1/repos/jc/{name}/archive/{HEAD}.tar.gz");
    Mock::given(method("GET"))
        .and(path(format!("{base}/Shako.ArchiveLink(Ref=@r,Format=@f)")))
        .and(query_param("@r", format!("'{HEAD}'")))
        .and(query_param("@f", "'tar.gz'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "value": format!("https://forge.example{download}") })),
        )
        .mount(forge)
        .await;
    Mock::given(method("GET"))
        .and(path(download))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ARCHIVE))
        .mount(forge)
        .await;
}

/// Two Organizations on one forge owner: `alfa` (the installation's: the configuration
/// repository `org`, project `alfa-city` with its App `air-map`) and `bravo` (project
/// `bravo-town`). ada administers alfa, bob administers bravo, mia reads alfa-city.
fn world(forge_uri: &str) -> AppState {
    let shako =
        ShakoClient::new(forge_uri.parse().expect("url"), "jc", "org", "token").expect("shako");
    let state = AppState::new(Config::for_tests(), None).with_shako(Arc::new(shako));
    let m = &state.mirror;
    m.upsert(org(
        "Organization",
        "alfa",
        json!({ "domain": "alfa.example", "installation": true }),
    ));
    m.upsert(org(
        "Organization",
        "bravo",
        json!({ "domain": "bravo.example", "installation": false }),
    ));
    m.upsert(org(
        "Project",
        "alfa-city",
        json!({ "organizationRef": "alfa" }),
    ));
    m.upsert(org(
        "Project",
        "bravo-town",
        json!({ "organizationRef": "bravo" }),
    ));
    m.set_repositories(BTreeMap::from([
        ("alfa-city".to_owned(), "alfa-city".to_owned()),
        ("bravo-town".to_owned(), "bravo-town".to_owned()),
    ]));
    m.upsert(envelope(
        "App",
        "air-map",
        "alfa-city",
        json!({ "source": { "git": { "url": "https://forge.example/jc/alfa-city_air-map.git" } } }),
    ));
    m.upsert(envelope(
        "App",
        "noise-map",
        "bravo-town",
        json!({ "source": { "git": { "url": "https://forge.example/jc/bravo-town_noise-map.git" } } }),
    ));
    m.upsert(org(
        "Role",
        "org-admin",
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding", "Project"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    m.upsert(org(
        "Role",
        "reader",
        json!({ "rules": [{ "kinds": ["*"], "verbs": ["read"] }] }),
    ));
    for (name, who, role, scope) in [
        (
            "ada-admin",
            "ada@hel.fi",
            "org-admin",
            json!({ "organization": "alfa" }),
        ),
        (
            "bob-admin",
            "bob@hel.fi",
            "org-admin",
            json!({ "organization": "bravo" }),
        ),
        (
            "mia-reader",
            "mia@hel.fi",
            "reader",
            json!({ "project": "alfa-city" }),
        ),
    ] {
        m.upsert(org(
            "RoleBinding",
            name,
            json!({ "subjects": [{ "user": who }], "role": role, "scope": scope }),
        ));
    }
    state
}

async fn forge_with_every_repository() -> MockServer {
    let forge = MockServer::start().await;
    for name in [
        "org",
        "alfa-city",
        "alfa-city_air-map",
        "bravo-town",
        "bravo-town_noise-map",
    ] {
        repository(&forge, name, name == "alfa-city_air-map").await;
    }
    forge
}

#[tokio::test]
async fn the_list_holds_only_this_organizations_repositories() {
    let forge = forge_with_every_repository().await;
    let state = world(&forge.uri());
    let answer = send(&state, person("ada"), "GET", DOOR, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(body["organization"], "alfa");
    let rows = body["repositories"].as_array().expect("rows");
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, ["org", "alfa-city", "alfa-city_air-map"]);
    assert!(!answer.text.contains("bravo"), "{}", answer.text);
    let app = &rows[2];
    assert_eq!(app["role"], "application");
    assert_eq!(app["project"], "alfa-city");
    assert_eq!(app["app"], "air-map");
    assert_eq!(app["archived"], true);
    assert_eq!(app["sizeBytes"], 20480);
    assert_eq!(app["updatedAt"], "2026-11-18T09:30:00+00:00");
    assert_eq!(rows[0]["role"], "configuration");
    assert_eq!(rows[1]["role"], "project");
    assert_eq!(body["maxArchiveBytes"], MAX_ARCHIVE_BYTES);
}

/// The configuration repository holds every Organization's settings, so only the installation's
/// Organization lists it.
#[tokio::test]
async fn another_organization_lists_no_configuration_repository() {
    let forge = forge_with_every_repository().await;
    let state = world(&forge.uri());
    // The installation is bravo now: ada's alfa is the other Organization.
    state.mirror.upsert(org(
        "Organization",
        "alfa",
        json!({ "domain": "alfa.example", "installation": false }),
    ));
    state.mirror.upsert(org(
        "Organization",
        "bravo",
        json!({ "domain": "bravo.example", "installation": true }),
    ));
    let answer = send(&state, person("bob"), "GET", DOOR, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    let names: Vec<&str> = body["repositories"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(names, ["org", "bravo-town", "bravo-town_noise-map"]);
}

#[tokio::test]
async fn only_the_organizations_administrators_list_and_download() {
    let forge = forge_with_every_repository().await;
    let state = world(&forge.uri());
    for who in ["mia", "bob"] {
        for door in [DOOR.to_owned(), format!("{DOOR}/alfa-city/archive")] {
            let answer = send(&state, person(who), "GET", &door, None).await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{who} {door}: {}",
                answer.text
            );
        }
    }
    let downloads = forge
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.url.path().contains("/archive/"))
        .count();
    assert_eq!(downloads, 0, "a refused caller reached the forge");
}

#[tokio::test]
async fn a_foreign_repository_answers_as_an_unknown_one() {
    let forge = forge_with_every_repository().await;
    let state = world(&forge.uri());
    let foreign = send(
        &state,
        person("ada"),
        "GET",
        &format!("{DOOR}/bravo-town/archive"),
        None,
    )
    .await;
    let unknown = send(
        &state,
        person("ada"),
        "GET",
        &format!("{DOOR}/zulu-town/archive"),
        None,
    )
    .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text);
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    assert_eq!(
        foreign.text.replace("bravo-town", "X"),
        unknown.text.replace("zulu-town", "X")
    );
    let reached = forge
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .any(|r| r.url.path().contains("bravo-town"));
    assert!(!reached, "the foreign repository was asked of the forge");
}

async fn download(state: &AppState, name: &str) -> axum::response::Response {
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("{DOOR}/{name}/archive"))
                .header(header::COOKIE, common::cookie(&state.config, person("ada")))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

#[tokio::test]
async fn the_archive_is_the_branch_head_and_each_download_is_audited() {
    let forge = forge_with_every_repository().await;
    let state = world(&forge.uri());
    let response = download(&state, "alfa-city").await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers[header::CONTENT_TYPE], "application/gzip");
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        format!("attachment; filename=\"alfa-city-{HEAD}.tar.gz\"").as_str()
    );
    assert_eq!(headers["x-commit"], HEAD);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(&body[..], ARCHIVE);
    // The archive was asked for by the head commit, never by the branch that may move.
    let asked: Vec<String> = forge
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .map(|r| r.url.path().to_owned())
        .filter(|p| p.contains("/archive/"))
        .collect();
    assert_eq!(
        asked,
        [format!("/api/v1/repos/jc/alfa-city/archive/{HEAD}.tar.gz")]
    );

    let _ = download(&state, "alfa-city").await;
    let feed = state
        .activity
        .list(
            ORG_NAMESPACE,
            &ActivityFilter {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("feed");
    let rows: Vec<_> = feed
        .items
        .iter()
        .filter(|e| e.kind == "repository.downloaded")
        .collect();
    assert_eq!(rows.len(), 2, "one row per download");
    assert_eq!(rows[0].details["repository"], "alfa-city");
    assert_eq!(rows[0].details["commit"], HEAD);
    assert_eq!(rows[0].details["organization"], "alfa");
    assert_eq!(rows[0].details["actor"], person("ada").username);
}

#[tokio::test]
async fn an_archive_the_forge_says_is_too_large_is_refused_before_it_streams() {
    let forge = MockServer::start().await;
    for name in ["org", "alfa-city_air-map"] {
        repository(&forge, name, false).await;
    }
    let base = "/odata/Repositories(Owner='jc',Name='alfa-city')";
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&forge)
        .await;
    let download_path = format!("/api/v1/repos/jc/alfa-city/archive/{HEAD}.tar.gz");
    Mock::given(method("GET"))
        .and(path(format!("{base}/Shako.ArchiveLink(Ref=@r,Format=@f)")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "value": format!("https://forge.example{download_path}") })),
        )
        .mount(&forge)
        .await;
    let size = usize::try_from(MAX_ARCHIVE_BYTES).expect("fits") + 1;
    Mock::given(method("GET"))
        .and(path(download_path))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; size]))
        .mount(&forge)
        .await;
    let state = world(&forge.uri());
    let answer = send(
        &state,
        person("ada"),
        "GET",
        &format!("{DOOR}/alfa-city/archive"),
        None,
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("clone the repository with git"),
        "{}",
        answer.text
    );
}

/// A forge that sends an archive with no length and never ends: the Portal answers at once and
/// passes it on chunk by chunk, holding no more than the transport buffers, and cuts it at
/// [`MAX_ARCHIVE_BYTES`].
#[tokio::test]
async fn an_archive_streams_through_and_is_cut_at_the_limit() {
    use axum::extract::State;
    use axum::routing::get;

    const CHUNK: usize = 1024 * 1024;
    let sent = Arc::new(AtomicU64::new(0));
    let link = format!("/api/v1/repos/jc/alfa-city/archive/{HEAD}.tar.gz");
    let app = axum::Router::new()
        .route(
            "/odata/{*rest}",
            get(move |axum::extract::Path(rest): axum::extract::Path<String>| async move {
                let body = if rest.ends_with("Branches('main')") {
                    json!({ "HeadSha": HEAD })
                } else if rest.contains("Shako.ArchiveLink") {
                    json!({ "value": format!("https://forge.example/api/v1/repos/jc/alfa-city/archive/{HEAD}.tar.gz") })
                } else if rest.contains("Name='alfa-city'") || rest.contains("Name='org'") {
                    json!({ "DefaultBranch": "main" })
                } else {
                    return (StatusCode::NOT_FOUND, axum::Json(json!({ "error": { "code": "404", "message": "absent" } })));
                };
                (StatusCode::OK, axum::Json(body))
            }),
        )
        .route(
            &link,
            get(|State(sent): State<Arc<AtomicU64>>| async move {
                let stream = futures_util::stream::unfold(sent, |sent| async move {
                    sent.fetch_add(CHUNK as u64, Ordering::SeqCst);
                    Some((Ok::<_, std::io::Error>(vec![7u8; CHUNK]), sent))
                });
                Body::from_stream(stream)
            }),
        )
        .with_state(Arc::clone(&sent));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await });

    let state = world(&format!("http://{address}"));
    let response = download(&state, "alfa-city").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
    let mut body = response.into_body();
    let mut received = 0u64;
    let cut = loop {
        match body.frame().await {
            Some(Ok(frame)) => {
                received += frame.data_ref().map_or(0, |d| d.len() as u64);
                // Nothing is read ahead of the person: the forge is at most a few buffers ahead.
                let ahead = sent.load(Ordering::SeqCst).saturating_sub(received);
                assert!(
                    ahead <= 16 * CHUNK as u64,
                    "the Portal read {ahead} bytes ahead"
                );
            }
            Some(Err(_)) => break true,
            None => break false,
        }
    };
    assert!(cut, "an endless archive was not cut");
    assert!(
        received <= MAX_ARCHIVE_BYTES,
        "{received} bytes passed the limit"
    );
    assert!(
        received >= MAX_ARCHIVE_BYTES - CHUNK as u64,
        "cut early at {received}"
    );
}
