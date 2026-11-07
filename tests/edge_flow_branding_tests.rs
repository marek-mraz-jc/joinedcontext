//! Edge cases of the flow a blueprint starts and of the two branding routes (T-1993, T-1994, T-1995;
//! CC-24, CC-26, CC-59, OPS-46, UI-30).
//!
//! **The contract, in one sentence:** a flow is refused before the forge is touched, and the branding
//! is a file an operator writes — so nothing in it reaches a CSS custom property, a locale switcher or a
//! browser as a document unless it is what it claims to be.
//!
//! The happy paths live in `blueprint_gallery_tests.rs` (the cards, the flow that reaches the forge,
//! the version conflict, the violations) and `branding_api_tests.rs` (the public block, the served
//! logo, the traversal that is dropped). This file is the other side: a body that is not a flow, a
//! colour that is not one, a file that is not YAML, an asset name nobody configured.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "ovzdusie";
const CSRF: &str = "csrf-token-edge-flow";
const ROLE: &str = "domain-editor";

// -------------------------------------------------------------------------------------------------
// T-1993 `start_flow`
// -------------------------------------------------------------------------------------------------

const TEMPLATE: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Dashboard
metadata:
  name: alert-{{ title }}
spec:
  title: {{ title }}
";

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some("org".into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

/// One blueprint this caller may run, and the binding that lets them propose a Dashboard here.
fn flow_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(org(
        "Blueprint",
        "threshold-alert",
        json!({
            "version": "1.2.0",
            "category": "alerting",
            "riskClass": "green",
            "allowedRoles": [ROLE],
            "parameterSchema": {
                "type": "object",
                "required": ["title"],
                "properties": { "title": { "type": "string" } },
            },
            "templates": [{ "name": "dashboard", "template": TEMPLATE }],
        }),
    ));
    mirror.upsert(org(
        "Role",
        "dashboard-editor",
        json!({ "rules": [{ "kinds": ["Dashboard"], "verbs": ["propose"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "editors",
        json!({ "subjects": [{ "user": "jana.editor@banskabystrica.sk" }],
                "role": "dashboard-editor", "scope": { "project": PROJECT } }),
    ));
    mirror
}

fn cookie(config: &Config, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:jana.editor".into(),
            username: "jana.editor".into(),
            email: Some("jana.editor@banskabystrica.sk".into()),
            name: None,
            roles: roles.iter().map(|role| role.to_string()).collect(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

/// A Portal whose forge is a mock server with nothing mounted: every call to it would be visible in
/// `received_requests`, so a case can prove a refusal never reached it.
async fn flow_world() -> (MockServer, axum::Router, String) {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a client");
    let config = Config::for_tests();
    let session = cookie(&config, &[ROLE]);
    let state = AppState::new(config, None)
        .with_mirror(flow_mirror())
        .with_gitea(Arc::new(client));
    (forge, server::app(state), session)
}

async fn post(app: &axum::Router, cookie: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// CC-24, CC-26: a flow names a blueprint, the version the form was generated from and the values
/// filled in. A body that is not that is refused as a body, and a blueprint name that is not a name
/// finds nothing — the gallery's namespace is the only place looked in.
#[tokio::test]
async fn a_flow_names_a_blueprint_a_version_and_the_values_that_were_filled_in() {
    let (forge, app, session) = flow_world().await;
    let uri = format!("/api/v1/projects/{PROJECT}/flows");

    for body in [
        json!({}),
        json!({ "blueprint": "threshold-alert" }),
        json!({ "blueprint": "threshold-alert", "version": "1.2.0" }),
        json!({ "version": "1.2.0", "parameters": { "title": "air" } }),
        json!({ "blueprint": 7, "version": "1.2.0", "parameters": {} }),
        json!({ "blueprint": "threshold-alert", "version": 1.2, "parameters": {} }),
        // `deny_unknown_fields`: a field this route does not read is a mistake, not something to
        // drop in silence on a request that opens a merge request.
        json!({ "blueprint": "threshold-alert", "version": "1.2.0", "parameters": {},
                "lane": "green" }),
        json!([]),
        json!("threshold-alert"),
        Value::Null,
    ] {
        let (status, answer) = post(&app, &session, &uri, &body).await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {answer}",
        );
    }

    // A blueprint name that names nothing here, including one that tries to leave the namespace the
    // gallery reads: the 404 of a card that is not there.
    for name in [
        "",
        "Threshold-Alert",
        "../org/threshold-alert",
        "threshold-alert ",
        "ovzdusie/threshold-alert",
    ] {
        let (status, answer) = post(
            &app,
            &session,
            &uri,
            &json!({ "blueprint": name, "version": "1.2.0", "parameters": { "title": "air" } }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name:?}: {answer}");
    }

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused flow reached the forge",
    );
}

/// CC-59, CC-26, PF-50: every reason a flow is refused is read before the forge is touched — the role
/// on the card, the version the form was filled against, the parameters, and the binding that lets a
/// person propose this kind in this project. Not one of them leaves a branch behind.
#[tokio::test]
async fn every_refusal_of_a_flow_happens_before_a_branch_exists() {
    let (forge, app, session) = flow_world().await;
    let uri = format!("/api/v1/projects/{PROJECT}/flows");
    let good = json!({ "blueprint": "threshold-alert", "version": "1.2.0",
                       "parameters": { "title": "air" } });

    // The version the form was filled against is not the one in Git.
    let (status, _) = post(
        &app,
        &session,
        &uri,
        &json!({ "blueprint": "threshold-alert", "version": "1.1.0",
                 "parameters": { "title": "air" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The parameters do not satisfy the schema, and every violation comes back at once.
    let (status, answer) = post(
        &app,
        &session,
        &uri,
        &json!({ "blueprint": "threshold-alert", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["errors"]
            .as_array()
            .is_some_and(|list| !list.is_empty()),
        "the violations are not a list the form can mark fields from: {answer}",
    );

    // The realm role the card names, taken away: the blueprint is not there for this caller.
    let config = Config::for_tests();
    let roleless = cookie(&config, &[]);
    let roleless_app = server::app(
        AppState::new(config, None)
            .with_mirror(flow_mirror())
            .with_gitea(Arc::new(
                GiteaClient::new(
                    forge.uri().parse().expect("a url"),
                    "test-owner",
                    "test-repo",
                    "token-xyz",
                )
                .expect("a client"),
            )),
    );
    let (status, _) = post(&roleless_app, &roleless, &uri, &good).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A project no binding of this person covers: the same flow, refused on the manifest's own gate.
    let (status, _) = post(&app, &session, "/api/v1/projects/doprava/flows", &good).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A project name that could not be a project at all.
    for project in ["Ovzdusie", "ovzdusie_1", "%2e%2e", ""] {
        let (status, answer) = post(
            &app,
            &session,
            &format!("/api/v1/projects/{project}/flows"),
            &good,
        )
        .await;
        assert!(
            status.is_client_error(),
            "{project:?} answered {status}: {answer}",
        );
    }

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused flow reached the forge",
    );
}

// -------------------------------------------------------------------------------------------------
// T-1994 `get_branding`
// -------------------------------------------------------------------------------------------------

fn branding_file(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-edge-branding-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the branding directory");
    let file = dir.join("branding.yaml");
    std::fs::write(&file, contents).expect("the branding file");
    file
}

async fn branding_of(path: Option<String>) -> (StatusCode, Value) {
    let mut config = Config::for_tests();
    config.branding_file = path;
    let app = server::app(AppState::new(config, None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// OPS-46: the branding file is written by whoever renders the ConfigMap, and every value the UI
/// writes into a CSS custom property is checked before it is served. A colour that is not a hex
/// colour — a keyword, a `var()`, a declaration that would close the property — is the default, and
/// the readable foreground is computed from the primary colour rather than taken from the file.
#[tokio::test]
async fn nothing_the_branding_file_says_reaches_a_css_property_unchecked() {
    let file = branding_file(
        "colours",
        concat!(
            "instanceName: \"Banská Bystrica\"\n",
            "primaryForeground: \"#00ff00\"\n",
            "colours:\n",
            "  primary: \"red\"\n",
            "  secondary: \"#12\"\n",
            "  accent: \"#1234567\"\n",
            "  background: \"#fff;}body{display:none\"\n",
            "  text: \"var(--stolen)\"\n",
        ),
    );
    let (status, branding) = branding_of(Some(file.display().to_string())).await;
    assert_eq!(status, StatusCode::OK, "{branding}");

    // Every colour fell back to the installation's default.
    let colours = &branding["colours"];
    assert_eq!(colours["primary"], json!("#1d4ed8"), "{branding}");
    assert_eq!(colours["secondary"], json!("#0f766e"), "{branding}");
    assert_eq!(colours["accent"], json!("#f59e0b"), "{branding}");
    assert_eq!(colours["background"], json!("#ffffff"), "{branding}");
    assert_eq!(colours["text"], json!("#0f172a"), "{branding}");
    for value in colours.as_object().into_iter().flatten().map(|(_, v)| v) {
        let text = value.as_str().unwrap_or_default();
        assert!(
            text.starts_with('#') && (text.len() == 4 || text.len() == 7),
            "a value that is not a hex colour is served: {text}",
        );
    }
    // The foreground is computed from the primary colour and never read from the file: white on the
    // dark default, dark on a light primary (WCAG 1.4.3).
    assert_eq!(
        branding["primaryForeground"],
        json!("#ffffff"),
        "{branding}"
    );
    let light = branding_file("light", "colours:\n  primary: \"#ffffff\"\n");
    let (_, on_light) = branding_of(Some(light.display().to_string())).await;
    assert_eq!(
        on_light["primaryForeground"],
        json!("#0f172a"),
        "{on_light}"
    );

    // The language switcher offers locale tags and always reaches the one the page starts in.
    let locales = branding_file(
        "locales",
        concat!(
            "languages:\n",
            "  default: \"en_US\"\n",
            "  offered: [\"sk\", \"<script>\", \"\", \"a\", \"toolongalocaletag\", \"de-AT\"]\n",
        ),
    );
    let (_, switcher) = branding_of(Some(locales.display().to_string())).await;
    assert_eq!(switcher["languages"]["default"], json!("en"), "{switcher}");
    assert_eq!(
        switcher["languages"]["offered"],
        json!(["en", "sk", "de-AT"]),
        "{switcher}",
    );

    let _ = std::fs::remove_dir_all(file.parent().unwrap_or(&file));
    let _ = std::fs::remove_dir_all(light.parent().unwrap_or(&light));
    let _ = std::fs::remove_dir_all(locales.parent().unwrap_or(&locales));
}

/// UI-30: an installation whose ConfigMap has not been rendered, or has been rendered wrongly, looks
/// plain instead of failing to load — the login page needs a name and a colour before anyone has
/// signed in. Every unusable file is the same 200 with the installation's defaults, and a block that
/// says nothing about a short name gets the full one.
#[tokio::test]
async fn a_branding_file_that_is_not_one_serves_the_installations_defaults() {
    let unusable = [
        branding_file("empty", ""),
        branding_file("comment", "# nothing yet\n"),
        branding_file("not-yaml", "instanceName: \"a\"\n\tshortName: [unclosed\n"),
        branding_file("a-list", "- instanceName: \"a\"\n"),
        branding_file("a-scalar", "\"just a string\"\n"),
        branding_file("blank-name", "instanceName: \"   \"\n"),
        branding_file("wrong-types", "instanceName: 7\ncolours: \"blue\"\n"),
    ];

    for file in &unusable {
        let (status, branding) = branding_of(Some(file.display().to_string())).await;
        assert_eq!(status, StatusCode::OK, "{file:?}: {branding}");
        assert!(
            branding["instanceName"]
                .as_str()
                .is_some_and(|name| !name.trim().is_empty()),
            "{file:?} served no instance name: {branding}",
        );
        assert!(
            branding["colours"]["primary"]
                .as_str()
                .is_some_and(|colour| colour.starts_with('#')),
            "{file:?} served no colour: {branding}",
        );
    }

    // A path that is not a file at all, and no branding file configured: the same defaults.
    for path in [
        Some("/nowhere/at/all/branding.yaml".to_owned()),
        Some(std::env::temp_dir().display().to_string()),
        None,
    ] {
        let (status, branding) = branding_of(path.clone()).await;
        assert_eq!(status, StatusCode::OK, "{path:?}: {branding}");
        assert_eq!(branding["instanceName"], json!("joinedcontext"), "{path:?}");
    }

    // A block that names only the full name is served with the short name filled in from it, so a
    // sidebar never renders an empty label.
    let named = branding_file("named", "instanceName: \"Banská Bystrica Context\"\n");
    let (_, branding) = branding_of(Some(named.display().to_string())).await;
    assert_eq!(
        branding["shortName"],
        json!("Banská Bystrica Context"),
        "{branding}",
    );

    for file in unusable.iter().chain(std::iter::once(&named)) {
        let _ = std::fs::remove_dir_all(file.parent().unwrap_or(file));
    }
}

// -------------------------------------------------------------------------------------------------
// T-1995 `get_asset`
// -------------------------------------------------------------------------------------------------

async fn asset(
    file: &std::path::Path,
    name: &str,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    let app = server::app(AppState::new(config, None));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/branding/{name}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let nosniff = response
        .headers()
        .get("x-content-type-options")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes()
        .to_vec();
    (status, content_type, nosniff, bytes)
}

/// UI-30: two assets are configured and two are reachable. Every other name is a 404, and the 404
/// says which asset was asked for and nothing about the container's filesystem.
#[tokio::test]
async fn only_the_two_assets_the_block_names_are_reachable() {
    let file = branding_file("assets", "logo: \"logo.svg\"\nfavicon: \"favicon.png\"\n");
    let dir = file.parent().expect("a directory").to_owned();
    std::fs::write(
        dir.join("logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .expect("the logo");
    std::fs::write(dir.join("favicon.png"), [0x89, b'P', b'N', b'G']).expect("the favicon");
    // A third file beside them, named by nothing in the block: not reachable by its own name.
    std::fs::write(
        dir.join("branding.yaml.bak"),
        "logo: \"../../etc/passwd\"\n",
    )
    .expect("a file");

    for (name, expected) in [("logo", "image/svg+xml"), ("favicon", "image/png")] {
        let (status, content_type, nosniff, bytes) = asset(&file, name).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(content_type, expected, "{name}");
        assert_eq!(nosniff.as_deref(), Some("nosniff"), "{name}");
        assert!(!bytes.is_empty(), "{name}");
    }

    for name in [
        "logo.svg",
        "LOGO",
        "Logo",
        "branding.yaml",
        "branding.yaml.bak",
        "..",
        "%2e%2e%2fbranding.yaml",
        "favicon.png",
        "logos",
    ] {
        let (status, _, _, bytes) = asset(&file, name).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}");
        let body = String::from_utf8_lossy(&bytes);
        for leaked in ["/tmp", "No such file", "os error", "passwd"] {
            assert!(
                !body.contains(leaked),
                "the 404 for {name} carried {leaked:?}: {body}",
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// UI-30: an asset is served as the image type its own name says, and anything else as a download.
/// A page dropped into the ConfigMap under the logo's name is `application/octet-stream` behind
/// `nosniff` and the Portal's own CSP, so it is a file a browser saves and never a document it runs.
#[tokio::test]
async fn a_page_dropped_in_the_configmap_is_a_download_and_never_a_document() {
    for (name, contents, expected) in [
        (
            "logo.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            "image/svg+xml",
        ),
        ("logo.png", "\u{89}PNG", "image/png"),
        ("logo.jpg", "\u{ff}\u{d8}", "image/jpeg"),
        ("logo.webp", "RIFF", "image/webp"),
        ("logo.ico", "\u{0}\u{0}", "image/vnd.microsoft.icon"),
        // Not an image, by extension or by absence of one.
        (
            "logo.html",
            "<script>fetch('/api/v1/projects')</script>",
            "application/octet-stream",
        ),
        ("logo.SVG", "<svg/>", "application/octet-stream"),
        ("logo", "<svg/>", "application/octet-stream"),
        (
            "logo.svg.html",
            "<script>1</script>",
            "application/octet-stream",
        ),
    ] {
        let file = branding_file("media", &format!("logo: \"{name}\"\n"));
        let dir = file.parent().expect("a directory").to_owned();
        std::fs::write(dir.join(name), contents).expect("the asset");

        let (status, content_type, nosniff, _) = asset(&file, "logo").await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(content_type, expected, "{name}");
        assert_eq!(nosniff.as_deref(), Some("nosniff"), "{name}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
