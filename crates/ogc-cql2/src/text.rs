// SPDX-License-Identifier: EUPL-1.2
//! `cql2-text` (OGC 21-065r2 annex B): a tokenizer, then one function per precedence level.
//!
//! ```text
//! or        = and {OR and}
//! and       = not {AND not}
//! not       = NOT not | predicate
//! predicate = arith [compare arith | [NOT] LIKE arith | [NOT] BETWEEN arith AND arith
//!                   | [NOT] IN '(' list ')' | IS [NOT] NULL]
//! arith     = term {(+|-) term};  term = power {(*|/|%|div) power};  power = unary [^ unary]
//! unary     = - unary | primary
//! primary   = literal | property | call | '(' or ')' | '(' list ')'
//! ```

use crate::{
    apply, instant, interval_end, is_date, is_timestamp, Budget, Error, Expr, Geometry, Op,
    Position,
};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A bare identifier or keyword.
    Word(String),
    /// A double-quoted identifier.
    Quoted(String),
    /// A single-quoted string.
    Str(String),
    /// An unsigned number.
    Num(f64),
    /// Punctuation and comparison operators.
    Sym(&'static str),
}

pub(crate) fn starts_identifier(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':'
}

pub(crate) fn continues_identifier(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == ':' || c == '.'
}

/// `(token, byte offset)` pairs.
fn tokens(input: &str) -> Result<Vec<(Tok, usize)>, Error> {
    const SYMS: &[&str] = &[
        "<>", "<=", ">=", "(", ")", ",", "=", "<", ">", "+", "-", "*", "/", "%", "^",
    ];
    let mut out = Vec::new();
    let mut rest = input.char_indices().peekable();
    while let Some(&(at, c)) = rest.peek() {
        let tail = input.get(at..).unwrap_or_default();
        if c.is_whitespace() {
            rest.next();
            continue;
        }
        let tok = if c == '\'' || c == '"' {
            rest.next();
            let mut text = String::new();
            loop {
                match rest.next() {
                    Some((_, q)) if q == c => {
                        if rest.next_if(|&(_, n)| n == c).is_none() {
                            break;
                        }
                        text.push(c);
                    }
                    Some((_, other)) => text.push(other),
                    None => return Err(Error::new(at, "an unterminated quote")),
                }
            }
            if c == '"' {
                if text.is_empty() {
                    return Err(Error::new(at, "an empty identifier"));
                }
                Tok::Quoted(text)
            } else {
                Tok::Str(text)
            }
        } else if c.is_ascii_digit()
            || (c == '.' && tail.chars().nth(1).is_some_and(|d| d.is_ascii_digit()))
        {
            let len = number_len(tail);
            let text = tail.get(..len).unwrap_or_default();
            let n = text
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or(Error::new(at, "a number out of range"))?;
            for _ in text.chars() {
                rest.next();
            }
            Tok::Num(n)
        } else if starts_identifier(c) {
            let mut word = String::new();
            while let Some((_, ch)) = rest.next_if(|&(_, ch)| continues_identifier(ch)) {
                word.push(ch);
            }
            Tok::Word(word)
        } else if let Some(sym) = SYMS.iter().find(|s| tail.starts_with(**s)) {
            for _ in sym.chars() {
                rest.next();
            }
            Tok::Sym(sym)
        } else {
            return Err(Error::new(at, "an unexpected character"));
        };
        out.push((tok, at));
    }
    Ok(out)
}

/// The length of the number at the start of `text`: `digits [. digits] [e [+-] digits]`.
fn number_len(text: &str) -> usize {
    let b = text.as_bytes();
    let run = |from: usize| {
        from.saturating_add(
            b.iter()
                .skip(from)
                .take_while(|c| c.is_ascii_digit())
                .count(),
        )
    };
    let mut end = run(0);
    if b.get(end) == Some(&b'.') {
        end = run(end.saturating_add(1));
    }
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(end.saturating_add(1)), Some(b'+' | b'-')));
        let exp = end.saturating_add(1).saturating_add(sign);
        if b.get(exp).is_some_and(u8::is_ascii_digit) {
            end = run(exp);
        }
    }
    end
}

pub(crate) fn parse(input: &str) -> Result<Expr, Error> {
    let mut p = Parser {
        toks: tokens(input)?,
        pos: 0,
        end: input.len(),
        budget: Budget::new(),
    };
    let expr = p.or()?;
    match p.toks.get(p.pos) {
        None => Ok(expr),
        Some(_) => Err(p.error("the end of the filter")),
    }
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    end: usize,
    budget: Budget,
}

