// SPDX-License-Identifier: EUPL-1.2
//! The residual of a standard query over an NGSI-LD view (ADR-0028 rules 2 and 7): one
//! intermediate form every surface compiles the part of its filter NGSI-LD cannot say into, and
//! one three-valued evaluator over a normalized NGSI-LD Entity.
//!
//! An [`Expr`] names values by [`Path`] (an Attribute's value, a sub-Attribute, `observedAt`,
//! `createdAt`, `modifiedAt`, the Entity's `id` and `type`), literals ([`Lit`]), comparison
//! ([`Cmp`]), `and`/`or`/`not`, `IS NULL`, `IN`, `any`/`all` over a list value, calls from the
//! closed table [`Func`] and spatial relations of `GeoJSON` geometries ([`Spatial`]). [`compile`] checks it against the bounds of rule 7 ([`MAX_DEPTH`],
//! [`MAX_NODES`]), folds `now` to the one value the request was given and builds each pattern
//! once; [`eval()`] answers [`Truth`] for one Entity.
//!
//! A missing value, a hidden one (the view removed it before the residual sees the Entity) and
//! a value of the wrong type are all *unknown*: a comparison with one is [`Truth::Unknown`], and
//! the caller keeps an Entity only on [`Truth::True`]. `not` is exact under that logic. An
//! integer overflow, a division by zero and a string past [`MAX_STRING_BYTES`] are unknown too,
//! never a wrapped or truncated value.
//!
#![doc = include_str!("../README.md")]

mod eval;
mod geometry;
mod pattern;
mod time;

use std::fmt;

use regex::Regex;
use serde_json::Value;

pub use geometry::MAX_GEO_VERTICES;
pub use pattern::{MAX_PATTERN_BYTES, MAX_PROGRAM_BYTES};
pub use time::{Date, DateTime, Time};

/// The deepest nesting of an expression (rule 7).
pub const MAX_DEPTH: usize = 64;
/// The most nodes one expression holds, literals and list members included (rule 7).
pub const MAX_NODES: usize = 4096;
/// The longest string a function may build; a longer result is unknown. Under Miri the bound
/// is 4 KiB, so the test that crosses it runs the same code in seconds rather than an hour.
pub const MAX_STRING_BYTES: usize = if cfg!(miri) { 4096 } else { 1024 * 1024 };
/// The most nodes one evaluation visits, lambda bodies counted once per list member: nested
/// `any`/`all` over long lists cost their product, and past this the evaluation is refused with
/// [`Error::TooComplex`] instead of answering a guess.
pub const MAX_STEPS: usize = 1 << 20;

/// The three-valued result of a filter (`OData` URL Conventions 5.1.1.1, CQL2 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// The Entity is in the answer.
    True,
    /// The Entity is not.
    False,
    /// A value it needed is missing, hidden or of another type; not in the answer.
    Unknown,
}

impl Truth {
    /// Kleene `and`: `False` wins, then `Unknown`.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    /// Kleene `or`: `True` wins, then `Unknown`.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

impl std::ops::Not for Truth {
    type Output = Self;

    /// Kleene `not`: `Unknown` stays.
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// Where a value sits in a normalized NGSI-LD Entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Path {
    /// The Entity id.
    Id,
    /// The Entity type: a string, or a list for an Entity of several types.
    Type,
    /// The Entity's `createdAt` (present when the request asked for `sysAttrs`).
    CreatedAt,
    /// The Entity's `modifiedAt`.
    ModifiedAt,
    /// An Attribute, or a sub-Attribute of one.
    Attribute {
        /// The Attribute's term, then the sub-Attribute terms below it; never empty.
        attrs: Vec<String>,
        /// What of the last one.
        member: Member,
        /// Keys into a JSON object value (a complex value, F20); only with [`Member::Value`].
        keys: Vec<String>,
    },
}

impl Path {
    /// The value of the top-level Attribute `attr`.
    #[must_use]
    pub fn value(attr: &str) -> Self {
        Self::Attribute {
            attrs: vec![attr.to_owned()],
            member: Member::Value,
            keys: Vec::new(),
        }
    }
}

/// The part of an Attribute a [`Path`] reads. An Attribute with several instances is read at
/// its default instance, the one without a `datasetId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member {
    /// The value by the Attribute's kind: `value` (Property, `GeoProperty`), `object`
    /// (Relationship), `languageMap`, `valueList`, `objectList`, `vocab` or `json`.
    Value,
    /// `observedAt`.
    ObservedAt,
    /// `createdAt`.
    CreatedAt,
    /// `modifiedAt`.
    ModifiedAt,
    /// `unitCode`.
    UnitCode,
}

