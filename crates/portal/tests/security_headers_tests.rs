//! T-1732: the response headers a browser enforces the Portal's own rules with (PF-50, UI-01).
//!
//! Verified against dev on 2026-09-20 before this was written: the Portal's headers reach the
//! browser through the edge unchanged (`curl -sI` on `/api/v1/branding` carries the policy below).
//! The one header the edge overrides is `Referrer-Policy`, which is asserted in the deployment
//! tests rather than here, because it is the edge that sets it.
//!
//! What this file holds is the Portal's own promise: a policy with no way to run a string as
//! script, and no way for another origin to put the Portal in a frame.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

/// The headers a page of the Portal is served with, from the running router.
async fn page_headers() -> HeaderMap {
    let app = server::app(AppState::new(Config::for_tests(), None));
    app.oneshot(
        Request::builder()
            .uri("/catalogue")
            .body(Body::empty())
            .expect("request"),
    )
    .await
    .expect("response")
    .headers()
    .clone()
}

/// The policy a page is served with, read from the running router rather than from the source.
async fn policy() -> String {
    page_headers()
        .await
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every page carries a Content-Security-Policy")
        .to_str()
        .expect("a policy is ASCII")
        .to_owned()
}

/// A directive of the policy, by name.
fn directive<'a>(policy: &'a str, name: &str) -> Option<&'a str> {
    policy
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(name)?.strip_prefix(' '))
}

#[tokio::test]
async fn the_portal_sends_a_csp_without_unsafe_inline_scripts() {
    let policy = policy().await;
    // `script-src` is not named, so scripts fall back to `default-src`; whichever applies, a
    // string must not become code.
    let scripts = directive(&policy, "script-src")
        .or_else(|| directive(&policy, "default-src"))
        .expect("scripts are covered by a directive");
    assert!(
        !scripts.contains("unsafe-inline"),
        "scripts may not be inline: {policy}"
    );
    assert!(
        !scripts.contains("unsafe-eval"),
        "scripts may not be built from a string: {policy}"
    );
    assert!(
        !policy.contains("unsafe-eval"),
        "nothing may be built from a string: {policy}"
    );
    assert_eq!(
        scripts, "'self'",
        "scripts come from this origin only: {policy}"
    );
}

/// T-5828 (ZAP 10055): a `<style>` element applies only with the answer's own nonce, so markup
/// injected into a page cannot bring a stylesheet; a nonce is never reused across answers.
#[tokio::test]
async fn the_portal_takes_no_inline_stylesheet_without_its_nonce() {
    let first = policy().await;
    let styles = directive(&first, "style-src").expect("styles have their own directive");
    assert!(
        !styles.contains("unsafe-inline"),
        "a stylesheet may not be inline without the nonce: {first}"
    );
    let nonce = styles
        .split_whitespace()
        .find_map(|source| source.strip_prefix("'nonce-")?.strip_suffix('\''))
        .expect("style-src names the answer's nonce");
    assert!(nonce.len() >= 22, "a nonce of 128 bits: {nonce}");
    assert!(
        !policy().await.contains(nonce),
        "every answer gets a nonce of its own"
    );
    // Monaco's `style="top:..."` attributes: an attribute holds no selector.
    assert_eq!(directive(&first, "style-src-attr"), Some("'unsafe-inline'"));
}

#[tokio::test]
async fn the_portal_cannot_be_framed_by_another_origin() {
    let policy = policy().await;
    assert_eq!(
        directive(&policy, "frame-ancestors"),
        Some("'self'"),
        "only the Portal may frame the Portal: {policy}"
    );
    // The older header says the same thing for a browser that does not read `frame-ancestors`.
    let legacy = page_headers()
        .await
        .get("x-frame-options")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_uppercase();
    assert!(
        legacy == "SAMEORIGIN" || legacy == "DENY",
        "x-frame-options was {legacy:?}"
    );
}

#[tokio::test]
async fn a_page_reaches_nothing_it_was_not_served_from() {
    let policy = policy().await;
    assert_eq!(directive(&policy, "default-src"), Some("'self'"));
    assert_eq!(directive(&policy, "connect-src"), Some("'self'"));
    assert_eq!(directive(&policy, "base-uri"), Some("'self'"));
    assert_eq!(directive(&policy, "form-action"), Some("'self'"));
    // A plugin document is a way to run code that `script-src` does not cover.
    assert_eq!(directive(&policy, "object-src"), Some("'none'"));
}

