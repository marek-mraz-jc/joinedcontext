//! Double-submit CSRF protection for every mutating Portal API call.
//!
//! `SameSite=Lax` already keeps the session cookie off cross-site POSTs; this is the second
//! lock: the UI reads the readable `__Host-jc_csrf` cookie and echoes it in `X-CSRF-Token`.
//!
//! A signed-in request's token is bound to its session (RTE-12, SEC-10): an HMAC under the
//! cookie key over the Organization, the person and the instant the session began, so a token
//! of session A, planted or carried over, fails on session B. A request with no session rides no
//! credential, and there the cookie and the header only have to agree.

use crate::auth::session::{CurrentUser, Session};
use crate::state::AppState;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, Key, SameSite};
use hmac::{Hmac, Mac};
use openidconnect::CsrfToken;

/// Readable by the UI on purpose: the double-submit token is not a secret to the page.
pub const CSRF_COOKIE: &str = "__Host-jc_csrf";

/// The name an App built before T-4395 reads its token from: its bundle carries the SDK it was
/// built with until it is rebuilt. The apps origin hands it a copy of the same token; nothing
/// reads it back, `verify` compares the header with `CSRF_COOKIE` alone, so a planted copy can
/// make an old App's write fail but never pass one.
// ponytail: dropped once every App is rebuilt on an SDK that reads `CSRF_COOKIE`.
pub const LEGACY_APP_CSRF_COOKIE: &str = "jc_csrf";

/// A cookie that tells the browser to drop the CSRF token: same path, `Secure` (a browser ignores
/// a `__Host-` removal without it), `Max-Age=0`, expired. Unconditional like the session's own
/// removal (AP-29).
pub fn removal() -> Cookie<'static> {
    let mut cookie = Cookie::build((CSRF_COOKIE, ""))
        .path("/")
        .secure(true)
        .build();
    cookie.make_removal();
    cookie
}
pub const CSRF_HEADER: &str = "x-csrf-token";
const CSRF_TTL_SECS: i64 = 12 * 60 * 60;

/// A minted double-submit token, and the only thing `cookie` accepts (T-2289).
///
/// The cookie is written with the token in its value, so anything in the token is cookie syntax: a
/// `;` ends the value and starts an attribute (`; Domain=…` widens the cookie to a sibling host,
/// `; HttpOnly` takes the token away from the page that has to read it) and a CR/LF aims at the
/// response header. `cookie` used to take a `String`, which invited exactly that; no caller ever
/// passed one, because they all mint here, and this type is what keeps it that way — the value comes
/// from the CSPRNG and nothing else can build one.
#[derive(Clone)]
pub struct Token(String);

/// The value stays out of every log line (T-4321).
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Token").field(&"[redacted]").finish()
    }
}

impl Token {
    /// Fresh token, minted with the same CSPRNG the OIDC state uses. On an Organization's own
    /// Portal host it starts with that Organization's name and a `.` (a name is `[a-z0-9-]`, a
    /// random part base64url, so neither holds one): [`double_submitted`] accepts a token only
    /// on the host it was minted for.
    pub fn mint() -> Self {
        let random = CsrfToken::new_random().secret().clone();
        Self(match crate::organization_host::organization() {
            Some(organization) => format!("{organization}.{random}"),
            None => random,
        })
    }

    /// The token of `session` under `key`, prefixed like [`Token::mint`] on an Organization's
    /// host: `{organization}.{mac}` or `{mac}`, the mac base64url (no `.`).
    pub fn bound(key: &Key, session: &Session) -> Self {
        let organization = crate::organization_host::organization();
        let mac = session_mac(key, organization.as_deref(), session);
        Self(match organization {
            Some(organization) => format!("{organization}.{mac}"),
            None => mac,
        })
    }