/// A typed literal.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A floating-point number; an integer compared with one is read as one (numeric promotion).
    Num(f64),
    /// A string.
    Str(String),
    /// A date-time.
    DateTime(DateTime),
    /// A date.
    Date(Date),
    /// A time of day.
    Time(Time),
    /// A `GeoJSON` geometry object (`type`, `coordinates`), checked at [`compile`] as the
    /// NGSI-LD `georel` checks its `geometry`: RFC 7946 shape, WGS84 range, at most
    /// [`MAX_GEO_VERTICES`] positions.
    Geometry(Value),
}

/// A spatial relation (DE-9IM): the NGSI-LD georels but `near` (TS 104 175 7.2.4), CQL2's
/// spatial operators (`S_INTERSECTS` to `S_CROSSES`) and `OData`'s `geo.intersects`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spatial {
    /// The two share a point.
    Intersects,
    /// The first lies in the second and touches its interior.
    Within,
    /// The second lies in the first and touches its interior.
    Contains,
    /// The same point set.
    Equals,
    /// No point shared.
    Disjoint,
    /// Interiors meet, of the same dimension, neither holds the other.
    Overlaps,
    /// Boundaries meet, interiors do not.
    Touches,
    /// Interiors meet in a lower dimension than the larger of the two.
    Crosses,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
}

/// The closed function table (`OData` F14, F16 to F19; CQL2 `CASEI`, `ACCENTI`, `LIKE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    /// `contains(s, t)`.
    Contains,
    /// `startswith(s, t)`.
    StartsWith,
    /// `endswith(s, t)`.
    EndsWith,
    /// `length(s)`, in characters.
    Length,
    /// `indexof(s, t)`, in characters; -1 when absent.
    IndexOf,
    /// `substring(s, start)` or `substring(s, start, length)`, in characters; a negative
    /// argument is unknown, a start past the end the empty string.
    Substring,
    /// `tolower(s)`.
    ToLower,
    /// `toupper(s)`.
    ToUpper,
    /// `trim(s)`: leading and trailing white space removed.
    Trim,
    /// `concat(s, t)`.
    Concat,
    /// CQL2 `CASEI(s)`: the string case-folded, for a case-insensitive comparison.
    CaseI,
    /// CQL2 `ACCENTI(s)`: the string with its combining marks removed after canonical
    /// decomposition, for an accent-insensitive comparison.
    AccentI,
    /// `matchesPattern(s, pattern)`: an ECMAScript pattern, a literal (ADR-0028 rule 7).
    MatchesPattern,
    /// CQL2 `s LIKE pattern`: the whole value, `%`, `_` and `\`, a literal pattern.
    Like,
    /// `year(d)` of a date or date-time, in its own offset.
    Year,
    /// `month(d)`.
    Month,
    /// `day(d)`.
    Day,
    /// `hour(t)` of a time or date-time.
    Hour,
    /// `minute(t)`.
    Minute,
    /// `second(t)`.
    Second,
    /// `fractionalseconds(t)`, a number below 1.
    FractionalSeconds,
    /// `totaloffsetminutes(dt)`.
    TotalOffsetMinutes,
    /// `date(dt)`: the date in its own offset.
    Date,
    /// `time(dt)`: the time of day in its own offset.
    Time,
    /// `now()`: the request's start time, one value per [`compile`].
    Now,
    /// `mindatetime()`: [`DateTime::MIN`].
    MinDateTime,
    /// `maxdatetime()`: [`DateTime::MAX`].
    MaxDateTime,
    /// `round(n)`: half away from zero.
    Round,
    /// `floor(n)`.
    Floor,
    /// `ceiling(n)`.
    Ceiling,
    /// `a add b`.
    Add,
    /// `a sub b`.
    Sub,
    /// `a mul b`.
    Mul,
    /// `a div b`: truncating for two integers.
    Div,
    /// `a divby b`: always a number.
    DivBy,
    /// `a mod b`: the sign of `a`.
    Mod,
    /// CQL2 `a ^ b`: an integer for two integers and an exponent from 0, a number otherwise;
    /// an overflow or a result that is no finite number is unknown.
    Pow,
    /// `-a`.
    Neg,
}

