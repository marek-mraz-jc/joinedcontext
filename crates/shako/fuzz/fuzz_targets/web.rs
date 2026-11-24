//! Input: a selector byte, then text. 0: Markdown a repository holds, rendered for a page; 1: a
//! page path, mapped to its Portal page and asked without a credential; 2: the `redirect_to` of
//! the Gitea login path; 3: a page path asked as a reader. No answer may run script, leave the
//! origin or panic.
#![no_main]

mod reader;

use std::sync::LazyLock;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use libfuzzer_sys::fuzz_target;
use shako_web::{PortalBase, portal_page, render_safe_markdown, web_router};
use tower::ServiceExt;

static APP: LazyLock<Router> = LazyLock::new(|| {
    web_router()
        .layer(axum::Extension(PortalBase("https://portal.example".into())))
        .with_state(reader::state())
});

/// A local path: one leading `/`, never `//` or `\`, no control or space byte.
fn local(target: &str) -> bool {
    target.starts_with('/')
        && !target.starts_with("//")
        && !target.contains('\\')
        && !target
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
}

fn ask(uri: &str, token: Option<&str>) -> Option<(u16, String)> {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("token {t}"));
    }
    let req = req.body(Body::empty()).ok()?;
    reader::RT.block_on(async {
        let Ok(resp) = APP.clone().oneshot(req).await;
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let _ = axum::body::to_bytes(resp.into_body(), 1 << 24).await;
        assert!((200..600).contains(&status));
        Some((status, location))
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    match which % 4 {
        0 => {
            let html = render_safe_markdown(&text).to_ascii_lowercase();
            // Text escapes every `<`, so each `<` opens a tag the renderer wrote; quotes in text
            // stay as they are and are no attribute.
            for tag in html.split('<').skip(1) {
                let tag = tag.split('>').next().unwrap_or_default();
                for bad in ["script", "iframe", "svg", "object", "embed", "style"] {
                    assert!(!tag.starts_with(bad), "<{tag}> in {html}");
                }
                for bad in ["=\"javascript:", "=\"data:", "=\"vbscript:", "\" on"] {
                    assert!(!tag.contains(bad), "{bad} in <{tag}>");
                }
            }
        }
        1 => {
            let page = portal_page(&text);
            assert!(page == "/" || page.starts_with("/projects/"), "{page}");
            assert!(local(&page), "{page}");
            if let Some((_, location)) = ask(&format!("/{}", text.trim_start_matches('/')), None) {
                if !location.is_empty() {
                    let path = location
                        .strip_prefix("https://portal.example")
                        .expect("the Portal's origin");
                    assert!(local(path), "{location}");
                }
            }
        }
        2 => {
            let uri = format!("/user/oauth2/keycloak?redirect_to={text}");
            if let Some((_, location)) = ask(&uri, None) {
                assert!(location.is_empty() || local(&location), "{location}");
            }
        }
        _ => {
            let _ = ask(
                &format!("/{}", text.trim_start_matches('/')),
                Some(reader::READER),
            );
        }
    }
});
