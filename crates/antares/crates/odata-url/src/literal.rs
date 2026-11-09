// SPDX-License-Identifier: MIT
//! `primitiveLiteral` (ABNF section 7): the subset of T-3441 F26 and F29 as values, the other
//! literal kinds recognised and answered as not implemented.

use crate::cursor::Cursor;
use crate::{Feature, Geography, LimitKind, Limits, Literal, Position, Shape, UrlError};

/// A literal at the cursor, `None` (cursor unmoved) when none starts there. Once a literal has
/// certainly started (an opening quote, a `geography'` prefix) a malformed rest is an error.
///
/// The literal is read through a cursor that ends one byte past `max_literal_len`, so a longer
/// one is refused before its bytes are read or copied.
pub(crate) fn literal(c: &mut Cursor<'_>, limits: &Limits) -> Result<Option<Literal>, UrlError> {
    let start = c.pos();
    let end = start
        .saturating_add(limits.max_literal_len)
        .saturating_add(1);
    // No bound inside the URL: the rest is shorter, or the bound falls inside a non-ASCII
    // character, which no literal takes.
    let Some(mut bounded) = c.until(end) else {
        return any_literal(c);
    };
    let found = any_literal(&mut bounded);
    c.take(&bounded);
    // Any reading that ran into the bound is too long: the literal itself, or a longer kind
    // tried first (an overlong date-time is not read as the date it starts with). A failure
    // within a percent escape of the bound ran into it too: the bound cut the escape.
    let ran_into_bound = match found {
        Ok(None) => false,
        Ok(Some(_)) => bounded.reached_end(),
        Err(_) => bounded.reached_end_but(2),
    };
    if ran_into_bound {
        c.reset(start);
        return Err(c.limit(LimitKind::LiteralLength, limits.max_literal_len));
    }
    found
}

fn any_literal(c: &mut Cursor<'_>) -> Result<Option<Literal>, UrlError> {
    let start = c.pos();
    if keyword(c, "null", true) {
        return Ok(Some(Literal::Null));
    }
    for (word, value) in [("true", true), ("false", false)] {
        if keyword(c, word, false) {
            return Ok(Some(Literal::Bool(value)));
        }
    }
    if prefixed(c, "geography") {
        return geography(c).map(|g| Some(Literal::Geography(g)));
    }
    if prefixed(c, "geometry") {
        return geography(c).map(|g| Some(Literal::Geometry(g)));
    }
    if prefixed(c, "duration") {
        // `SIGN` before the body: `+` writes itself `%2B`, which the body scan stops at (T-6100)
        let sign = c.sign().map(String::from).unwrap_or_default();
        return quoted(c, "durationValue", is_duration).map(|d| Some(Literal::Duration(sign + &d)));
    }
    if prefixed(c, "binary") {
        return quoted(c, "binaryValue", is_base64url).map(|b| Some(Literal::Binary(b)));
    }
    if enum_prefix(c)? {
        c.reset(start);
        return Err(c.not_implemented(Feature::Literal));
    }
    if c.squote() {
        return string_body(c).map(|s| Some(Literal::String(s)));
    }
    if guid(c) {
        return Ok(Some(Literal::Guid(c.slice(start).to_ascii_lowercase())));
    }
    c.reset(start);
    if let Some(text) = date_time_offset(c) {
        return Ok(Some(Literal::DateTimeOffset(text)));
    }
    c.reset(start);
    if let Some(text) = date(c) {
        return Ok(Some(Literal::Date(text)));
    }
    c.reset(start);
    if let Some(text) = time_of_day(c) {
        return Ok(Some(Literal::TimeOfDay(text)));
    }
    c.reset(start);
    Ok(number(c))
}

