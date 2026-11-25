//! The session cookie a browser presents (`auth::session::load_from`) over an arbitrary `Cookie`
//! line, beside `Authorization` and the edge's `X-Access-Token` (`Front::of`): never panics, a
//! line nobody sealed with the Portal's key opens no session, and the front follows the headers.
#![no_main]
use std::sync::OnceLock;

use axum::http::{header, HeaderMap, HeaderValue};
use joinedcontext_portal::auth::session::{load_from, Front, EDGE_TOKEN_HEADER};
use joinedcontext_portal::config::Config;
use libfuzzer_sys::fuzz_target;

fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(Config::for_tests)
}

fuzz_target!(|data: &[u8]| {
    // A request's headers are capped far below this by the edge and hyper.
    if data.len() > 16 * 1024 {
        return;
    }
    // The first byte picks the other headers; the rest is the cookie line.
    let Some((flags, cookie)) = data.split_first() else {
        return;
    };
    let mut headers = HeaderMap::new();
    let Ok(value) = HeaderValue::from_bytes(cookie) else {
        return;
    };
    headers.insert(header::COOKIE, value);
    if flags & 1 != 0 {
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
    }
    if flags & 2 != 0 {
        headers.insert(EDGE_TOKEN_HEADER, HeaderValue::from_static("x"));
    }
    let trust = flags & 4 != 0;
    assert!(load_from(&headers, config()).is_none(), "{cookie:?}");
    let expected = if flags & 1 != 0 {
        Front::Bearer
    } else if flags & 2 != 0 && trust {
        Front::Edge
    } else {
        Front::Portal
    };
    assert_eq!(Front::of(&headers, trust), expected);
});
