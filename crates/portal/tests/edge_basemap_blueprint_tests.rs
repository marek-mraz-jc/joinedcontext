//! Edge cases of the basemap pair and the blueprint gallery (T-1990, T-1991, T-1992; AP-67, CC-59,
//! SDK-16).
//!
//! **The contract, in one sentence:** the basemap is one style and one grid of tiles of one project,
//! refused by name and by coordinate before anything upstream is called, and the gallery shows a
//! blueprint only to a caller whose realm role the blueprint itself names.
//!
//! The basemap pair carries no `CurrentUser` on purpose: a sandboxed preview frame has no session, so
//! a map it draws would otherwise have to reach a tile provider directly and hand it the person's
//! coordinates. The Portal proxies instead, and the edge is what keeps the route inside the
//! installation (`joinedcontext-deployment/components/portal/apisix-plugins.yaml` runs
//! `openid-connect` in session mode over `/api/v1/*`). What this file proves is the rest: a style
//! nobody serves, a project the mirror does not hold, a coordinate that is not one, and an answer that
//! is the same for everybody because it is about a map and not about a person.
//!
//! The happy paths live in `basemap_tests.rs` (a style document, a cached tile, a redacted key) and
//! `blueprint_gallery_tests.rs` (the cards a caller may run, the flow one starts). This file is the
//! other side.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::{BasemapConfig, Config};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "helsinki";
const UPSTREAM_KEY: &str = "basemap-key-no-answer-carries";

// -------------------------------------------------------------------------------------------------
// The basemap: one project of the mirror, one upstream that counts what reaches it
// -------------------------------------------------------------------------------------------------

fn basemap_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ContextSpace".into(),
        metadata: ObjectMeta {
            name: PROJECT.into(),
            namespace: Some(PROJECT.into()),
            ..Default::default()
        },
        spec: json!({}),
        status: None,
    });
    mirror
}

/// A tile server that answers a one-pixel PNG and counts every call, so a case can prove that a
/// refusal happened before anything left the Portal.
async fn upstream() -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address").to_string();
    let app = axum::Router::new().route(
        "/{z}/{x}/{tile}",
        axum::routing::get(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'],
                )
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, calls)
}

fn cache_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "jc-edge-basemap-{name}-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ))
}

/// A Portal proxying `address`, with an upstream key in the template, so no case can pass by having
/// nothing to leak.
fn basemap_app(address: &str, cache: std::path::PathBuf) -> axum::Router {
    let mut config = Config::for_tests();
    let mut basemap = BasemapConfig::for_tests(
        format!("http://{address}/{{z}}/{{x}}/{{y}}.png?key={{key}}"),
        "© OpenStreetMap contributors".to_owned(),
        cache,
    );
    basemap.key = Some(UPSTREAM_KEY.to_owned());
    config.basemap = Some(basemap);
    server::app(AppState::new(config, None).with_mirror(basemap_mirror()))
}