const COMPARISONS: &[(&str, Op)] = &[
    ("=", Op::Eq),
    ("<>", Op::Ne),
    ("<", Op::Lt),
    ("<=", Op::Le),
    (">", Op::Gt),
    (">=", Op::Ge),
];

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn at(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |(_, at)| *at)
    }

    fn error(&self, what: &'static str) -> Error {
        Error::new(self.at(), what)
    }

    fn bump(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).map(|(t, _)| t.clone());
        self.pos = self.pos.saturating_add(1);
        tok
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case(word))
    }

    fn take_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        if found {
            self.bump();
        }
        found
    }

    fn take_sym(&mut self, sym: &str) -> bool {
        let found = matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym);
        if found {
            self.bump();
        }
        found
    }

    fn expect_sym(&mut self, sym: &str, what: &'static str) -> Result<(), Error> {
        if self.take_sym(sym) {
            Ok(())
        } else {
            Err(self.error(what))
        }
    }

    fn node(&mut self, expr: Expr) -> Result<Expr, Error> {
        self.budget.node(self.at())?;
        Ok(expr)
    }

    fn op(&mut self, op: Op, args: Vec<Expr>, at: usize) -> Result<Expr, Error> {
        self.budget.node(at)?;
        apply(op, args, at)
    }

    fn or(&mut self) -> Result<Expr, Error> {
        self.chain("OR", Op::Or, Self::and)
    }

    fn and(&mut self) -> Result<Expr, Error> {
        self.chain("AND", Op::And, Self::not)
    }

    fn chain(
        &mut self,
        word: &str,
        op: Op,
        next: fn(&mut Self) -> Result<Expr, Error>,
    ) -> Result<Expr, Error> {
        let at = self.at();
        let first = next(self)?;
        if !self.is_word(word) {
            return Ok(first);
        }
        let mut args = vec![first];
        while self.take_word(word) {
            args.push(next(self)?);
        }
        self.op(op, args, at)
    }

    fn not(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        if !self.take_word("NOT") {
            return self.predicate();
        }
        self.budget.enter(at)?;
        let inner = self.not()?;
        self.budget.leave();
        self.op(Op::Not, vec![inner], at)
    }

    fn predicate(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        let left = self.arith()?;
        if let Some(Tok::Sym(sym)) = self.peek() {
            if let Some((_, op)) = COMPARISONS.iter().find(|(s, _)| s == sym) {
                let op = op.clone();
                self.bump();
                let right = self.arith()?;
                return self.op(op, vec![left, right], at);
            }
        }
        if self.take_word("IS") {
            let negated = self.take_word("NOT");
            if !self.take_word("NULL") {
                return Err(self.error("NULL"));
            }
            let test = self.op(Op::IsNull, vec![left], at)?;
            return self.negated(test, negated, at);
        }
        let negated = self.take_word("NOT");
        let test = if self.take_word("LIKE") {
            let pattern = self.arith()?;
            self.op(Op::Like, vec![left, pattern], at)?
        } else if self.take_word("BETWEEN") {
            let low = self.arith()?;
            if !self.take_word("AND") {
                return Err(self.error("AND of BETWEEN"));
            }
            let high = self.arith()?;
            self.op(Op::Between, vec![left, low, high], at)?
        } else if self.take_word("IN") {
            let list = self.list()?;
            self.op(Op::In, vec![left, list], at)?
        } else if negated {
            return Err(self.error("LIKE, BETWEEN or IN after NOT"));
        } else {
            return Ok(left);
        };
        self.negated(test, negated, at)
    }

    fn negated(&mut self, test: Expr, negated: bool, at: usize) -> Result<Expr, Error> {
        if negated {
            self.op(Op::Not, vec![test], at)
        } else {
            Ok(test)
        }
    }

    fn arith(&mut self) -> Result<Expr, Error> {
        self.binary(&[("+", Op::Add), ("-", Op::Sub)], Self::term)
    }

    fn term(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        let mut left = self.power()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("*")) => Op::Mul,
                Some(Tok::Sym("/")) => Op::Div,
                Some(Tok::Sym("%")) => Op::Mod,
                Some(Tok::Word(w)) if w.eq_ignore_ascii_case("div") => Op::IntDiv,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.power()?;
            left = self.op(op, vec![left, right], at)?;
        }
    }

    fn power(&mut self) -> Result<Expr, Error> {
        self.binary(&[("^", Op::Pow)], Self::unary)
    }

    fn binary(
        &mut self,
        ops: &[(&str, Op)],
        next: fn(&mut Self) -> Result<Expr, Error>,
    ) -> Result<Expr, Error> {
        let at = self.at();
        let mut left = next(self)?;
        while let Some(Tok::Sym(sym)) = self.peek() {
            let Some((_, op)) = ops.iter().find(|(s, _)| s == sym) else {
                break;
            };
            let op = op.clone();
            self.bump();
            let right = next(self)?;
            left = self.op(op, vec![left, right], at)?;
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        if !self.take_sym("-") {
            return self.primary();
        }
        if let Some(Tok::Num(n)) = self.peek() {
            let n = -*n;
            self.bump();
            return self.node(Expr::Num(n));
        }
        self.budget.enter(at)?;
        let operand = self.unary()?;
        self.budget.leave();
        let minus_one = self.node(Expr::Num(-1.0))?;
        self.op(Op::Mul, vec![minus_one, operand], at)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        match self.bump() {
            Some(Tok::Num(n)) => self.node(Expr::Num(n)),
            Some(Tok::Str(s)) => self.node(Expr::Str(s)),
            Some(Tok::Quoted(name)) => self.node(Expr::Property(name)),
            Some(Tok::Sym("(")) => {
                self.pos = self.pos.saturating_sub(1);
                self.group()
            }
            Some(Tok::Word(word)) => self.word(word, at),
            _ => Err(Error::new(at, "an expression")),
        }
    }

    /// `(` or `)` as a group, `()` or `(a, b, ...)` as an array.
    fn group(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        self.expect_sym("(", "(")?;
        self.budget.enter(at)?;
        if self.take_sym(")") {
            self.budget.leave();
            return self.node(Expr::Array(Vec::new()));
        }
        let first = self.or()?;
        let expr = if self.take_sym(",") {
            let mut items = vec![first];
            loop {
                items.push(self.or()?);
                if !self.take_sym(",") {
                    break;
                }
            }
            self.node(Expr::Array(items))?
        } else {
            first
        };
        self.expect_sym(")", ")")?;
        self.budget.leave();
        Ok(expr)
    }

    /// `( a, b, ... )`, always an array (`IN`, and arrays in `A_*` calls).
    fn list(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        self.expect_sym("(", "( of a list")?;
        self.budget.enter(at)?;
        let mut items = Vec::new();
        if !self.take_sym(")") {
            loop {
                items.push(self.or()?);
                if !self.take_sym(",") {
                    break;
                }
            }
            self.expect_sym(")", ") of a list")?;
        }
        self.budget.leave();
        self.node(Expr::Array(items))
    }

    fn word(&mut self, word: String, at: usize) -> Result<Expr, Error> {
        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            "TRUE" | "FALSE" => return self.node(Expr::Bool(upper == "TRUE")),
            "AND" | "OR" | "NOT" | "LIKE" | "BETWEEN" | "IN" | "IS" | "NULL" | "DIV" => {
                return Err(Error::new(at, "an expression"));
            }
            _ => {}
        }
        if self.peek() != Some(&Tok::Sym("(")) {
            return self.node(Expr::Property(word));
        }
        match upper.as_str() {
            "TIMESTAMP" | "DATE" => {
                self.expect_sym("(", "(")?;
                let at = self.at();
                let Some(Tok::Str(text)) = self.bump() else {
                    return Err(Error::new(at, "a quoted instant"));
                };
                self.expect_sym(")", ")")?;
                let instant = match upper.as_str() {
                    "DATE" if is_date(&text) => Expr::Date(text),
                    "TIMESTAMP" if is_timestamp(&text) => Expr::Timestamp(text),
                    _ => return Err(Error::new(at, "a date YYYY-MM-DD or a UTC timestamp")),
                };
                self.node(instant)
            }
            "INTERVAL" => {
                self.expect_sym("(", "(")?;
                self.budget.enter(at)?;
                let start = self.interval_end()?;
                self.expect_sym(",", ", of an interval")?;
                let end = self.interval_end()?;
                self.expect_sym(")", ")")?;
                self.budget.leave();
                self.node(Expr::Interval(Box::new(start), Box::new(end)))
            }
            "BBOX" => {
                self.expect_sym("(", "(")?;
                let mut corners = vec![self.signed()?];
                while self.take_sym(",") {
                    corners.push(self.signed()?);
                }
                self.expect_sym(")", ")")?;
                if corners.len() != 4 && corners.len() != 6 {
                    return Err(Error::new(at, "a BBOX of 4 or 6 numbers"));
                }
                self.node(Expr::BBox(corners))
            }
            "POINT" | "LINESTRING" | "POLYGON" | "MULTIPOINT" | "MULTILINESTRING"
            | "MULTIPOLYGON" | "GEOMETRYCOLLECTION" => {
                let g = self.geometry(&upper, at)?;
                self.node(Expr::Geometry(g))
            }
            _ => {
                let op = Op::from_text_call(&word);
                let arrays = op.name().starts_with("a_");
                self.expect_sym("(", "(")?;
                self.budget.enter(at)?;
                let mut args = Vec::new();
                if !self.take_sym(")") {
                    loop {
                        let arg = if arrays && self.peek() == Some(&Tok::Sym("(")) {
                            self.list()?
                        } else {
                            self.or()?
                        };
                        args.push(arg);
                        if !self.take_sym(",") {
                            break;
                        }
                    }
                    self.expect_sym(")", ") of a call")?;
                }
                self.budget.leave();
                self.op(op, args, at)
            }
        }
    }

    fn interval_end(&mut self) -> Result<Expr, Error> {
        let at = self.at();
        if let Some(Tok::Str(text)) = self.peek() {
            let text = text.clone();
            self.bump();
            return self.node(instant(&text, at)?);
        }
        let end = self.arith()?;
        interval_end(end, at)
    }

    fn signed(&mut self) -> Result<f64, Error> {
        let at = self.at();
        let negative = self.take_sym("-");
        match self.bump() {
            Some(Tok::Num(n)) => Ok(if negative { -n } else { n }),
            _ => Err(Error::new(at, "a number")),
        }
    }

    /// `x y [z]`
    fn position(&mut self) -> Result<Position, Error> {
        self.budget.node(self.at())?;
        let mut p = vec![self.signed()?, self.signed()?];
        if matches!(self.peek(), Some(Tok::Num(_) | Tok::Sym("-"))) {
            p.push(self.signed()?);
        }
        Ok(p)
    }

    /// `( item, item, ... )`
    fn many<T>(&mut self, item: fn(&mut Self) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let at = self.at();
        self.expect_sym("(", "( of a geometry")?;
        self.budget.enter(at)?;
        let mut items = vec![item(self)?];
        while self.take_sym(",") {
            items.push(item(self)?);
        }
        self.expect_sym(")", ") of a geometry")?;
        self.budget.leave();
        Ok(items)
    }

    fn line(&mut self) -> Result<Vec<Position>, Error> {
        self.many(Self::position)
    }

    fn rings(&mut self) -> Result<Vec<Vec<Position>>, Error> {
        self.many(Self::line)
    }

    /// A position of a MULTIPOINT, bare or in its own parentheses.
    fn point(&mut self) -> Result<Position, Error> {
        if self.peek() == Some(&Tok::Sym("(")) {
            let at = self.at();
            let mut one = self.line()?;
            return match (one.pop(), one.is_empty()) {
                (Some(p), true) => Ok(p),
                _ => Err(Error::new(at, "one position")),
            };
        }
        self.position()
    }

    fn member(&mut self) -> Result<Geometry, Error> {
        let at = self.at();
        match self.bump() {
            Some(Tok::Word(w)) => self.geometry(&w.to_ascii_uppercase(), at),
            _ => Err(Error::new(at, "a geometry")),
        }
    }

    fn geometry(&mut self, kind: &str, at: usize) -> Result<Geometry, Error> {
        if self.is_word("Z") {
            self.bump();
        }
        Ok(match kind {
            "POINT" => {
                let mut one = self.line()?;
                match (one.pop(), one.is_empty()) {
                    (Some(p), true) => Geometry::Point(p),
                    _ => return Err(Error::new(at, "one position")),
                }
            }
            "LINESTRING" => Geometry::LineString(self.line()?),
            "POLYGON" => Geometry::Polygon(self.rings()?),
            "MULTIPOINT" => Geometry::MultiPoint(self.many(Self::point)?),
            "MULTILINESTRING" => Geometry::MultiLineString(self.rings()?),
            "MULTIPOLYGON" => Geometry::MultiPolygon(self.many(Self::rings)?),
            "GEOMETRYCOLLECTION" => Geometry::Collection(self.many(Self::member)?),
            _ => return Err(Error::new(at, "a geometry")),
        })
    }
}
