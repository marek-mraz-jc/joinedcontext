// SPDX-License-Identifier: EUPL-1.2
//! One page of a feed as Atom (RFC 4287), RSS 2.0 or JSON Feed 1.1. Every text is text: XML
//! escaped (characters XML 1.0 excludes dropped), RSS `title` and `description` HTML-escaped once more,
//! JSON Feed `content_text` alone. No markup of an Entity reaches a reader.

use crate::output::Entry;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::fmt::Write as _;

/// A feed format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `atom.xml`.
    Atom,
    /// `rss.xml`.
    Rss,
    /// `feed.json`.
    Json,
}

impl Format {
    /// The format a file name names.
    #[must_use]
    pub fn of(file: &str) -> Option<Self> {
        match file {
            "atom.xml" => Some(Self::Atom),
            "rss.xml" => Some(Self::Rss),
            "feed.json" => Some(Self::Json),
            _ => None,
        }
    }

    /// Its media type.
    #[must_use]
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Atom => "application/atom+xml; charset=utf-8",
            Self::Rss => "application/rss+xml; charset=utf-8",
            Self::Json => "application/feed+json; charset=utf-8",
        }
    }
}

/// What a page says about its feed.
#[derive(Debug, Clone)]
pub struct Head {
    /// The feed's id, a tag URI.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its description.
    pub description: Option<String>,
    /// Its language.
    pub lang: Option<String>,
    /// The newest entry's `updated`.
    pub updated: DateTime<Utc>,
    /// This page's absolute URL.
    pub this: String,
    /// The first page's.
    pub first: String,
    /// The previous page's, if any.
    pub previous: Option<String>,
    /// The next page's, if any.
    pub next: Option<String>,
    /// The publisher: the broker's public host.
    pub author: String,
    /// The tag URI prefix of each entry: `tag:{host},2026:{view}/`.
    pub entry_ids: String,
}

/// XML 1.0 character data: `&`, `<`, `>`, `"` and `'` escaped, the characters XML 1.0 2.2
/// excludes dropped.
#[must_use]
pub fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c < ' ' || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Text an RSS reader renders as HTML: escaped as HTML, then as XML.
fn html_text(text: &str) -> String {
    xml(&xml(text))
}

