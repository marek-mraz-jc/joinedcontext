//! A browser for the jc end-to-end tests: a cookie jar, redirects followed by hand, and the
//! Portal's sign-in through kaisatsu (code + PKCE).
// Each test binary uses its own part of it.
#![allow(dead_code)]

use reqwest::{Client, Method, StatusCode, header};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const REALM: &str = "jc";

/// A browser-like cookie jar: name -> (value, path). Secure cookies are kept on
/// `http://localhost`, as browsers do for localhost.
#[derive(Default)]
pub struct Jar(BTreeMap<String, (String, String)>);

impl Jar {
    fn store(&mut self, headers: &header::HeaderMap) {
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
                if lower == "max-age=0" {
                    expired = true;
                }
            }
            if expired || value.is_empty() {
                self.0.remove(name);
            } else {
                self.0.insert(name.to_owned(), (value.to_owned(), path));
            }
        }
    }

    fn header_for(&self, url: &str) -> String {
        let path = url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/");
        self.0
            .iter()
            .filter(|(_, (_, p))| path.starts_with(p.as_str()))
            .map(|(n, (v, _))| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|(v, _)| v.as_str())
    }
}

pub struct Browser {
    client: Client,
    jar: Jar,
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
        if csrf && let Some(token) = self.jar.get("__Host-jc_csrf") {
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
        self.jar.store(&headers);
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
pub async fn sign_in(base: &str, browser: &mut Browser, username: &str, password: &str) {
    let url = |path: &str| format!("{base}{path}");
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
