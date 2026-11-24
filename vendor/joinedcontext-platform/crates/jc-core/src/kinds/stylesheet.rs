//! `kind: Portal` `spec.appearance.css`: one open data portal's own stylesheet (T-7745).
//!
//! The text is tokenized with cssparser and written out again rule by rule, so what a page links
//! is what this module read, never the author's bytes. A stylesheet holds style rules and
//! `@media` alone; every selector is prefixed with the portal's body class, every declaration
//! names a property of [`ALLOWED_PROPERTIES`] and its value calls only [`ALLOWED_FUNCTIONS`]. No
//! construct fetches from another host (`url()` names only `/files/{sha256}/{name}` on the
//! portal's own host), runs script, generates text, covers the page or hides what the page must
//! show (the licence and publisher lines): `display: none`, `position`, offsets, sizes, grid
//! placement, `z-index`, `opacity`, `visibility`, `transform`, outlines, shadows over 8px,
//! `!important` and negative margins are refused.
//!
//! The Portal checks the stylesheet at propose (422 naming the line and the construct) and jc
//! checks it again when it hands the portal's branding to the open data portal.

use cssparser::{
    Delimiter, ParseError, ParseErrorKind, Parser, ParserInput, ToCss, Token,
    TokenSerializationType,
};

use crate::error::{Error, Result};

/// The largest stylesheet a Portal carries, in bytes.
pub const MAX_PORTAL_CSS: usize = 32 * 1024;

/// The manifest path every refusal names.
pub const CSS_FIELD: &str = "spec.appearance.css";

/// The class [`scoped_stylesheet`] scopes to when only the check matters (propose).
pub const CHECK_CLASS: &str = "jc-portal";

/// The properties a declaration may set: colours, backgrounds, borders, type, spacing and the
/// direction and alignment of flex or grid layout. Nothing that positions, sizes, places grid
/// items, layers, hides, transforms, outlines or generates content: each of those can lay one
/// part of the page over another, and the licence and publisher lines must stay readable.
pub const ALLOWED_PROPERTIES: &[&str] = &[
    "align-content",
    "align-items",
    "align-self",
    "background",
    "background-color",
    "background-image",
    "background-position",
    "background-repeat",
    "background-size",
    "border",
    "border-bottom",
    "border-bottom-color",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-bottom-style",
    "border-bottom-width",
    "border-collapse",
    "border-color",
    "border-left",
    "border-left-color",
    "border-left-style",
    "border-left-width",
    "border-radius",
    "border-right",
    "border-right-color",
    "border-right-style",
    "border-right-width",
    "border-spacing",
    "border-style",
    "border-top",
    "border-top-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-top-style",
    "border-top-width",
    "border-width",
    "box-shadow",
    "color",
    "column-gap",
    "cursor",
    "display",
    "flex-direction",
    "flex-wrap",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-variant",
    "font-weight",
    "gap",
    "justify-content",
    "justify-items",
    "justify-self",
    "letter-spacing",
    "line-height",
    "list-style",
    "list-style-position",
    "list-style-type",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "outline-color",
    "outline-style",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "row-gap",
    "text-align",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-style",
    "text-shadow",
    "text-transform",
    "text-underline-offset",
    "white-space",
    "word-spacing",
];

/// The functions a value may call, besides `url()` (checked on its own).
pub const ALLOWED_FUNCTIONS: &[&str] = &[
    "calc",
    "clamp",
    "color-mix",
    "conic-gradient",
    "fit-content",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "linear-gradient",
    "max",
    "min",
    "minmax",
    "oklab",
    "oklch",
    "radial-gradient",
    "repeat",
    "repeating-conic-gradient",
    "repeating-linear-gradient",
    "repeating-radial-gradient",
    "rgb",
    "rgba",
    "var",
];

/// The functional pseudo-classes a selector may use; `:has()` is refused (a portal's rules
/// would then follow what the page contains, the attribute probes' cousin).
const SELECTOR_FUNCTIONS: &[&str] = &[
    "dir",
    "is",
    "lang",
    "not",
    "nth-child",
    "nth-last-child",
    "nth-last-of-type",
    "nth-of-type",
    "where",
];

