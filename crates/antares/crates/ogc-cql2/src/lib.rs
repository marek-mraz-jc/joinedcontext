// SPDX-License-Identifier: EUPL-1.2
//! OGC Common Query Language CQL2 1.0 (OGC 21-065r2) to one AST, from either encoding
//! (ADR-0029 decision 2).
//!
//! [`parse`] reads `cql2-text` (a hand-written recursive descent over the BNF of annex B) or
//! `cql2-json` (the JSON schema of annex C) into the same [`Expr`]: the JSON encoding's
//! `{"op": .., "args": [..]}` is the tree's own shape, so a text filter and its JSON form
//! compare equal. Every conformance class of the standard is read; nothing is evaluated and
//! nothing knows NGSI-LD, so deciding what a backend can run is the caller's job.
//!
//! Input is bounded before it can cost: at most [`MAX_LEN`] bytes, [`MAX_DEPTH`] nested
//! expressions and [`MAX_NODES`] tree nodes (geometry positions included); past any bound the
//! parse stops with an [`Error`] instead of recursing or allocating further.
//!
//! [`Expr`] writes itself as `cql2-text` ([`std::fmt::Display`]), which parses to the same tree,
//! but for a one-element array outside `IN` and the `A_*` functions (a `cql2-json` argument
//! `[1]`): `cql2-text` writes it `(1)`, the same text as a parenthesised value.
//!
//! ```
//! use ogc_cql2::{parse, Expr, Lang, Op};
//!
//! let text = parse(Lang::Text, "pm10 > 50 AND name LIKE 'Sta%'")?;
//! let json = parse(
//!     Lang::Json,
//!     r#"{"op": "and", "args": [
//!         {"op": ">", "args": [{"property": "pm10"}, 50]},
//!         {"op": "like", "args": [{"property": "name"}, "Sta%"]}]}"#,
//! )?;
//! assert_eq!(text, json);
//! assert!(matches!(text, Expr::Op(Op::And, _)));
//! assert_eq!(text.to_string(), "(pm10 > 50) AND (name LIKE 'Sta%')");
//! # Ok::<(), ogc_cql2::Error>(())
//! ```

mod json;
mod text;

use std::fmt;

/// The largest filter, in bytes, either encoding.
pub const MAX_LEN: usize = 16 * 1024;
/// The deepest nesting of expressions (parentheses, operands, function arguments).
pub const MAX_DEPTH: usize = 32;
/// The most tree nodes one filter yields, geometry positions included.
pub const MAX_NODES: usize = 4096;

/// The filter encodings of OGC API - Features Part 3 (`filter-lang`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// `cql2-text`, the default.
    Text,
    /// `cql2-json`.
    Json,
}

