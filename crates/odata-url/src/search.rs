// SPDX-License-Identifier: EUPL-1.2
//! `$search` (URL Conventions 4.01 5.1.7, ABNF `search`): words, phrases, `NOT`, `AND` (also
//! written as whitespace) and `OR`, with parentheses. The ABNF reads the operators right to
//! left; 5.1.7 gives them precedence ("NOT ... has the highest precedence, followed by AND and
//! OR"), which the parse applies, so `a b OR c` is `(a AND b) OR c`.
//!
//! Words and phrases are held percent-decoded. A `+` stands for whitespace, as a form encoder
//! writes a space (T-3719); a word holding a plus writes it `%2B`. A `searchExpr-incomplete`
//! (`'...'`, with `''` for a quote) is read as one phrase.

use crate::cursor::{count, Cursor};
use crate::error::{LimitKind, UrlError};
use crate::literal::pct_encoded;
use crate::Limits;
use std::fmt::{self, Display, Formatter};

/// A parsed `$search` expression.
///
/// ```
/// use odata_url::Search;
/// let s = Search::Or(
///     Box::new(Search::Word("blue".into())),
///     Box::new(Search::Not(Box::new(Search::Phrase("dark red".into())))),
/// );
/// assert_eq!(s.to_string(), "blue%20OR%20NOT%20%22dark%20red%22");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// A `searchWord`, decoded; never `AND`, `OR` or `NOT`.
    Word(String),
    /// A `searchPhrase` without its quotes, decoded.
    Phrase(String),
    /// `NOT operand`.
    Not(Box<Search>),
    /// `left AND right`, or the two side by side.
    And(Box<Search>, Box<Search>),
    /// `left OR right`.
    Or(Box<Search>, Box<Search>),
}

/// The value of `$search` after its `=`, up to the end of the option; inside `$apply`'s
/// `search(...)` (`closed`), up to its `CLOSE`.
pub(crate) fn search(
    c: &mut Cursor<'_>,
    limits: &Limits,
    nodes: &mut usize,
    closed: bool,
) -> Result<Search, UrlError> {
    c.bws();
    let mut p = Parser {
        c,
        limits,
        nodes,
        depth: 0,
    };
    let found = if p.c.squote() {
        p.incomplete()?
    } else {
        p.or()?
    };
    p.c.bws();
    let end = if closed {
        p.at_close()
    } else {
        p.c.at_end() || p.c.peek() == Some(b'&')
    };
    if !end {
        return Err(p.c.syntax("searchExpr"));
    }
    Ok(found)
}

struct Parser<'c, 'a> {
    c: &'c mut Cursor<'a>,
    limits: &'c Limits,
    nodes: &'c mut usize,
    depth: usize,
}

impl Parser<'_, '_> {
    fn node(&mut self) -> Result<(), UrlError> {
        count(
            self.nodes,
            self.limits.max_expr_nodes,
            LimitKind::ExprNodes,
            self.c,
        )
    }

    /// One more level of `NOT` or parentheses, within `max_expr_depth`.
    fn deeper(&mut self) -> Result<(), UrlError> {
        count(
            &mut self.depth,
            self.limits.max_expr_depth,
            LimitKind::ExprDepth,
            self.c,
        )
    }

