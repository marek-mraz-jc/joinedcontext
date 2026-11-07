// SPDX-License-Identifier: EUPL-1.2
//! `$filter` and `$orderby` over the resources a collection serves (T-5918): what no NGSI-LD
//! `q` or `orderBy` can say, such as an Observation's `result` or a Location's `name`, is judged
//! here on the SensorThings JSON the view already answered, so it reads only members the view
//! shows. The caller reads the collection whole first, at most [`MAX_SCAN`] resources.

use std::cmp::Ordering;

use serde_json::Value;

/// A query option this surface cannot honour, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bad(pub &'static str, pub String);

/// The most resources one request reads to filter or order a collection.
pub const MAX_SCAN: usize = 1000;

/// The navigation properties a member path may not start with: a related resource's member is
/// no member of this one, so a comparison on it would silently hold for none.
const NAVIGATION: [&str; 10] = [
    "Thing",
    "Things",
    "Location",
    "Locations",
    "HistoricalLocations",
    "Datastream",
    "Datastreams",
    "Sensor",
    "ObservedProperty",
    "Observations",
];

/// A `$filter` expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `a or b`.
    Or(Box<Expr>, Box<Expr>),
    /// `a and b`.
    And(Box<Expr>, Box<Expr>),
    /// `not a`.
    Not(Box<Expr>),
    /// `left op right`.
    Cmp(Operand, Op, Operand),
    /// `substringof('t', p)`, `startswith(p, 't')`, `endswith(p, 't')`.
    Text(Func, Operand, Operand),
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `eq`
    Eq,
    /// `ne`
    Ne,
    /// `gt`
    Gt,
    /// `ge`
    Ge,
    /// `lt`
    Lt,
    /// `le`
    Le,
}

/// A text function: the needle is the second operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    /// `substringof`
    Contains,
    /// `startswith`
    Starts,
    /// `endswith`
    Ends,
}

/// An operand: a literal or a member path.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A text literal.
    Text(String),
    /// A number.
    Number(f64),
    /// `true` or `false`.
    Bool(bool),
    /// `null`.
    Null,
    /// An RFC 3339 instant, in milliseconds since the epoch.
    Instant(i64),
    /// A member path, `/`-separated (`properties/owner`, `unitOfMeasurement/name`).
    Path(Vec<String>),
}

/// One token of a filter.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open,
    Close,
    Comma,
    Text(String),
    Word(String),
}

fn bad(detail: impl Into<String>) -> Bad {
    Bad("$filter", detail.into())
}

fn tokens(filter: &str) -> Result<Vec<Token>, Bad> {
    let mut out = Vec::new();
    let mut chars = filter.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '(' => out.push(Token::Open),
            ')' => out.push(Token::Close),
            ',' => out.push(Token::Comma),
            '\'' => {
                let mut text = String::new();
                loop {
                    match chars.next() {
                        Some('\'') if chars.next_if_eq(&'\'').is_some() => text.push('\''),
                        Some('\'') => break,
                        Some(c) => text.push(c),
                        None => return Err(bad("a text literal is not closed with '")),
                    }
                }
                out.push(Token::Text(text));
            }
            c => {
                let mut word = String::from(c);
                while let Some(n) = chars.next_if(|n| !n.is_whitespace() && !"(),'".contains(*n)) {
                    word.push(n);
                }
                out.push(Token::Word(word));
            }
        }
    }
    Ok(out)
}

/// A parser over the tokens, one expression deep per nesting level.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
}