/// What the walk refuses: the 1-based line and a sentence naming the construct.
#[derive(Debug)]
struct Refusal(u32, String);

type Refused<'i> = ParseError<'i, Refusal>;

/// The text written so far and the kind of its last token: two tokens that would read back as
/// one (`url` and `(`, which the author kept apart with a comment) get `/**/` between them.
struct Css {
    text: String,
    last: TokenSerializationType,
}

impl Css {
    fn new() -> Self {
        Self {
            text: String::new(),
            last: TokenSerializationType::Nothing,
        }
    }

    fn token(&mut self, t: &Token<'_>) {
        // Whitespace, a newline or a run of it, is one space: the walk keeps no layout.
        if matches!(t, Token::WhiteSpace(_)) {
            if !self.text.ends_with(' ') {
                self.text.push(' ');
            }
            self.last = Token::WhiteSpace(" ").serialization_type();
            return;
        }
        let kind = t.serialization_type();
        if self.last.needs_separator_when_before(kind) {
            self.text.push_str("/**/");
        }
        // Writing to a String does not fail.
        let _ = t.to_css(&mut self.text);
        self.last = kind;
    }
}

/// `css` checked and rewritten with every selector under `body.{class}`; `Err` names the line
/// and the construct, under [`CSS_FIELD`]. `class` is lower-case letters, digits and dashes.
pub fn scoped_stylesheet(css: &str, class: &str) -> Result<String> {
    if css.len() > MAX_PORTAL_CSS {
        return Err(refused(0, "a stylesheet of at most 32 KiB"));
    }
    if class.is_empty()
        || !class
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(refused(
            0,
            "scoped to a class of letters, digits and dashes",
        ));
    }
    if let Some(at) = css.find("</") {
        return Err(refused(
            line_at(css, at),
            "free of `</`, which would end the page's markup",
        ));
    }
    // An escape respells a name (`\75 rl(` is `url(`) and writes back respelled again, so the text
    // a browser reads could differ from the text checked: a portal stylesheet holds none.
    if let Some(at) = css.find('\\') {
        return Err(refused(
            line_at(css, at),
            "free of backslash escapes; write each name and character as it is",
        ));
    }
    if css.contains('\0') {
        return Err(refused(
            line_at(css, css.find('\0').unwrap_or(0)),
            "free of NUL characters",
        ));
    }
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut out = String::new();
    rules(&mut parser, &mut out, class, false).map_err(|e| match e.kind {
        ParseErrorKind::Custom(Refusal(line, what)) => refused(line, &what),
        ParseErrorKind::Basic(_) => refused(e.location.line + 1, "CSS that parses"),
    })?;
    if out.contains('\\') {
        // A name only an escape can write back (a control character in it): refused like one.
        return Err(refused(
            0,
            "free of characters a name can only hold escaped, such as control characters",
        ));
    }
    if out.contains("</") {
        return Err(refused(0, "free of `</`, also as an escape in a string"));
    }
    Ok(out)
}

/// The class a portal's body carries and its stylesheet is scoped to: `jc-portal-` and 12 hex
/// digits of the portal's catalogue key, stable for the portal and distinct from every other.
pub fn portal_class(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("jc-portal-{hex}")
}

/// Whether `url` names a file of the portal's own store on its own host:
/// `/files/{sha256}/{name}`, the name letters, digits, `.`, `_` and `-`.
pub fn is_portal_file_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("/files/") else {
        return false;
    };
    let Some((sha, name)) = rest.split_once('/') else {
        return false;
    };
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn refused(line: u32, reason: &str) -> Error {
    let reason = if line == 0 {
        format!("must be {reason}")
    } else {
        format!("must be {reason} (line {line})")
    };
    Error::Invalid {
        field: CSS_FIELD.to_owned(),
        reason,
    }
}

