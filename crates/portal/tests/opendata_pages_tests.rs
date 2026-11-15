//! The texts of an Organization's open data portal pages (T-7031): its administrators read the
//! built-in Markdown and their own, preview a text with the catalogue's renderer and propose it
//! per page and language; nobody else, another Organization's administrator included, does.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::organization_host::{scope, OrganizationHost};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;

fn person(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn bbsk_spec() -> Value {
    json!({ "domain": "bbsk.sk", "locales": ["sk", "en"], "defaultLocale": "sk",
        "appearance": { "instanceName": "Banskobystrický kraj" } })
}

/// Two Organizations on their own hosts, each with one administrator, and `citatel`, who reads
/// every kind in `bbsk` and administers nothing.
async fn world() -> (AppState, wiremock::MockServer) {
    let shako = common::forge().await;
    let client = joinedcontext_portal::forge::ShakoClient::new(
        shako.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let mut config = Config::for_tests();
    config.organization_hosts = joinedcontext_portal::organization_host::parse(
        "bbsk=https://portal.bbsk.example,praha=https://portal.praha.example",
    )
    .expect("hosts");
    let state = AppState::new(config, None)
        .with_shako(Arc::new(client))
        .with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        (
            "ops",
            json!({ "domain": "ops.example", "installation": true }),
        ),
        ("bbsk", bbsk_spec()),
        (
            "praha",
            json!({ "domain": "praha.eu", "locales": ["cs"], "defaultLocale": "cs" }),
        ),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    for (role, verbs) in [
        ("org-admin", json!(["read", "propose", "approve", "delete"])),
        ("reader", json!(["read"])),
    ] {
        m.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (binding, who, role, org) in [
        ("bbsk-admins", "bbadmin", "org-admin", "bbsk"),
        ("praha-admins", "prahaadmin", "org-admin", "praha"),
        ("bbsk-readers", "citatel", "reader", "bbsk"),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@example.org") }], "role": role,
                "scope": { "organization": org } }),
        ));
    }
    (state, shako)
}