/// The deepest nesting of parentheses and `not` a filter may hold.
const MAX_DEPTH: usize = 32;

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at = self.at.saturating_add(1);
        token
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.eq_ignore_ascii_case(word))
    }

    fn expect(&mut self, token: &Token, what: &str) -> Result<(), Bad> {
        if self.next().as_ref() == Some(token) {
            Ok(())
        } else {
            Err(bad(format!("expected {what}")))
        }
    }

    fn or(&mut self) -> Result<Expr, Bad> {
        let mut left = self.and()?;
        while self.keyword("or") {
            self.at = self.at.saturating_add(1);
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, Bad> {
        let mut left = self.unary()?;
        while self.keyword("and") {
            self.at = self.at.saturating_add(1);
            left = Expr::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn deeper(&mut self) -> Result<(), Bad> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_DEPTH {
            return Err(bad(format!("at most {MAX_DEPTH} nested levels")));
        }
        Ok(())
    }

    fn unary(&mut self) -> Result<Expr, Bad> {
        if self.keyword("not") {
            self.at = self.at.saturating_add(1);
            self.deeper()?;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.peek() == Some(&Token::Open) {
            self.at = self.at.saturating_add(1);
            self.deeper()?;
            let inner = self.or()?;
            self.expect(&Token::Close, ")")?;
            return Ok(inner);
        }
        let func = match self.peek() {
            Some(Token::Word(w))
                if self.tokens.get(self.at.saturating_add(1)) == Some(&Token::Open) =>
            {
                Some(w.to_ascii_lowercase())
            }
            _ => None,
        };
        if let Some(func) = func {
            return self.function(&func);
        }
        let left = self.operand()?;
        let op = match self.next() {
            Some(Token::Word(w)) => match w.to_ascii_lowercase().as_str() {
                "eq" => Op::Eq,
                "ne" => Op::Ne,
                "gt" => Op::Gt,
                "ge" => Op::Ge,
                "lt" => Op::Lt,
                "le" => Op::Le,
                other => return Err(bad(format!("{other} is not supported"))),
            },
            _ => return Err(bad("a comparison needs eq, ne, gt, ge, lt or le")),
        };
        Ok(Expr::Cmp(left, op, self.operand()?))
    }

    fn function(&mut self, name: &str) -> Result<Expr, Bad> {
        let func = match name {
            "substringof" => Func::Contains,
            "startswith" => Func::Starts,
            "endswith" => Func::Ends,
            other => return Err(bad(format!("{other}() is not supported"))),
        };
        self.at = self.at.saturating_add(2);
        let first = self.operand()?;
        self.expect(&Token::Comma, ",")?;
        let second = self.operand()?;
        self.expect(&Token::Close, ")")?;
        // substringof names the needle first.
        Ok(match func {
            Func::Contains => Expr::Text(func, second, first),
            _ => Expr::Text(func, first, second),
        })
    }

    fn operand(&mut self) -> Result<Operand, Bad> {
        match self.next() {
            Some(Token::Text(t)) => Ok(Operand::Text(t)),
            Some(Token::Word(w)) => word(&w),
            _ => Err(bad("expected a property or a value")),
        }
    }
}

/// An unquoted operand: `true`, `false`, `null`, a number, an instant or a member path.
fn word(w: &str) -> Result<Operand, Bad> {
    match w {
        "true" => return Ok(Operand::Bool(true)),
        "false" => return Ok(Operand::Bool(false)),
        "null" => return Ok(Operand::Null),
        _ => {}
    }
    if let Some(ms) = instant(w) {
        return Ok(Operand::Instant(ms));
    }
    if w.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
        return w
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(Operand::Number)
            .ok_or_else(|| {
                bad(format!(
                    "{w} is not a number, true, false, null or an instant such as 2026-01-01T00:00:00Z; quote a text"
                ))
            });
    }
    let path: Vec<String> = w
        .split('/')
        .map(|s| if s == "id" { "@iot.id" } else { s }.to_owned())
        .collect();
    let named = path.iter().all(|s| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_@.".contains(c))
    });
    if !named {
        return Err(bad(format!("{w} is not a property name")));
    }
    if path.len() > 1
        && path
            .first()
            .is_some_and(|s| NAVIGATION.contains(&s.as_str()))
    {
        return Err(bad(format!(
            "{w}: a related resource's property filters a top-level set only"
        )));
    }
    Ok(Operand::Path(path))
}

/// `$filter` as an expression.
///
/// # Errors
/// [`Bad`] naming the construct it cannot read.
pub fn parse(filter: &str) -> Result<Expr, Bad> {
    let mut parser = Parser {
        tokens: tokens(filter)?,
        at: 0,
        depth: 0,
    };
    if parser.tokens.is_empty() {
        return Err(bad("the filter is empty"));
    }
    let expr = parser.or()?;
    if parser.at < parser.tokens.len() {
        return Err(bad("unexpected text after the expression"));
    }
    Ok(expr)
}

/// The days from 1970-01-01 to a civil date (H. Hinnant's algorithm).
fn days(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 {
        year.saturating_sub(1)
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.saturating_sub(era.saturating_mul(400));
    let mp = month.saturating_add(9).rem_euclid(12);
    let doy = mp
        .saturating_mul(153)
        .saturating_add(2)
        .div_euclid(5)
        .saturating_add(day)
        .saturating_sub(1);
    let doe = yoe
        .saturating_mul(365)
        .saturating_add(yoe.div_euclid(4))
        .saturating_sub(yoe.div_euclid(100))
        .saturating_add(doy);
    era.saturating_mul(146_097)
        .saturating_add(doe)
        .saturating_sub(719_468)
}

/// An RFC 3339 instant (`2026-01-01T00:00:00Z`, `…00.5+02:00`) in milliseconds since the
/// epoch; the start of an interval `a/b`.
#[must_use]
pub fn instant(text: &str) -> Option<i64> {
    let text = text.split('/').next()?;
    let shape: String = text
        .chars()
        .take(19)
        .map(|c| if c.is_ascii_digit() { '9' } else { c })
        .collect();
    if shape != "9999-99-99T99:99:99" {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let clock = [
        (month, 1, 12),
        (day, 1, 31),
        (hour, 0, 23),
        (minute, 0, 59),
        (second, 0, 60),
    ];
    if clock.iter().any(|(v, lo, hi)| !(lo..=hi).contains(&v)) {
        return None;
    }
    let mut rest = text.get(19..)?;
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let ms: String = fraction.chars().take(digits.min(3)).collect();
        millis = format!("{ms:0<3}").parse().ok()?;
        rest = fraction.get(digits..)?;
    }
    let offset = match rest {
        "Z" | "z" => 0,
        zone => {
            let east = match zone.as_bytes().first() {
                Some(b'+') => true,
                Some(b'-') => false,
                _ => return None,
            };
            if zone.len() != 6 || zone.as_bytes().get(3) != Some(&b':') {
                return None;
            }
            let digits = |r: std::ops::Range<usize>| -> Option<i64> { zone.get(r)?.parse().ok() };
            let (h, m) = (digits(1..3)?, digits(4..6)?);
            let minutes = h.saturating_mul(60).saturating_add(m);
            if east {
                minutes
            } else {
                minutes.saturating_neg()
            }
        }
    };
    let minutes = days(year, month, day)
        .saturating_mul(1440)
        .saturating_add(hour.saturating_mul(60))
        .saturating_add(minute)
        .saturating_sub(offset);
    Some(
        minutes
            .saturating_mul(60)
            .saturating_add(second)
            .saturating_mul(1000)
            .saturating_add(millis),
    )
}

/// The member at `path`, `None` when absent.
fn member<'a>(item: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(item, |v, k| v.get(k))
}

/// An operand's value on `item`, a literal as JSON.
fn value(item: &Value, operand: &Operand) -> Value {
    match operand {
        Operand::Path(p) => member(item, p).cloned().unwrap_or(Value::Null),
        Operand::Text(t) => Value::String(t.clone()),
        Operand::Number(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
        Operand::Bool(b) => Value::Bool(*b),
        Operand::Null => Value::Null,
        Operand::Instant(ms) => Value::from(*ms),
    }
}

/// Two values in order, `None` when they are not comparable: numbers by value, texts by code
/// point, a number and a text as texts (`result gt '3'`), booleans `false` first; an instant
/// literal against a member reads the member as an instant.
fn order(left: &Value, right: &Value, instants: bool) -> Option<Ordering> {
    if instants {
        let ms = |v: &Value| v.as_i64().or_else(|| v.as_str().and_then(instant));
        return Some(ms(left)?.cmp(&ms(right)?));
    }
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Number(a), Value::String(b)) => Some(a.to_string().as_str().cmp(b.as_str())),
        (Value::String(a), Value::Number(b)) => Some(a.as_str().cmp(b.to_string().as_str())),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn is_instant(operand: &Operand) -> bool {
    matches!(operand, Operand::Instant(_))
}

/// Whether `item` matches `expr`. A comparison with `null` holds for `eq` when both are null and
/// for `ne` when one is; any other comparison with a missing member holds for none.
#[must_use]
pub fn matches(expr: &Expr, item: &Value) -> bool {
    match expr {
        Expr::Or(a, b) => matches(a, item) || matches(b, item),
        Expr::And(a, b) => matches(a, item) && matches(b, item),
        Expr::Not(a) => !matches(a, item),
        Expr::Text(func, hay, needle) => {
            let (Value::String(h), Value::String(n)) = (value(item, hay), value(item, needle))
            else {
                return false;
            };
            match func {
                Func::Contains => h.contains(&n),
                Func::Starts => h.starts_with(&n),
                Func::Ends => h.ends_with(&n),
            }
        }
        Expr::Cmp(left, op, right) => {
            let (a, b) = (value(item, left), value(item, right));
            if a.is_null() || b.is_null() {
                let same = a.is_null() && b.is_null();
                return match op {
                    Op::Eq => same,
                    Op::Ne => !same,
                    _ => false,
                };
            }
            let ordering = if a.is_object() || a.is_array() || b.is_object() || b.is_array() {
                (a == b).then_some(Ordering::Equal)
            } else {
                order(&a, &b, is_instant(left) || is_instant(right))
            };
            match (op, ordering) {
                (Op::Ne, None) => true,
                (_, None) => false,
                (Op::Eq, Some(ord)) => ord.is_eq(),
                (Op::Ne, Some(ord)) => ord.is_ne(),
                (Op::Gt, Some(ord)) => ord.is_gt(),
                (Op::Ge, Some(ord)) => ord.is_ge(),
                (Op::Lt, Some(ord)) => ord.is_lt(),
                (Op::Le, Some(ord)) => ord.is_le(),
            }
        }
    }
}

/// The first member of each path `expr` reads.
#[must_use]
pub fn heads(expr: &Expr) -> Vec<&str> {
    fn walk<'a>(expr: &'a Expr, out: &mut Vec<&'a str>) {
        match expr {
            Expr::Or(a, b) | Expr::And(a, b) => {
                walk(a, out);
                walk(b, out);
            }
            Expr::Not(a) => walk(a, out),
            Expr::Cmp(l, _, r) | Expr::Text(_, l, r) => {
                for operand in [l, r] {
                    if let Operand::Path(p) = operand {
                        out.extend(p.first().map(String::as_str));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

/// `$orderby` as member paths, each with `true` for `desc`.
///
/// # Errors
/// [`Bad`] naming an item that is no member path with an optional `asc` or `desc`.
pub fn orderby(value: &str) -> Result<Vec<(Vec<String>, bool)>, Bad> {
    let usage = |item: &str| {
        Bad(
            "$orderby",
            format!("{item} is not a property followed by asc or desc"),
        )
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let mut words = item.split_whitespace();
            let name = words.next().ok_or_else(|| usage(item))?;
            let desc = match words.next().map(str::to_ascii_lowercase).as_deref() {
                None | Some("asc") => false,
                Some("desc") => true,
                Some(_) => return Err(usage(item)),
            };
            if words.next().is_some() {
                return Err(usage(item));
            }
            match word(name) {
                Ok(Operand::Path(p)) => Ok((p, desc)),
                _ => Err(usage(item)),
            }
        })
        .collect()
}

/// Two members in `$orderby` order: null first, then by [`order`]; objects and arrays and values
/// of two kinds by their JSON text.
fn rank(a: Option<&Value>, b: Option<&Value>) -> Ordering {
    let (a, b) = (a.unwrap_or(&Value::Null), b.unwrap_or(&Value::Null));
    match (a.is_null(), b.is_null()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    order(a, b, false).unwrap_or_else(|| a.to_string().cmp(&b.to_string()))
}

/// `items` in `$orderby` order, stable for ties.
pub fn sort(items: &mut [Value], keys: &[(Vec<String>, bool)]) {
    items.sort_by(|a, b| {
        keys.iter()
            .map(|(path, desc)| {
                let o = rank(member(a, path), member(b, path));
                if *desc {
                    o.reverse()
                } else {
                    o
                }
            })
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal)
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn kept(filter: &str, items: &[Value]) -> Result<Vec<i64>, Bad> {
        let expr = parse(filter)?;
        Ok(items
            .iter()
            .filter(|i| matches(&expr, i))
            .filter_map(|i| i.get("n").and_then(Value::as_i64))
            .collect())
    }

    fn observations() -> Vec<Value> {
        (1..=12)
            .map(|n| {
                json!({"n": n, "result": n,
                    "phenomenonTime": format!("2015-03-{n:02}T00:00:00.000Z"),
                    "name": format!("o {n}"), "properties": {"k": n % 2 == 0}, "tags": [n]})
            })
            .collect()
    }

    #[test]
    fn a_comparison_reads_each_member_kind() {
        let o = observations();
        let none: Vec<i64> = Vec::new();
        let every: Vec<i64> = (1..=12).collect();
        for (filter, want) in [
            ("result gt 10", vec![11, 12]),
            // a number against a text compares as text (ets-sta10 Capability3Tests)
            ("result gt '3'", vec![4, 5, 6, 7, 8, 9]),
            ("'5' gt result", vec![1, 2, 3, 4, 10, 11, 12]),
            ("result le +2", vec![1, 2]),
            ("result lt .5e1", vec![1, 2, 3, 4]),
            ("result lt -1", none.clone()),
            ("phenomenonTime le 2015-03-02T00:00:00Z", vec![1, 2]),
            ("phenomenonTime eq 2015-03-02T01:00:00+01:00", vec![2]),
            ("2015-03-11T00:00:00Z lt phenomenonTime", vec![12]),
            ("name eq 'o 3' or (name eq 'o 4')", vec![3, 4]),
            ("properties/k eq true and result lt 5", vec![2, 4]),
            ("properties/k eq false and result lt 5", vec![1, 3]),
            ("properties/k eq 1", none.clone()),
            ("properties/k ne 1 and result le 1", vec![1]),
            ("result ne 1 and result lt 3", vec![2]),
            ("not (result ge 2)", vec![1]),
            ("substringof('o 1', name)", vec![1, 10, 11, 12]),
            ("startswith(name,'o 1') and endswith(name, '2')", vec![12]),
            ("substringof('1', result)", none.clone()),
            ("missing eq null and result le 1", vec![1]),
            ("null eq missing and result le 1", vec![1]),
            ("missing eq 'x'", none.clone()),
            ("name eq null", none.clone()),
            ("missing ne 'x' and result le 1", vec![1]),
            ("missing gt 'x'", none.clone()),
            ("properties eq properties", every.clone()),
            ("tags eq tags", every.clone()),
            ("name eq properties", none.clone()),
            ("name ne tags", every.clone()),
        ] {
            assert_eq!(kept(filter, &o), Ok(want), "{filter}");
        }
    }

    #[test]
    fn heads_are_the_first_member_of_each_path() {
        let expr = parse("not (a/b eq 1) or startswith(c, 'x') and 2 eq d");
        assert_eq!(expr.as_ref().map(heads), Ok(vec!["a", "c", "d"]));
    }

    #[test]
    fn a_text_literal_keeps_its_doubled_quote_and_spaces() {
        let items = [
            json!({"n": 1, "name": "it's (a) b"}),
            json!({"n": 2, "name": "x"}),
        ];
        assert_eq!(kept("name eq 'it''s (a) b'", &items), Ok(vec![1]));
    }

    #[test]
    fn a_filter_it_cannot_read_is_named() {
        for (filter, says) in [
            ("", "empty"),
            ("name eq 'a", "not closed"),
            ("name has 'a'", "has is not supported"),
            ("length(name) eq 1", "length() is not supported"),
            ("Thing/name eq 'a'", "top-level set only"),
            ("name eq 08x", "not a number"),
            ("na;me eq 1", "not a property name"),
            ("(name eq 'a'", "expected )"),
            ("name eq 'a' 'b'", "unexpected text"),
            ("name", "needs eq"),
            ("a//b eq 1", "not a property name"),
            ("startswith(name 'a')", "expected ,"),
            ("eq eq", "expected a property"),
        ] {
            let refused = parse(filter).err().unwrap_or(Bad("none", String::new()));
            assert_eq!(refused.0, "$filter", "{filter}");
            assert!(refused.1.contains(says), "{filter}: {}", refused.1);
        }
        let deep = format!("{}name eq 'a'{}", "(".repeat(40), ")".repeat(40));
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn an_order_sorts_by_each_key_nulls_first() {
        let mut items = vec![
            json!({"n": 1, "a": "b", "b": 2}),
            json!({"n": 2, "a": "a", "b": 1}),
            json!({"n": 3, "b": 9}),
            json!({"n": 4, "a": "b", "b": 1}),
        ];
        sort(&mut items, &orderby("a, b desc").unwrap_or_default());
        let order: Vec<i64> = items.iter().filter_map(|i| i["n"].as_i64()).collect();
        assert_eq!(order, vec![3, 2, 1, 4]);
        sort(&mut items, &orderby("a desc,b asc").unwrap_or_default());
        let order: Vec<i64> = items.iter().filter_map(|i| i["n"].as_i64()).collect();
        assert_eq!(order, vec![4, 1, 2, 3]);
        assert!(matches!(orderby("a sideways"), Err(Bad("$orderby", _))));
        assert!(orderby("a asc desc").is_err());
        assert!(orderby("'a'").is_err());
        let mut missing = vec![json!({"n": 1}), json!({"n": 2, "a": 1}), json!({"n": 3})];
        sort(&mut missing, &orderby("a desc").unwrap_or_default());
        let order: Vec<i64> = missing
            .iter()
            .filter_map(|i| i.get("n").and_then(Value::as_i64))
            .collect();
        assert_eq!(order, vec![2, 1, 3]);
    }

    #[test]
    fn an_instant_reads_offsets_fractions_and_interval_starts() {
        assert_eq!(instant("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(instant("1970-01-01T01:00:00.5+01:00"), Some(500));
        assert_eq!(
            instant("2015-03-01T00:00:00Z/2015-03-02T00:00:00Z"),
            instant("2015-03-01T00:00:00.000Z")
        );
        assert_eq!(instant("1970-01-01T00:00:00-01:00"), Some(3_600_000));
        for bad in [
            "2015-13-01T00:00:00Z",
            "2015-03-32T00:00:00Z",
            "2015-03-01T24:00:00Z",
            "2015-03-01T23:60:00Z",
            "2015-03-01T23:59:61Z",
            "2015-03-01T00:00:00+0100",
            "2015-03-01T00:00:00x01:00",
            "2015-03-01T00:00:00+01-00",
        ] {
            assert_eq!(instant(bad), None, "{bad}");
        }
        assert_eq!(instant("2015-03-01 00:00:00Z"), None);
        assert_eq!(instant("2015-03-01T00:00:00"), None);
        assert_eq!(instant("2015-03-01T00:00:00.Z"), None);
    }
}