/// The entry's id: a tag URI (RFC 4151), the same in every format.
fn entry_id(head: &Head, entry: &Entry) -> String {
    format!("{}{}", head.entry_ids, entry.entity)
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn rfc822(t: DateTime<Utc>) -> String {
    t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// The page in `format`.
#[must_use]
pub fn render(format: Format, head: &Head, entries: &[Entry]) -> String {
    match format {
        Format::Atom => atom(head, entries),
        Format::Rss => rss(head, entries),
        Format::Json => json_feed(head, entries),
    }
}

fn paging(out: &mut String, head: &Head, element: &str) {
    let _ = write!(
        out,
        "<{element} rel=\"first\" href=\"{}\"/>",
        xml(&head.first)
    );
    for (rel, href) in [("previous", &head.previous), ("next", &head.next)] {
        if let Some(href) = href {
            let _ = write!(out, "<{element} rel=\"{rel}\" href=\"{}\"/>", xml(href));
        }
    }
}

fn atom(head: &Head, entries: &[Entry]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\"",
    );
    if let Some(lang) = &head.lang {
        let _ = write!(out, " xml:lang=\"{}\"", xml(lang));
    }
    let _ = write!(
        out,
        "><id>{}</id><title type=\"text\">{}</title>",
        xml(&head.id),
        xml(&head.title)
    );
    if let Some(d) = &head.description {
        let _ = write!(out, "<subtitle type=\"text\">{}</subtitle>", xml(d));
    }
    let _ = write!(
        out,
        "<updated>{}</updated><author><name>{}</name></author><generator>Antares</generator><link rel=\"self\" type=\"application/atom+xml\" href=\"{}\"/>",
        rfc3339(head.updated),
        xml(&head.author),
        xml(&head.this)
    );
    paging(&mut out, head, "link");
    for e in entries {
        let _ = write!(
            out,
            "\n<entry><id>{}</id><title type=\"text\">{}</title><updated>{}</updated>",
            xml(&entry_id(head, e)),
            xml(&e.title),
            rfc3339(e.updated)
        );
        if let Some(p) = e.published {
            let _ = write!(out, "<published>{}</published>", rfc3339(p));
        }
        for a in &e.authors {
            let _ = write!(out, "<author><name>{}</name></author>", xml(a));
        }
        for c in &e.categories {
            let _ = write!(out, "<category term=\"{}\"/>", xml(c));
        }
        if let Some(l) = &e.link {
            let _ = write!(out, "<link rel=\"alternate\" href=\"{}\"/>", xml(l));
        }
        if let Some(i) = &e.image {
            let _ = write!(out, "<link rel=\"enclosure\" href=\"{}\"/>", xml(i));
        }
        if let Some(s) = &e.summary {
            let _ = write!(out, "<summary type=\"text\">{}</summary>", xml(s));
        }
        // RFC 4287 4.1.2: an entry without content has an alternate link
        match (&e.content, &e.link) {
            (Some(c), _) => {
                let _ = write!(out, "<content type=\"text\">{}</content>", xml(c));
            }
            (None, None) => {
                let text = e.summary.as_deref().unwrap_or(&e.title);
                let _ = write!(out, "<content type=\"text\">{}</content>", xml(text));
            }
            (None, Some(_)) => {}
        }
        out.push_str("</entry>");
    }
    out.push_str("\n</feed>\n");
    out
}

fn rss(head: &Head, entries: &[Entry]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><channel>");
    let _ = write!(
        out,
        "<title>{}</title><link>{}</link><description>{}</description>",
        xml(&head.title),
        xml(&head.this),
        html_text(head.description.as_deref().unwrap_or(&head.title))
    );
    if let Some(lang) = &head.lang {
        let _ = write!(out, "<language>{}</language>", xml(lang));
    }
    let _ = write!(
        out,
        "<lastBuildDate>{}</lastBuildDate><generator>Antares</generator><atom:link rel=\"self\" type=\"application/rss+xml\" href=\"{}\"/>",
        rfc822(head.updated),
        xml(&head.this)
    );
    paging(&mut out, head, "atom:link");
    for e in entries {
        let _ = write!(out, "\n<item><title>{}</title>", html_text(&e.title));
        if let Some(l) = &e.link {
            let _ = write!(out, "<link>{}</link>", xml(l));
        }
        if let Some(d) = e.summary.as_ref().or(e.content.as_ref()) {
            let _ = write!(out, "<description>{}</description>", html_text(d));
        }
        let _ = write!(
            out,
            "<guid isPermaLink=\"false\">{}</guid><pubDate>{}</pubDate>",
            xml(&entry_id(head, e)),
            rfc822(e.published.unwrap_or(e.updated))
        );
        for a in &e.authors {
            let _ = write!(out, "<dc:creator>{}</dc:creator>", xml(a));
        }
        for c in &e.categories {
            let _ = write!(out, "<category>{}</category>", xml(c));
        }
        out.push_str("</item>");
    }
    out.push_str("\n</channel></rss>\n");
    out
}

fn json_feed(head: &Head, entries: &[Entry]) -> String {
    let items: Vec<Value> = entries
        .iter()
        .map(|e| {
            let mut item = Map::new();
            item.insert("id".into(), json!(entry_id(head, e)));
            item.insert("title".into(), json!(e.title));
            let text = e
                .content
                .as_deref()
                .or(e.summary.as_deref())
                .unwrap_or(&e.title);
            item.insert("content_text".into(), json!(text));
            let optional = [
                ("url", e.link.clone()),
                ("summary", e.summary.clone()),
                ("image", e.image.clone()),
                ("date_published", e.published.map(rfc3339)),
            ];
            for (key, value) in optional {
                if let Some(v) = value {
                    item.insert(key.into(), json!(v));
                }
            }
            item.insert("date_modified".into(), json!(rfc3339(e.updated)));
            if !e.authors.is_empty() {
                let authors: Vec<Value> = e.authors.iter().map(|a| json!({ "name": a })).collect();
                item.insert("authors".into(), json!(authors));
            }
            if !e.categories.is_empty() {
                item.insert("tags".into(), json!(e.categories));
            }
            Value::Object(item)
        })
        .collect();
    let mut feed = json!({
        "version": "https://jsonfeed.org/version/1.1",
        "title": head.title,
        "feed_url": head.this,
        "authors": [{ "name": head.author }],
        "items": items,
    });
    let optional = [
        ("description", head.description.clone()),
        ("language", head.lang.clone()),
        ("next_url", head.next.clone()),
    ];
    for (key, value) in optional {
        if let (Some(v), Some(map)) = (value, feed.as_object_mut()) {
            map.insert(key.into(), json!(v));
        }
    }
    feed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escapes_markup_and_drops_what_xml_excludes() {
        assert_eq!(
            xml("a & <b> ]]> \"c\" 'd'\u{1}\u{8}\u{b}\u{fffe}\u{ffff}\te"),
            "a &amp; &lt;b&gt; ]]&gt; &quot;c&quot; &apos;d&apos;\te"
        );
        assert_eq!(html_text("<script>"), "&amp;lt;script&amp;gt;");
    }

    #[test]
    fn a_format_is_one_of_three_files() {
        assert_eq!(Format::of("atom.xml"), Some(Format::Atom));
        assert_eq!(Format::of("rss.xml"), Some(Format::Rss));
        assert_eq!(Format::of("feed.json"), Some(Format::Json));
        assert_eq!(Format::of("feed.xml"), None);
        assert!(Format::Json
            .media_type()
            .starts_with("application/feed+json"));
    }
}