    /// `and *( RWS "OR" RWS and )`.
    fn or(&mut self) -> Result<Search, UrlError> {
        let mut left = self.and()?;
        while self.operator("OR") {
            self.node()?;
            let right = self.and()?;
            left = Search::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `unary *( RWS [ "AND" RWS ] unary )`; `OR` or the end of the group ends it.
    fn and(&mut self) -> Result<Search, UrlError> {
        let mut left = self.unary()?;
        loop {
            let before = self.c.pos();
            if !self.c.rws() {
                return Ok(left);
            }
            if self.c.at_end()
                || self.c.peek() == Some(b'&')
                || self.at_close()
                || self.keyword("OR")
            {
                self.c.reset(before);
                return Ok(left);
            }
            self.keyword_eat("AND");
            self.node()?;
            let right = self.unary()?;
            left = Search::And(Box::new(left), Box::new(right));
        }
    }

    /// `"NOT" RWS unary`, a group, a phrase or a word.
    fn unary(&mut self) -> Result<Search, UrlError> {
        self.node()?;
        if self.keyword_eat("NOT") {
            self.deeper()?;
            let operand = self.unary()?;
            self.depth = self.depth.saturating_sub(1);
            return Ok(Search::Not(Box::new(operand)));
        }
        if self.c.open() {
            self.deeper()?;
            self.c.bws();
            let inner = self.or()?;
            self.c.bws();
            if !self.c.close() {
                return Err(self.c.syntax("CLOSE"));
            }
            self.depth = self.depth.saturating_sub(1);
            return Ok(inner);
        }
        if self.quote() {
            return self.phrase();
        }
        self.word()
    }

    /// `RWS word RWS`, consumed when `word` stands there as an operator.
    fn operator(&mut self, word: &str) -> bool {
        let before = self.c.pos();
        if self.c.rws() && self.keyword_eat(word) {
            return true;
        }
        self.c.reset(before);
        false
    }

    /// Whether `word` (case-sensitive) and whitespace come next; nothing consumed.
    fn keyword(&mut self, word: &str) -> bool {
        let before = self.c.pos();
        let found = self.keyword_eat(word);
        self.c.reset(before);
        found
    }

    /// `word` (case-sensitive) and its `RWS`, consumed; the cursor unmoved otherwise.
    fn keyword_eat(&mut self, word: &str) -> bool {
        let before = self.c.pos();
        if self.c.eat(word) && self.c.rws() {
            return true;
        }
        self.c.reset(before);
        false
    }

    fn at_close(&mut self) -> bool {
        let before = self.c.pos();
        let found = self.c.close();
        self.c.reset(before);
        found
    }

    /// `quotation-mark`, consumed.
    fn quote(&mut self) -> bool {
        self.c.eat("\"") || self.c.eat_ci("%22")
    }

    /// The rest of a `searchPhrase` after its opening quote.
    fn phrase(&mut self) -> Result<Search, UrlError> {
        let start = self.c.pos();
        let mut bytes = Vec::new();
        loop {
            if self.quote() {
                break;
            }
            if self.c.eat("+") {
                bytes.push(b' ');
                continue;
            }
            match self.byte() {
                Some(b'"') | None => return Err(self.c.syntax("quotation-mark")),
                Some(b) => bytes.push(b),
            }
            self.length(start)?;
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Err(self.c.syntax("searchPhrase"));
        }
        Ok(Search::Phrase(self.text(bytes)?))
    }

    /// A `searchWord`: up to whitespace, a parenthesis, a quote, a literal `;` or the end of
    /// the option; `;`, `&` and `#` percent-encoded.
    fn word(&mut self) -> Result<Search, UrlError> {
        let start = self.c.pos();
        let mut bytes = Vec::new();
        loop {
            let at = self.c.pos();
            // looked at, not matched: a syntax error stands at the `;` (the ABNF runner's offset)
            if matches!(self.c.peek(), Some(b'+' | b';')) {
                break;
            }
            match self.byte() {
                Some(b) if !(b.is_ascii_whitespace() || b"()\"".contains(&b)) => {
                    bytes.push(b);
                }
                _ => {
                    self.c.reset(at);
                    break;
                }
            }
            self.length(start)?;
        }
        // a quote may follow the first character, never lead (`searchWord`)
        if bytes.is_empty() || bytes.first() == Some(&b'\'') {
            return Err(self.c.syntax("searchWord"));
        }
        // An operator standing where it cannot operate (`$search=OR`) is searched for as text. It
        // is a phrase, the one form it writes back to: written bare it would operate again.
        if [&b"AND"[..], b"OR", b"NOT"].contains(&bytes.as_slice()) {
            return Ok(Search::Phrase(self.text(bytes)?));
        }
        Ok(Search::Word(self.text(bytes)?))
    }

    /// `searchExpr-incomplete` after its opening `'`: one phrase, `''` a quote.
    fn incomplete(&mut self) -> Result<Search, UrlError> {
        let start = self.c.pos();
        let mut bytes = Vec::new();
        loop {
            if self.c.squote() {
                if self.c.squote() {
                    bytes.push(b'\'');
                    continue;
                }
                break;
            }
            if self.c.eat("+") {
                bytes.push(b' ');
                continue;
            }
            match self.byte() {
                Some(b) => bytes.push(b),
                None => return Err(self.c.syntax("SQUOTE")),
            }
            self.length(start)?;
        }
        self.node()?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Err(self.c.syntax("searchExpr-incomplete"));
        }
        Ok(Search::Phrase(self.text(bytes)?))
    }

    /// One byte, percent-decoded; `&` and the end of the URL are `None`.
    fn byte(&mut self) -> Option<u8> {
        if let Some(b) = pct_encoded(self.c) {
            return Some(b);
        }
        if self.c.peek() == Some(b'%') {
            return None;
        }
        self.c.eat_if(|b| b != b'&' && b != b'#')
    }

    /// A word or phrase within `max_literal_len` bytes as written.
    fn length(&self, start: usize) -> Result<(), UrlError> {
        let max = self.limits.max_literal_len;
        if self.c.pos().saturating_sub(start) > max {
            return Err(self.c.limit(LimitKind::LiteralLength, max));
        }
        Ok(())
    }

    fn text(&self, bytes: Vec<u8>) -> Result<String, UrlError> {
        String::from_utf8(bytes).map_err(|_| self.c.syntax("UTF-8"))
    }
}

impl Display for Search {
    /// The canonical form: operators spelled out, `%20` between tokens, a group around each
    /// compound operand of a tighter operator, every byte outside `unreserved` and
    /// `!*,:@/?$=` percent-encoded.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            // a word built by hand as an operator keeps its meaning as a phrase
            Search::Word(w) if !["AND", "OR", "NOT"].contains(&w.as_str()) => encoded(f, w),
            // A `searchPhrase` cannot hold a quotation mark; the `searchExpr-incomplete` that read
            // one can, with `''` for a quote (T-4205). The parse yields such a phrase only as the
            // whole expression, the one place that form is read again.
            Search::Phrase(p) if p.contains('"') => {
                f.write_str("%27")?;
                for (n, part) in p.split('\'').enumerate() {
                    if n > 0 {
                        f.write_str("%27%27")?;
                    }
                    encoded(f, part)?;
                }
                f.write_str("%27")
            }
            Search::Word(p) | Search::Phrase(p) => {
                f.write_str("%22")?;
                encoded(f, p)?;
                f.write_str("%22")
            }
            Search::Not(operand) => {
                f.write_str("NOT%20")?;
                grouped(f, operand, |s| {
                    matches!(s, Search::And(..) | Search::Or(..))
                })
            }
            Search::And(left, right) => {
                let loose = |s: &Search| matches!(s, Search::Or(..));
                grouped(f, left, loose)?;
                f.write_str("%20AND%20")?;
                grouped(f, right, |s| matches!(s, Search::Or(..) | Search::And(..)))
            }
            Search::Or(left, right) => {
                write!(f, "{left}%20OR%20")?;
                grouped(f, right, |s| matches!(s, Search::Or(..)))
            }
        }
    }
}

/// `operand`, in parentheses when `group` says so.
fn grouped(
    f: &mut Formatter<'_>,
    operand: &Search,
    group: impl Fn(&Search) -> bool,
) -> fmt::Result {
    if group(operand) {
        write!(f, "({operand})")
    } else {
        write!(f, "{operand}")
    }
}

fn encoded(f: &mut Formatter<'_>, text: &str) -> fmt::Result {
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!*,:@/?$=".contains(&b) {
            write!(f, "{}", char::from(b))?;
        } else {
            write!(f, "%{b:02X}")?;
        }
    }
    Ok(())
}