    /// The token as the page echoes it back in `X-CSRF-Token`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// HMAC-SHA256 under the cookie key's signing half over the Organization, the person and the
/// instant the session began (`Session::started_at`, the same across refreshes), base64url.
fn session_mac(key: &Key, organization: Option<&str>, session: &Session) -> String {
    use base64::Engine as _;
    // HMAC takes a key of any length, so `new_from_slice` never fails and the default (an empty
    // mac) is never produced.
    <Hmac<sha2::Sha256> as Mac>::new_from_slice(key.signing())
        .map(|mut mac| {
            for part in [
                "jc-csrf",
                organization.unwrap_or(""),
                session.identity.subject.as_str(),
                &session.started_at().to_string(),
            ] {
                mac.update(part.as_bytes());
                mac.update(&[0]);
            }
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        })
        .unwrap_or_default()
}

/// The session a request's token must be bound to, with the keys that may have minted it.
#[derive(Clone, Copy)]
pub struct Bound<'a> {
    pub session: &'a Session,
    pub config: &'a crate::config::Config,
}

impl Bound<'_> {
    /// Whether `presented` is this session's token under the active key or one a rotation
    /// still lets in.
    fn accepts(&self, presented: &str) -> bool {
        let organization = crate::organization_host::organization();
        std::iter::once(&self.config.cookie_key)
            .chain(&self.config.cookie_keys_previous)
            .any(|key| {
                let mac = session_mac(key, organization.as_deref(), self.session);
                let expected = match organization.as_deref() {
                    Some(organization) => format!("{organization}.{mac}"),
                    None => mac,
                };
                constant_time_eq(&expected, presented)
            })
    }

    /// The token to hand this session: its own under the active key.
    pub fn token(&self) -> Token {
        Token::bound(&self.config.cookie_key, self.session)
    }
}

/// The signed-in session of a request, `None` for an anonymous one.
pub async fn session_of(state: &AppState, headers: &HeaderMap) -> Option<Session> {
    let mut parts = Request::builder().body(()).ok()?.into_parts().0;
    parts.headers = headers.clone();
    CurrentUser::from_request_parts(&mut parts, state)
        .await
        .ok()
        .map(|user| user.0)
}

/// Whether the request's `CSRF_COOKIE` is missing or no token of `bound`'s session, so the
/// page needs a new one (an edge or App session that changed under an old cookie).
pub fn needs_token(jar: &CookieJar, bound: Option<Bound<'_>>) -> bool {
    match (jar.get(CSRF_COOKIE).map(Cookie::value), bound) {
        (None, _) => true,
        (Some(value), Some(bound)) => !bound.accepts(value),
        (Some(_), None) => false,
    }
}

/// Fresh random string from the same CSPRNG, for a caller that needs an unguessable value.
pub fn new_token() -> String {
    CsrfToken::new_random().secret().clone()
}

/// The Organization a token was minted for ([`Token::mint`]), `None` for one of no host's.
fn minted_for(token: &str) -> Option<&str> {
    token.split_once('.').map(|(organization, _)| organization)
}

/// The cookie the UI reads; `HttpOnly` is deliberately off, `Secure` and `Lax` are not.
pub fn cookie(token: &Token) -> Cookie<'static> {
    named(CSRF_COOKIE, token)
}

/// `cookie` under `LEGACY_APP_CSRF_COOKIE`, for an App built before T-4395.
pub fn legacy_app_cookie(token: &Token) -> Cookie<'static> {
    named(LEGACY_APP_CSRF_COOKIE, token)
}

fn named(name: &'static str, token: &Token) -> Cookie<'static> {
    Cookie::build((name, token.0.clone()))
        .path("/")
        .http_only(false)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(CSRF_TTL_SECS))
        .build()
}

/// Byte-for-byte comparison that does not stop at the first difference. Used for the CSRF
/// token here and for the agent proxy's bearer on the internal listener (AG-52).
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    constant_time_bytes_eq(a.as_bytes(), b.as_bytes())
}

/// The bytes form of [`constant_time_eq`], the Portal's one such comparison.
pub(crate) fn constant_time_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    // black_box keeps the optimiser from turning the fold into an early-exit comparison.
    a.len() == b.len()
        && std::hint::black_box(a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y))) == 0
}