/// The Open page frames an App from the apps origin or its own host under it (AP-122, AP-133,
/// T-2871): `frame-src` names those and nothing else, and without an apps origin the Portal
/// frames only itself.
#[tokio::test]
async fn a_page_frames_only_itself_and_the_apps_origin() {
    assert_eq!(directive(&policy().await, "frame-src"), Some("'self'"));

    let mut config = Config::for_tests();
    config.apps_url = Some("https://apps.city.example".parse().expect("a url"));
    let app = server::app(AppState::new(config, None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/catalogue")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("ASCII")
        .to_owned();
    assert_eq!(
        directive(&policy, "frame-src"),
        Some("'self' https://apps.city.example https://*.apps.apps.city.example")
    );
    assert_eq!(directive(&policy, "frame-ancestors"), Some("'self'"));
}

/// An App host signs its visitor in through the realm, a redirect that happens inside the Open
/// page's frame (AP-122, T-2911): with an apps origin and a realm, `frame-src` names the realm's
/// origin (never its path), and a Portal without an apps origin frames only itself.
#[tokio::test]
async fn a_page_frames_the_realm_an_app_host_signs_in_through() {
    let realm = |key: &str| {
        match key {
            "JC_OIDC_ISSUER" => Some("https://idm.city.example/realms/city"),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    };
    let frame_src = |config: Config| async move {
        let response = server::app(AppState::new(config, None))
            .oneshot(
                Request::builder()
                    .uri("/catalogue")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("ASCII")
            .to_owned();
        directive(&policy, "frame-src").map(str::to_owned)
    };

    let mut config = Config::from_vars(realm).expect("a realm");
    config.apps_url = Some("https://apps.city.example".parse().expect("a url"));
    assert_eq!(
        frame_src(config).await.as_deref(),
        Some("'self' https://apps.city.example https://*.apps.apps.city.example https://idm.city.example")
    );

    let config = Config::from_vars(realm).expect("a realm");
    assert_eq!(frame_src(config).await.as_deref(), Some("'self'"));
}

#[tokio::test]
async fn every_answer_carries_the_headers_that_do_not_depend_on_the_route() {
    let headers = page_headers().await;
    let value = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    // A browser must not guess a type: an uploaded file answered as text is not script.
    assert_eq!(value("x-content-type-options"), "nosniff");
    // The Portal's own value; the edge sets a weaker one on top, which is a deployment matter.
    assert_eq!(value("referrer-policy"), "no-referrer");
    // A window this one opened cannot reach back into it.
    assert_eq!(value("cross-origin-opener-policy"), "same-origin");
    // The devices a page may ask for: none of them without being on this origin.
    let permissions = value("permissions-policy");
    assert!(permissions.contains("camera=()"), "{permissions}");
    assert!(permissions.contains("microphone=()"), "{permissions}");
}

/// EP-101 (T-3266): a public form's page may be framed by exactly the sites its Endpoint names,
/// and nothing else relaxes. A form that names none, an unknown slug, a list that breaks the
/// Endpoint's rule and every other page keep `frame-ancestors 'self'` and `SAMEORIGIN`.
#[tokio::test]
async fn a_form_page_is_framed_by_exactly_the_sites_its_endpoint_names() {
    use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    let state = AppState::new(Config::for_tests(), None);
    let endpoint = |name: &str, slug: &str, creates: serde_json::Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Endpoint".into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some("helsinki".into()),
            ..Default::default()
        },
        spec: json!({ "slug": slug, "creates": creates }),
        status: None,
    };
    state.mirror.upsert(endpoint(
        "feedback-form",
        "ep-framed",
        json!({ "mintIds": true, "embedOrigins": ["https://www.hel.fi", "https://news.hel.fi:8443"] }),
    ));
    state.mirror.upsert(endpoint(
        "plain-form",
        "ep-plain",
        json!({ "mintIds": true }),
    ));
    // Not a valid list: a directive smuggled after a `;` must never reach the header.
    state.mirror.upsert(endpoint(
        "broken-form",
        "ep-broken",
        json!({ "mintIds": true, "embedOrigins": ["https://a.example; script-src *"] }),
    ));
    let app = server::app(state);
    let headers = |uri: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response")
            .headers()
            .clone()
        }
    };

    let framed = headers("/f/ep-framed").await;
    let policy = framed[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("ASCII");
    assert_eq!(
        directive(policy, "frame-ancestors"),
        Some("'self' https://www.hel.fi https://news.hel.fi:8443")
    );
    assert_eq!(directive(policy, "default-src"), Some("'self'"), "{policy}");
    assert!(framed.get("x-frame-options").is_none());

    for uri in [
        "/f/ep-plain",
        "/f/ep-broken",
        "/f/ep-unknown",
        "/f/ep-framed/more",
        "/projects/helsinki/home",
        "/",
    ] {
        let kept = headers(uri).await;
        let policy = kept[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("ASCII");
        assert_eq!(
            directive(policy, "frame-ancestors"),
            Some("'self'"),
            "{uri}"
        );
        assert_eq!(kept["x-frame-options"], "SAMEORIGIN", "{uri}");
    }
}
