//! Each Organization's own name, colours and logo (UIX-16, T-6772): `spec.appearance` of its
//! `Organization` manifest over the installation's branding file, member by member, for a request
//! on that Organization's Portal host; edited through a Change by its own administrators only.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::doors::{http, mcp, mcp_rpc};
use common::envelope;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><rect width=\"1\" height=\"1\"/></svg>";

fn admin(name: &str) -> Identity {
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
        "appearance": { "instanceName": "Banskobystrický kraj", "colours": { "primary": "#0b5394" }, "logoSvg": LOGO } })
}

/// The installation's file and two Organizations on their own hosts: `bbsk` with an appearance,
/// `praha` with none; each has one administrator.
fn world(test: &str) -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("jc-appearance-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the branding directory");
    let file = dir.join("branding.yaml");
    std::fs::write(
        &file,
        "instanceName: \"Installation\"\ncity: \"Slovakia\"\ncolours:\n  primary: \"#111111\"\n  secondary: \"#222222\"\n",
    )
    .expect("branding");
    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    config.organization_hosts = joinedcontext_portal::organization_host::parse(
        "bbsk=https://portal.bbsk.example,praha=https://portal.praha.example",
    )
    .expect("hosts");
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
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
    m.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    for (who, org) in [("bbadmin", "bbsk"), ("prahaadmin", "praha")] {
        m.upsert(envelope(
            "RoleBinding",
            &format!("{org}-admins"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@example.org") }], "role": "org-admin",
                "scope": { "organization": org } }),
        ));
    }
    (state, dir)
}

async fn get(state: &AppState, host: &str, uri: &str) -> axum::response::Response {
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::HOST, host)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

async fn branding(state: &AppState, host: &str) -> Value {
    let response = get(state, host, "/api/v1/branding").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

/// Each Organization's host shows its own name and colours; what it leaves out, and every host
/// that pins no Organization, shows the installation's file.
#[tokio::test]
async fn each_organizations_host_shows_its_own_appearance_over_the_installations() {
    let (state, dir) = world("each_organizations_host_");

    let bbsk = branding(&state, "portal.bbsk.example").await;
    assert_eq!(bbsk["instanceName"], "Banskobystrický kraj");
    assert_eq!(bbsk["shortName"], "Banskobystrický kraj");
    assert_eq!(bbsk["colours"]["primary"], "#0b5394");
    assert_eq!(
        bbsk["languages"],
        json!({ "default": "sk", "offered": ["sk", "en"] })
    );
    assert_eq!(bbsk["orgDomain"], "bbsk.sk");
    // Left out: the installation's.
    assert_eq!(bbsk["colours"]["secondary"], "#222222");
    assert_eq!(bbsk["city"], "Slovakia");
    assert!(!bbsk["logo"].as_str().unwrap_or_default().is_empty());

    let praha = branding(&state, "portal.praha.example").await;
    assert_eq!(praha["instanceName"], "Installation");
    assert_eq!(praha["colours"]["primary"], "#111111");
    assert_eq!(praha["languages"]["default"], "cs");
    assert_eq!(praha["logo"], "");

    let plain = branding(&state, "portal.installation.example").await;
    assert_eq!(plain["instanceName"], "Installation");

    let _ = std::fs::remove_dir_all(&dir);
}

/// RTE-14: the Organization's logo is served as an attachment that is never sniffed into a page
/// and runs nothing; an Organization without one has none to serve.
#[tokio::test]
async fn an_organizations_logo_is_an_attachment_that_never_becomes_a_page() {
    let (state, dir) = world("an_organizations_logo_is");
    let response = get(&state, "portal.bbsk.example", "/api/v1/branding/logo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    let value = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(value(header::CONTENT_TYPE), "image/svg+xml");
    assert_eq!(value(header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert_eq!(
        value(header::CONTENT_DISPOSITION),
        "attachment; filename=\"logo.svg\""
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(bytes, LOGO.as_bytes());

    for (host, asset) in [
        ("portal.praha.example", "logo"),
        ("portal.bbsk.example", "favicon"),
    ] {
        let none = get(&state, host, &format!("/api/v1/branding/{asset}")).await;
        assert_eq!(none.status(), StatusCode::NOT_FOUND, "{host} {asset}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn organization(name: &str, spec: Value) -> Value {
    json!({ "apiVersion": joinedcontext_portal::resource::API_VERSION, "kind": "Organization",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE }, "spec": spec })
}

fn refused(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND
}

/// `bbsk`'s administrator proposes no appearance for `praha`, over HTTP or MCP.
#[tokio::test]
async fn an_administrator_edits_no_other_organizations_appearance() {
    let (state, dir) = world("an_administrator_edits_n");
    let mut spec = json!({ "domain": "praha.eu", "locales": ["cs"], "defaultLocale": "cs" });
    spec["appearance"] =
        json!({ "instanceName": "Ovládnuto", "colours": { "primary": "#ff0000" } });
    let manifest = organization("praha", spec);

    let rest = http(
        &state,
        admin("bbadmin"),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/praha?dryRun=All"),
        Some(manifest.clone()),
    )
    .await;
    assert!(refused(rest.status), "rest: {} {}", rest.status, rest.body);

    let answer = mcp_rpc(
        &mcp(admin("bbadmin")),
        &state,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "jc_resource_propose",
            "arguments": { "project": ORG_NAMESPACE, "manifest": manifest } } }),
    )
    .await;
    assert_eq!(answer["result"]["isError"], json!(true), "mcp: {answer}");
    assert_ne!(
        answer["result"]["status"],
        json!("input_required"),
        "mcp: {answer}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A colour token that is CSS rather than a colour: the dry run names the member, and the
/// proposal is a 422 naming it, before any Change.
#[tokio::test]
async fn a_colour_that_is_css_is_refused_naming_the_field() {
    let (state, dir) = world("css");
    let mut spec = bbsk_spec();
    spec["appearance"]["colours"]["primary"] = json!("red;}body{background:url(//evil.example)}");
    let uri = format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/bbsk");
    let field = "spec.appearance.colours.primary";

    let dry = http(
        &state,
        admin("bbadmin"),
        "PUT",
        &format!("{uri}?dryRun=All"),
        Some(organization("bbsk", spec.clone())),
    )
    .await;
    assert_eq!(dry.body["valid"], json!(false), "{}", dry.body);
    assert!(dry.text().contains(field), "{}", dry.body);

    let proposed = http(
        &state,
        admin("bbadmin"),
        "PUT",
        &uri,
        Some(organization("bbsk", spec)),
    )
    .await;
    assert_eq!(
        proposed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        proposed.body
    );
    assert!(proposed.text().contains(field), "{}", proposed.body);
    let _ = std::fs::remove_dir_all(&dir);
}