/// True when the request may proceed: safe methods always, mutations only with a matching token.
pub fn is_allowed(method: &Method, headers: &HeaderMap, bound: Option<Bound<'_>>) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    // A request that carries `Authorization` is authenticated by that header alone (the
    // session's `Front` rule: the bearer wins over the cookie), so no cookie is ever the
    // credential behind it and there is nothing for a cross-site form to ride on. Scripts,
    // the smoke, MCP clients and agents mutate this way (AG-59).
    headers.contains_key(axum::http::header::AUTHORIZATION) || double_submitted(headers, bound)
}

/// True when the request echoes its token in `X-CSRF-Token`, whatever else it carries: with a
/// session, that session's own token ([`Token::bound`]); without one, the value of its
/// `CSRF_COOKIE`. The App doors use it alone: there the edge sets the person's token from the
/// session cookie even beside an `Authorization` header, so that header is no reason to skip
/// the check (T-4915).
pub fn double_submitted(headers: &HeaderMap, bound: Option<Bound<'_>>) -> bool {
    let Some(presented) = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if let Some(bound) = bound {
        return bound.accepts(presented);
    }
    let jar = CookieJar::from_headers(headers);
    let Some(expected) = jar.get(CSRF_COOKIE).map(|c| c.value().to_string()) else {
        return false;
    };
    !expected.is_empty()
        && minted_for(&expected) == crate::organization_host::organization().as_deref()
        && constant_time_eq(&expected, presented)
}

