//! The texts of an open data portal's own pages (T-7030, T-7031): the connector, privacy,
//! terms, support and accessibility pages, in Markdown.
//!
//! Each page has a built-in text in Slovak and English (any other language reads the English
//! one); an Organization replaces it per page and language in `spec.appearance.pages`. Both go
//! through [`render`], which is the only way a page's Markdown becomes HTML: raw HTML is shown as
//! text, an image shows its alternative text, a link is relative, `http(s)` or `mailto` (any
//! other scheme, `javascript:` and `data:` included, keeps its words and loses the link), and a
//! link to another host carries `rel="noopener nofollow"`. The Portal's preview and the open
//! data portal render with this one function, so a preview is what the portal shows.

use pulldown_cmark::{html, CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

/// The pages an open data portal serves at `/{page}`, in the order its menus list them.
pub const PAGES: [&str; 5] = ["connector", "privacy", "terms", "support", "accessibility"];

/// The largest Markdown one page in one language holds, in bytes.
pub const MAX_PAGE_MARKDOWN: usize = 16 * 1024;

/// A page as HTML: its title, the first `# heading` of the Markdown, and the rest of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    /// The text of the first level-one heading, `None` when the Markdown has none.
    pub title: Option<String>,
    /// The body without that heading; every other level-one heading is a level-two one.
    pub html: String,
}

/// The built-in Markdown of `page` in `lang`: Slovak for `sk`, English for any other language;
/// `None` for a page that is not one of [`PAGES`].
pub fn default_markdown(page: &str, lang: &str) -> Option<&'static str> {
    let sk = lang == "sk";
    Some(match (page, sk) {
        ("connector", true) => CONNECTOR_SK,
        ("connector", false) => CONNECTOR_EN,
        ("privacy", true) => PRIVACY_SK,
        ("privacy", false) => PRIVACY_EN,
        ("terms", true) => TERMS_SK,
        ("terms", false) => TERMS_EN,
        ("support", true) => SUPPORT_SK,
        ("support", false) => SUPPORT_EN,
        ("accessibility", true) => ACCESSIBILITY_SK,
        ("accessibility", false) => ACCESSIBILITY_EN,
        _ => return None,
    })
}

/// `markdown` as safe HTML, each `{name}` of `vars` in its text replaced by the value (the value
/// is text, never Markdown or HTML).
pub fn render(markdown: &str, vars: &[(&str, &str)]) -> RenderedPage {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut title: Option<String> = None;
    // 0: before the first level-one heading, 1: inside it, 2: after it.
    let mut heading = 0u8;
    let mut events: Vec<Event<'_>> = Vec::new();
    // Per open link: whether its start was written (an allowed link) and how.
    let mut links: Vec<Option<bool>> = Vec::new();
    for event in Parser::new_ext(markdown, options) {
        let event = match event {
            Event::Text(text) => Event::Text(filled(text, vars)),
            Event::Code(code) => Event::Code(filled(code, vars)),
            // Raw HTML is shown as the characters it is.
            Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
            // An image shows only its alternative text, which follows as text events.
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => continue,
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let dest_url: CowStr<'_> = if link_type == LinkType::Email {
                    format!("mailto:{dest_url}").into()
                } else {
                    dest_url
                };
                match target(&dest_url) {
                    Target::Refused => {
                        links.push(None);
                        continue;
                    }
                    Target::Here => {
                        links.push(Some(false));
                        Event::InlineHtml(format!("<a href=\"{}\">", attribute(&dest_url)).into())
                    }
                    Target::Elsewhere => {
                        links.push(Some(true));
                        Event::InlineHtml(
                            format!(
                                "<a href=\"{}\" rel=\"noopener nofollow\">",
                                attribute(&dest_url)
                            )
                            .into(),
                        )
                    }
                }
            }
            Event::End(TagEnd::Link) => match links.pop().flatten() {
                Some(_) => Event::InlineHtml("</a>".into()),
                None => continue,
            },
            Event::Start(Tag::Heading { level, .. }) => {
                if level == HeadingLevel::H1 && heading == 0 {
                    heading = 1;
                    title = Some(String::new());
                    continue;
                }
                Event::Start(Tag::Heading {
                    level: level.max(HeadingLevel::H2),
                    id: None,
                    classes: Vec::new(),
                    attrs: Vec::new(),
                })
            }
            Event::End(TagEnd::Heading(level)) => {
                if heading == 1 {
                    heading = 2;
                    continue;
                }
                Event::End(TagEnd::Heading(level.max(HeadingLevel::H2)))
            }
            other => other,
        };
        if heading == 1 {
            if let (Some(title), Event::Text(text) | Event::Code(text)) = (&mut title, &event) {
                title.push_str(text);
            }
            continue;
        }
        events.push(event);
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    RenderedPage {
        title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        html: out,
    }
}

fn filled<'a>(text: CowStr<'a>, vars: &[(&str, &str)]) -> CowStr<'a> {
    if !text.contains('{') {
        return text;
    }
    let mut out = text.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out.into()
}

