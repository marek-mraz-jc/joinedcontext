#![forbid(unsafe_code)]
//! The sign-in page against old prod's Keycloak theme (joinedcontext on keycloak.v2): per
//! language the tab title names the organization once, the header carries its name and logo,
//! the branding colour is the configured one, and every text old login.ftl showed is the text
//! old prod showed (shot from the old image in evidence/jc-login-pages). No field hints, and one
//! message for a wrong password and an unknown user.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const CALLBACK: &str = "https://portal.dev.example.test/callback";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";
const MEMBER: &str = "demo.orgmember@hel.fi";
const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000bf"/></svg>"##;

/// What old prod's sign-in page said in one language (realm display name "joinedcontext demo",
/// client `${jcClientPortal}`, e-mail as username).
struct Old {
    lang: &'static str,
    title: &'static str,
    heading: &'static str,
    username: &'static str,
    password: &'static str,
    forgot: &'static str,
    sign_in: &'static str,
    show_password: &'static str,
    wrong: &'static str,
    languages: [&'static str; 4],
}

const OLD: [Old; 4] = [
    Old {
        lang: "en",
        title: "Sign in to the Portal · joinedcontext demo",
        heading: "Sign in to the Portal",
        username: "Email",
        password: "Password",
        forgot: "Forgot your password?",
        sign_in: "Sign in",
        show_password: "Show password",
        wrong: "The e-mail or password is not right. Check both and try again.",
        languages: [
            "Czech (Čeština)",
            "English",
            "German (Deutsch)",
            "Slovak (Slovenčina)",
        ],
    },
    Old {
        lang: "sk",
        title: "Prihlásenie: Portál · joinedcontext demo",
        heading: "Prihlásenie: Portál",
        username: "E-mail",
        password: "Heslo",
        forgot: "Zabudli ste heslo?",
        sign_in: "Prihlásiť sa",
        show_password: "Zobraziť heslo",
        wrong: "E-mail alebo heslo nie je správne. Skontrolujte oboje a skúste to znova.",
        languages: [
            "Angličtina (English)",
            "Čeština (Čeština)",
            "Nemčina (Deutsch)",
            "Slovenčina",
        ],
    },
    Old {
        lang: "cs",
        title: "Přihlášení: Portál · joinedcontext demo",
        heading: "Přihlášení: Portál",
        username: "E-mail",
        password: "Heslo",
        forgot: "Zapomněli jste heslo?",
        sign_in: "Přihlásit se",
        show_password: "Zobrazit heslo",
        wrong: "E-mail nebo heslo není správné. Zkontrolujte obojí a zkuste to znovu.",
        languages: [
            "Angličtina (English)",
            "Čeština",
            "Němčina (Deutsch)",
            "Slovenština (Slovenčina)",
        ],
    },
    Old {
        lang: "de",
        title: "Anmelden: Portal · joinedcontext demo",
        heading: "Anmelden: Portal",
        username: "E-Mail",
        password: "Passwort",
        forgot: "Passwort vergessen?",
        sign_in: "Anmelden",
        show_password: "Passwort einblenden",
        wrong: "E-Mail oder Passwort stimmt nicht. Prüfen Sie beides und versuchen Sie es erneut.",
        languages: [
            "Deutsch",
            "Englisch (English)",
            "Slowakisch (Slovenčina)",
            "Tschechisch (Čeština)",
        ],
    },
];

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

/// The dev realm set up as old prod's smartcity realm: its display name, e-mail as username,
/// en/sk/cs/de, the Portal client's translated name and the branding block.
async fn app(display_name: &str) -> (Router, Arc<MemoryStore>) {
    let (state, store, _) = setup_dev_realm_state().await;
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm.display_name = display_name.to_owned();
    realm.registration_email_as_username = true;
    realm.login_with_email_allowed = true;
    realm.internationalization_enabled = true;
    realm.supported_locales = ["en", "sk", "cs", "de"].map(str::to_owned).to_vec();
    realm.default_locale = "en".into();
    let realm_id = realm.id.clone();
    store.upsert_realm(realm).await.unwrap();
    let mut edge = store
        .get_client_by_client_id(&realm_id, "edge")
        .await
        .unwrap()
        .unwrap();
    edge.name = Some("${jcClientPortal}".into());
    store.upsert_client(edge).await.unwrap();
    let mut branding = BrandingBlock {
        realm_id,
        organisation: display_name.to_owned(),
        logo: "logo.svg".into(),
        logo_svg: LOGO.into(),
        ..BrandingBlock::default()
    };
    branding.colours.primary = "#0000bf".into();
    store.upsert_branding(branding).await.unwrap();
    (router(state, mount()).layer(Extension(mount())), store)
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    form: Option<&[(&str, &str)]>,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let body = match form {
        Some(f) => {
            b = b.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(f)
                    .finish(),
            )
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

/// The sign-in page of a fresh authorization request in `lang`: its URL, cookie and HTML.
async fn sign_in_page(app: &Router, lang: &str) -> (String, String, String) {
    let uri = format!(
        "{AUTH}?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=st&code_challenge={}&code_challenge_method=S256&ui_locales={lang}",
        url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    );
    let (st, h, _) = send(app, Method::GET, &uri, None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let url = h[header::LOCATION].to_str().unwrap().to_owned();
    let cookie = h[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (st, _, html) = send(app, Method::GET, &url, None, Some(&cookie)).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    (url, cookie, html)
}

/// The page's text: tags dropped, entities of the texts above decoded, whitespace collapsed.
fn text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn title(html: &str) -> &str {
    let start = html.find("<title>").expect("a title") + "<title>".len();
    &html[start..start + html[start..].find("</title>").unwrap()]
}

#[tokio::test]
async fn the_sign_in_page_says_what_old_prod_said_in_every_language() {
    let (app, _) = app("joinedcontext demo").await;
    for old in &OLD {
        let lang = old.lang;
        let (url, cookie, html) = sign_in_page(&app, lang).await;
        assert!(html.contains(&format!("lang=\"{lang}\"")), "{lang}: {html}");
        assert_eq!(title(&html), old.title, "{lang}");
        assert_eq!(
            title(&html).matches("joinedcontext demo").count(),
            1,
            "{lang}"
        );
        assert!(
            html.contains("<span class=\"jc-org\">joinedcontext demo</span>"),
            "{lang}: the header names the organization: {html}"
        );
        assert!(
            html.contains("<img class=\"jc-logo\" src=\"/realms/dev/login-resources/logo.svg\""),
            "{lang}: the header shows the logo"
        );
        assert!(
            html.contains("--jc-primary: #0000bf;"),
            "{lang}: the configured colour"
        );
        let page = text(&html);
        for (what, want) in [
            ("heading", old.heading),
            ("username label", old.username),
            ("password label", old.password),
            ("forgot link", old.forgot),
            ("sign-in button", old.sign_in),
        ] {
            assert!(page.contains(want), "{lang}: {what} `{want}` in `{page}`");
        }
        assert!(
            html.contains(&format!("aria-label=\"{}\"", old.show_password)),
            "{lang}"
        );
        for label in old.languages {
            assert!(page.contains(label), "{lang}: picker offers `{label}`");
        }
        assert!(
            html.contains("aria-label=\"languages\""),
            "{lang}: old prod's picker label"
        );
        assert!(
            !html.contains("kc-field-hint"),
            "{lang}: old prod showed no field hints"
        );

        // One message for a wrong password and for an unknown user (no account enumeration).
        let csrf = extract_hidden_input(&html, "csrf_token");
        let session = extract_hidden_input(&html, "session_code");
        let mut seen = Vec::new();
        for user in [MEMBER, "nobody@example.invalid"] {
            let (st, _, html) = send(
                &app,
                Method::POST,
                &url,
                Some(&[
                    ("csrf_token", csrf.as_str()),
                    ("session_code", session.as_str()),
                    ("username", user),
                    ("password", "Not-The-Password-1"),
                ]),
                Some(&cookie),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert!(text(&html).contains(old.wrong), "{lang} {user}: {html}");
            seen.push(text(&html).replace(user, ""));
        }
        assert_eq!(seen[0], seen[1], "{lang}: the two refusals read the same");
    }
}

#[tokio::test]
async fn the_page_names_the_configured_organization_and_no_demo() {
    let (app, _) = app("City of Example").await;
    let (_, _, html) = sign_in_page(&app, "en").await;
    assert_eq!(title(&html), "Sign in to the Portal · City of Example");
    assert!(html.contains("<span class=\"jc-org\">City of Example</span>"));
    assert!(!html.to_lowercase().contains("demo"), "{html}");
}

/// An Organization's own block, keyed by its Portal host (UIX-16, T-6777): a sign-in that
/// returns there shows its name, colour and logo over the realm's; another host keeps the
/// realm's (the test above).
#[tokio::test]
async fn a_sign_in_from_an_organizations_portal_shows_its_own_block() {
    let (app, store) = app("City of Example").await;
    let realm_id = store.get_realm("dev").await.unwrap().unwrap().id;
    let host = url::Url::parse(CALLBACK)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned();
    let own = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle r="5" fill="#0b5394"/></svg>"##;
    let mut block = BrandingBlock {
        realm_id,
        org_id: Some(host.clone()),
        organisation: "Banskobystrický kraj".into(),
        logo_svg: own.into(),
        ..BrandingBlock::default()
    };
    block.colours.primary = "#0b5394".into();
    store.upsert_branding(block).await.unwrap();
    let (_, _, html) = sign_in_page(&app, "en").await;
    assert_eq!(title(&html), "Sign in to the Portal · Banskobystrický kraj");
    assert!(html.contains("#0b5394"), "its colour: {html}");
    let logo = format!("/realms/dev/login-resources/logo.svg?org={host}");
    assert!(html.contains(&logo), "its logo: {html}");
    let (st, h, body) = send(&app, Method::GET, &logo, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(body, own);
    let (_, _, realms) = send(
        &app,
        Method::GET,
        "/realms/dev/login-resources/logo.svg?org=elsewhere.example",
        None,
        None,
    )
    .await;
    assert_eq!(realms, LOGO, "an unknown key serves the realm's logo");
}

#[tokio::test]
async fn the_logo_is_served_as_an_image_that_runs_nothing() {
    let (app, _) = app("joinedcontext demo").await;
    let (st, h, body) = send(
        &app,
        Method::GET,
        "/realms/dev/login-resources/logo.svg",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "image/svg+xml");
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(body, LOGO);
}

#[tokio::test]
async fn the_signed_out_page_follows_its_language_picker_and_ends_nothing() {
    let (app, _) = app("joinedcontext demo").await;
    let (st, h, html) = send(
        &app,
        Method::GET,
        "/realms/dev/logged-out?kc_locale=sk",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        html.contains("lang=\"sk\"") && html.contains("data-page-id=\"login-info\""),
        "{html}"
    );
    assert!(
        html.contains("/realms/dev/logged-out?kc_locale=de"),
        "the picker stays on the page"
    );
    assert!(h.get(header::SET_COOKIE).is_none(), "it touches no session");
    let (st, _, _) = send(&app, Method::GET, "/realms/nope/logged-out", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// "Start signing in again": only the browser holding the session restarts it, and nothing the
/// person had passed survives.
#[tokio::test]
async fn restart_needs_the_sessions_browser_and_clears_what_was_passed() {
    let (app, store) = app("joinedcontext demo").await;
    let (_, cookie, html) = sign_in_page(&app, "en").await;
    let code = extract_hidden_input(&html, "session_code");
    let mut s = store
        .get_auth_session_by_id("dev", &code)
        .await
        .unwrap()
        .unwrap();
    s.user_id = Some("someone".into());
    s.pending_email_code_hash = Some("hash".into());
    s.kc_action_done = true;
    store.put_auth_session(s).await.unwrap();
    let restart = format!("/realms/dev/login-actions/restart?session_code={code}&client_id=edge");

    let (st, _, html) = send(&app, Method::GET, &restart, None, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{html}");
    let kept = store
        .get_auth_session_by_id("dev", &code)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        kept.user_id.as_deref(),
        Some("someone"),
        "another browser changes nothing"
    );

    let (st, h, _) = send(&app, Method::GET, &restart, None, Some(&cookie)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        h[header::LOCATION]
            .to_str()
            .unwrap()
            .contains("/login-actions/authenticate?session_code=")
    );
    let s = store
        .get_auth_session_by_id("dev", &code)
        .await
        .unwrap()
        .unwrap();
    assert!(s.user_id.is_none() && s.pending_email_code_hash.is_none() && !s.kc_action_done);
}