async fn anonymous(app: &axum::Router, uri: &str) -> (StatusCode, HeaderMap, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

// -------------------------------------------------------------------------------------------------
// T-1990 `get_style`
// -------------------------------------------------------------------------------------------------

/// AP-67: this Portal serves one style, called `default`. Every other name is a problem document, and
/// the name is echoed inside JSON — a style called `</script>` comes back as text of a document, never
/// as markup a frame would run.
#[tokio::test]
async fn a_style_this_portal_does_not_serve_is_not_found_whatever_it_is_called() {
    let (address, calls) = upstream().await;
    let cache = cache_dir("style-name");
    let app = basemap_app(&address, cache.clone());

    for style in [
        "Default",
        "satellite",
        "default%20",
        "%2e%2e",
        "..%2Fdefault",
        "%3C%2Fscript%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E",
    ] {
        let (status, headers, body) = anonymous(
            &app,
            &format!("/api/v1/projects/{PROJECT}/basemap/{style}/style.json"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{style}: {body}");
        assert_eq!(
            content_type(&headers),
            "application/problem+json",
            "{style} was not answered as a problem document",
        );
        // The name is echoed inside a JSON string, so a name that looks like markup stays one
        // value of one document: `application/problem+json` is never parsed as HTML, and the
        // escaping holds whatever was sent.
        let problem: Value = serde_json::from_str(&body).expect("a problem document");
        assert!(problem["detail"].is_string(), "{style}: {body}");
        assert!(!body.contains(UPSTREAM_KEY), "{style}: {body}");
    }

    // The one name it serves is served, and nothing was asked of the tile server for any of it.
    let (status, _, body) = anonymous(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_dir_all(&cache);
}

/// SDK-16: the style hands a preview frame one origin to talk to — this Portal's own basemap prefix.
/// The tile server behind it and the key that opens it are named nowhere in the document, so a frame
/// whose `connect-src` is this Portal draws a map without ever reaching a third party.
#[tokio::test]
async fn the_style_names_this_portal_and_never_the_upstream_it_proxies() {
    let (address, calls) = upstream().await;
    let cache = cache_dir("style-origin");
    let app = basemap_app(&address, cache.clone());

    let (status, _, body) = anonymous(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let style: Value = serde_json::from_str(&body).expect("a style document");

    let tiles = style["sources"]["raster-tiles"]["tiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(tiles.len(), 1, "{style}");
    let url = tiles[0].as_str().unwrap_or_default();
    assert!(
        url.starts_with("http://localhost:8080/api/v1/projects/helsinki/basemap/default/"),
        "the frame is sent somewhere else: {url}",
    );
    for never in [UPSTREAM_KEY, address.as_str(), "key="] {
        assert!(!body.contains(never), "the style carried {never:?}: {body}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "a style asked for a tile");
    let _ = std::fs::remove_dir_all(&cache);
}

/// AP-67: the basemap is a project's. A project the mirror does not hold has none, on both routes, and
/// the rule is the mirror's own namespaces — membership is not consulted here, because the caller of
/// this route is a preview frame with no session. The edge is what keeps the route inside the
/// installation (`components/portal/apisix-plugins.yaml`).
#[tokio::test]
async fn a_project_the_mirror_does_not_hold_has_no_basemap_on_either_route() {
    let (address, calls) = upstream().await;
    let cache = cache_dir("project");
    let app = basemap_app(&address, cache.clone());

    for project in [
        "espoo",
        "no-such-project",
        "%2e%2e",
        "Helsinki",
        "helsinki%20",
    ] {
        for uri in [
            format!("/api/v1/projects/{project}/basemap/default/style.json"),
            format!("/api/v1/projects/{project}/basemap/default/1/0/0.png"),
        ] {
            let (status, headers, body) = anonymous(&app, &uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
            assert_eq!(content_type(&headers), "application/problem+json", "{uri}");
            assert!(!body.contains(UPSTREAM_KEY), "{uri}: {body}");
        }
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a project nobody has reached the tile server",
    );
    let _ = std::fs::remove_dir_all(&cache);
}

// -------------------------------------------------------------------------------------------------
// T-1991 `get_tile`
// -------------------------------------------------------------------------------------------------

/// AP-67: a tile is three numbers and an image extension. Anything else is refused here, before a
/// request is built for the tile server, so a coordinate can neither steer the upstream URL nor spend
/// the installation's quota on the way to being rejected.
#[tokio::test]
async fn a_coordinate_that_is_not_a_tiles_never_reaches_the_upstream() {
    let (address, calls) = upstream().await;
    let cache = cache_dir("coordinates");
    let app = basemap_app(&address, cache.clone());

    for (z, x, tile) in [
        // A zoom that is not a number, or not one a `u8` holds.
        ("abc", "0", "0.png"),
        ("", "0", "0.png"),
        ("256", "0", "0.png"),
        ("-1", "0", "0.png"),
        ("1.0", "0", "0.png"),
        ("%201", "0", "0.png"),
        ("0x1", "0", "0.png"),
        // An x that is not a number, or not one a `u32` holds.
        ("1", "4294967296", "0.png"),
        ("1", "1e2", "0.png"),
        ("1", "-0", "0.png"),
        // A y that is not a number.
        ("1", "0", "y.png"),
        ("1", "0", ".png"),
        ("1", "0", "1%2e0.png"),
        // An extension this Portal does not serve, case included: the comparison is exact.
        ("1", "0", "0.PNG"),
        ("1", "0", "0.webp"),
        ("1", "0", "0.png.svg"),
    ] {
        let uri = format!("/api/v1/projects/{PROJECT}/basemap/default/{z}/{x}/{tile}");
        let (status, headers, body) = anonymous(&app, &uri).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{uri} answered {status}: {body}",
        );
        assert_eq!(content_type(&headers), "application/problem+json", "{uri}");
        assert!(!body.contains(UPSTREAM_KEY), "{uri}: {body}");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a coordinate that is not one reached the tile server",
    );

    // A tile that is three numbers is fetched, so the refusals above are the check and not a
    // route that answers nothing.
    let (status, headers, _) = anonymous(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/1/0/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type(&headers), "image/png");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(&cache);
}

/// AP-67, SDK-16: the basemap answers a frame that has no session, so its answer is about a map and
/// never about a person — no cookie is set, nothing varies by cookie, and any origin may read it.
#[tokio::test]
async fn a_basemap_answer_is_the_same_for_everybody_and_carries_nothing_of_a_person() {
    let (address, _) = upstream().await;
    let cache = cache_dir("anonymous");
    let app = basemap_app(&address, cache.clone());

    for uri in [
        format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
        format!("/api/v1/projects/{PROJECT}/basemap/default/2/1/1.png"),
    ] {
        let (status, headers, _) = anonymous(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            headers.get(header::SET_COOKIE).is_none(),
            "{uri} set a cookie on a frame with no session",
        );
        assert!(
            !headers
                .get(header::VARY)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("cookie"),
            "{uri} answers differently per person",
        );
        assert_eq!(
            headers
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("*"),
            "{uri} is not readable by a sandboxed frame",
        );
        // The route's own header survives the blanket API rule, which is what lets a browser
        // keep a tile instead of refetching the whole grid on every pan (T-2295). It is safe to
        // keep exactly because of the three assertions above: no cookie, no `Vary: Cookie`, and
        // readable by any origin — the answer is about a map and not about a person.
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("public, max-age=3600"),
            "{uri} may not be kept by a browser",
        );
    }
    let _ = std::fs::remove_dir_all(&cache);
}

// -------------------------------------------------------------------------------------------------
// T-1992 `list_blueprints`
// -------------------------------------------------------------------------------------------------

const TEMPLATE: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Dashboard
metadata:
  name: alert-{{ title }}
spec:
  title: {{ title }}
";

fn blueprint(name: &str, namespace: &str, allowed_roles: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Blueprint".into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec: json!({
            "version": "1.0.0",
            "category": "alerting",
            "riskClass": "green",
            "allowedRoles": allowed_roles,
            "parameterSchema": {
                "type": "object",
                "required": ["title"],
                "properties": { "title": { "type": "string" } },
            },
            "templates": [{ "name": "dashboard", "template": TEMPLATE }],
        }),
        status: None,
    }
}

fn gallery_cookie(config: &Config, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@hel.fi".into()),
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
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The gallery of a caller holding `roles`, over a mirror seeded with `blueprints`.
async fn gallery(blueprints: Vec<ResourceEnvelope>, roles: &[&str]) -> Value {
    let config = Config::for_tests();
    let cookie = gallery_cookie(&config, roles);
    let mirror = Arc::new(Mirror::new());
    for envelope in blueprints {
        mirror.upsert(envelope);
    }
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blueprints")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("a list")
}

fn listed(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            item["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// CC-59, fail closed: `spec.allowedRoles` names the roles that may run a blueprint, and a blueprint
/// that does not name one is visible to nobody — not to a caller holding every role in the realm, and
/// not to the installation's bootstrap admin. A malformed list is a broken manifest, never an open
/// door.
#[tokio::test]
async fn a_blueprint_whose_roles_are_not_a_list_of_names_is_visible_to_nobody() {
    let malformed = vec![
        blueprint("no-roles-key", "org", Value::Null),
        blueprint("empty-list", "org", json!([])),
        blueprint("a-string", "org", json!("domain-editor")),
        blueprint("an-object", "org", json!({ "role": "domain-editor" })),
        blueprint("a-number", "org", json!(7)),
        blueprint("nulls-and-numbers", "org", json!([Value::Null, 7])),
        blueprint("an-empty-name", "org", json!([""])),
        blueprint("named", "org", json!(["domain-editor"])),
    ];

    for roles in [
        vec!["domain-editor"],
        vec!["portal-approver"],
        vec!["domain-editor", "portal-approver", "org-admin"],
        vec![],
    ] {
        let list = gallery(malformed.clone(), &roles).await;
        let names = listed(&list);
        for hidden in [
            "no-roles-key",
            "empty-list",
            "a-string",
            "an-object",
            "a-number",
            "nulls-and-numbers",
            "an-empty-name",
        ] {
            assert!(
                !names.contains(&hidden.to_owned()),
                "{roles:?} was shown '{hidden}', which names no role",
            );
        }
    }

    // The one that names a role is shown to whoever holds it, so the rule is the roles and not an
    // empty gallery.
    assert_eq!(
        listed(&gallery(malformed.clone(), &["domain-editor"]).await),
        vec!["named".to_owned()],
    );
}

/// CC-59: a role is matched by its whole name. A near miss — another case, a prefix, a suffix, a name
/// with space around it — is not the role, and holding the role twice shows the card once.
#[tokio::test]
async fn a_role_is_matched_by_its_whole_name_and_a_card_is_shown_once() {
    let one = vec![blueprint(
        "threshold-alert",
        "org",
        json!(["domain-editor"]),
    )];

    for near_miss in [
        "Domain-Editor",
        "DOMAIN-EDITOR",
        "domain",
        "domain-editor-x",
        "x-domain-editor",
        " domain-editor",
        "domain-editor ",
        "domain_editor",
    ] {
        assert!(
            listed(&gallery(one.clone(), &[near_miss]).await).is_empty(),
            "'{near_miss}' was read as the role 'domain-editor'",
        );
    }

    assert_eq!(
        listed(&gallery(one.clone(), &["domain-editor"]).await),
        vec!["threshold-alert".to_owned()],
    );
    // The same role twice, and a blueprint naming two roles the caller holds: one card either way.
    assert_eq!(
        listed(&gallery(one, &["domain-editor", "domain-editor"]).await),
        vec!["threshold-alert".to_owned()],
    );
    assert_eq!(
        listed(
            &gallery(
                vec![blueprint(
                    "threshold-alert",
                    "org",
                    json!(["domain-editor", "org-admin"]),
                )],
                &["domain-editor", "org-admin"],
            )
            .await
        ),
        vec!["threshold-alert".to_owned()],
    );
}

/// CC-59: the gallery is the organization's, so a `Blueprint` filed under a project is not in it —
/// whoever wrote it there cannot publish a template to the whole installation by choosing a namespace.
/// A caller who holds none of the roles gets an empty list rather than a refusal: a gallery with no
/// cards is a screen, not an error.
#[tokio::test]
async fn the_gallery_is_the_organizations_and_an_empty_one_is_still_a_list() {
    let mixed = vec![
        blueprint("organization-wide", "org", json!(["domain-editor"])),
        blueprint("planted-in-a-project", PROJECT, json!(["domain-editor"])),
        blueprint("planted-in-another-org", "org-2", json!(["domain-editor"])),
    ];

    assert_eq!(
        listed(&gallery(mixed.clone(), &["domain-editor"]).await),
        vec!["organization-wide".to_owned()],
    );

    let empty = gallery(mixed, &["someone-elses-role"]).await;
    assert_eq!(empty["kind"], json!("List"), "{empty}");
    assert_eq!(empty["items"], json!([]), "{empty}");
    assert!(
        empty["metadata"]["continue"].is_null(),
        "an empty gallery offered a next page: {empty}",
    );
}
