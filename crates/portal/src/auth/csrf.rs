//! Double-submit CSRF protection for every mutating Portal API call.
//!
//! `SameSite=Lax` already keeps the session cookie off cross-site POSTs; this is the second
//! lock: the UI reads the readable `__Host-jc_csrf` cookie and echoes it in `X-CSRF-Token`.

use axum::extract::Request;
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
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
    /// Fresh token, minted with the same CSPRNG the OIDC state uses.
    pub fn mint() -> Self {
        Self(CsrfToken::new_random().secret().clone())
    }

    /// The token as the page echoes it back in `X-CSRF-Token`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Fresh token as a string, for a caller that only hands it to the page.
pub fn new_token() -> String {
    Token::mint().0
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
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// True when the request may proceed: safe methods always, mutations only with a matching token.
pub fn is_allowed(method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    // A request that carries `Authorization` is authenticated by that header alone (the
    // session's `Front` rule: the bearer wins over the cookie), so no cookie is ever the
    // credential behind it and there is nothing for a cross-site form to ride on. Scripts,
    // the smoke, MCP clients and agents mutate this way (AG-59).
    headers.contains_key(axum::http::header::AUTHORIZATION) || double_submitted(headers)
}

/// True when the request echoes its `CSRF_COOKIE` in `X-CSRF-Token`, whatever else it carries.
/// The App doors use it alone: there the edge sets the person's token from the session cookie
/// even beside an `Authorization` header, so that header is no reason to skip the check (T-4915).
pub fn double_submitted(headers: &HeaderMap) -> bool {
    let jar = CookieJar::from_headers(headers);
    let Some(expected) = jar.get(CSRF_COOKIE).map(|c| c.value().to_string()) else {
        return false;
    };
    let Some(presented) = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    !expected.is_empty() && constant_time_eq(&expected, presented)
}

/// Middleware for the `/api/v1` router. Rejects with problem+json, never with a bare 403 body.
pub async fn require_csrf(
    request: Request,
    next: Next,
) -> Result<Response, crate::error::ApiError> {
    if is_allowed(request.method(), request.headers()) {
        Ok(next.run(request).await)
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

/// Adds a CSRF cookie to a jar, returning the token that was issued.
pub fn issue(jar: CookieJar) -> (CookieJar, String) {
    let token = Token::mint();
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
        assert!(is_allowed(&Method::POST, &headers));
        assert!(!double_submitted(&headers));
        let mut headers = headers_with(Some("abc"), Some("abc"));
        assert!(double_submitted(&headers));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer planted".parse().expect("bearer"),
        );
        assert!(double_submitted(&headers));
    }

    #[test]
    fn safe_methods_need_no_token() {
        assert!(is_allowed(&Method::GET, &headers_with(None, None)));
        assert!(is_allowed(&Method::HEAD, &headers_with(None, None)));
    }

    #[test]
    fn mutation_without_token_is_refused() {
        assert!(!is_allowed(&Method::POST, &headers_with(None, None)));
        assert!(!is_allowed(
            &Method::DELETE,
            &headers_with(Some("abc"), None)
        ));
        assert!(!is_allowed(
            &Method::PATCH,
            &headers_with(None, Some("abc"))
        ));
    }

    #[test]
    fn mutation_with_mismatched_token_is_refused() {
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abd"))
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abcd"))
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some(""), Some(""))
        ));
    }

    #[test]
    fn mutation_with_a_bearer_and_no_cookie_passes() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer eyJ.x.y".parse().expect("authorization header"),
        );
        assert!(is_allowed(&Method::POST, &headers));
    }

    #[test]
    fn mutation_with_matching_token_passes() {
        let token = new_token();
        assert!(is_allowed(
            &Method::POST,
            &headers_with(Some(&token), Some(&token))
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