fn line_at(css: &str, at: usize) -> u32 {
    u32::try_from(css[..at].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

fn line(p: &Parser<'_, '_>) -> u32 {
    p.current_source_location().line + 1
}

fn refuse<'i>(p: &Parser<'i, '_>, line: u32, what: impl Into<String>) -> Refused<'i> {
    p.new_custom_error(Refusal(line, what.into()))
}

/// Style rules and, outside a `@media` block, `@media` rules.
fn rules<'i>(
    p: &mut Parser<'i, '_>,
    out: &mut String,
    class: &str,
    in_media: bool,
) -> std::result::Result<(), Refused<'i>> {
    loop {
        let at = line(p);
        let Ok(token) = p.next().cloned() else {
            return Ok(());
        };
        match token {
            Token::AtKeyword(name) if !in_media && name.eq_ignore_ascii_case("media") => {
                let mut prelude = Css::new();
                loop {
                    let Ok(t) = p.next_including_whitespace().cloned() else {
                        return Err(refuse(p, at, "a `@media` rule with a block"));
                    };
                    if t == Token::CurlyBracketBlock {
                        break;
                    }
                    media(p, &t, at, &mut prelude)?;
                }
                if prelude.text.trim().is_empty() {
                    return Err(refuse(p, at, "a `@media` rule with a query"));
                }
                out.push_str("@media ");
                out.push_str(prelude.text.trim());
                out.push_str(" {\n");
                p.parse_nested_block(|p| rules(p, out, class, true))?;
                out.push_str("}\n");
            }
            Token::AtKeyword(name) => {
                return Err(refuse(
                    p,
                    at,
                    format!(
                        "style rules and `@media` alone; `@{name}` is refused (no imports, fonts, \
                         namespaces or nested `@media`)"
                    ),
                ));
            }
            Token::CurlyBracketBlock => {
                return Err(refuse(p, at, "a rule with a selector before its block"));
            }
            Token::CDO | Token::CDC => {
                return Err(refuse(p, at, "free of `<!--` and `-->`"));
            }
            first => {
                let mut parts = vec![Css::new()];
                let mut next = Some(first);
                loop {
                    let t = match next.take() {
                        Some(t) => t,
                        None => match p.next_including_whitespace() {
                            Ok(t) => t.clone(),
                            Err(_) => return Err(refuse(p, at, "a rule with a block")),
                        },
                    };
                    match t {
                        Token::CurlyBracketBlock => break,
                        Token::Comma => parts.push(Css::new()),
                        t => {
                            let last = parts.len() - 1;
                            selector(p, &t, at, &mut parts[last])?;
                        }
                    }
                }
                let mut scoped = Vec::with_capacity(parts.len());
                for part in &parts {
                    let part = part.text.trim();
                    if part.is_empty() {
                        return Err(refuse(p, at, "a selector list without an empty member"));
                    }
                    scoped.push(format!("body.{class} {part}"));
                }
                if in_media {
                    out.push_str("  ");
                }
                out.push_str(&scoped.join(", "));
                out.push_str(" {\n");
                p.parse_nested_block(|p| declarations(p, out, at, in_media))?;
                if in_media {
                    out.push_str("  ");
                }
                out.push_str("}\n");
            }
        }
    }
}