impl Func {
    /// How many arguments the function takes, at least and at most.
    #[must_use]
    pub fn arity(self) -> (usize, usize) {
        match self {
            Self::Now | Self::MinDateTime | Self::MaxDateTime => (0, 0),
            Self::Length
            | Self::ToLower
            | Self::ToUpper
            | Self::Trim
            | Self::CaseI
            | Self::AccentI
            | Self::Year
            | Self::Month
            | Self::Day
            | Self::Hour
            | Self::Minute
            | Self::Second
            | Self::FractionalSeconds
            | Self::TotalOffsetMinutes
            | Self::Date
            | Self::Time
            | Self::Round
            | Self::Floor
            | Self::Ceiling
            | Self::Neg => (1, 1),
            Self::Substring => (2, 3),
            _ => (2, 2),
        }
    }
}

/// A residual expression, as a surface's compiler builds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal.
    Lit(Lit),
    /// A value of the Entity.
    Path(Path),
    /// A member of an enclosing `any`/`all` list: `up` 0 is the innermost lambda's, then
    /// `keys` into it when it is a JSON object.
    Var {
        /// How many lambdas out.
        up: usize,
        /// Keys into the member.
        keys: Vec<String>,
    },
    /// A comparison; unknown when either side is unknown or the two do not compare.
    Cmp(Cmp, Box<Expr>, Box<Expr>),
    /// Every operand true; empty is true.
    And(Vec<Expr>),
    /// Some operand true; empty is false.
    Or(Vec<Expr>),
    /// The exact three-valued negation.
    Not(Box<Expr>),
    /// True when the operand is unknown (missing, hidden, mistyped), false otherwise; never
    /// unknown itself (CQL2 `IS NULL`, `OData` `eq null`).
    IsNull(Box<Expr>),
    /// Equal to one of the literals; unknown when none is equal and one did not compare.
    In(Box<Expr>, Vec<Lit>),
    /// Some member of the list value satisfies the body; with no body, the list is not empty.
    Any(Box<Expr>, Option<Box<Expr>>),
    /// Every member of the list value satisfies the body; an empty list is true.
    All(Box<Expr>, Box<Expr>),
    /// A function of the closed table.
    Call(Func, Vec<Expr>),
    /// The first geometry's relation to the second; each a geometry value of the Entity (a
    /// `GeoProperty`'s value) or a [`Lit::Geometry`]. A value that is no geometry, or has more
    /// than [`MAX_GEO_VERTICES`] positions, is unknown.
    Spatial(Spatial, Box<Expr>, Box<Expr>),
}

