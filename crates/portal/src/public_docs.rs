//! The documentation (T-4063): the API chapter and the User Guide, served at
//! `/docs/{chapter}/{page}` to a signed-in person only (T-6200, API/00 §1): a visitor without a
//! session is sent to the login page, a script without a token gets 401. A public Endpoint's own
//! API page (`/d/...`, EP-99) stays open to anyone. The pages are the markdown of a pinned docs commit, bundled by
//! `scripts/public-docs.sh`; no other chapter is in the bundle, so none can be served.
//!
//! The markdown is rendered on the server with raw HTML shown as text and every link either
//! relative, `http(s)` or `mailto`, so a page carries no script whatever the source holds.
//!
//! A section that documents a broker surface built behind a Cargo feature is named in its page's
//! front matter (`surfaces: sta=7-ogc-sensorthings-api-sta-v11`, docs STYLE.md); the page leaves
//! it out, and every page leaves out its links to it, unless the platform broker's `/q/health`
//! lists that surface (T-4484, TS-19). A broker that does not answer confirms none.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use pulldown_cmark::{html, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rust_embed::RustEmbed;

use crate::auth::refresh::{accepts_html, login_redirect};
use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "public-docs"]
#[include = "API/*.md"]
#[include = "User-Guide/*.md"]
#[include = "COMMIT"]
struct Pages;

/// The chapters whose pages are public; the bundle holds no other.
const CHAPTERS: [&str; 2] = ["API", "User-Guide"];

pub fn router() -> Router<AppState> {
    Router::new().route("/docs/{chapter}/{page}", get(serve))
}

async fn serve(
    user: Result<CurrentUser, ApiError>,
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((chapter, page)): Path<(String, String)>,
) -> Response {
    if let Err(refusal) = user {
        if !accepts_html(&headers) {
            return refusal.into_response();
        }
        let back = uri.path_and_query().map_or(uri.path(), |pq| pq.as_str());
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Redirect::to(&login_redirect(back)),
        )
            .into_response();
    }
    let mounted = state.space_usage.surfaces().await.unwrap_or_default();
    match render(
        &chapter,
        page.strip_suffix(".md").unwrap_or(&page),
        &mounted,
    ) {
        Some(document) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                // signed-in only: no shared cache may hand the page to a visitor
                (header::CACHE_CONTROL, "private, max-age=300"),
            ],
            document,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "No such page in the public documentation\n",
        )
            .into_response(),
    }
}

/// The page as a whole HTML document, without the sections of the surfaces `mounted` does not
/// name; `None` for a chapter that is not public or a page that is not in the bundle.
pub fn render(chapter: &str, page: &str, mounted: &BTreeSet<String>) -> Option<String> {
    let named = !page.is_empty()
        && page
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !CHAPTERS.contains(&chapter) || !named {
        return None;
    }
    let file = Pages::get(&format!("{chapter}/{page}.md"))?;
    let text = std::str::from_utf8(&file.data).ok()?;
    let (title, body) = split_front_matter(text);
    let title = title.or_else(|| first_heading(body)).unwrap_or(page);
    let hidden: Vec<(&str, &str)> = GATES
        .iter()
        .filter(|gate| !mounted.contains(&gate.surface))
        .map(|gate| (gate.page.as_str(), gate.id.as_str()))
        .collect();
    let content = content(&format!("{chapter}/{page}"), body, &hidden);
    let commit = docs_commit();
    Some(format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n{content}</main>\n\
         <footer><p>joinedcontext platform documentation, {chapter}, docs {commit}, \
         Portal {release}</p></footer>\n</body>\n</html>\n",
        title = escape(title),
        commit = escape(short(&commit)),
        release = escape(&release()),
    ))
}

/// The markdown of page `here` (`{chapter}/{page}`) as HTML, without the sections `hidden`
/// names and without the links to any of them.
fn content(here: &str, body: &str, hidden: &[(&str, &str)]) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_HEADING_ATTRIBUTES;
    let events: Vec<Event> = Parser::new_ext(body, options).map(safe).collect();
    let mut out = String::new();
    html::push_html(
        &mut out,
        without(anchored(events), here, hidden).into_iter(),
    );
    out
}

/// A section a page names in its front matter as documenting a broker surface.
struct Gate {
    /// `{chapter}/{page}`.
    page: String,
    /// The surface's name as the broker's `/q/health` lists it.
    surface: String,
    /// The id of the section's heading.
    id: String,
}