/// One selector token, written to `out`; nested blocks are walked token by token.
fn selector<'i>(
    p: &mut Parser<'i, '_>,
    t: &Token<'i>,
    at: u32,
    out: &mut Css,
) -> std::result::Result<(), Refused<'i>> {
    match t {
        Token::Function(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "has" {
                return Err(refuse(p, at, "a selector without `:has()`"));
            }
            if !SELECTOR_FUNCTIONS.contains(&lower.as_str()) {
                return Err(refuse(
                    p,
                    at,
                    format!("a selector without `{name}()`, which a portal stylesheet may not use"),
                ));
            }
            out.token(&Token::Function(lower.clone().into()));
            p.parse_nested_block(|p| {
                while let Ok(t) = p.next_including_whitespace().cloned() {
                    if t == Token::Comma {
                        out.token(&t);
                    } else {
                        selector(p, &t, at, out)?;
                    }
                }
                Ok(())
            })?;
            out.token(&Token::CloseParenthesis);
        }
        Token::SquareBracketBlock => {
            out.token(t);
            p.parse_nested_block(|p| {
                while let Ok(t) = p.next_including_whitespace().cloned() {
                    match t {
                        Token::PrefixMatch
                        | Token::SuffixMatch
                        | Token::SubstringMatch
                        | Token::IncludeMatch => {
                            return Err(refuse(
                                p,
                                at,
                                format!(
                                    "a selector without the attribute match `{}`, which probes \
                                     attribute values",
                                    t.to_css_string()
                                ),
                            ));
                        }
                        Token::Ident(_)
                        | Token::QuotedString(_)
                        | Token::WhiteSpace(_)
                        | Token::Delim('=')
                        | Token::DashMatch => out.token(&t),
                        _ => {
                            return Err(refuse(
                                p,
                                at,
                                "an attribute selector `[name]` or `[name=value]`",
                            ))
                        }
                    }
                }
                Ok(())
            })?;
            out.token(&Token::CloseSquareBracket);
        }
        Token::Number { .. } | Token::Dimension { .. } if !plain_number(t) => {
            return Err(refuse(p, at, "numbers from 0.0001 up to 1000000"));
        }
        Token::Ident(_)
        | Token::IDHash(_)
        | Token::Hash(_)
        | Token::Colon
        | Token::WhiteSpace(_)
        | Token::Number { .. }
        | Token::Dimension { .. }
        | Token::Delim('.' | '*' | '>' | '+' | '~' | '-') => out.token(t),
        _ => {
            return Err(refuse(
                p,
                at,
                format!("a selector without `{}`", t.to_css_string()),
            ));
        }
    }
    Ok(())
}

/// One token of a `@media` query: media types, features in parentheses, numbers and lengths.
fn media<'i>(
    p: &mut Parser<'i, '_>,
    t: &Token<'i>,
    at: u32,
    out: &mut Css,
) -> std::result::Result<(), Refused<'i>> {
    match t {
        Token::ParenthesisBlock => {
            out.token(t);
            p.parse_nested_block(|p| {
                while let Ok(t) = p.next_including_whitespace().cloned() {
                    media(p, &t, at, out)?;
                }
                Ok(())
            })?;
            out.token(&Token::CloseParenthesis);
        }
        Token::Number { .. } | Token::Dimension { .. } | Token::Percentage { .. }
            if !plain_number(t) =>
        {
            return Err(refuse(p, at, "numbers from 0.0001 up to 1000000"));
        }
        Token::Ident(_)
        | Token::WhiteSpace(_)
        | Token::Colon
        | Token::Comma
        | Token::Number { .. }
        | Token::Dimension { .. }
        | Token::Percentage { .. }
        | Token::Delim('/' | '<' | '>' | '=') => out.token(t),
        _ => {
            return Err(refuse(
                p,
                at,
                format!("a media query without `{}`", t.to_css_string()),
            ));
        }
    }
    Ok(())
}

/// The declarations of one rule's block, each `property: value;`.
fn declarations<'i>(
    p: &mut Parser<'i, '_>,
    out: &mut String,
    rule_at: u32,
    in_media: bool,
) -> std::result::Result<(), Refused<'i>> {
    loop {
        let at = line(p).max(rule_at);
        let Ok(token) = p.next().cloned() else {
            return Ok(());
        };
        let name = match token {
            Token::Semicolon => continue,
            Token::Ident(name) => name.to_ascii_lowercase(),
            t => {
                return Err(refuse(
                    p,
                    at,
                    format!(
                        "declarations `property: value;` alone in a rule's block, not `{}`",
                        t.to_css_string()
                    ),
                ));
            }
        };
        if name.starts_with("--") {
            return Err(refuse(
                p,
                at,
                format!("free of custom properties such as `{name}`; read jc's with `var()`"),
            ));
        }
        if !ALLOWED_PROPERTIES.contains(&name.as_str()) {
            return Err(refuse(
                p,
                at,
                format!("free of `{name}`, a property a portal stylesheet may not set"),
            ));
        }
        if p.expect_colon().is_err() {
            return Err(refuse(
                p,
                at,
                format!("`{name}` followed by `:` and its value"),
            ));
        }
        let mut value = Css::new();
        p.parse_until_after(Delimiter::Semicolon, |p| {
            while let Ok(t) = p.next_including_whitespace().cloned() {
                value_token(p, &t, &name, at, &mut value)?;
            }
            Ok(())
        })?;
        let value = value.text.trim();
        if value.is_empty() {
            return Err(refuse(p, at, format!("`{name}` with a value")));
        }
        out.push_str(if in_media { "    " } else { "  " });
        out.push_str(&name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str(";\n");
    }
}