/// Why an expression was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Past [`MAX_DEPTH`], [`MAX_NODES`], a pattern bound, [`MAX_GEO_VERTICES`] in a literal or
    /// [`MAX_STEPS`]: `OData` 400 `TooComplexQuery`.
    TooComplex,
    /// A construct the evaluator does not read with the standard's meaning (a pattern construct
    /// of rule 7, a pattern that is not a literal): 501 `NotImplemented`.
    Unsupported(String),
    /// An expression no compiler should build (a wrong argument count, a variable outside any
    /// lambda, a broken LIKE escape, a geometry literal that is no `GeoJSON` geometry): 400.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooComplex => write!(
                f,
                "the filter is larger than the residual evaluator allows ({MAX_DEPTH} levels, {MAX_NODES} nodes)"
            ),
            Self::Unsupported(what) => write!(f, "{what} is not supported"),
            Self::Invalid(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A compiled residual: bounded, `now` folded, patterns built.
#[derive(Debug, Clone)]
pub struct Program {
    root: Node,
}

/// The compiled form of [`Expr`].
#[derive(Debug, Clone)]
enum Node {
    Lit(Lit),
    Path(Path),
    Var(usize, Vec<String>),
    Cmp(Cmp, Box<Node>, Box<Node>),
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    IsNull(Box<Node>),
    In(Box<Node>, Vec<Lit>),
    Any(Box<Node>, Option<Box<Node>>),
    All(Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
    Match(Box<Node>, Regex),
    Spatial(Spatial, Shape, Shape),
}

/// An operand of a spatial relation: a literal parsed at compile, or a value read per Entity.
#[derive(Debug, Clone)]
enum Shape {
    Fixed(Box<geo::Geometry<f64>>),
    Value(Box<Node>),
}

/// Checks `expr` against the bounds, folds `now()` to `now` and builds its patterns.
///
/// # Errors
///
/// [`Error::TooComplex`] past [`MAX_DEPTH`] or [`MAX_NODES`] or a pattern bound or a geometry
/// literal past [`MAX_GEO_VERTICES`], [`Error::Unsupported`] for a pattern construct outside the
/// shared subset or a pattern that is not a literal, [`Error::Invalid`] for a wrong argument
/// count, a variable outside every lambda or a geometry literal that is no `GeoJSON` geometry.
pub fn compile(expr: &Expr, now: DateTime) -> Result<Program, Error> {
    let mut c = Compiler { now, nodes: 0 };
    Ok(Program {
        root: c.node(expr, 0, 0)?,
    })
}

/// The truth of `program` for one normalized NGSI-LD Entity.
///
/// # Errors
///
/// [`Error::TooComplex`] when the evaluation visits more than [`MAX_STEPS`] nodes.
pub fn eval(program: &Program, entity: &Value) -> Result<Truth, Error> {
    eval::run(&program.root, entity)
}

impl Program {
    /// [`eval()`] as a method.
    ///
    /// # Errors
    ///
    /// As [`eval()`].
    pub fn eval(&self, entity: &Value) -> Result<Truth, Error> {
        eval(self, entity)
    }

    /// The value of the program for one normalized NGSI-LD Entity, as JSON: a number,
    /// string, boolean, list or object, a date, time or date-time as its RFC 3339 text;
    /// `None` when it is unknown (a missing, hidden or mistyped operand), never a default.
    ///
    /// ```
    /// use ngsild_residual::{compile, DateTime, Expr, Func, Lit, Path};
    /// use serde_json::json;
    ///
    /// let double = Expr::Call(Func::Mul, vec![Expr::Path(Path::value("pm10")), Expr::Lit(Lit::Int(2))]);
    /// let now = DateTime::parse("2026-10-11T12:00:00Z").ok_or("now")?;
    /// let program = compile(&double, now)?;
    /// let station = json!({"id": "urn:ngsi-ld:AirQuality:1", "type": "AirQuality",
    ///     "pm10": {"type": "Property", "value": 12}});
    /// assert_eq!(program.value(&station)?, Some(json!(24)));
    /// let silent = json!({"id": "urn:ngsi-ld:AirQuality:2", "type": "AirQuality"});
    /// assert_eq!(program.value(&silent)?, None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Self::eval`].
    pub fn value(&self, entity: &Value) -> Result<Option<Value>, Error> {
        eval::run_value(&self.root, entity)
    }

    /// The top-level Attribute terms the program reads, each once in the order first read: what
    /// a pushdown's `attrs` must hold for the residual to see them (ADR-0028 rule 3).
    #[must_use]
    pub fn attributes(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        self.root.paths(&mut |p| {
            if let Path::Attribute { attrs, .. } = p {
                out.extend(attrs.first().map(String::as_str));
            }
        });
        let mut seen = std::collections::HashSet::new();
        out.retain(|a| seen.insert(*a));
        out
    }

    /// Whether it reads a `createdAt` or `modifiedAt`, of the Entity or of an Attribute: the
    /// pushdown then asks for `options=sysAttrs` (F33).
    #[must_use]
    pub fn reads_system_members(&self) -> bool {
        let mut system = false;
        self.root.paths(&mut |p| {
            system |= matches!(
                p,
                Path::CreatedAt
                    | Path::ModifiedAt
                    | Path::Attribute {
                        member: Member::CreatedAt | Member::ModifiedAt,
                        ..
                    }
            );
        });
        system
    }
}

impl Node {
    /// Every [`Path`] the node reads, in evaluation order.
    fn paths<'a>(&'a self, f: &mut impl FnMut(&'a Path)) {
        match self {
            Self::Lit(_) | Self::Var(..) => {}
            Self::Path(p) => f(p),
            Self::Cmp(_, a, b) => {
                a.paths(f);
                b.paths(f);
            }
            Self::And(es) | Self::Or(es) | Self::Call(_, es) => es.iter().for_each(|e| e.paths(f)),
            Self::Not(a) | Self::IsNull(a) | Self::In(a, _) | Self::Match(a, _) => a.paths(f),
            Self::Any(list, body) => {
                list.paths(f);
                if let Some(b) = body {
                    b.paths(f);
                }
            }
            Self::All(list, body) => {
                list.paths(f);
                body.paths(f);
            }
            Self::Spatial(_, a, b) => {
                for shape in [a, b] {
                    if let Shape::Value(n) = shape {
                        n.paths(f);
                    }
                }
            }
        }
    }
}

struct Compiler {
    now: DateTime,
    nodes: usize,
}

impl Compiler {
    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.nodes = self.nodes.saturating_add(n);
        if self.nodes > MAX_NODES {
            return Err(Error::TooComplex);
        }
        Ok(())
    }

    fn boxed(&mut self, e: &Expr, depth: usize, lambdas: usize) -> Result<Box<Node>, Error> {
        Ok(Box::new(self.node(e, depth, lambdas)?))
    }

    fn all(&mut self, es: &[Expr], depth: usize, lambdas: usize) -> Result<Vec<Node>, Error> {
        es.iter().map(|e| self.node(e, depth, lambdas)).collect()
    }

    fn node(&mut self, e: &Expr, depth: usize, lambdas: usize) -> Result<Node, Error> {
        self.count(1)?;
        if depth >= MAX_DEPTH {
            return Err(Error::TooComplex);
        }
        let d = depth.saturating_add(1);
        Ok(match e {
            Expr::Lit(l) => Node::Lit(l.clone()),
            Expr::Path(Path::Attribute { attrs, .. }) if attrs.is_empty() => {
                return Err(Error::Invalid(
                    "an Attribute path names no Attribute".into(),
                ));
            }
            Expr::Path(p) => Node::Path(p.clone()),
            Expr::Var { up, .. } if *up >= lambdas => {
                return Err(Error::Invalid(
                    "a lambda variable outside its lambda".into(),
                ));
            }
            Expr::Var { up, keys } => Node::Var(*up, keys.clone()),
            Expr::Cmp(op, a, b) => {
                Node::Cmp(*op, self.boxed(a, d, lambdas)?, self.boxed(b, d, lambdas)?)
            }
            Expr::And(es) => Node::And(self.all(es, d, lambdas)?),
            Expr::Or(es) => Node::Or(self.all(es, d, lambdas)?),
            Expr::Not(a) => Node::Not(self.boxed(a, d, lambdas)?),
            Expr::IsNull(a) => Node::IsNull(self.boxed(a, d, lambdas)?),
            Expr::In(a, list) => {
                self.count(list.len())?;
                Node::In(self.boxed(a, d, lambdas)?, list.clone())
            }
            Expr::Any(list, body) => {
                let inner = lambdas.saturating_add(1);
                let body = match body {
                    Some(b) => Some(self.boxed(b, d, inner)?),
                    None => None,
                };
                Node::Any(self.boxed(list, d, lambdas)?, body)
            }
            Expr::All(list, body) => {
                let inner = lambdas.saturating_add(1);
                Node::All(self.boxed(list, d, lambdas)?, self.boxed(body, d, inner)?)
            }
            Expr::Call(f, args) => self.call(*f, args, d, lambdas)?,
            Expr::Spatial(rel, a, b) => {
                Node::Spatial(*rel, self.shape(a, d, lambdas)?, self.shape(b, d, lambdas)?)
            }
        })
    }

    fn shape(&mut self, e: &Expr, depth: usize, lambdas: usize) -> Result<Shape, Error> {
        match e {
            Expr::Lit(Lit::Geometry(g)) => {
                self.count(1)?;
                Ok(Shape::Fixed(Box::new(geometry::literal(g)?)))
            }
            _ => Ok(Shape::Value(self.boxed(e, depth, lambdas)?)),
        }
    }

    fn call(
        &mut self,
        f: Func,
        args: &[Expr],
        depth: usize,
        lambdas: usize,
    ) -> Result<Node, Error> {
        let (least, most) = f.arity();
        if args.len() < least || args.len() > most {
            return Err(Error::Invalid(format!(
                "{f:?} takes {least} to {most} arguments, not {}",
                args.len()
            )));
        }
        Ok(match (f, args) {
            (Func::Now, _) => Node::Lit(Lit::DateTime(self.now)),
            (Func::MinDateTime, _) => Node::Lit(Lit::DateTime(DateTime::MIN)),
            (Func::MaxDateTime, _) => Node::Lit(Lit::DateTime(DateTime::MAX)),
            (Func::MatchesPattern | Func::Like, [value, pattern]) => {
                let text = literal_text(pattern).ok_or_else(|| {
                    Error::Unsupported("a pattern that is not a string literal".into())
                })?;
                let regex = if f == Func::Like {
                    pattern::like(&text)?
                } else {
                    pattern::ecmascript(&text)?
                };
                Node::Match(self.boxed(value, depth, lambdas)?, regex)
            }
            _ => Node::Call(f, self.all(args, depth, lambdas)?),
        })
    }
}

/// A string literal, through any `CASEI`/`ACCENTI` around it (CQL2 writes
/// `CASEI(name) LIKE CASEI('sta%')`).
fn literal_text(e: &Expr) -> Option<String> {
    match e {
        Expr::Lit(Lit::Str(s)) => Some(s.clone()),
        Expr::Call(f @ (Func::CaseI | Func::AccentI), args) => match args.as_slice() {
            [inner] => literal_text(inner).map(|s| eval::fold_text(*f, &s)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{compile, DateTime, Error, Expr, Func, Lit, Truth};

    #[test]
    fn kleene_tables() {
        use Truth::{False as F, True as T, Unknown as U};
        let all = [T, F, U];
        let and = [[T, F, U], [F, F, F], [U, F, U]];
        let or = [[T, T, T], [T, F, U], [T, U, U]];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(Some(a.and(*b)), and.get(i).and_then(|r| r.get(j)).copied());
                assert_eq!(Some(a.or(*b)), or.get(i).and_then(|r| r.get(j)).copied());
            }
        }
        assert_eq!((!T, !F, !U), (F, T, U));
    }

    #[test]
    fn errors_read_as_sentences() {
        assert!(Error::TooComplex.to_string().contains("64 levels"));
        assert_eq!(
            Error::Unsupported("look-around in a pattern".into()).to_string(),
            "look-around in a pattern is not supported"
        );
        assert_eq!(Error::Invalid("x".into()).to_string(), "x");
    }

    #[test]
    fn a_pattern_must_be_a_literal() {
        let now = DateTime::MIN;
        let name = || Expr::Call(Func::ToLower, vec![Expr::Lit(Lit::Str("A".into()))]);
        let e = Expr::Call(Func::Like, vec![name(), name()]);
        assert!(matches!(compile(&e, now), Err(Error::Unsupported(_))));
        let bad_casei = Expr::Call(Func::Like, vec![name(), Expr::Call(Func::CaseI, vec![])]);
        assert!(matches!(
            compile(&bad_casei, now),
            Err(Error::Unsupported(_))
        ));
    }
}