enum Target {
    /// A relative address or `mailto:`.
    Here,
    /// An `http(s)` address or one starting `//`.
    Elsewhere,
    /// Any other scheme.
    Refused,
}

/// Where a link goes. A scheme is everything before the first `:` that comes before any `/`,
/// `?` or `#`, so `java\tscript:` (which a browser reads as `javascript:`) is a scheme too.
fn target(dest: &str) -> Target {
    let dest = dest.trim();
    if dest.starts_with("//") {
        return Target::Elsewhere;
    }
    let end = dest.find(['/', '?', '#']).unwrap_or(dest.len());
    match dest[..end].split_once(':') {
        None => Target::Here,
        Some((scheme, _)) => match scheme.to_ascii_lowercase().as_str() {
            "http" | "https" => Target::Elsewhere,
            "mailto" => Target::Here,
            _ => Target::Refused,
        },
    }
}

/// `value` inside a double-quoted HTML attribute.
fn attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.trim().chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

const CONNECTOR_EN: &str = "# MCP connector

Ask an AI assistant about this catalogue's open data: air quality now, hospitals in the region, a city's contracts. The connector is an MCP server at the address above: add it to your assistant as a custom connector. It needs no account.

## What it can do

- `list_endpoints`: finds the datasets.
- `describe_access`: says what a call may read.
- `list_types`: lists the kinds of records a dataset holds.
- `get_type`: describes one kind of record.
- `describe_schema`: reads a dataset's data model.
- `query_entities`: reads the current records, by place, time or value.
- `get_entity`: reads one record by its id.
- `query_temporal`: reads the history of many records.
- `retrieve_temporal`: reads the history of one record.

## Read only

Every tool here reads. The open data Endpoints refuse every write, for every caller, signed in or not.
";

const CONNECTOR_SK: &str = "# MCP konektor

Pýtajte sa AI asistenta na otvorené dáta tohto katalógu: aktuálnu kvalitu ovzdušia, nemocnice v kraji, zmluvy mesta. Konektor je MCP server na adrese uvedenej vyššie: pridajte ho do asistenta ako vlastný konektor. Netreba účet.

## Čo dokáže

- `list_endpoints`: nájde datasety.
- `describe_access`: povie, čo smie volanie čítať.
- `list_types`: vypíše druhy záznamov v datasete.
- `get_type`: opíše jeden druh záznamu.
- `describe_schema`: prečíta dátový model datasetu.
- `query_entities`: číta aktuálne záznamy podľa miesta, času alebo hodnoty.
- `get_entity`: prečíta jeden záznam podľa jeho id.
- `query_temporal`: číta históriu viacerých záznamov.
- `retrieve_temporal`: číta históriu jedného záznamu.

## Iba čítanie

Každý nástroj tu iba číta. Endpointy otvorených dát odmietnu každý zápis, každému volajúcemu, prihlásenému aj neprihlásenému.
";

const PRIVACY_EN: &str = "# Privacy

This page covers the MCP connector at {mcp_url}.

## No account, no cookie

The connector answers without an account. It sets no cookie and keeps no profile of you.

## The data

The connector serves only datasets their publishers released as open data.
";

const PRIVACY_SK: &str = "# Ochrana súkromia

Táto stránka sa týka MCP konektora na adrese {mcp_url}.

## Bez účtu, bez cookies

Konektor odpovedá bez účtu. Nenastavuje žiadne cookie a nevedie o vás žiadny profil.

## Dáta

Konektor poskytuje iba datasety, ktoré ich vydavatelia zverejnili ako otvorené dáta.
";

const TERMS_EN: &str = "# Terms of use

## Licences

Each dataset names its licence on its catalogue page, and that licence governs its reuse.

## Read only

The connector never changes data: the open data Endpoints refuse every write, for every caller.

## As is

{domain} runs as a development service.
";

const TERMS_SK: &str = "# Podmienky používania

## Licencie

Každý dataset uvádza svoju licenciu na svojej stránke v katalógu a tá určuje, ako ho smiete použiť.

## Iba čítanie

Konektor dáta nikdy nemení: Endpointy otvorených dát odmietnu každý zápis, každému volajúcemu.

## Bez záruky

{domain} beží ako vývojová služba.
";

const SUPPORT_EN: &str = "# Support

Write to us when the connector fails or a number looks wrong. Name the dataset or the tool, the question you asked and the time.

A wrong number in a dataset: tell its publisher too, named on the dataset page.
";

const SUPPORT_SK: &str = "# Podpora

Napíšte nám, keď konektor zlyhá alebo číslo nesedí. Uveďte dataset alebo nástroj, otázku, ktorú ste položili, a čas.

Chybné číslo v datasete: oznámte ho aj jeho vydavateľovi, uvedenému na stránke datasetu.
";

const ACCESSIBILITY_EN: &str = "# Accessibility statement

We want everyone to be able to use this catalogue, including people who work only with a keyboard, a screen reader or magnification.

## Legal basis

Accessibility is governed by Act No. 95/2019 Coll. and Decree No. 78/2020 Coll., which take WCAG 2.1 level AA (EN 301 549) as the standard.

