// SPDX-License-Identifier: EUPL-1.2
//! A byte cursor over the relative URL that remembers the furthest byte any match reached: the
//! offset of a syntax error, as the OASIS test tool reports it (`maxMatched` of `apg-js`).

use crate::{Feature, LimitKind, UrlError, UrlErrorKind};

#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    src: &'a str,
    pos: usize,
    max: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Cursor {
            src,
            pos: 0,
            max: 0,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Back to `pos`; the furthest match is kept.
    pub(crate) fn reset(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub(crate) fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or_default()
    }

    pub(crate) fn slice(&self, from: usize) -> &'a str {
        self.src.get(from..self.pos).unwrap_or_default()
    }

    /// This cursor over the URL up to byte `end`, its furthest match starting here: a scan that
    /// reaches `end` ran into a bound. `None` when `end` is past the URL or inside a character.
    pub(crate) fn until(&self, end: usize) -> Option<Cursor<'a>> {
        Some(Cursor {
            src: self.src.get(..end)?,
            pos: self.pos,
            max: self.pos,
        })
    }

    /// The position and the furthest match of `inner`, a cursor from [`Cursor::until`].
    pub(crate) fn take(&mut self, inner: &Cursor<'_>) {
        self.pos = inner.pos;
        self.max = self.max.max(inner.max);
    }

    /// Whether a scan reached the end of the bytes this cursor sees.
    pub(crate) fn reached_end(&self) -> bool {
        self.reached_end_but(0)
    }

    /// Whether a scan reached the last `bytes` bytes this cursor sees.
    pub(crate) fn reached_end_but(&self, bytes: usize) -> bool {
        self.max.saturating_add(bytes) >= self.src.len()
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    pub(crate) fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.src
            .as_bytes()
            .get(self.pos.saturating_add(ahead))
            .copied()
    }

    pub(crate) fn advance(&mut self, bytes: usize) {
        self.pos = self.pos.saturating_add(bytes).min(self.src.len());
        self.max = self.max.max(self.pos);
    }

    /// One byte that `accept` takes.
    pub(crate) fn eat_if(&mut self, accept: impl Fn(u8) -> bool) -> Option<u8> {
        let byte = self.peek().filter(|b| accept(*b))?;
        self.advance(1);
        Some(byte)
    }

    /// `literal`, case-sensitive (`%s"..."` in the ABNF).
    pub(crate) fn eat(&mut self, literal: &str) -> bool {
        let found = self.rest().starts_with(literal);
        if found {
            self.advance(literal.len());
        }
        found
    }

    /// `literal`, ASCII case-insensitive (a plain `"..."` in the ABNF).
    pub(crate) fn eat_ci(&mut self, literal: &str) -> bool {
        let found = self
            .rest()
            .as_bytes()
            .get(..literal.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(literal.as_bytes()));
        if found {
            self.advance(literal.len());
        }
        found
    }

    /// One of the ABNF delimiter rules: the character or its percent-encoding.
    fn token(&mut self, plain: &str, encoded: &str) -> bool {
        self.eat(plain) || self.eat_ci(encoded)
    }

    pub(crate) fn open(&mut self) -> bool {
        self.token("(", "%28")
    }

    pub(crate) fn close(&mut self) -> bool {
        self.token(")", "%29")
    }

    pub(crate) fn comma(&mut self) -> bool {
        self.token(",", "%2C")
    }

    pub(crate) fn squote(&mut self) -> bool {
        self.token("'", "%27")
    }

    pub(crate) fn colon(&mut self) -> bool {
        self.token(":", "%3A")
    }

    pub(crate) fn semi(&mut self) -> bool {
        self.token(";", "%3B")
    }

    pub(crate) fn star(&mut self) -> bool {
        self.token("*", "%2A")
    }

    pub(crate) fn at(&mut self) -> bool {
        self.token("@", "%40")
    }

    /// `/` inside a query option, or `%2F` as a form encoder writes it (T-3719): there the ABNF
    /// has no other reading of `%2F`. A resource path keeps `%2F` as part of a key.
    pub(crate) fn slash(&mut self) -> bool {
        self.token("/", "%2F")
    }

    /// `EQ` after an option or alias name, or `%3D` as a form encoder writes it inside
    /// `$expand(...)` (T-3719). A custom option's `=` stays as written.
    pub(crate) fn eq_sign(&mut self) -> bool {
        self.token("=", "%3D")
    }

    /// `SIGN`, read as `+` or `-`.
    pub(crate) fn sign(&mut self) -> Option<char> {
        if self.eat("+") || self.eat_ci("%2B") {
            Some('+')
        } else if self.eat("-") {
            Some('-')
        } else {
            None
        }
    }

    /// One whitespace unit of `RWS`/`BWS`.
    fn space(&mut self) -> bool {
        self.eat(" ") || self.eat("\t") || self.eat("%20") || self.eat_ci("%09")
    }

    /// `BWS`: optional whitespace.
    pub(crate) fn bws(&mut self) {
        while self.space() {}
    }

    /// `RWS`: required whitespace, where `+` also counts (T-3719): a form encoder (Go's
    /// `url.Values`, Java's `URLEncoder`) writes a space as `+`. Where `RWS` stands, a literal
    /// `+` can only be the sign of the next number, and that sign means nothing (`gt +5` is
    /// `gt 5`), so no valid URL reads differently; `BWS` and literals keep `+` as written.
    pub(crate) fn rws(&mut self) -> bool {
        let unit = |c: &mut Self| c.space() || c.eat("+");
        let found = unit(self);
        while found && unit(self) {}
        found
    }

    /// `1*DIGIT`, as a slice.
    pub(crate) fn digits(&mut self) -> &'a str {
        let start = self.pos;
        while self.eat_if(|b| b.is_ascii_digit()).is_some() {}
        self.slice(start)
    }

    /// `odataIdentifier`: `ALPHA / "_"` then up to 127 of `ALPHA / "_" / DIGIT`. A
    /// percent-encoded non-ASCII character, which the ABNF allows from the letter and mark
    /// classes, is not implemented.
    pub(crate) fn identifier(&mut self) -> Result<Option<&'a str>, UrlError> {
        let start = self.pos;
        if self
            .eat_if(|b| b.is_ascii_alphabetic() || b == b'_')
            .is_none()
        {
            return self.non_ascii().map(|()| None);
        }
        for _ in 0..127 {
            if self
                .eat_if(|b| b.is_ascii_alphanumeric() || b == b'_')
                .is_none()
            {
                break;
            }
        }
        self.non_ascii()?;
        Ok(Some(self.slice(start)))
    }

    /// A percent-encoded byte of a UTF-8 sequence (`%80` to `%FF`) where an identifier goes on.
    fn non_ascii(&self) -> Result<(), UrlError> {
        let bytes = self.rest().as_bytes();
        match (bytes.first(), bytes.get(1)) {
            (Some(b'%'), Some(b'8'..=b'9' | b'a'..=b'f' | b'A'..=b'F')) => {
                Err(self.error(UrlErrorKind::NotImplemented(Feature::NonAsciiIdentifier)))
            }
            _ => Ok(()),
        }
    }

    /// A syntax error at the furthest byte matched.
    pub(crate) fn syntax(&self, expected: &'static str) -> UrlError {
        UrlError {
            kind: UrlErrorKind::Syntax { expected },
            offset: self.max,
        }
    }

    /// Any other error, at the current byte.
    pub(crate) fn error(&self, kind: UrlErrorKind) -> UrlError {
        UrlError {
            kind,
            offset: self.pos,
        }
    }

    pub(crate) fn not_implemented(&self, feature: Feature) -> UrlError {
        self.error(UrlErrorKind::NotImplemented(feature))
    }

    pub(crate) fn limit(&self, limit: LimitKind, max: usize) -> UrlError {
        self.error(UrlErrorKind::LimitExceeded { limit, max })
    }
}

/// Counts one more against `max`, as a [`LimitExceeded`](UrlErrorKind::LimitExceeded) at the
/// cursor when it goes past.
pub(crate) fn count(
    counter: &mut usize,
    max: usize,
    limit: LimitKind,
    at: &Cursor<'_>,
) -> Result<(), UrlError> {
    match counter.checked_add(1) {
        Some(next) if next <= max => {
            *counter = next;
            Ok(())
        }
        _ => Err(at.limit(limit, max)),
    }
}