/// Every gated section of the bundle, read once from the pages' `surfaces:`.
static GATES: LazyLock<Vec<Gate>> = LazyLock::new(|| {
    Pages::iter()
        .filter_map(|path| {
            let page = path.strip_suffix(".md")?.to_owned();
            let file = Pages::get(&path)?;
            let text = std::str::from_utf8(&file.data).ok()?;
            Some(
                surfaces_of(text)
                    .into_iter()
                    .map(|(surface, id)| Gate {
                        page: page.clone(),
                        surface,
                        id,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
});

/// The `name=heading-id` pairs of a page's `surfaces:` front matter.
fn surfaces_of(text: &str) -> Vec<(String, String)> {
    let Some(front) = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(front, _)| front)
    else {
        return Vec::new();
    };
    front
        .lines()
        .find_map(|line| line.strip_prefix("surfaces:"))
        .map(|pairs| {
            pairs
                .split_whitespace()
                .filter_map(|pair| pair.split_once('='))
                .map(|(name, id)| (name.to_owned(), id.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// The events of page `here` without each hidden section (its heading up to the next heading of
/// the same or a higher level) and with each link into a hidden section reduced to its text.
fn without<'a>(events: Vec<Event<'a>>, here: &str, hidden: &[(&str, &str)]) -> Vec<Event<'a>> {
    let is_hidden = |page: &str, id: &str| hidden.iter().any(|&(p, i)| p == page && i == id);
    let mut out = Vec::with_capacity(events.len());
    let mut skipping: Option<HeadingLevel> = None;
    let mut unlinked = false;
    for event in events {
        if let Event::Start(Tag::Heading { level, id, .. }) = &event {
            if skipping.is_some_and(|section| *level > section) {
                continue;
            }
            skipping = id
                .as_deref()
                .filter(|id| is_hidden(here, id))
                .map(|_| *level);
        }
        if skipping.is_some() {
            continue;
        }
        match &event {
            Event::Start(Tag::Link { dest_url, .. })
                if target(here, dest_url).is_some_and(|(page, id)| is_hidden(&page, id)) =>
            {
                unlinked = true;
                continue;
            }
            Event::End(TagEnd::Link) if unlinked => {
                unlinked = false;
                continue;
            }
            _ => {}
        }
        out.push(event);
    }
    out
}

/// The page (`{chapter}/{page}`) and heading id a link from page `here` points at, once `link`
/// has rewritten it; `None` for a link with no anchor or to another site.
fn target<'d>(here: &str, dest: &'d str) -> Option<(String, &'d str)> {
    let (path, id) = dest.split_once('#')?;
    if path.is_empty() {
        return Some((here.to_owned(), id));
    }
    if path.contains(':') {
        return None;
    }
    let (chapter, _) = here.split_once('/')?;
    let mut parts = path.rsplit('/');
    let page = parts.next()?;
    let chapter = match parts.next() {
        Some(named) if named != "." && named != ".." => named,
        _ => chapter,
    };
    Some((format!("{chapter}/{page}"), id))
}

/// The docs commit the bundle was made from (`public-docs/COMMIT`), which the deployment pins
/// beside the Portal's digest (T-4483).
pub fn docs_commit() -> String {
    Pages::get("COMMIT")
        .and_then(|file| String::from_utf8(file.data.into_owned()).ok())
        .map(|commit| commit.trim().to_owned())
        .unwrap_or_default()
}

/// The release these pages belong to: the Portal's version and, in an image, the commit it was
/// built from (the `JC_PORTAL_COMMIT` build argument, T-4483).
pub fn release() -> String {
    match option_env!("JC_PORTAL_COMMIT").map(str::trim) {
        Some(built) if !built.is_empty() => format!("{} {}", crate::APP_VERSION, short(built)),
        _ => crate::APP_VERSION.to_owned(),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// The CSP source that admits the pages' one stylesheet, its hash, so the Portal's policy needs
/// no `'unsafe-inline'` for them (T-5828).
pub fn style_source() -> String {
    crate::agents::kit::script_hash(STYLE)
}

const STYLE: &str = "body{font:16px/1.6 system-ui,sans-serif;max-width:52rem;margin:0 auto;\
padding:1rem;color:#1b1f24;background:#fff}pre,code{font-family:ui-monospace,monospace}\
pre{overflow-x:auto;padding:.75rem;background:#f4f5f7}table{border-collapse:collapse;\
display:block;overflow-x:auto}td,th{border:1px solid #ccd;padding:.25rem .5rem}\
footer{color:#5b636e;font-size:.875rem}@media (prefers-color-scheme:dark){body{color:#e6e8eb;\
background:#14171a}pre{background:#1e2226}a{color:#8ab4ff}footer{color:#a0a8b3}}";

/// The `title:` of a page's front matter and the markdown after it.
fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let Some(end) = rest.find("\n---\n") else {
        return (None, text);
    };
    let title = rest[..end]
        .lines()
        .find_map(|line| line.strip_prefix("title:"))
        .map(|title| title.trim().trim_matches(['"', '\'']))
        .filter(|title| !title.is_empty());
    (title, &rest[end + "\n---\n".len()..])
}

fn first_heading(body: &str) -> Option<&str> {
    body.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
}

/// Raw HTML becomes text and every link a safe one; nothing else changes.
fn safe(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: link(dest_url),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: link(dest_url),
            title,
            id,
        }),
        other => other,
    }
}

/// Every heading gets the id a link to it uses (T-4485): its own `{#id}`, else its text the way
/// the docs site and GitHub make it (lower case, punctuation dropped, spaces to `-`), so
/// `07-websocket#limits` and `09-errors#quota` land on their section. Classes and attributes a
/// page writes are dropped; an id is all a heading carries.
fn anchored(mut events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    for start in 0..events.len() {
        let Event::Start(Tag::Heading { level, id, .. }) = &events[start] else {
            continue;
        };
        let (level, id) = (*level, id.clone());
        let id = id.unwrap_or_else(|| {
            let text: String = events[start + 1..]
                .iter()
                .take_while(|event| !matches!(event, Event::End(TagEnd::Heading(_))))
                .filter_map(|event| match event {
                    Event::Text(text) | Event::Code(text) => Some(text.as_ref()),
                    _ => None,
                })
                .collect();
            CowStr::from(heading_id(&text))
        });
        events[start] = Event::Start(Tag::Heading {
            level,
            id: Some(id),
            classes: Vec::new(),
            attrs: Vec::new(),
        });
    }
    events
}

fn heading_id(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// `http(s)` and `mailto` stay, any other scheme becomes `#`; a relative link to another page
/// loses its `.md`, so `07-websocket.md#limits` opens `07-websocket#limits` beside it.
fn link(dest: CowStr<'_>) -> CowStr<'_> {
    let bare: String = dest
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
        .collect::<String>()
        .to_ascii_lowercase();
    if let Some((scheme, _)) = bare.split_once(':') {
        if !scheme.contains(['/', '#', '?']) {
            return if matches!(scheme, "http" | "https" | "mailto") {
                dest
            } else {
                CowStr::Borrowed("#")
            };
        }
    }
    let (path, anchor) = dest.split_at(dest.find('#').unwrap_or(dest.len()));
    match path
        .strip_suffix(".md")
        .map(|stem| format!("{stem}{anchor}"))
    {
        Some(rewritten) => CowStr::from(rewritten),
        None => dest,
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every surface any page names: nothing is left out.
    fn every() -> BTreeSet<String> {
        GATES.iter().map(|gate| gate.surface.clone()).collect()
    }

    fn rendered(markdown: &str) -> String {
        let mut out = String::new();
        html::push_html(
            &mut out,
            Parser::new_ext(markdown, Options::empty()).map(safe),
        );
        out
    }

    #[test]
    fn every_bundled_page_renders_with_a_title() {
        let mut pages = 0;
        for path in Pages::iter().filter(|path| path.ends_with(".md")) {
            let (chapter, file) = path.split_once('/').expect("chapter/page.md");
            let page = file.strip_suffix(".md").expect("a markdown page");
            let document =
                render(chapter, page, &every()).unwrap_or_else(|| panic!("{path} renders"));
            assert!(!document.contains("<title></title>"), "{path} has a title");
            pages += 1;
        }
        assert!(pages > 10, "the bundle holds the API and User Guide pages");
    }

    #[test]
    fn the_websocket_guide_is_titled_from_its_front_matter() {
        let document = render("API", "07-websocket", &every()).expect("the guide is public");
        assert!(document.contains("<title>Live Updates over WebSocket</title>"));
        assert!(
            !document.contains("sidebar_position"),
            "front matter is not shown"
        );
    }

    #[test]
    fn only_the_public_chapters_and_plain_names_are_served() {
        for (chapter, page) in [
            ("Operations", "00-intro"),
            ("Deployment", "00-intro"),
            ("Architecture", "09-portal"),
            ("api", "07-websocket"),
            ("API", ""),
            ("API", "..%2FOperations%2F00-intro"),
            ("API", "../Operations/00-intro"),
            ("API", "no-such-page"),
        ] {
            assert!(
                render(chapter, page, &every()).is_none(),
                "{chapter}/{page} is not served"
            );
        }
    }

    #[test]
    fn raw_html_is_text_and_links_are_safe() {
        let out = rendered(
            "<script>alert(1)</script>\n\n[a](javascript:alert(1)) [b](JaVa\tScript:x) \
             [c](07-websocket.md#limits) [d](https://example.org/x.md) [e](mailto:a@b.c) \
             [f](data:text/html,x) [g](#here) ![i](vbscript:x)",
        );
        assert!(!out.contains("<script"), "{out}");
        assert!(out.contains("&lt;script&gt;"), "{out}");
        assert!(!out.to_ascii_lowercase().contains("javascript:"), "{out}");
        assert!(
            !out.contains("data:") && !out.contains("vbscript:"),
            "{out}"
        );
        assert!(out.contains("href=\"07-websocket#limits\""), "{out}");
        assert!(out.contains("href=\"https://example.org/x.md\""), "{out}");
        assert!(out.contains("href=\"mailto:a@b.c\""), "{out}");
        assert!(out.contains("href=\"#here\""), "{out}");
    }

    #[test]
    fn front_matter_without_a_title_falls_back_to_the_heading() {
        assert_eq!(split_front_matter("---\nx: 1\n---\n# H\n").0, None);
        assert_eq!(
            first_heading("intro\n# The heading \n"),
            Some("The heading")
        );
        assert_eq!(
            split_front_matter("# no front matter\n"),
            (None, "# no front matter\n")
        );
        assert_eq!(
            split_front_matter("---\ntitle: 'Q'\n---\nbody").0,
            Some("Q")
        );
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn a_heading_carries_the_id_a_link_to_it_uses() {
        let page = |markdown: &str| {
            let mut out = String::new();
            let options = Options::ENABLE_HEADING_ATTRIBUTES;
            let events: Vec<Event> = Parser::new_ext(markdown, options).map(safe).collect();
            html::push_html(&mut out, anchored(events).into_iter());
            out
        };
        assert!(page("## 2. Limits & `ws` frames\n").contains(r#"<h2 id="2-limits--ws-frames">"#));
        assert!(page("### `quota` {#quota}\n").contains(r#"<h3 id="quota">"#));
        let crafted = page("### x {#a .evil onclick=alert(1)}\n");
        assert!(crafted.contains(r#"<h3 id="a">"#), "{crafted}");
        assert!(
            !crafted.contains("evil") && !crafted.contains("onclick"),
            "{crafted}"
        );
        let bundled = render("API", "07-websocket", &every()).expect("bundled");
        assert!(
            bundled.contains(" id=\""),
            "no heading id in a bundled page"
        );
    }

    #[test]
    fn every_gate_names_a_heading_of_its_page() {
        assert!(
            GATES.iter().any(|gate| gate.surface == "sta"),
            "API/02 names its SensorThings section"
        );
        for gate in GATES.iter() {
            let (chapter, page) = gate.page.split_once('/').expect("chapter/page");
            let document = render(chapter, page, &every()).expect("bundled");
            assert!(
                document.contains(&format!(" id=\"{}\"", gate.id)),
                "{} names {}={}, which is no heading there",
                gate.page,
                gate.surface,
                gate.id
            );
        }
    }

    #[test]
    fn a_hidden_section_goes_with_its_subsections_and_every_link_to_it() {
        let body = "## 1. Kept\n\nSee [sta](#2-sta) and [api](02-x.md#2-sta).\n\n\
                    ## 2. STA\n\nGone.\n\n### Sub\n\nGone too.\n\n## 3. After\n\n\
                    [other page](../User-Guide/02-x.md#2-sta) [kept](02-x.md#3-after)\n";
        let out = content("API/02-x", body, &[("API/02-x", "2-sta")]);
        assert!(!out.contains("Gone") && !out.contains("Sub"), "{out}");
        assert!(!out.contains("href=\"#2-sta\""), "{out}");
        assert!(!out.contains("href=\"02-x#2-sta\""), "{out}");
        assert!(
            out.contains("See sta and api."),
            "the link text stays: {out}"
        );
        assert!(
            out.contains("href=\"../User-Guide/02-x#2-sta\""),
            "another page's section: {out}"
        );
        assert!(out.contains("href=\"02-x#3-after\""), "{out}");
        assert!(
            out.contains("id=\"3-after\""),
            "the next section stays: {out}"
        );
        assert_eq!(content("API/02-x", body, &[]).matches("Gone").count(), 2);
    }

    #[test]
    fn a_link_resolves_to_its_page_and_heading() {
        assert_eq!(target("API/02-x", "#a"), Some(("API/02-x".to_owned(), "a")));
        assert_eq!(
            target("API/00-intro", "02-x#a"),
            Some(("API/02-x".to_owned(), "a"))
        );
        assert_eq!(
            target("API/00-intro", "./02-x#a"),
            Some(("API/02-x".to_owned(), "a"))
        );
        assert_eq!(
            target("User-Guide/01", "../API/02-x#a"),
            Some(("API/02-x".to_owned(), "a"))
        );
        assert_eq!(target("API/00", "02-x"), None);
        assert_eq!(target("API/00", "https://example.org/#a"), None);
        assert_eq!(
            surfaces_of("---\ntitle: T\nsurfaces: sta=7-a ogc=6-b junk\n---\n# T\n"),
            vec![
                ("sta".to_owned(), "7-a".to_owned()),
                ("ogc".to_owned(), "6-b".to_owned())
            ]
        );
        assert!(surfaces_of("# no front matter\n").is_empty());
    }
}