/// One token of `property`'s value, written to `out`.
fn value_token<'i>(
    p: &mut Parser<'i, '_>,
    t: &Token<'i>,
    property: &str,
    at: u32,
    out: &mut Css,
) -> std::result::Result<(), Refused<'i>> {
    match t {
        Token::Function(name) if name.eq_ignore_ascii_case("url") => {
            let url = p.parse_nested_block(|p| {
                let url = match p.next() {
                    Ok(Token::QuotedString(s)) => s.to_string(),
                    _ => return Err(refuse(p, at, "`url()` with one quoted address")),
                };
                if p.is_exhausted() {
                    Ok(url)
                } else {
                    Err(refuse(
                        p,
                        at,
                        "`url()` with one quoted address and nothing else",
                    ))
                }
            })?;
            portal_url(p, &url, at, out)?;
        }
        Token::UnquotedUrl(url) => portal_url(p, url, at, out)?,
        Token::Function(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "expression" {
                return Err(refuse(p, at, "free of `expression()`, which runs script"));
            }
            if !ALLOWED_FUNCTIONS.contains(&lower.as_str()) {
                return Err(refuse(
                    p,
                    at,
                    format!("free of `{name}()`, a function a portal stylesheet may not call"),
                ));
            }
            out.token(&Token::Function(lower.clone().into()));
            p.parse_nested_block(|p| {
                while let Ok(t) = p.next_including_whitespace().cloned() {
                    value_token(p, &t, property, at, out)?;
                }
                Ok(())
            })?;
            out.token(&Token::CloseParenthesis);
        }
        Token::ParenthesisBlock | Token::SquareBracketBlock => {
            let close = if *t == Token::ParenthesisBlock {
                Token::CloseParenthesis
            } else {
                Token::CloseSquareBracket
            };
            out.token(t);
            p.parse_nested_block(|p| {
                while let Ok(t) = p.next_including_whitespace().cloned() {
                    value_token(p, &t, property, at, out)?;
                }
                Ok(())
            })?;
            out.token(&close);
        }
        Token::Delim('!') => {
            return Err(refuse(
                p,
                at,
                "free of `!important`; a portal's rules already apply over the theme's",
            ));
        }
        Token::Ident(word) => {
            let word = word.to_ascii_lowercase();
            if property == "display" && (word == "none" || word == "contents") {
                return Err(refuse(
                    p,
                    at,
                    format!(
                        "free of `display: {word}`; hide a section with the portal's display \
                         settings"
                    ),
                ));
            }
            out.token(t);
        }
        Token::Number { value, .. }
        | Token::Dimension { value, .. }
        | Token::Percentage {
            unit_value: value, ..
        } => {
            if !plain_number(t) {
                return Err(refuse(p, at, "numbers from 0.0001 up to 1000000"));
            }
            if property.ends_with("-shadow") && !small_shadow(t) {
                return Err(refuse(
                    p,
                    at,
                    "a shadow of at most 8px or 0.5em, which never covers its neighbours",
                ));
            }
            if is_spacing(property) && !small_spacing(property, t) {
                return Err(refuse(
                    p,
                    at,
                    format!(
                        "a `{property}` of at most 64px, 4em or 4rem (or a small percentage), \
                         which never pushes a line out of reach"
                    ),
                ));
            }
            if property.starts_with("margin") && *value < 0.0 {
                return Err(refuse(
                    p,
                    at,
                    "free of negative margins, which pull one part of the page over another",
                ));
            }
            out.token(t);
        }
        Token::QuotedString(_) => out.token(t),
        Token::Hash(_)
        | Token::IDHash(_)
        | Token::Comma
        | Token::WhiteSpace(_)
        | Token::Delim('/' | '+' | '-' | '*' | '.' | '%') => out.token(t),
        _ => {
            return Err(refuse(
                p,
                at,
                format!("a value of `{property}` without `{}`", t.to_css_string()),
            ));
        }
    }
    Ok(())
}