## Compliance status

This catalogue is partially compliant with WCAG 2.1 level AA.
";

const ACCESSIBILITY_SK: &str = "# Vyhlásenie o prístupnosti

Chceme, aby tento katalóg mohol používať každý, aj človek, ktorý pracuje iba s klávesnicou, s čítačkou obrazovky alebo so zväčšením.

## Právny rámec

Prístupnosť upravuje zákon č. 95/2019 Z. z. o informačných technológiách vo verejnej správe a vyhláška č. 78/2020 Z. z., ktorá za štandard berie WCAG 2.1 na úrovni AA (EN 301 549).

## Stav súladu

Katalóg je čiastočne v súlade s WCAG 2.1 na úrovni AA.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_titled_default_in_slovak_and_english() {
        for page in PAGES {
            for lang in ["sk", "en", "de"] {
                let md = default_markdown(page, lang).expect(page);
                let shown = render(md, &[("mcp_url", "https://api.x/api/mcp"), ("domain", "x")]);
                assert!(shown.title.is_some(), "{page} {lang}");
                assert!(!shown.html.contains("{mcp_url}") && !shown.html.contains("{domain}"));
            }
        }
        assert_eq!(default_markdown("admin", "en"), None);
        assert_ne!(
            default_markdown("terms", "sk"),
            default_markdown("terms", "en")
        );
    }

    #[test]
    fn the_connector_lists_its_tools_in_code_and_says_writes_are_refused_for_everyone() {
        let en = render(default_markdown("connector", "en").unwrap(), &[]);
        assert_eq!(en.title.as_deref(), Some("MCP connector"));
        assert!(en
            .html
            .contains("<li><code>list_endpoints</code>: finds the datasets.</li>"));
        assert!(en.html.contains("refuse every write, for every caller"));
        assert!(!en.html.contains("without a sign-in"));
        let sk = render(default_markdown("connector", "sk").unwrap(), &[]);
        assert!(sk.html.contains("odmietnu každý zápis, každému volajúcemu"));
    }

    #[test]
    fn raw_html_and_scripts_show_as_text() {
        let shown = render(
            "<script>alert(1)</script>\n\nhi <img src=x onerror=alert(1)> **b**",
            &[],
        );
        assert!(!shown.html.contains("<script"), "{}", shown.html);
        assert!(!shown.html.contains("<img"), "{}", shown.html);
        assert!(shown.html.contains("&lt;script&gt;"), "{}", shown.html);
        assert!(shown.html.contains("<strong>b</strong>"));
    }

    #[test]
    fn links_keep_safe_schemes_and_mark_other_hosts() {
        let shown = render(
            "[a](javascript:alert(1)) [b](JaVa\tScRiPt:x) [c](data:text/html,x) [d](/dataset/) \
             [e](https://other.example/?q=\"x\") [f](mailto:a@b.sk) <c@d.sk> [g](//evil.example) \
             ![pic](https://other.example/p.png)",
            &[],
        );
        let html = &shown.html;
        assert!(!html.to_lowercase().contains("href=\"java"), "{html}");
        assert!(matches!(target(" java\tscript:x"), Target::Refused));
        assert!(matches!(target("VBScript:x"), Target::Refused));
        assert!(!html.contains("data:"), "{html}");
        assert!(html.contains("c <a href=\"/dataset/\">d</a>"), "{html}");
        assert!(
            html.contains("<a href=\"https://other.example/?q=&quot;x&quot;\" rel=\"noopener nofollow\">e</a>")
                || html.contains("<a href=\"https://other.example/?q=%22x%22\" rel=\"noopener nofollow\">e</a>"),
            "{html}"
        );
        assert!(html.contains("<a href=\"mailto:a@b.sk\">f</a>"), "{html}");
        assert!(
            html.contains("<a href=\"mailto:c@d.sk\">c@d.sk</a>"),
            "{html}"
        );
        assert!(html.contains("<a href=\"//evil.example\" rel=\"noopener nofollow\">g</a>"));
        assert!(!html.contains("<img") && html.contains("pic"), "{html}");
    }

    #[test]
    fn the_first_level_one_heading_is_the_title_and_later_ones_step_down() {
        let shown = render(
            "# One `two`\n\ntext\n\n# Again\n\n| a | b |\n|---|---|\n| 1 | 2 |",
            &[],
        );
        assert_eq!(shown.title.as_deref(), Some("One two"));
        assert!(!shown.html.contains("<h1"));
        assert!(shown.html.contains("<h2>Again</h2>"));
        assert!(shown.html.contains("<table>"));
        assert_eq!(render("no heading", &[]).title, None);
    }

    #[test]
    fn a_value_is_text_never_markup() {
        let shown = render("at {mcp_url}", &[("mcp_url", "<b>x</b> [l](javascript:1)")]);
        assert!(
            shown
                .html
                .contains("&lt;b&gt;x&lt;/b&gt; [l](javascript:1)"),
            "{}",
            shown.html
        );
    }
}