/// Middleware for the `/api/v1` router. Rejects with problem+json, never with a bare 403 body.
pub async fn require_csrf(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, crate::error::ApiError> {
    // `is_allowed` lets these by whatever the session; skip resolving it.
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) || request
        .headers()
        .contains_key(axum::http::header::AUTHORIZATION)
    {
        return Ok(next.run(request).await);
    }
    let session = session_of(&state, request.headers()).await;
    let bound = session.as_ref().map(|session| Bound {
        session,
        config: &state.config,
    });
    if is_allowed(request.method(), request.headers(), bound) {
        Ok(next.run(request).await)
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

/// Adds a CSRF cookie bound to `bound`'s session (a fresh random one without a session) to a
/// jar, returning the token that was issued.
pub fn issue(jar: CookieJar, bound: Option<Bound<'_>>) -> (CookieJar, String) {
    let token = bound.map_or_else(Token::mint, |bound| bound.token());
    let value = token.as_str().to_owned();
    (jar.add(cookie(&token)), value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_debug_never_shows_the_value() {
        let token = Token::mint();
        let text = format!("{token:?}");
        assert!(
            !text.contains(&token.0) && text.contains("[redacted]"),
            "{text}"
        );
    }

    fn headers_with(cookie_value: Option<&str>, header_value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(v) = cookie_value {
            headers.insert(
                axum::http::header::COOKIE,
                format!("{CSRF_COOKIE}={v}").parse().expect("cookie header"),
            );
        }
        if let Some(v) = header_value {
            headers.insert(CSRF_HEADER, v.parse().expect("csrf header"));
        }
        headers
    }

    /// T-4915: on an App door the edge's cookie token rides beside any `Authorization` a page
    /// adds, so the bearer that `is_allowed` trusts waives nothing there.
    #[test]
    fn the_double_submit_ignores_an_authorization_header() {
        let mut headers = headers_with(None, None);
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer planted".parse().expect("bearer"),
        );
        assert!(is_allowed(&Method::POST, &headers, None));
        assert!(!double_submitted(&headers, None));
        let mut headers = headers_with(Some("abc"), Some("abc"));
        assert!(double_submitted(&headers, None));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer planted".parse().expect("bearer"),
        );
        assert!(double_submitted(&headers, None));
    }

    /// jc-multiorg-portal: a token minted on Organization A's Portal host passes there alone,
    /// never on B's or on the deployment's own address, nor an unpinned one on A's.
    #[tokio::test]
    async fn a_token_of_one_organizations_host_is_refused_on_another() {
        use crate::organization_host::{scope, OrganizationHost};
        let host = |name: &str| OrganizationHost {
            organization: name.to_owned(),
            base: format!("https://portal.{name}.example")
                .parse()
                .expect("url"),
        };
        let on_a = scope(host("a"), async { Token::mint() }).await;
        assert!(on_a.as_str().starts_with("a."));
        let unpinned = Token::mint();
        let both = |t: &Token| headers_with(Some(t.as_str()), Some(t.as_str()));
        let (a, b) = (both(&on_a), both(&on_a));
        assert!(scope(host("a"), async move { double_submitted(&a, None) }).await);
        assert!(!scope(host("b"), async move { double_submitted(&b, None) }).await);
        assert!(!double_submitted(&both(&on_a), None));
        let u = both(&unpinned);
        assert!(!scope(host("a"), async move { double_submitted(&u, None) }).await);
        assert!(double_submitted(&both(&unpinned), None));
    }

    fn person(subject: &str) -> Session {
        let now = crate::auth::session::now_unix();
        Session {
            identity: crate::auth::session::Identity {
                client: None,
                subject: subject.into(),
                username: subject.into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: now + 3600,
            issued_at: now,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: "opaque-id-token-for-tests".into(),
            access_expires_at: now + 3600,
            refresh_token: Some("opaque-refresh-token-for-tests".into()),
        }
    }

    /// The `Cookie` line a browser sends back for the session cookies `session` sets.
    fn session_cookie(config: &crate::config::Config, session: &Session) -> String {
        use axum::response::IntoResponse;
        let jar = axum_extra::extract::cookie::PrivateCookieJar::new(config.cookie_key.clone());
        let jar = crate::auth::session::store(jar, session).expect("store");
        let response = (jar, axum::http::StatusCode::OK).into_response();
        response
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_owned))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// RTE-12, SEC-10 (S1-52): a token is bound to its session. Session A's token, planted as
    /// session B's cookie and echoed, is refused on B; so is a random double-submitted pair once
    /// a session is present; B's own token passes, also when a rotation retired its key.
    #[tokio::test]
    async fn a_token_of_one_session_is_refused_on_another() {
        use tower::ServiceExt;
        let mut config = crate::config::Config::for_tests();
        let (a, b) = (person("f:1:alice"), person("f:1:bob"));
        let state = AppState::new(config.clone(), None);
        for session in [&a, &b] {
            state.keep_session_claims(session);
            state.keep_access_token(session, "opaque-access-token-for-tests".into());
        }
        let app = crate::server::app(state.clone());
        let post = |cookies: String, token: String| {
            let request = axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(axum::http::header::COOKIE, cookies)
                .header(CSRF_HEADER, token)
                .body(axum::body::Body::empty())
                .expect("request");
            app.clone().oneshot(request)
        };
        let own = |session: &Session| Token::bound(&config.cookie_key, session).0;
        let on_b = session_cookie(&config, &b);
        let planted = |token: &str| format!("{on_b}; {CSRF_COOKIE}={token}");
        let refused = post(planted(&own(&a)), own(&a)).await.expect("answer");
        assert_eq!(refused.status(), axum::http::StatusCode::FORBIDDEN);
        let random = new_token();
        let refused = post(planted(&random), random.clone())
            .await
            .expect("answer");
        assert_eq!(refused.status(), axum::http::StatusCode::FORBIDDEN);
        let passed = post(planted(&own(&b)), own(&b)).await.expect("answer");
        assert_ne!(passed.status(), axum::http::StatusCode::FORBIDDEN);
        // Without a session the cookie and the header only have to agree.
        let anonymous = post(format!("{CSRF_COOKIE}={random}"), random.clone())
            .await
            .expect("answer");
        assert_ne!(anonymous.status(), axum::http::StatusCode::FORBIDDEN);
        // A token minted under a key a rotation still lets in.
        let retired = Token::bound(&config.cookie_key, &b);
        config.cookie_keys_previous = vec![config.cookie_key.clone()];
        config.cookie_key = Key::generate();
        let bound = Bound {
            session: &b,
            config: &config,
        };
        assert!(bound.accepts(retired.as_str()));
        assert!(!bound.accepts(Token::bound(&config.cookie_key, &a).as_str()));
        assert!(needs_token(&CookieJar::new(), Some(bound)));
    }

    /// The session's token on an Organization's host carries that host's prefix and is taken
    /// there alone; the jar `issue` writes holds the token it returns, so it needs no new one.
    #[tokio::test]
    async fn an_issued_token_is_the_sessions_own_and_its_hosts() {
        use crate::organization_host::{scope, OrganizationHost};
        let config = crate::config::Config::for_tests();
        let alice = person("f:1:alice");
        let host = OrganizationHost {
            organization: "a".to_owned(),
            base: "https://portal.a.example".parse().expect("url"),
        };
        let (config_a, alice_a) = (config.clone(), alice.clone());
        let (on_a, taken_on_a) = scope(host, async move {
            let bound = Bound { session: &alice_a, config: &config_a };
            let token = bound.token();
            let taken = bound.accepts(token.as_str());
            (token, taken)
        })
        .await;
        assert!(on_a.as_str().starts_with("a."), "prefixed by its host");
        assert!(taken_on_a);
        let bound = Bound { session: &alice, config: &config };
        assert!(!bound.accepts(on_a.as_str()), "refused off its host");
        assert!(!bound.accepts(""));

        let (jar, issued) = issue(CookieJar::new(), Some(bound));
        assert_eq!(issued, bound.token().as_str());
        assert_eq!(jar.get(CSRF_COOKIE).map(Cookie::value), Some(issued.as_str()));
        assert!(!needs_token(&jar, Some(bound)));
        // Another session's cookie is no token of this one.
        let bob = person("f:1:bob");
        let (bobs, _) = issue(CookieJar::new(), Some(Bound { session: &bob, config: &config }));
        assert!(needs_token(&bobs, Some(bound)));
        // Without a session any cookie will do, and no cookie needs one.
        let (anonymous, minted) = issue(CookieJar::new(), None);
        assert_eq!(anonymous.get(CSRF_COOKIE).map(Cookie::value), Some(minted.as_str()));
        assert_ne!(minted, issued);
        assert!(!needs_token(&anonymous, None));
        assert!(needs_token(&CookieJar::new(), None));
    }

    /// The removal drops the `__Host-` cookie at once (a browser ignores one without `Secure`),
    /// and an old App's copy has the attributes of the real one under its old name.
    #[test]
    fn the_removal_and_the_legacy_copy_keep_the_cookies_attributes() {
        let removal = removal();
        assert_eq!((removal.name(), removal.value()), (CSRF_COOKIE, ""));
        assert_eq!(removal.path(), Some("/"));
        assert_eq!(removal.secure(), Some(true));
        assert_eq!(removal.max_age(), Some(time::Duration::ZERO));
        let token = Token::mint();
        let legacy = legacy_app_cookie(&token);
        let current = cookie(&token);
        assert_eq!(legacy.name(), LEGACY_APP_CSRF_COOKIE);
        assert_eq!(legacy.value(), token.as_str());
        assert_eq!(
            (legacy.path(), legacy.http_only(), legacy.secure(), legacy.same_site(), legacy.max_age()),
            (current.path(), current.http_only(), current.secure(), current.same_site(), current.max_age())
        );
        assert_eq!(current.max_age(), Some(time::Duration::hours(12)));
    }

    /// A header that is not text, a header with no cookie beside it and a session's bound check
    /// with no header at all are each refused.
    #[test]
    fn a_header_alone_or_unreadable_is_no_double_submit() {
        let token = new_token();
        assert!(!double_submitted(&headers_with(None, Some(&token)), None));
        let mut opaque = headers_with(Some(&token), None);
        opaque.insert(
            CSRF_HEADER,
            axum::http::HeaderValue::from_bytes(b"\xfftoken").expect("bytes"),
        );
        assert!(!double_submitted(&opaque, None));
        let config = crate::config::Config::for_tests();
        let alice = person("f:1:alice");
        let bound = Bound { session: &alice, config: &config };
        let own = bound.token();
        assert!(!double_submitted(&headers_with(Some(own.as_str()), None), Some(bound)));
        assert!(double_submitted(&headers_with(None, Some(own.as_str())), Some(bound)));
        assert!(!is_allowed(&Method::PUT, &headers_with(None, Some(&token)), Some(bound)));
        assert!(is_allowed(&Method::OPTIONS, &HeaderMap::new(), Some(bound)));
    }

    /// The `/api/v1` middleware: safe methods and a bearer pass untouched, a mutation without
    /// its token is the problem+json 403, with the anonymous pair it runs.
    #[tokio::test]
    async fn the_middleware_refuses_a_mutation_without_its_token() {
        use tower::ServiceExt;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let app = axum::Router::new()
            .route(
                "/x",
                axum::routing::get(|| async { "read" }).post(|| async { "written" }),
            )
            .layer(axum::middleware::from_fn_with_state(state, require_csrf));
        let call = |method: &str, headers: HeaderMap| {
            let mut request = axum::http::Request::builder()
                .method(method)
                .uri("/x")
                .body(axum::body::Body::empty())
                .expect("request");
            *request.headers_mut() = headers;
            app.clone().oneshot(request)
        };
        let token = new_token();
        let mut bearer = HeaderMap::new();
        bearer.insert(axum::http::header::AUTHORIZATION, "Bearer eyJ.x.y".parse().expect("h"));
        for (method, headers, status) in [
            ("GET", HeaderMap::new(), 200),
            ("HEAD", HeaderMap::new(), 200),
            ("POST", bearer, 200),
            ("POST", headers_with(Some(&token), Some(&token)), 200),
            ("POST", HeaderMap::new(), 403),
            ("POST", headers_with(Some(&token), Some("forged")), 403),
        ] {
            let answer = call(method, headers).await.expect("answer");
            assert_eq!(answer.status().as_u16(), status, "{method}");
            if status == 403 {
                assert_eq!(
                    answer.headers().get(axum::http::header::CONTENT_TYPE).map(|v| v.as_bytes()),
                    Some(&b"application/problem+json"[..])
                );
            }
        }
    }

    #[test]
    fn safe_methods_need_no_token() {
        assert!(is_allowed(&Method::GET, &headers_with(None, None), None));
        assert!(is_allowed(&Method::HEAD, &headers_with(None, None), None));
    }

    #[test]
    fn mutation_without_token_is_refused() {
        assert!(!is_allowed(&Method::POST, &headers_with(None, None), None));
        assert!(!is_allowed(
            &Method::DELETE,
            &headers_with(Some("abc"), None),
            None
        ));
        assert!(!is_allowed(
            &Method::PATCH,
            &headers_with(None, Some("abc")),
            None
        ));
    }

    #[test]
    fn mutation_with_mismatched_token_is_refused() {
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abd")),
            None
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abcd")),
            None
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some(""), Some("")),
            None
        ));
    }

    #[test]
    fn mutation_with_a_bearer_and_no_cookie_passes() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer eyJ.x.y".parse().expect("authorization header"),
        );
        assert!(is_allowed(&Method::POST, &headers, None));
    }

    #[test]
    fn mutation_with_matching_token_passes() {
        let token = new_token();
        assert!(is_allowed(
            &Method::POST,
            &headers_with(Some(&token), Some(&token)),
            None
        ));
    }

    #[test]
    fn cookie_is_readable_by_the_ui_but_still_secure() {
        let c = cookie(&Token::mint());
        assert_eq!(c.http_only(), Some(false));
        assert_eq!(c.secure(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
    }
}