/// Whether a number writes back as itself: 0, or between 0.0001 and 1000000 either side of 0.
/// Larger or smaller ones come back in exponent form with fewer digits, and a unit that reads as
/// an exponent (`e2`) comes back as part of the number.
fn plain_number(t: &Token<'_>) -> bool {
    let value = match t {
        // `3300` with the unit `e2` writes back as `3300e2`, which reads as one number.
        Token::Dimension { unit, .. }
            if unit.len() > 1
                && unit.starts_with(['e', 'E'])
                && unit[1..].starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') =>
        {
            return false;
        }
        Token::Number { value, .. } | Token::Dimension { value, .. } => *value,
        Token::Percentage { unit_value, .. } => *unit_value * 100.0,
        _ => return true,
    };
    value == 0.0 || (1e-4..=1e6).contains(&value.abs())
}

/// The properties whose lengths are capped: spacing, borders and text size, any of which pushes
/// a line out of reach or out of the page when large.
fn is_spacing(property: &str) -> bool {
    property.starts_with("margin")
        || property.starts_with("padding")
        || property.ends_with("gap")
        || matches!(
            property,
            "border-spacing"
                | "font"
                | "font-size"
                | "line-height"
                | "letter-spacing"
                | "word-spacing"
        )
        || (property.starts_with("border")
            && !property.contains("radius")
            && !property.ends_with("-color")
            && !property.ends_with("-style")
            && property != "border-collapse")
}

/// Whether one number of a spacing property stays within 64px, 4em or 4rem, 10 % (400 % for
/// the text size), or is unitless (a line height, a weight, a zero).
fn small_spacing(property: &str, t: &Token<'_>) -> bool {
    match t {
        Token::Number { .. } => true,
        Token::Percentage { unit_value, .. } => {
            let limit = if property.starts_with("font") || property == "line-height" {
                4.0
            } else {
                0.1
            };
            unit_value.abs() <= limit
        }
        Token::Dimension { value, unit, .. } => match unit.to_ascii_lowercase().as_str() {
            "px" => value.abs() <= 64.0,
            "em" | "rem" => value.abs() <= 4.0,
            _ => false,
        },
        _ => true,
    }
}

/// Whether a shadow's length stays within 8px or 0.5em (a unitless 0 included).
fn small_shadow(t: &Token<'_>) -> bool {
    match t {
        Token::Number { value, .. } => *value == 0.0,
        Token::Dimension { value, unit, .. } => {
            let limit = match unit.to_ascii_lowercase().as_str() {
                "px" => 8.0,
                "em" | "rem" => 0.5,
                _ => return false,
            };
            value.abs() <= limit
        }
        _ => false,
    }
}