impl Lang {
    /// The encoding a `filter-lang` value names, if it names one.
    #[must_use]
    pub fn from_param(value: &str) -> Option<Self> {
        match value {
            "cql2-text" => Some(Self::Text),
            "cql2-json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Parses a filter of the given encoding.
///
/// # Errors
/// [`Error`] when the input is not CQL2 of that encoding, is not a boolean expression, or
/// exceeds [`MAX_LEN`], [`MAX_DEPTH`] or [`MAX_NODES`].
pub fn parse(lang: Lang, input: &str) -> Result<Expr, Error> {
    if input.len() > MAX_LEN {
        return Err(Error::new(MAX_LEN, "the filter is longer than the limit"));
    }
    let expr = match lang {
        Lang::Text => text::parse(input)?,
        Lang::Json => json::parse(input)?,
    };
    if expr.is_boolean() {
        Ok(expr)
    } else {
        Err(Error::new(0, "a filter is a boolean expression"))
    }
}

/// Why a filter was refused, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Byte offset in the input; 0 for a `cql2-json` filter whose JSON parsed but whose
    /// structure is not CQL2.
    pub at: usize,
    /// What was expected or exceeded.
    pub what: &'static str,
}

impl Error {
    fn new(at: usize, what: &'static str) -> Self {
        Self { at, what }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CQL2 error at {}: {}", self.at, self.what)
    }
}

impl std::error::Error for Error {}

/// A CQL2 expression: the `cql2-json` shape, read from either encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `{"property": name}`; in text a bare or double-quoted identifier.
    Property(String),
    /// A character string.
    Str(String),
    /// A number (always finite).
    Num(f64),
    /// `TRUE` / `FALSE`.
    Bool(bool),
    /// `TIMESTAMP('…Z')` / `{"timestamp": …}`: an RFC 3339 UTC instant, as written.
    Timestamp(String),
    /// `DATE('YYYY-MM-DD')` / `{"date": …}`.
    Date(String),
    /// `INTERVAL(a, b)` / `{"interval": [a, b]}`; each end a [`Expr::Timestamp`],
    /// [`Expr::Date`], [`Expr::Unbounded`], a property or a function.
    Interval(Box<Expr>, Box<Expr>),
    /// `'..'`, the open end of an interval.
    Unbounded,
    /// A geometry literal: WKT in text, `GeoJSON` in JSON.
    Geometry(Geometry),
    /// `BBOX(…)` / `{"bbox": […]}`: 4 or 6 numbers.
    BBox(Vec<f64>),
    /// An array: `( … )` in text, `[ … ]` in JSON.
    Array(Vec<Expr>),
    /// An operator or function applied to its arguments.
    Op(Op, Vec<Expr>),
}

impl Expr {
    /// Whether the expression yields a boolean, which a filter's root must.
    #[must_use]
    pub fn is_boolean(&self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Op(op, _) => op.is_boolean(),
            _ => false,
        }
    }
}

/// A position: 2 or 3 coordinates.
pub type Position = Vec<f64>;

/// The geometry literals of CQL2 (WKT / `GeoJSON`), coordinates as written.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// `POINT`
    Point(Position),
    /// `LINESTRING`
    LineString(Vec<Position>),
    /// `POLYGON`: rings, the first the exterior.
    Polygon(Vec<Vec<Position>>),
    /// `MULTIPOINT`
    MultiPoint(Vec<Position>),
    /// `MULTILINESTRING`
    MultiLineString(Vec<Vec<Position>>),
    /// `MULTIPOLYGON`
    MultiPolygon(Vec<Vec<Vec<Position>>>),
    /// `GEOMETRYCOLLECTION`
    Collection(Vec<Geometry>),
}

/// Declares [`Op`] with its `cql2-json` names, so the enum and the name table cannot drift.
macro_rules! ops {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Every operator and function name of CQL2, plus [`Op::Function`] for any other.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum Op {
            $(#[doc = concat!("`", $name, "`")] $variant,)*
            /// A function the standard does not define, by the name the filter gave it.
            Function(String),
        }

        impl Op {
            /// The `cql2-json` name (`op` member).
            #[must_use]
            pub fn name(&self) -> &str {
                match self {
                    $(Self::$variant => $name,)*
                    Self::Function(name) => name,
                }
            }

            /// The operator a `cql2-json` `op` member names; any other name is a [`Op::Function`].
            #[must_use]
            pub fn from_name(name: &str) -> Self {
                match name {
                    $($name => Self::$variant,)*
                    _ => Self::Function(name.to_owned()),
                }
            }
        }
    };
}