/// One request on `portal.{organization}.example`, as `who` (signed out when `None`), with a
/// session sealed on that host and the CSRF pair.
async fn on(
    state: &AppState,
    organization: &str,
    who: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let host = format!("portal.{organization}.example");
    let pin = OrganizationHost {
        organization: organization.to_owned(),
        base: format!("https://{host}").parse().expect("url"),
    };
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/organization/opendata/pages{path}"))
        .header(header::HOST, &host)
        .header(header::CONTENT_TYPE, "application/json");
    // A CSRF token passes only on the host of the Organization it names (jc-multiorg-portal).
    let token = format!("{organization}.{}", common::CSRF);
    let plain = format!("{CSRF_COOKIE}={}", common::CSRF);
    let csrf = format!("{CSRF_COOKIE}={token}");
    request = request.header(CSRF_HEADER, &token);
    request = match who {
        Some(who) => {
            let cookie = scope(pin, async { common::cookie(&state.config, person(who)) }).await;
            request.header(header::COOKIE, cookie.replace(&plain, &csrf))
        }
        None => request.header(header::COOKIE, csrf),
    };
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    let response = server::app(state.clone())
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The administrator reads every page in each of the Organization's languages, the built-in text
/// beside the own one, which shows once the Organization's manifest holds it.
#[tokio::test]
async fn an_administrator_reads_every_page_built_in_and_own() {
    let (state, _forge) = world().await;
    let (status, body) = on(&state, "bbsk", Some("bbadmin"), "GET", "", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["organization"], "bbsk");
    assert_eq!(body["languages"], json!(["sk", "en"]));
    let pages = body["pages"].as_array().cloned().unwrap_or_default();
    assert_eq!(pages.len(), 10, "{body}");
    let connector_sk = &pages[0];
    assert_eq!(
        (&connector_sk["page"], &connector_sk["lang"]),
        (&json!("connector"), &json!("sk"))
    );
    assert!(connector_sk.get("markdown").is_none(), "{connector_sk}");
    assert!(
        connector_sk["builtIn"]
            .as_str()
            .is_some_and(|md| md.contains("odmietnu každý zápis")),
        "{connector_sk}"
    );

    let mut spec = bbsk_spec();
    spec["appearance"]["pages"] =
        json!([{ "page": "support", "lang": "en", "markdown": "# Help desk\n\nCall us." }]);
    state
        .mirror
        .upsert(envelope("Organization", "bbsk", ORG_NAMESPACE, spec));
    let (_, body) = on(&state, "bbsk", Some("bbadmin"), "GET", "", None).await;
    let support_en = body["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["page"] == "support" && p["lang"] == "en")
        .cloned()
        .unwrap_or_default();
    assert_eq!(support_en["markdown"], "# Help desk\n\nCall us.");
    assert_eq!(support_en["title"], "Help desk");
}

/// The preview is the catalogue's own rendering: raw HTML shows as text and a script link loses
/// its link; an empty or oversized text is a 422 naming the field.
#[tokio::test]
async fn the_preview_renders_safely_and_refuses_what_the_manifest_would() {
    let (state, _forge) = world().await;
    let markdown = "# Podpora\n\n<script>alert(1)</script>\n\nText <b>tučne</b> [klik](javascript:alert(1)) ![x](https://evil.example/x.png)";
    let (status, body) = on(
        &state,
        "bbsk",
        Some("bbadmin"),
        "POST",
        "/preview",
        Some(json!({ "markdown": markdown })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Podpora");
    let html = body["html"].as_str().unwrap_or_default();
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(!html.contains("<script"), "{html}");
    assert!(html.contains("&lt;b&gt;tučne&lt;/b&gt;"), "{html}");
    assert!(!html.contains("href=\"javascript"), "{html}");
    assert!(html.contains("klik"), "{html}");
    assert!(!html.contains("<img"), "{html}");

    for markdown in [
        " \n".to_owned(),
        "a".repeat(16 * 1024 + 1),
        "a\u{0}b".to_owned(),
    ] {
        let (status, body) = on(
            &state,
            "bbsk",
            Some("bbadmin"),
            "POST",
            "/preview",
            Some(json!({ "markdown": markdown })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.to_string().contains("markdown"), "{body}");
    }
    let (status, _) = on(
        &state,
        "bbsk",
        Some("bbadmin"),
        "POST",
        "/preview",
        Some(json!({ "markdown": "x", "html": "<b>" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Signed out is 401 (403 on a write, the CSRF check first); a reader of the Organization, and
/// another Organization's administrator on this Organization's host, are 403 on every route.
#[tokio::test]
async fn nobody_but_the_organizations_own_administrator_reads_or_writes_its_pages() {
    let (state, _forge) = world().await;
    let text = || Some(json!({ "markdown": "# Ovládnuté\n\nx" }));
    let (status, _) = on(&state, "bbsk", None, "GET", "", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for who in ["citatel", "prahaadmin"] {
        for (method, path, body) in [
            ("GET", "", None),
            ("POST", "/preview", text()),
            ("PUT", "/support/sk", text()),
            ("DELETE", "/support/sk", None),
        ] {
            let (status, body) = on(&state, "bbsk", Some(who), method, path, body).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{who} {method} {path}: {body}"
            );
        }
    }
    // bbsk's administrator on praha's host is praha's stranger.
    let (status, _) = on(
        &state,
        "praha",
        Some("bbadmin"),
        "PUT",
        "/support/cs",
        text(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// An unknown page or a language code that is not two letters is 404; an empty text 422; a
/// revert of a page that shows its built-in text is 404.
#[tokio::test]
async fn a_page_or_language_that_is_not_one_is_refused_before_any_change() {
    let (state, _forge) = world().await;
    let text = || Some(json!({ "markdown": "# x\n\ny" }));
    for path in ["/home/sk", "/support/SK", "/support/svk", "/support/s1"] {
        let (status, body) = on(&state, "bbsk", Some("bbadmin"), "PUT", path, text()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    let (status, body) = on(
        &state,
        "bbsk",
        Some("bbadmin"),
        "PUT",
        "/support/sk",
        Some(json!({ "markdown": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = on(
        &state,
        "bbsk",
        Some("bbadmin"),
        "DELETE",
        "/support/sk",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// A publish proposes the Organization manifest with that one text set and every other kept,
/// through propose.
#[tokio::test]
async fn a_publish_proposes_the_manifest_with_the_text_and_keeps_the_rest() {
    let (state, shako) = world().await;
    let mut spec = bbsk_spec();
    spec["appearance"]["pages"] =
        json!([{ "page": "terms", "lang": "en", "markdown": "# Terms\n\nKeep." }]);
    state
        .mirror
        .upsert(envelope("Organization", "bbsk", ORG_NAMESPACE, spec));
    let (status, body) = on(
        &state,
        "bbsk",
        Some("bbadmin"),
        "PUT",
        "/support/sk",
        Some(json!({ "markdown": "# Podpora\n\nNapíšte nám." })),
    )
    .await;
    assert!(
        status == StatusCode::ACCEPTED || status == StatusCode::OK,
        "{status}: {body}"
    );
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let written = written(&shako).await;
    assert!(written.contains("Napíšte nám."), "{written}");
    assert!(written.contains("Keep."), "the other text stays: {written}");
    assert!(written.contains("Banskobystrický kraj"), "{written}");
}

/// A revert proposes the manifest without that one text.
#[tokio::test]
async fn a_revert_proposes_the_manifest_without_the_text() {
    let (state, shako) = world().await;
    let mut spec = bbsk_spec();
    spec["appearance"]["pages"] = json!([
        { "page": "terms", "lang": "en", "markdown": "# Terms\n\nGone." },
        { "page": "terms", "lang": "sk", "markdown": "# Podmienky\n\nOstáva." }
    ]);
    state
        .mirror
        .upsert(envelope("Organization", "bbsk", ORG_NAMESPACE, spec));
    let (status, body) = on(&state, "bbsk", Some("bbadmin"), "DELETE", "/terms/en", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let written = written(&shako).await;
    assert!(!written.contains("Gone."), "{written}");
    assert!(written.contains("Ostáva."), "{written}");
}

/// Every file content the Portal wrote to the forge, decoded.
async fn written(shako: &wiremock::MockServer) -> String {
    use base64::Engine;
    let mut out = String::new();
    for request in shako.received_requests().await.unwrap_or_default() {
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            continue;
        };
        let mut contents = Vec::new();
        collect_contents(&body, &mut contents);
        for content in contents {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(content) {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    out
}

fn collect_contents<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                match v.as_str() {
                    Some(s) if key == "content" || key == "Content" => out.push(s),
                    _ => collect_contents(v, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_contents(v, out)),
        _ => {}
    }
}