fn continues_identifier(c: &Cursor<'_>) -> bool {
    c.peek()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A word literal not followed by identifier characters (`nullable` is a name, not `null`).
fn keyword(c: &mut Cursor<'_>, word: &str, case_sensitive: bool) -> bool {
    let start = c.pos();
    let found = if case_sensitive {
        c.eat(word)
    } else {
        c.eat_ci(word)
    };
    if found && !continues_identifier(c) {
        return true;
    }
    c.reset(start);
    false
}

/// `prefix` directly followed by a quote, both consumed.
fn prefixed(c: &mut Cursor<'_>, prefix: &str) -> bool {
    let start = c.pos();
    if c.eat_ci(prefix) && c.squote() {
        return true;
    }
    c.reset(start);
    false
}

/// The text up to the closing quote of a `duration'..'` or `binary'..'` literal, when `valid`
/// holds for it; a syntax error naming `expected` otherwise. The text is ASCII by both rules,
/// and its length is bounded by the literal's (`max_literal_len`).
fn quoted(
    c: &mut Cursor<'_>,
    expected: &'static str,
    valid: fn(&str) -> bool,
) -> Result<String, UrlError> {
    let body = c.pos();
    while c.peek().is_some_and(|b| b != b'\'' && b != b'%') {
        c.advance(1);
    }
    let text = c.slice(body).to_owned();
    if valid(&text) && c.squote() {
        Ok(text)
    } else {
        c.reset(body);
        Err(c.syntax(expected))
    }
}

/// `durationValue` (ABNF) after its `[ sign ]`, which the caller reads: `"P" [ 1*DIGIT "D" ]
/// [ "T" [ 1*DIGIT "H" ] [ 1*DIGIT "M" ] [ 1*DIGIT [ "." 1*DIGIT ] "S" ] ]`, with at least one
/// component and none after a bare `T`.
pub(crate) fn is_duration(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('P') else {
        return false;
    };
    let (days, time) = rest
        .split_once('T')
        .map_or((rest, None), |(d, t)| (d, Some(t)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let days_ok = days.is_empty() || days.strip_suffix('D').is_some_and(digits);
    let time_ok = time.is_none_or(|mut t| {
        let mut any = false;
        for unit in ['H', 'M'] {
            if let Some((n, after)) = t.split_once(unit) {
                if !digits(n) {
                    return false;
                }
                (t, any) = (after, true);
            }
        }
        if let Some(seconds) = t.strip_suffix('S') {
            let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, "1"));
            return digits(whole) && digits(fraction);
        }
        any && t.is_empty()
    });
    days_ok && time_ok && (!days.is_empty() || time.is_some())
}

/// ABNF form (`OData` JSON 7.1: `Edm.Date`, `DateTimeOffset`, `TimeOfDay`, `Duration`,
/// `Guid` and `Binary` are strings, each in the lexical form its literal has);
/// Guid and Binary are strings, each in the lexical form its literal has);
/// `true` for every other type. A body holds JSON text, so the `%3A`/`%2B`
/// spellings a URL literal may use are refused here.
pub fn is_primitive_text(name: &str, text: &str) -> bool {
    let whole = |rule: fn(&mut Cursor<'_>) -> bool| {
        let mut c = Cursor::new(text);
        !text.contains('%') && rule(&mut c) && c.at_end()
    };
    match name {
        "Edm.Date" => whole(|c| date(c).is_some()),
        "Edm.DateTimeOffset" => whole(|c| date_time_offset(c).is_some()),
        "Edm.TimeOfDay" => whole(|c| time_of_day(c).is_some()),
        "Edm.Duration" => is_duration(text.strip_prefix('-').unwrap_or(text)),
        "Edm.Guid" => whole(guid),
        "Edm.Binary" => is_base64url(text),
        _ => true,
    }
}

/// `binaryValue` (ABNF): base64url (`A-Z a-z 0-9 - _`), padded with `=` to a multiple of four
/// or unpadded; a length of one past a multiple of four encodes no whole byte.
pub(crate) fn is_base64url(text: &str) -> bool {
    let data = text.trim_end_matches('=');
    let padding = text.len().saturating_sub(data.len());
    data.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && data.len() % 4 != 1
        && (padding == 0 || (padding <= 2 && text.len().is_multiple_of(4)))
}

/// `qualifiedEnumTypeName SQUOTE`: a dotted name directly followed by a quote.
fn enum_prefix(c: &mut Cursor<'_>) -> Result<bool, UrlError> {
    let start = c.pos();
    let mut parts = 0_u8;
    while c.identifier()?.is_some() {
        parts = parts.saturating_add(1);
        if !c.eat(".") {
            break;
        }
    }
    let found = parts >= 2 && c.squote();
    c.reset(start);
    Ok(found)
}

/// The rest of `stringLiteral` after its opening quote, percent-decoded.
pub(crate) fn string_body(c: &mut Cursor<'_>) -> Result<String, UrlError> {
    let mut bytes = Vec::new();
    loop {
        if c.squote() {
            if c.squote() {
                bytes.push(b'\'');
                continue;
            }
            return String::from_utf8(bytes).map_err(|_| c.syntax("stringLiteral"));
        }
        // `%27` is a quote, taken by `squote` above.
        if let Some(byte) = pct_encoded(c) {
            bytes.push(byte);
        } else if let Some(byte) = c.eat_if(is_pchar_no_squote) {
            bytes.push(byte);
        } else {
            return Err(c.syntax("stringLiteral"));
        }
    }
}

/// `pchar-no-SQUOTE` without its percent-encoded part.
fn is_pchar_no_squote(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!()*+,;$&=:@".contains(&b)
}

/// `"%" HEXDIG HEXDIG`, decoded.
pub(crate) fn pct_encoded(c: &mut Cursor<'_>) -> Option<u8> {
    let rest = c.rest().as_bytes();
    let (Some(b'%'), Some(&hi), Some(&lo)) = (rest.first(), rest.get(1), rest.get(2)) else {
        return None;
    };
    let value = hex(hi)?.checked_mul(16)?.checked_add(hex(lo)?)?;
    c.advance(3);
    Some(value)
}

fn hex(b: u8) -> Option<u8> {
    char::from(b)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

fn hexdigits(c: &mut Cursor<'_>, n: usize) -> bool {
    (0..n).all(|_| c.eat_if(|b| b.is_ascii_hexdigit()).is_some())
}

/// `guid`, consumed (the caller resets).
fn guid(c: &mut Cursor<'_>) -> bool {
    [8, 4, 4, 4]
        .into_iter()
        .all(|n| hexdigits(c, n) && c.eat("-"))
        && hexdigits(c, 12)
}

/// `n` digits exactly.
fn fixed_digits(c: &mut Cursor<'_>, n: usize) -> bool {
    (0..n).all(|_| c.eat_if(|b| b.is_ascii_digit()).is_some())
}

/// `date`: `year "-" month "-" day`.
fn date(c: &mut Cursor<'_>) -> Option<String> {
    let start = c.pos();
    c.eat("-");
    let year_ok = if c.eat("0") {
        fixed_digits(c, 3)
    } else {
        c.eat_if(|b| (b'1'..=b'9').contains(&b)).is_some() && fixed_digits(c, 3) && {
            c.digits();
            true
        }
    };
    let ok = year_ok && c.eat("-") && two(c, 1, 12) && c.eat("-") && two(c, 1, 31);
    ok.then(|| c.slice(start).to_owned())
}

/// Two digits forming a number from `low` to `high`, consumed as far as they match the ABNF
/// rule's leading digits.
fn two(c: &mut Cursor<'_>, low: u8, high: u8) -> bool {
    let (Some(tens), Some(ones)) = (c.peek(), c.peek_at(1)) else {
        return false;
    };
    if !tens.is_ascii_digit() {
        return false;
    }
    let tens_max = high / 10;
    let tens_value = tens.wrapping_sub(b'0');
    if tens_value > tens_max {
        return false;
    }
    c.advance(1);
    if !ones.is_ascii_digit() {
        return false;
    }
    let value = tens_value
        .saturating_mul(10)
        .saturating_add(ones.wrapping_sub(b'0'));
    if value < low || value > high {
        return false;
    }
    c.advance(1);
    true
}

/// `timeOfDayLiteral` (`COLON` may be `%3A`), colons normalised.
fn time_of_day(c: &mut Cursor<'_>) -> Option<String> {
    let colon = Cursor::colon;
    let mut text = String::new();
    let hour = c.rest().get(..2)?.to_owned();
    if !two(c, 0, 23) || !colon(c) {
        return None;
    }
    text.push_str(&hour);
    text.push(':');
    let minute = c.rest().get(..2)?.to_owned();
    if !two(c, 0, 59) {
        return None;
    }
    text.push_str(&minute);
    let before = c.pos();
    if colon(c) {
        let second = c.rest().get(..2).unwrap_or_default().to_owned();
        if !(two(c, 0, 59) || c.eat("60")) {
            c.reset(before);
            return Some(text);
        }
        text.push(':');
        text.push_str(&second);
        let dot = c.pos();
        if c.eat(".") {
            let fraction = c.digits();
            if fraction.is_empty() || fraction.len() > 12 {
                c.reset(dot);
                return Some(text);
            }
            text.push('.');
            text.push_str(fraction);
        }
    }
    Some(text)
}

/// `dateTimeOffsetLiteral`: `date "T" timeOfDayLiteral ( "Z" / SIGN hour COLON minute )`.
fn date_time_offset(c: &mut Cursor<'_>) -> Option<String> {
    let mut text = date(c)?;
    if !c.eat_ci("T") {
        return None;
    }
    text.push('T');
    text.push_str(&time_of_day(c)?);
    if c.eat_ci("Z") {
        text.push('Z');
        return Some(text);
    }
    text.push(c.sign()?);
    let hour = c.rest().get(..2)?.to_owned();
    if !two(c, 0, 23) || !c.colon() {
        return None;
    }
    let minute = c.rest().get(..2)?.to_owned();
    if !two(c, 0, 59) {
        return None;
    }
    text.push_str(&hour);
    text.push(':');
    text.push_str(&minute);
    Some(text)
}

/// `decimalLiteral` (`SIGN` may be `%2B`): an `Int64` as [`Literal::Int`], everything else as
/// [`Literal::Decimal`] text.
pub(crate) fn number(c: &mut Cursor<'_>) -> Option<Literal> {
    let (text, integer) = number_text(c, true)?;
    // The `f` suffix of an `Edm.Single` literal (OData 2.0 and 3.0 URL conventions, still sent
    // by clients and the NLstn compliance suite): the same value, so `3f` is the Int `3` and
    // writes back as a literal that parses to the same tree.
    if !text.ends_with(['N', 'F']) && c.peek().is_some_and(|b| b == b'f' || b == b'F') {
        let at = c.pos();
        c.advance(1);
        if continues_identifier(c) {
            c.reset(at);
        }
    }
    if integer {
        if let Ok(value) = text.parse::<i64>() {
            return Some(Literal::Int(value));
        }
    }
    Some(Literal::Decimal(text))
}

/// `decimalLiteral` (`in_url`) or `decimalValue` as written, and whether it is an integer.
fn number_text(c: &mut Cursor<'_>, in_url: bool) -> Option<(String, bool)> {
    let start = c.pos();
    let sign = if in_url {
        c.sign()
    } else if c.eat("+") {
        Some('+')
    } else if c.eat("-") {
        Some('-')
    } else {
        None
    };
    let digits = c.digits();
    if digits.is_empty() {
        c.reset(start);
        for word in ["NaN", "-INF", "INF"] {
            if keyword(c, word, true) {
                return Some((word.to_owned(), false));
            }
        }
        return None;
    }
    let mut text: String = sign.into_iter().collect();
    text.push_str(digits);
    let mut integer = true;
    let dot = c.pos();
    if c.eat(".") {
        let fraction = c.digits();
        if fraction.is_empty() {
            c.reset(dot);
        } else {
            integer = false;
            text.push('.');
            text.push_str(fraction);
        }
    }
    let exponent = c.pos();
    if c.eat_ci("e") {
        let exp_sign = if in_url {
            c.sign()
        } else {
            c.eat("-")
                .then_some('-')
                .or_else(|| c.eat("+").then_some('+'))
        };
        let exp_digits = c.digits();
        if exp_digits.is_empty() {
            c.reset(exponent);
        } else {
            integer = false;
            text.push('e');
            text.extend(exp_sign);
            text.push_str(exp_digits);
        }
    }
    Some((text, integer))
}

/// The rest of a `geography'...'` literal after its quote: a point or a polygon of the
/// subset, any other shape not implemented.
fn geography(c: &mut Cursor<'_>) -> Result<Geography, UrlError> {
    if !c.eat_ci("SRID") || !c.eat("=") {
        return Err(c.syntax("sridLiteral"));
    }
    let digits = c.digits();
    let srid = Some(digits)
        .filter(|d| (1..=5).contains(&d.len()))
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| c.syntax("sridLiteral"))?;
    if !c.semi() {
        return Err(c.syntax("sridLiteral"));
    }
    let shape = if c.eat_ci("Point") {
        Shape::Point(point_data(c)?)
    } else if c.eat_ci("Polygon") {
        Shape::Polygon(polygon_data(c)?)
    } else if [
        "LineString",
        "MultiLineString",
        "MultiPoint",
        "MultiPolygon",
        "GeometryCollection",
    ]
    .into_iter()
    .any(|shape| {
        c.rest()
            .get(..shape.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(shape))
    }) {
        return Err(c.not_implemented(Feature::Literal));
    } else {
        return Err(c.syntax("geoLiteral"));
    };
    if !c.squote() {
        return Err(c.syntax("geographyPrefix SQUOTE"));
    }
    Ok(Geography { srid, shape })
}

fn point_data(c: &mut Cursor<'_>) -> Result<Position, UrlError> {
    if !c.open() {
        return Err(c.syntax("pointData"));
    }
    let position = position(c)?;
    if !c.close() {
        return Err(c.syntax("pointData"));
    }
    Ok(position)
}

fn polygon_data(c: &mut Cursor<'_>) -> Result<Vec<Vec<Position>>, UrlError> {
    if !c.open() {
        return Err(c.syntax("polygonData"));
    }
    let mut rings = Vec::new();
    loop {
        if !c.open() {
            return Err(c.syntax("ringLiteral"));
        }
        let mut ring = vec![position(c)?];
        while c.comma() {
            ring.push(position(c)?);
        }
        if !c.close() {
            return Err(c.syntax("ringLiteral"));
        }
        rings.push(ring);
        if !c.comma() {
            break;
        }
    }
    if !c.close() {
        return Err(c.syntax("polygonData"));
    }
    Ok(rings)
}

/// `positionLiteral`: two to four `doubleValue`s separated by `SP` (or `%20`).
fn position(c: &mut Cursor<'_>) -> Result<Position, UrlError> {
    let mut values = vec![double_value(c)?];
    for required in [true, false, false] {
        let before = c.pos();
        if !(c.eat(" ") || c.eat("%20")) {
            if required {
                return Err(c.syntax("positionLiteral"));
            }
            break;
        }
        match number_text(c, false) {
            Some((value, _)) => values.push(value),
            None if required => return Err(c.syntax("doubleValue")),
            None => {
                c.reset(before);
                break;
            }
        }
    }
    Ok(Position(values))
}

fn double_value(c: &mut Cursor<'_>) -> Result<String, UrlError> {
    number_text(c, false)
        .map(|(value, _)| value)
        .ok_or_else(|| c.syntax("doubleValue"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Result<Option<Literal>, UrlError> {
        let mut c = Cursor::new(input);
        let found = literal(&mut c, &Limits::strict())?;
        assert_eq!(found.as_ref().map_or("", |_| c.rest()), "", "{input}");
        Ok(found)
    }

    /// The literal at the start of `input` and what is left of it.
    fn first(input: &str) -> (Option<Literal>, String) {
        let mut c = Cursor::new(input);
        let found = literal(&mut c, &Limits::strict());
        assert!(found.is_ok(), "{input}: {found:?}");
        (found.unwrap_or_default(), c.rest().to_owned())
    }

    #[test]
    fn partial_matches_stop_where_the_rule_does() {
        let time = |t: &str| Some(Literal::TimeOfDay(t.into()));
        let date = |t: &str| Some(Literal::Date(t.into()));
        for (input, literal, rest) in [
            ("2024-13-01", Some(Literal::Int(2024)), "-13-01"),
            ("2024-00-01", Some(Literal::Int(2024)), "-00-01"),
            ("2024-1", Some(Literal::Int(2024)), "-1"),
            ("10:60", Some(Literal::Int(10)), ":60"),
            ("10:00:61", time("10:00"), ":61"),
            ("10:00:60", time("10:00:60"), ""),
            ("10:00:00.", time("10:00:00"), "."),
            ("10:00:00.1234567890123", time("10:00:00"), ".1234567890123"),
            ("2024-01-01T10:00+25:00", date("2024-01-01"), "T10:00+25:00"),
            ("2024-01-01T10:00+01x00", date("2024-01-01"), "T10:00+01x00"),
            ("2024-01-01T10:00+01:60", date("2024-01-01"), "T10:00+01:60"),
            ("2024-01-01X", date("2024-01-01"), "X"),
            ("2024-01x", Some(Literal::Int(2024)), "-01x"),
            ("geography", None, "geography"),
        ] {
            let found = first(input);
            assert_eq!(found, (literal, rest.to_owned()), "{input}");
        }
        let unclosed = read("'a%4'");
        assert!(
            matches!(
                unclosed,
                Err(UrlError {
                    kind: crate::UrlErrorKind::Syntax { .. },
                    offset: 2
                })
            ),
            "{unclosed:?}"
        );
    }

    #[test]
    fn geography_errors() {
        for (input, offset) in [
            ("geography'Point(1 2)'", 10),
            ("geography'SRID=x;Point(1 2)'", 15),
            ("geography'SRID=0;Polygon(1 2)'", 25),
            ("geography'SRID=0;Polygon 1'", 24),
            ("geography'SRID'", 14),
            ("geography'SRID=0;Polygon((0 0,1 1'", 33),
        ] {
            let e = read(input);
            assert!(
                matches!(
                    e,
                    Err(UrlError {
                        kind: crate::UrlErrorKind::Syntax { .. },
                        ..
                    })
                ),
                "{input}: {e:?}"
            );
            assert_eq!(e.err().map(|e| e.offset), Some(offset), "{input}");
        }
        let signed = ok("geography'SRID=0;Point(+1 -2)'");
        let point = Shape::Point(Position(vec!["+1".into(), "-2".into()]));
        assert_eq!(
            signed,
            Literal::Geography(Geography {
                srid: 0,
                shape: point
            })
        );
    }

    fn ok(input: &str) -> Literal {
        read(input).unwrap().unwrap()
    }

    #[test]
    fn words() {
        assert_eq!(ok("null"), Literal::Null);
        assert_eq!(ok("true"), Literal::Bool(true));
        assert_eq!(ok("FALSE"), Literal::Bool(false));
        assert_eq!(read("nullable").unwrap(), None);
        assert_eq!(read("NULL").unwrap(), None);
        assert_eq!(read("trueish").unwrap(), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(ok("42"), Literal::Int(42));
        assert_eq!(ok("-42"), Literal::Int(-42));
        assert_eq!(ok("%2B42"), Literal::Int(42));
        assert_eq!(ok("1.5"), Literal::Decimal("1.5".into()));
        assert_eq!(ok("-0.314e1"), Literal::Decimal("-0.314e1".into()));
        assert_eq!(ok("1E-3"), Literal::Decimal("1e-3".into()));
        assert_eq!(ok("1e%2B3"), Literal::Decimal("1e+3".into()));
        assert_eq!(ok("NaN"), Literal::Decimal("NaN".into()));
        assert_eq!(ok("-INF"), Literal::Decimal("-INF".into()));
        assert_eq!(ok("INF"), Literal::Decimal("INF".into()));
        assert_eq!(
            ok("99999999999999999999"),
            Literal::Decimal("99999999999999999999".into())
        );
        assert_eq!(read("INFO").unwrap(), None);
        // A trailing dot or a bare exponent is not part of the number.
        let mut c = Cursor::new("42.");
        assert_eq!(number(&mut c), Some(Literal::Int(42)));
        assert_eq!(c.rest(), ".");
        let mut c = Cursor::new("4e");
        assert_eq!(number(&mut c), Some(Literal::Int(4)));
        assert_eq!(c.rest(), "e");
    }

    #[test]
    fn strings() {
        assert_eq!(ok("'O''Neil'"), Literal::String("O'Neil".into()));
        assert_eq!(ok("%27O%27%27Neil%27"), Literal::String("O'Neil".into()));
        assert_eq!(ok("'a%20b%C3%A9'"), Literal::String("a bé".into()));
        assert_eq!(ok("''"), Literal::String(String::new()));
        let unterminated = read("'abc").unwrap_err();
        assert_eq!(unterminated.offset, 4);
        let slash = read("'a/b'").unwrap_err();
        assert_eq!(slash.offset, 2);
        let invalid_utf8 = read("'%FF'").unwrap_err();
        assert_eq!(invalid_utf8.offset, 5);
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(ok("2012-09-03"), Literal::Date("2012-09-03".into()));
        assert_eq!(ok("-0001-01-01"), Literal::Date("-0001-01-01".into()));
        assert_eq!(ok("12012-12-31"), Literal::Date("12012-12-31".into()));
        assert_eq!(
            ok("2012-09-03T13:52:00.123456789012Z"),
            Literal::DateTimeOffset("2012-09-03T13:52:00.123456789012Z".into())
        );
        assert_eq!(
            ok("2012-09-03t13%3A52-03%3A00"),
            Literal::DateTimeOffset("2012-09-03T13:52-03:00".into())
        );
        assert_eq!(ok("13:52"), Literal::TimeOfDay("13:52".into()));
        assert_eq!(ok("23:59:60"), Literal::TimeOfDay("23:59:60".into()));
        assert_eq!(
            ok("07:59:59.999"),
            Literal::TimeOfDay("07:59:59.999".into())
        );
        // Not a date: the digits are a number.
        assert_eq!(ok("2012"), Literal::Int(2012));
        let mut c = Cursor::new("2012-13-01");
        assert_eq!(
            literal(&mut c, &Limits::strict()).unwrap(),
            Some(Literal::Int(2012))
        );
        let mut c = Cursor::new("24:00");
        assert_eq!(
            literal(&mut c, &Limits::strict()).unwrap(),
            Some(Literal::Int(24))
        );
    }

    #[test]
    fn geography() {
        assert_eq!(
            ok("geography'SRID=4326;Point(-1.5 2E1)'"),
            Literal::Geography(Geography {
                srid: 4326,
                shape: Shape::Point(Position(vec!["-1.5".into(), "2e1".into()])),
            })
        );
        assert_eq!(
            ok("geography'SRID=0;Polygon((0 0,1%201,0 1 2 3,0 0),(5 5,6 6))'"),
            Literal::Geography(Geography {
                srid: 0,
                shape: Shape::Polygon(vec![
                    vec![
                        Position(vec!["0".into(), "0".into()]),
                        Position(vec!["1".into(), "1".into()]),
                        Position(vec!["0".into(), "1".into(), "2".into(), "3".into()]),
                        Position(vec!["0".into(), "0".into()]),
                    ],
                    vec![
                        Position(vec!["5".into(), "5".into()]),
                        Position(vec!["6".into(), "6".into()]),
                    ],
                ]),
            })
        );
        for bad in [
            "geography'Point(1 2)'",
            "geography'SRID=123456;Point(1 2)'",
            "geography'SRID=1,Point(1 2)'",
            "geography'SRID=1;Spot(1 2)'",
            "geography'SRID=1;Point 1 2)'",
            "geography'SRID=1;Point(1 2'",
            "geography'SRID=1;Point(1)'",
            "geography'SRID=1;Point(1 x)'",
            "geography'SRID=1;Point(1 2)",
            "geography'SRID=1;Polygon(0 0)'",
            "geography'SRID=1;Polygon((0 0)'",
            "geography'SRID=1;Polygon((0 0),'",
            "geography'SRID=1;Polygon((x 0))'",
        ] {
            assert!(
                matches!(
                    read(bad).unwrap_err().kind,
                    crate::UrlErrorKind::Syntax { .. }
                ),
                "{bad}"
            );
        }
        // A third coordinate that is not a number ends the position.
        let mut c = Cursor::new("1 2 x");
        assert_eq!(
            position(&mut c).unwrap(),
            Position(vec!["1".into(), "2".into()])
        );
        assert_eq!(c.rest(), " x");
    }

    #[test]
    fn geometry_takes_the_geography_shapes() {
        // geometryPoint and geometryPolygon (OData ABNF), the shape names case-insensitive
        assert_eq!(
            ok("geometry'SRID=0;point(1 1)'"),
            Literal::Geometry(Geography {
                srid: 0,
                shape: Shape::Point(Position(vec!["1".into(), "1".into()])),
            })
        );
        let area = ok("Geometry'SRID=0;Polygon((0 0,1 0,1 1,0 0))'");
        assert!(
            matches!(&area, Literal::Geometry(Geography { shape: Shape::Polygon(r), .. }) if r.len() == 1)
        );
        assert_eq!(
            area.to_string(),
            "geometry'SRID=0;Polygon((0%200,1%200,1%201,0%200))'"
        );
        for bad in ["geometry'Point(1 2)'", "geometry'SRID=1;Point(1 2)"] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn kinds_outside_the_subset() {
        for other in [
            "geography'SRID=0;LineString(1 1,2 2)'",
            "geometry'SRID=0;MultiPoint((1 1))'",
            "Namespace.Pattern'Yellow'",
        ] {
            assert_eq!(
                read(other).unwrap_err().kind,
                crate::UrlErrorKind::NotImplemented(Feature::Literal),
                "{other}"
            );
        }
        // A dotted name without a quote is a path, not an enum.
        assert_eq!(read("Namespace.Pattern").unwrap(), None);
        assert_eq!(read("Name").unwrap(), None);
    }

    #[test]
    fn durations_binaries_and_the_single_suffix_are_read() {
        // T-5905: the Edm types an NGSI-LD Property carries as text.
        for (text, want) in [
            ("duration'P1D'", Literal::Duration("P1D".into())),
            ("duration'-P1D'", Literal::Duration("-P1D".into())),
            ("duration'PT1.5S'", Literal::Duration("PT1.5S".into())),
            ("duration'P1DT2H30M'", Literal::Duration("P1DT2H30M".into())),
            (
                "duration'P12DT23H59M59.999999999999S'",
                Literal::Duration("P12DT23H59M59.999999999999S".into()),
            ),
            ("binary'T0RhdGE'", Literal::Binary("T0RhdGE".into())),
            ("binary'dGVzdA=='", Literal::Binary("dGVzdA==".into())),
            ("binary''", Literal::Binary(String::new())),
            ("binary'-_8'", Literal::Binary("-_8".into())),
            ("1.5f", Literal::Decimal("1.5".into())),
            ("0.0F", Literal::Decimal("0.0".into())),
            ("1.5e2f", Literal::Decimal("1.5e2".into())),
            ("-3f", Literal::Int(-3)),
            ("944F", Literal::Int(944)),
        ] {
            assert_eq!(ok(text), want, "{text}");
        }
        // a malformed body is a syntax error, before any operation runs
        for bad in [
            "duration'invalid'",
            "duration'P'",
            "duration'PT'",
            "duration'P1Y'",
            "duration'PT1S2H'",
            "duration'PT.5S'",
            "binary'!!!invalid!!!'",
            "binary'A'",
            "binary'AB='",
            "binary'A==='",
            "binary'AAAA==='",
            "duration'P1D",
        ] {
            assert!(
                matches!(
                    read(bad).unwrap_err().kind,
                    crate::UrlErrorKind::Syntax { .. }
                ),
                "{bad}"
            );
        }
        // each writes back the text it was read from
        for text in ["duration'-P1D'", "binary'dGVzdA'"] {
            assert_eq!(ok(text).to_string(), text);
        }
        // T-6100 (fuzz odata_url_parse): `SIGN` is `+`, `%2B` or `-`, and the `%2B` a plus writes
        // itself as reads back to the same literal; one sign only
        for text in [
            "duration'+P6DT23H'",
            "duration'%2BP6DT23H'",
            "duration'%2bP6DT23H'",
        ] {
            assert_eq!(ok(text), Literal::Duration("+P6DT23H".into()), "{text}");
        }
        assert_eq!(ok("duration'+P1D'").to_string(), "duration'%2BP1D'");
        assert_eq!(ok("duration'%2BP1D'").to_string(), "duration'%2BP1D'");
        for bad in [
            "duration'+-P1D'",
            "duration'-+P1D'",
            "duration'%2B%2BP1D'",
            "duration'+'",
        ] {
            assert!(
                matches!(
                    read(bad).unwrap_err().kind,
                    crate::UrlErrorKind::Syntax { .. }
                ),
                "{bad}"
            );
        }
        // `f` that starts a name is no suffix, and INF/NaN take none
        let mut c = Cursor::new("1foo");
        assert_eq!(number(&mut c), Some(Literal::Int(1)));
        assert_eq!(c.rest(), "foo");
        assert_eq!(read("INFf").unwrap(), None);
    }

    /// F-R2ANT4-012: a body string of a temporal, Guid or Binary type is
    /// held to the literal's lexical form.
    #[test]
    fn a_body_string_holds_its_primitive_form() {
        for (ty, good, bad) in [
            ("Edm.Date", "2026-10-10", "yesterday"),
            (
                "Edm.DateTimeOffset",
                "2026-10-10T10:00:00.5+02:00",
                "2026-10-10",
            ),
            ("Edm.TimeOfDay", "23:59:60", "24:00"),
            ("Edm.Duration", "-P1DT2H", "P"),
            ("Edm.Guid", "01234567-89ab-cdef-0123-456789abcdef", "0123"),
            ("Edm.Binary", "AQID", "A"),
        ] {
            assert!(is_primitive_text(ty, good), "{ty} {good}");
            assert!(!is_primitive_text(ty, bad), "{ty} {bad}");
        }
        assert!(is_primitive_text("Edm.String", "anything"));
    }

    #[test]
    fn a_guid_is_read_lower_cased() {
        // T-3720: the key of a set whose model names a GUID key.
        assert_eq!(
            ok("01234567-89AB-cdef-0123-456789abcdef"),
            Literal::Guid("01234567-89ab-cdef-0123-456789abcdef".into())
        );
        assert_eq!(
            Literal::Guid("01234567-89ab-cdef-0123-456789abcdef".into()).to_string(),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
        // A short group or a non-hex digit is no GUID.
        for other in [
            "01234567-89ab-cdef-456789abcdef",
            "01234g67-89ab-cdef-0123-456789abcdef",
        ] {
            assert!(!matches!(first(other).0, Some(Literal::Guid(_))), "{other}");
        }
    }

    #[test]
    fn a_literal_is_bounded() {
        let limits = Limits {
            max_literal_len: 4,
            ..Limits::strict()
        };
        let mut c = Cursor::new("'abc'");
        assert_eq!(
            literal(&mut c, &limits).unwrap_err().kind,
            crate::UrlErrorKind::LimitExceeded {
                limit: LimitKind::LiteralLength,
                max: 4
            }
        );
        let mut c = Cursor::new("'ab'");
        assert!(literal(&mut c, &limits).unwrap().is_some());
    }
}