ops! {
    And => "and",
    Or => "or",
    Not => "not",
    Eq => "=",
    Ne => "<>",
    Lt => "<",
    Le => "<=",
    Gt => ">",
    Ge => ">=",
    Like => "like",
    Between => "between",
    In => "in",
    IsNull => "isNull",
    Casei => "casei",
    Accenti => "accenti",
    SIntersects => "s_intersects",
    SEquals => "s_equals",
    SDisjoint => "s_disjoint",
    STouches => "s_touches",
    SWithin => "s_within",
    SOverlaps => "s_overlaps",
    SCrosses => "s_crosses",
    SContains => "s_contains",
    TAfter => "t_after",
    TBefore => "t_before",
    TContains => "t_contains",
    TDisjoint => "t_disjoint",
    TDuring => "t_during",
    TEquals => "t_equals",
    TFinishedBy => "t_finishedBy",
    TFinishes => "t_finishes",
    TIntersects => "t_intersects",
    TMeets => "t_meets",
    TMetBy => "t_metBy",
    TOverlappedBy => "t_overlappedBy",
    TOverlaps => "t_overlaps",
    TStartedBy => "t_startedBy",
    TStarts => "t_starts",
    AEquals => "a_equals",
    AContains => "a_contains",
    AContainedBy => "a_containedBy",
    AOverlaps => "a_overlaps",
    Add => "+",
    Sub => "-",
    Mul => "*",
    Div => "/",
    Mod => "%",
    IntDiv => "div",
    Pow => "^",
}

impl Op {
    /// Whether the operator yields a boolean (a predicate), so it may stand as a filter.
    #[must_use]
    pub fn is_boolean(&self) -> bool {
        !matches!(
            self,
            Self::Casei
                | Self::Accenti
                | Self::Add
                | Self::Sub
                | Self::Mul
                | Self::Div
                | Self::Mod
                | Self::IntDiv
                | Self::Pow
        )
    }
}

impl Op {
    /// Whether text writes the operator as a call, `NAME(args)`.
    fn is_call(&self) -> bool {
        matches!(self, Self::Casei | Self::Accenti | Self::Function(_)) || self.name().contains('_')
    }

    /// The operator a text call names, in any case; any other name is a [`Op::Function`] as
    /// written.
    fn from_text_call(name: &str) -> Self {
        const CAMEL: &[&str] = &[
            "isNull",
            "t_finishedBy",
            "t_metBy",
            "t_overlappedBy",
            "t_startedBy",
            "a_containedBy",
        ];
        let lower = name.to_ascii_lowercase();
        let canonical = CAMEL
            .iter()
            .copied()
            .find(|c| c.eq_ignore_ascii_case(&lower))
            .unwrap_or(&lower);
        match Self::from_name(canonical) {
            Self::Function(_) => Self::Function(name.to_owned()),
            op => op,
        }
    }

    /// Whether `n` arguments fit the operator (the arities of the `cql2-json` schema).
    fn takes(&self, n: usize) -> bool {
        match self {
            Self::And | Self::Or => n >= 2,
            Self::Not | Self::IsNull | Self::Casei | Self::Accenti => n == 1,
            Self::Between => n == 3,
            Self::Function(_) => true,
            _ => n == 2,
        }
    }
}

/// Builds `Op(op, args)` after checking the arity (and `IN`'s list).
fn apply(op: Op, args: Vec<Expr>, at: usize) -> Result<Expr, Error> {
    if !op.takes(args.len()) {
        return Err(Error::new(at, "wrong number of arguments for the operator"));
    }
    if op == Op::In && !matches!(args.get(1), Some(Expr::Array(_))) {
        return Err(Error::new(at, "IN takes a list"));
    }
    // CQL2 `arrayExpression`: a property, a function or an array literal, which also keeps
    // `A_CONTAINS((a), ...)` from reading two ways in text.
    let array = |e: &Expr| {
        matches!(e, Expr::Property(_) | Expr::Array(_))
            || matches!(e, Expr::Op(o, _) if o.is_call())
    };
    if op.name().starts_with("a_") && !args.iter().all(array) {
        return Err(Error::new(
            at,
            "an array function takes properties, functions or arrays",
        ));
    }
    Ok(Expr::Op(op, args))
}

/// The depth and node bounds of one parse.
struct Budget {
    depth: usize,
    nodes: usize,
}

impl Budget {
    fn new() -> Self {
        Self { depth: 0, nodes: 0 }
    }