/// `url` written as `url("…")` when it names a file of the portal's own store.
fn portal_url<'i>(
    p: &Parser<'i, '_>,
    url: &str,
    at: u32,
    out: &mut Css,
) -> std::result::Result<(), Refused<'i>> {
    if !is_portal_file_url(url) {
        return Err(refuse(
            p,
            at,
            "`url()` of a file of this portal or its Organization, \
             `/files/{sha256}/{name}` as the Files tab lists it",
        ));
    }
    out.token(&Token::Function("url".into()));
    out.token(&Token::QuotedString(url.into()));
    out.token(&Token::CloseParenthesis);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn reason(css: &str) -> String {
        match scoped_stylesheet(css, "jc-portal-x") {
            Err(Error::Invalid { field, reason }) => {
                assert_eq!(field, CSS_FIELD);
                reason
            }
            other => panic!("{css:?} was not refused: {other:?}"),
        }
    }

    #[test]
    fn a_stylesheet_is_scoped_rule_by_rule() {
        let css = format!(
            "/* header */\n.site-header, nav a:hover {{ background: #0b5394; color: rgb(255 255 255) }}\n\
             @media (max-width: 600px) {{ h1 {{ font-size: 1.5rem; }} }}\n\
             .hero {{ background-image: url(\"/files/{SHA}/hero.png\") }}\n\
             a[href] {{ text-decoration-line: underline }}"
        );
        let out = scoped_stylesheet(&css, "jc-portal-x").unwrap();
        assert_eq!(
            out,
            format!(
                "body.jc-portal-x .site-header, body.jc-portal-x nav a:hover {{\n  background: #0b5394;\n  color: rgb(255 255 255);\n}}\n\
                 @media (max-width: 600px) {{\n  body.jc-portal-x h1 {{\n    font-size: 1.5rem;\n  }}\n}}\n\
                 body.jc-portal-x .hero {{\n  background-image: url(\"/files/{SHA}/hero.png\");\n}}\n\
                 body.jc-portal-x a[href] {{\n  text-decoration-line: underline;\n}}\n"
            )
        );
        // What it writes reads back the same: the check is a fixed point.
        let again =
            scoped_stylesheet(&out.replace("body.jc-portal-x ", ""), "jc-portal-x").unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn each_refused_construct_is_named_with_its_line() {
        for (css, needle) in [
            ("@import \"/x.css\";", "`@import` is refused"),
            ("@font-face { font-family: x }", "`@font-face` is refused"),
            ("@namespace svg url(x);", "`@namespace` is refused"),
            (
                "@media screen { @media print { a { color: red } } }",
                "`@media` is refused",
            ),
            ("a { content: \"x\" }", "free of `content`"),
            ("a { position: fixed }", "free of `position`"),
            ("a { position: sticky }", "free of `position`"),
            ("a { z-index: 10 }", "free of `z-index`"),
            ("a { behavior: url(x.htc) }", "free of `behavior`"),
            ("a { -moz-binding: url(x.xml) }", "free of `-moz-binding`"),
            ("a { color: expression(alert(1)) }", "`expression()`"),
            (
                "a { background: url(https://evil.example/x.png) }",
                "`url()` of a file",
            ),
            (
                "a { background: url(\"//evil.example/x.png\") }",
                "`url()` of a file",
            ),
            (
                "a { background: url(/files/abc/x.png) }",
                "`url()` of a file",
            ),
            (
                "a { background: url(\"/files/../x\") }",
                "`url()` of a file",
            ),
            ("a { background: image-set(\"x.png\" 1x) }", "`image-set()`"),
            ("a[href^=\"https\"] { color: red }", "`^=`"),
            ("a[href$=\".pdf\"] { color: red }", "`$=`"),
            ("a[href*=\"x\"] { color: red }", "`*=`"),
            ("a[class~=\"x\"] { color: red }", "`~=`"),
            ("main:has(.x) { color: red }", "`:has()`"),
            (
                "a { color: red }\n</style><script>",
                "`</`, which would end the page's markup (line 2)",
            ),
            ("a { color: red !important }", "`!important`"),
            ("a { display: none }", "`display: none`"),
            ("a { margin-top: -40px }", "negative margins"),
            (".x { margin-left: 3000px }", "at most 64px"),
            (
                ".x { line-height: 3e38 }",
                "numbers from 0.0001 up to 1000000",
            ),
            (".x { padding: 0 0 0 50% }", "at most 64px"),
            (".x { gap: 10vw }", "at most 64px"),
            (".x { font-size: 300px }", "at most 64px"),
            (".x { border-left: 900px solid #000000 }", "at most 64px"),
            ("a { --jc-text: red }", "custom properties"),
            ("a { opacity: 0 }", "free of `opacity`"),
            ("a { visibility: hidden }", "free of `visibility`"),
            ("a { transform: scale(0) }", "free of `transform`"),
            ("a { top: 0 }", "free of `top`"),
            ("a { display: contents }", "`display: contents`"),
            ("a { width: 0 }", "free of `width`"),
            ("dt { grid-area: 1 / 1 }", "free of `grid-area`"),
            ("a { outline: 9999px solid #000000 }", "free of `outline`"),
            (
                "a { text-decoration-thickness: 2em }",
                "free of `text-decoration-thickness`",
            ),
            (
                "a { box-shadow: 0 0 0 9999px #000000 }",
                "a shadow of at most 8px",
            ),
            (
                "a { text-shadow: 0 0 3vw #000000 }",
                "a shadow of at most 8px",
            ),
            ("a { color: red; b { color: blue } }", "free of `b`"),
            ("<!-- a { color: red } -->", "`<!--`"),
            ("{ color: red }", "a selector"),
        ] {
            let got = reason(css);
            assert!(got.contains(needle), "{css:?}: {got}");
        }
        assert!(reason("\n\na { position: fixed }").contains("(line 3)"));
        assert!(reason(&"a{color:red}".repeat(3000)).contains("32 KiB"));
    }

    #[test]
    fn escapes_do_not_hide_a_refused_name() {
        for css in [
            "a { background: \\75 rl(https://evil.example/) }",
            "a { color: expr\\65 ssion(1) }",
            "a { \\7a -index: 1 }",
            "a { font-family: \"\\3c/style>\" }",
        ] {
            assert!(reason(css).contains("backslash escapes"), "{css}");
        }
    }

    #[test]
    fn an_organization_refuses_a_stylesheet_its_portals_carry() {
        let yaml = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  \
                    name: bbsk\n  namespace: org\nspec:\n  domain: bbsk.sk\n  \
                    gitRepositoryUrl: https://forge.bbsk.sk/o/config.git\n  locales: [sk]\n  \
                    defaultLocale: sk\n  appearance:\n    css: \".x { color: #ffffff }\"\n";
        let org = crate::kinds::Organization::from_yaml(yaml).expect("parses");
        match org.spec.validate() {
            Err(Error::Invalid { field, .. }) => assert_eq!(field, CSS_FIELD),
            other => panic!("not refused: {other:?}"),
        }
    }

    /// Found by the fuzz target: tokens the author kept apart with a comment stay apart, so
    /// `url` and a parenthesis never read back as a `url()` that fetches from another host.
    #[test]
    fn tokens_kept_apart_by_a_comment_stay_apart() {
        for css in [
            "a { background: url/**/(\"https://evil.example/x.png\") }",
            "a { color: expression/**/(1) }",
            "a { background: #0b0/**/.5em }",
        ] {
            let out = scoped_stylesheet(css, "jc-portal-x").expect(css);
            let lower = out.to_ascii_lowercase();
            assert!(
                !lower.contains("url(\"https") && !lower.contains("expression("),
                "{out}"
            );
            let again =
                scoped_stylesheet(&out.replace("body.jc-portal-x ", ""), "jc-portal-x").unwrap();
            assert_eq!(again, out, "{css}");
        }
        let out = scoped_stylesheet("a\n\n  +\tb { color:  #ffffff }", "jc-portal-x").unwrap();
        assert_eq!(out, "body.jc-portal-x a + b {\n  color: #ffffff;\n}\n");
    }

    /// The hostile stylesheets the open data portal's Playwright check draws over the licence and
    /// publisher lines (crates/portal/ui/e2e/portal_css_keep.spec.ts): each one is accepted, and
    /// the file holds what this module writes of it. `JC_WRITE_KEEP_CORPUS=1` writes it again.
    #[test]
    fn the_keep_corpus_is_accepted_and_current() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Entry {
            css: String,
            scoped: String,
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/kinds/stylesheet_keep.json"
        );
        let mut corpus: Vec<Entry> =
            serde_json::from_str(include_str!("stylesheet_keep.json")).expect("the corpus parses");
        let mut stale = Vec::new();
        for entry in &mut corpus {
            let scoped = scoped_stylesheet(&entry.css, "jc-portal-test").expect(&entry.css);
            if scoped != entry.scoped {
                stale.push(entry.css.clone());
                entry.scoped = scoped;
            }
        }
        if std::env::var_os("JC_WRITE_KEEP_CORPUS").is_some() {
            let text = serde_json::to_string_pretty(&corpus).expect("writes") + "\n";
            std::fs::write(path, text).expect("writes the corpus");
        } else {
            assert!(
                stale.is_empty(),
                "stale scoped text, rerun with JC_WRITE_KEEP_CORPUS=1: {stale:?}"
            );
        }
    }

    #[test]
    fn the_class_is_stable_and_checked() {
        assert_eq!(portal_class("a"), portal_class("a"));
        assert_ne!(portal_class("a"), portal_class("b"));
        assert!(portal_class("a").starts_with("jc-portal-"));
        assert!(scoped_stylesheet("a{color:red}", "x y").is_err());
    }
}
