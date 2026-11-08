//! A browser for the jc end-to-end tests: a cookie jar, redirects followed by hand, and the
//! Portal's sign-in through kaisatsu (code + PKCE).
// Each test binary uses its own part of it.
#![allow(dead_code)]

use reqwest::{Client, Method, StatusCode, header};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const REALM: &str = "jc";

/// A browser-like cookie jar: (host, name) -> (value, path), host-only cookies as jc sets them
/// (no `Domain`), so a cookie of one host never goes to another. Secure cookies are kept on
/// `http://localhost`, as browsers do for localhost.
#[derive(Default)]
pub struct Jar(BTreeMap<(String, String), (String, String)>);

/// The host (with port) and path of `url`.
fn host_and_path(url: &str) -> (String, String) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    match rest.find('/') {
        Some(i) => (rest[..i].to_owned(), rest[i..].to_owned()),
        None => (rest.to_owned(), "/".to_owned()),
    }
}

impl Jar {
    fn store(&mut self, url: &str, headers: &header::HeaderMap) {
        let (host, _) = host_and_path(url);
        for raw in headers.get_all(header::SET_COOKIE) {
            let raw = raw.to_str().unwrap_or_default();
            let mut parts = raw.split(';').map(str::trim);
            let Some((name, value)) = parts.next().and_then(|kv| kv.split_once('=')) else {
                continue;
            };
            let mut path = "/".to_owned();
            let mut expired = false;
            for attr in parts {
                let lower = attr.to_ascii_lowercase();
                if let Some(p) = lower.strip_prefix("path=") {
                    path = attr[5..5 + p.len()].to_owned();
                }
                assert!(
                    !lower.starts_with("domain="),
                    "jc sets host-only cookies: {raw}"
                );
                if lower == "max-age=0" {
                    expired = true;
                }
            }
            let key = (host.clone(), name.to_owned());
            if expired || value.is_empty() {
                self.0.remove(&key);
            } else {
                self.0.insert(key, (value.to_owned(), path));
            }
        }
    }

    fn header_for(&self, url: &str) -> String {
        let (host, path) = host_and_path(url);
        self.0
            .iter()
            .filter(|((h, _), (_, p))| *h == host && path.starts_with(p.as_str()))
            .map(|((_, n), (v, _))| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn get(&self, url: &str, name: &str) -> Option<&str> {
        let (host, _) = host_and_path(url);
        self.0
            .get(&(host, name.to_owned()))
            .map(|(v, _)| v.as_str())
    }

    /// The names of the cookies held for `url`'s host.
    pub fn names_for(&self, url: &str) -> Vec<String> {
        let (host, _) = host_and_path(url);
        self.0
            .keys()
            .filter(|(h, _)| *h == host)
            .map(|(_, n)| n.clone())
            .collect()
    }
}

pub struct Browser {
    client: Client,
    pub jar: Jar,
}

pub struct Answer {
    pub status: StatusCode,
    pub headers: header::HeaderMap,
    pub text: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    pub fn location(&self) -> String {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

impl Browser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
            jar: Jar::default(),
        }
    }

    pub async fn send(
        &mut self,
        method: Method,
        url: &str,
        body: Option<Body<'_>>,
        csrf: bool,
    ) -> Answer {
        let mut req = self.client.request(method, url);
        let cookies = self.jar.header_for(url);
        if !cookies.is_empty() {
            req = req.header(header::COOKIE, cookies);
        }
        if csrf && let Some(token) = self.jar.get(url, "__Host-jc_csrf") {
            req = req.header("x-csrf-token", token);
        }
        req = match body {
            Some(Body::Form(fields)) => req.form(fields),
            Some(Body::Json(value)) => req.json(value),
            None => req,
        };
        let resp = req.send().await.unwrap_or_else(|e| panic!("{url}: {e}"));
        let status = resp.status();
        let headers = resp.headers().clone();
        self.jar.store(url, &headers);
        Answer {
            status,
            headers,
            text: resp.text().await.unwrap_or_default(),
        }
    }

    pub async fn get(&mut self, url: &str) -> Answer {
        self.send(Method::GET, url, None, false).await
    }

    /// Follows redirects by hand, keeping cookies, until a non-redirect answer or `stop` matches
    /// the next location.
    pub async fn follow(
        &mut self,
        mut url: String,
        stop: impl Fn(&str) -> bool,
    ) -> (String, Answer) {
        for _ in 0..12 {
            let answer = self.get(&url).await;
            if !answer.status.is_redirection() {
                return (url, answer);
            }
            let next = answer.location();
            let next = if next.starts_with("http") {
                next
            } else {
                let origin = &url[..url[8..].find('/').map_or(url.len(), |i| i + 8)];
                format!("{origin}{next}")
            };
            if stop(&next) {
                return (next, answer);
            }
            url = next;
        }
        panic!("too many redirects from {url}");
    }
}

pub enum Body<'a> {
    Form(&'a [(&'a str, &'a str)]),
    Json(&'a Value),
}

fn hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    let at = html
        .find(&marker)
        .unwrap_or_else(|| panic!("no {name} in {html}"));
    let tag_start = html[..at].rfind('<').unwrap();
    let tag_end = at + html[at..].find('>').unwrap();
    let tag = &html[tag_start..tag_end];
    let v = tag.find("value=\"").unwrap() + 7;
    tag[v..v + tag[v..].find('"').unwrap()].replace("&amp;", "&")
}

/// Signs `username` in to the Portal through kaisatsu: the Portal's login redirect, kaisatsu's
/// authorization endpoint (code + PKCE), its login form, back to the Portal's callback.
/// `url` maps a path of the one-domain layout to the address jc serves it at.
pub async fn sign_in(
    url: &dyn Fn(&str) -> String,
    browser: &mut Browser,
    username: &str,
    password: &str,
) {
    let (login_url, page) = browser.follow(url("/api/v1/auth/login"), |_| false).await;
    assert_eq!(page.status, StatusCode::OK, "{login_url}: {}", page.text);
    assert!(
        login_url.starts_with(&url(&format!(
            "/id/realms/{REALM}/login-actions/authenticate"
        ))),
        "the Portal sends the browser to kaisatsu: {login_url}"
    );
    let csrf = hidden(&page.text, "csrf_token");
    let session = hidden(&page.text, "session_code");
    let fields = [
        ("csrf_token", csrf.as_str()),
        ("session_code", session.as_str()),
        ("username", username),
        ("password", password),
    ];
    let posted = browser
        .send(Method::POST, &login_url, Some(Body::Form(&fields)), false)
        .await;
    assert!(
        posted.status.is_redirection(),
        "kaisatsu accepts the password: {} {}",
        posted.status,
        posted.text
    );
    let callback = posted.location();
    assert!(
        callback.starts_with(&url("/api/v1/auth/callback?")),
        "{callback}"
    );
    assert!(callback.contains("code="), "{callback}");
    let (_, home) = browser.follow(callback, |_| false).await;
    assert!(home.status.is_success(), "{} {}", home.status, home.text);
}