    fn enter(&mut self, at: usize) -> Result<(), Error> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_DEPTH {
            return Err(Error::new(at, "the filter nests deeper than the limit"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn node(&mut self, at: usize) -> Result<(), Error> {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_NODES {
            return Err(Error::new(at, "the filter has more nodes than the limit"));
        }
        Ok(())
    }
}

/// The value an interval end written as a string stands for.
fn instant(text: &str, at: usize) -> Result<Expr, Error> {
    if text == ".." {
        Ok(Expr::Unbounded)
    } else if is_date(text) {
        Ok(Expr::Date(text.to_owned()))
    } else if is_timestamp(text) {
        Ok(Expr::Timestamp(text.to_owned()))
    } else {
        Err(Error::new(
            at,
            "an interval end is a date, a timestamp or '..'",
        ))
    }
}

/// An interval end that is not a string: a property or a function.
fn interval_end(end: Expr, at: usize) -> Result<Expr, Error> {
    match end {
        Expr::Property(_) | Expr::Op(..) => Ok(end),
        _ => Err(Error::new(
            at,
            "an interval end is a date, a timestamp or '..'",
        )),
    }
}

/// The digits of `text` between `from` and `to` as a number, if they are all digits.
fn digits(text: &[u8], from: usize, to: usize) -> Option<u32> {
    let part = text.get(from..to)?;
    part.iter().try_fold(0u32, |n, b| {
        b.is_ascii_digit()
            .then(|| {
                n.checked_mul(10)?
                    .checked_add(u32::from(b.wrapping_sub(b'0')))
            })
            .flatten()
    })
}

/// `YYYY-MM-DD` (CQL2 `fullDate`).
fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.get(4) == Some(&b'-')
        && b.get(7) == Some(&b'-')
        && digits(b, 0, 4).is_some()
        && digits(b, 5, 7).is_some_and(|m| (1..=12).contains(&m))
        && digits(b, 8, 10).is_some_and(|d| (1..=31).contains(&d))
}

/// `YYYY-MM-DDThh:mm:ss[.fraction]Z` (CQL2 `timestamp`: UTC only).
fn is_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    let fraction = b.get(19..b.len().saturating_sub(1)).unwrap_or_default();
    text.get(..10).is_some_and(is_date)
        && b.get(10) == Some(&b'T')
        && b.get(13) == Some(&b':')
        && b.get(16) == Some(&b':')
        && digits(b, 11, 13).is_some_and(|h| h < 24)
        && digits(b, 14, 16).is_some_and(|m| m < 60)
        && digits(b, 17, 19).is_some_and(|s| s <= 60)
        && b.last() == Some(&b'Z')
        && (fraction.is_empty()
            || (fraction.len() > 1
                && fraction.first() == Some(&b'.')
                && fraction.iter().skip(1).all(u8::is_ascii_digit)))
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Property(name) => write_property(f, name),
            Self::Str(s) => write!(f, "'{}'", s.replace('\'', "''")),
            Self::Num(n) => write!(f, "{n}"),
            Self::Bool(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            Self::Timestamp(t) => write!(f, "TIMESTAMP('{t}')"),
            Self::Date(d) => write!(f, "DATE('{d}')"),
            Self::Interval(a, b) => {
                f.write_str("INTERVAL(")?;
                write_end(f, a)?;
                f.write_str(", ")?;
                write_end(f, b)?;
                f.write_str(")")
            }
            Self::Unbounded => f.write_str("'..'"),
            Self::Geometry(g) => write!(f, "{g}"),
            Self::BBox(n) => {
                f.write_str("BBOX(")?;
                write_list(f, n)?;
                f.write_str(")")
            }
            Self::Array(items) => {
                f.write_str("(")?;
                write_list(f, items)?;
                f.write_str(")")
            }
            Self::Op(op, args) => write_op(f, op, args),
        }
    }
}

const RESERVED: &[&str] = &[
    "AND", "OR", "NOT", "LIKE", "BETWEEN", "IN", "IS", "NULL", "TRUE", "FALSE", "DIV",
];

fn write_property(f: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    let mut chars = name.chars();
    let plain = chars.next().is_some_and(text::starts_identifier)
        && chars.all(text::continues_identifier)
        && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(name));
    if plain {
        f.write_str(name)
    } else {
        write!(f, "\"{}\"", name.replace('"', "\"\""))
    }
}

fn write_end(f: &mut fmt::Formatter<'_>, end: &Expr) -> fmt::Result {
    match end {
        Expr::Timestamp(s) | Expr::Date(s) => write!(f, "'{s}'"),
        other => write!(f, "{other}"),
    }
}

fn write_list<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

/// An operand: an operator that is not a call is parenthesised, so the text parses back to
/// the same tree whatever the precedence.
fn write_operand(f: &mut fmt::Formatter<'_>, e: &Expr) -> fmt::Result {
    match e {
        Expr::Op(op, _) if !op.is_call() => write!(f, "({e})"),
        _ => write!(f, "{e}"),
    }
}

fn write_op(f: &mut fmt::Formatter<'_>, op: &Op, args: &[Expr]) -> fmt::Result {
    let infix = match op {
        Op::And => " AND ",
        Op::Or => " OR ",
        Op::Like => " LIKE ",
        Op::In => " IN ",
        Op::IntDiv => " div ",
        Op::Eq
        | Op::Ne
        | Op::Lt
        | Op::Le
        | Op::Gt
        | Op::Ge
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Mod
        | Op::Pow => return write_binary(f, &format!(" {} ", op.name()), args),
        Op::Not => {
            f.write_str("NOT ")?;
            return args.iter().try_for_each(|a| write!(f, "({a})"));
        }
        Op::IsNull => {
            args.iter().try_for_each(|a| write_operand(f, a))?;
            return f.write_str(" IS NULL");
        }
        Op::Between => {
            let separators = ["", " BETWEEN "]
                .into_iter()
                .chain(std::iter::repeat(" AND "));
            for (separator, a) in separators.zip(args) {
                f.write_str(separator)?;
                write_operand(f, a)?;
            }
            return Ok(());
        }
        Op::Function(name) => {
            write!(f, "{name}(")?;
            write_list(f, args)?;
            return f.write_str(")");
        }
        call => {
            write!(f, "{}(", call.name().to_ascii_uppercase())?;
            write_list(f, args)?;
            return f.write_str(")");
        }
    };
    write_binary(f, infix, args)
}

fn write_binary(f: &mut fmt::Formatter<'_>, infix: &str, args: &[Expr]) -> fmt::Result {
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            f.write_str(infix)?;
        }
        write_operand(f, a)?;
    }
    Ok(())
}

impl fmt::Display for Geometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Point(p) => {
                f.write_str("POINT")?;
                write_positions(f, std::slice::from_ref(p))
            }
            Self::LineString(line) => {
                f.write_str("LINESTRING")?;
                write_positions(f, line)
            }
            Self::Polygon(rings) => {
                f.write_str("POLYGON")?;
                write_rings(f, rings)
            }
            Self::MultiPoint(points) => {
                f.write_str("MULTIPOINT")?;
                write_positions(f, points)
            }
            Self::MultiLineString(lines) => {
                f.write_str("MULTILINESTRING")?;
                write_rings(f, lines)
            }
            Self::MultiPolygon(polygons) => {
                f.write_str("MULTIPOLYGON(")?;
                for (i, rings) in polygons.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write_rings(f, rings)?;
                }
                f.write_str(")")
            }
            Self::Collection(members) => {
                f.write_str("GEOMETRYCOLLECTION(")?;
                write_list(f, members)?;
                f.write_str(")")
            }
        }
    }
}

/// `(x y, x y z)`
fn write_positions(f: &mut fmt::Formatter<'_>, positions: &[Position]) -> fmt::Result {
    f.write_str("(")?;
    for (i, p) in positions.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        for (j, n) in p.iter().enumerate() {
            if j > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{n}")?;
        }
    }
    f.write_str(")")
}

/// `((x y, ...), (x y, ...))`
fn write_rings(f: &mut fmt::Formatter<'_>, rings: &[Vec<Position>]) -> fmt::Result {
    f.write_str("(")?;
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write_positions(f, ring)?;
    }
    f.write_str(")")
}
