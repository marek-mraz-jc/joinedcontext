// SPDX-License-Identifier: EUPL-1.2
//! NGSI-LD Query Language (CIM 009 clause 4.9): one AST, two backends.
//!
//! [`parse_q`] turns a `q=` expression into a [`QNode`]; [`eval`] evaluates
//! it against an in-memory expanded entity (the broker's query path and its
//! subscription matcher share this evaluator), [`sql`] lowers it to a
//! bind-parameter jsonpath predicate for Postgres. The AST is `Serialize`
//! and `Clone`, and renders back to `q=` syntax through `Display`, so a
//! gateway can inspect or rewrite a query (strip an attribute, AND in an
//! authorization predicate) and forward it with the broker's own semantics.
//!
#![doc = include_str!("../README.md")]
// T-3468: every surface calls into this crate on a request path, where a
// panic is a remote DoS. Outside tests it may not unwrap, expect, panic,
// index out of bounds or overflow.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

/// What a model is told before it calls an NGSI-LD tool (T-4532, EP-116): that the API is
/// standard NGSI-LD, the query language and every tool argument. One text: Antares' per-view MCP
/// sends it as its instructions, the joinedcontext gateway's hub prepends only its catalogue
/// lines. Static: it names no data (MC11).
pub const PRIMER: &str = include_str!("primer.md");

pub mod eval;
pub mod geo;
pub mod regex;
mod render;
pub mod scope;
pub mod sql;

use antares_model::NgsiError;

/// The pinned core `@context`, parsed once for every test of the crate:
/// under Miri one parse takes minutes (T-3706).
#[cfg(test)]
pub(crate) fn test_core() -> std::sync::Arc<antares_jsonld::Context> {
    static CORE: std::sync::LazyLock<std::sync::Arc<antares_jsonld::Context>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(antares_jsonld::core_context()));
    std::sync::Arc::clone(&CORE)
}

/// Entity Type Selection Language (4.17) match against expanded type IRIs:
/// `,`/`|` = OR of alternatives, `(a;b)` = AND within one alternative.
pub fn type_selection_matches(sel: &str, types: &[&str], ctx: &antares_jsonld::Context) -> bool {
    sel.split([',', '|']).any(|alt| {
        alt.trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(';')
            .all(|t| types.contains(&ctx.expand_key(t.trim()).as_str()))
    })
}

/// The 4.17 grammar of a type selection: `EntityTypes = OrEntityType
/// *(orOp OrEntityType)`, `OrEntityType = "(" EntityType *(";" EntityType)
/// ")" / EntityType`, an EntityType never empty.
pub fn check_type_selection(sel: &str) -> Result<(), NgsiError> {
    let bad = || NgsiError::BadRequestData(format!("type {sel:?} violates the 4.17 grammar"));
    for alt in sel.split([',', '|']).map(str::trim) {
        let inner = match alt.strip_prefix('(') {
            Some(rest) => rest.strip_suffix(')').ok_or_else(bad)?,
            None if alt.contains(';') => return Err(bad()),
            None => alt,
        };
        if inner
            .split(';')
            .any(|t| t.trim().is_empty() || t.contains(['(', ')']))
        {
            return Err(bad());
        }
    }
    Ok(())
}

/// RFC 3986 percent-decoding of a query value (`q`, `scopeQ` in a
/// subscription body may arrive encoded, 4.9).
pub fn percent_decode(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(input.len());
    let mut rest = input;
    while let Some((&b, tail)) = rest.split_first() {
        // from_str_radix accepts a leading sign, so "%+1" would decode as
        // 0x01. RFC 3986 clause 2.1 admits two hex digits and nothing else.
        let decoded = (b == b'%')
            .then(|| tail.get(..2))
            .flatten()
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match decoded {
            Some(v) => {
                out.push(v);
                rest = tail.get(2..).unwrap_or_default();
            }
            None => {
                out.push(b);
                rest = tail;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One parsed 4.9 query expression.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum QNode {
    /// `a;b` — every operand must hold.
    And(Vec<QNode>),
    /// `a|b` — any operand holds.
    Or(Vec<QNode>),
    /// `path op value` — one comparison term.
    Cmp {
        /// The attribute (path) the term targets.
        path: QPath,
        /// The comparison operator.
        op: CmpOp,
        /// The literal compared against.
        value: QValue,
    },
    /// Bare attribute path = existence check (`q=temperature`).
    Exists {
        /// The attribute (path) whose presence is tested.
        path: QPath,
        /// `!path` — the attribute must be absent.
        negated: bool,
    },
}

/// 4.9 `Attribute = LinkedEntityRelation` — zero or more `attr{[T[,T]:]…}`
/// hops (EXAMPLE 13/14), then `ValuePath = DottedPath *1([DottedPath])`:
/// a dotted path plus an optional single trailing bracket that is either a
/// compound-value member path (EXAMPLE 9/10/11) or a language filter
/// (`[en]` / `[*]`, Equal/Unequal languageMap semantics).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QPath {
    /// Linked-entity hops (`attr{…}`) preceding the path, outermost first.
    pub links: Vec<Link>,
    /// The dotted attribute path (terms, expanded at evaluation time).
    pub path: Vec<String>,
    /// The optional trailing `[…]`: a compound-value member path, or a
    /// language filter (`[en]`, `[*]`).
    pub bracket: Option<Vec<String>>,
}

/// One `attr{…}` linked-entity hop with its optional EntityType hints.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Link {
    /// The Relationship followed.
    pub attr: String,
    /// EntityType hints (`attr{T1,T2:…}`), empty when none.
    pub types: Vec<String>,
}

impl QPath {
    /// Plain dotted path (the pre-4.9-extension shape).
    pub fn dotted(path: Vec<String>) -> Self {
        Self {
            links: Vec::new(),
            path,
            bracket: None,
        }
    }

    /// The top-level Attribute name this path filters on.
    pub fn top(&self) -> Option<&str> {
        self.links
            .first()
            .map(|l| l.attr.as_str())
            .or_else(|| self.path.first().map(String::as_str))
    }
}

impl QNode {
    /// Every top-level attribute name this expression references, in source
    /// order.
    ///
    /// Purge (5.6.21.4 b/c) qualifies an `attrs` list or a `q` only when it
    /// includes "at least one non-system Attribute", so the caller needs the
    /// referenced names rather than just "is there a q".
    pub fn attribute_paths(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_paths(&mut out);
        out
    }

    /// True when any referenced Attribute path uses a `attr{…}` linked-entity
    /// hop (4.9 LinkedEntityRelation). Purge (5.6.21.4) must reject filter
    /// conditions that include Linked Entity attributes.
    pub fn has_linked_paths(&self) -> bool {
        match self {
            QNode::And(ns) | QNode::Or(ns) => ns.iter().any(Self::has_linked_paths),
            QNode::Cmp { path, .. } | QNode::Exists { path, .. } => !path.links.is_empty(),
        }
    }

    /// Deepest chain of `attr{…}` hops any referenced path uses — the number
    /// of Linked Entity levels the query needs (5.7.2.4: must not exceed
    /// joinLevel).
    pub fn max_link_depth(&self) -> usize {
        match self {
            QNode::And(ns) | QNode::Or(ns) => {
                ns.iter().map(Self::max_link_depth).max().unwrap_or(0)
            }
            QNode::Cmp { path, .. } | QNode::Exists { path, .. } => path.links.len(),
        }
    }

    fn collect_paths<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            QNode::And(ns) | QNode::Or(ns) => {
                for n in ns {
                    n.collect_paths(out);
                }
            }
            QNode::Cmp { path, .. } | QNode::Exists { path, .. } => out.extend(path.top()),
        }
    }
}

/// System-generated members that never count as a "non-system Attribute"
/// (5.6.21.4). `id`/`type`/`scope` are Entity members, the timestamps are the
/// system temporal attributes of 6.3.11.
pub const SYSTEM_ATTRS: &[&str] = &[
    "id",
    "type",
    "scope",
    "createdAt",
    "modifiedAt",
    "expiresAt",
    "deletedAt",
    "instanceId",
];

/// True when `name` is an ordinary (non-system) Attribute name.
pub fn is_non_system_attr(name: &str) -> bool {
    !SYSTEM_ATTRS.contains(&name)
}

/// The 4.9 comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `~=` (patternOp)
    Pattern,
    /// `!~=` (notPatternOp)
    NotPattern,
}

/// A query term literal.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum QValue {
    /// A string (quoted, or an unquoted non-numeric token such as a date).
    Str(String),
    /// A number.
    Num(f64),
    /// `true` / `false`.
    Bool(bool),
    /// 4.9 `ValueList = Value 1*(, Value)` — scalars only, `==`/`!=` only.
    List(Vec<QValue>),
    /// 4.9 `Range = ComparableValue dots ComparableValue` — `==`/`!=` only.
    /// Endpoints are the same scalar variant (Num..Num or Str..Str; dates and
    /// times ride in Str and order correctly because 4.6.3 pins them to
    /// fixed-width UTC forms).
    Range(Box<QValue>, Box<QValue>),
}

/// Longest `q=` accepted, before parsing. The URI cap is 8 KiB but a POST
/// query body carries `q` too, where the only other ceiling is the 4 MiB body
/// limit — so the string needs its own bound at the one entry point.
pub const MAX_Q_BYTES: usize = 4096;

/// Nesting depth ceiling. `(` costs three stack frames (`or_expr` → `and_expr`
/// → `term`), and a Rust stack overflow is a guard-page abort, NOT a catchable
/// panic — no tower layer can contain it, so the parser must refuse before it
/// recurses rather than be rescued afterwards. 64 is far past any real query
/// and far below the ~2 MiB tokio worker stack.
pub const MAX_Q_DEPTH: usize = 64;

/// AST size cap — checked after parsing, which is safe once depth and
/// length are bounded first. Public because `/q/health` publishes it: a
/// second constant carrying the same number is one that can drift from the
/// one actually enforced.
pub const MAX_Q_NODES: usize = 512;

/// Parse an NGSI-LD `q=` expression. Complexity ceilings raise
/// TooComplexQuery per 5.5.6 ("a query operation … so complex that cannot
/// be resolved").
pub fn parse_q(input: &str) -> Result<QNode, NgsiError> {
    if input.len() > MAX_Q_BYTES {
        return Err(NgsiError::TooComplexQuery(format!(
            "q expression exceeds {MAX_Q_BYTES} bytes"
        )));
    }
    parse_capped(input, MAX_Q_DEPTH, MAX_Q_NODES)
}

/// Paren depth a narrowed `q` may reach: the caller's [`MAX_Q_DEPTH`], one
/// more where a hidden term becomes `(a;!a)`, one more for the conjunction,
/// and a policy part held below it by [`fits_policy_q`].
const MAX_NARROWED_Q_DEPTH: usize = 2 * MAX_Q_DEPTH;

/// Nodes a narrowed `q` may hold: the caller's [`MAX_Q_NODES`], each term of
/// it at most three once a hidden-Attribute rewrite has turned it into
/// `(a;!a)`, the conjunction, and a policy part held below the rest by
/// [`fits_policy_q`].
pub const MAX_NARROWED_Q_NODES: usize = 8 * MAX_Q_NODES;

/// Nodes the policy part of a narrowed `q` (an engine's, a view's, or both
/// joined) may hold: what [`MAX_NARROWED_Q_NODES`] leaves after the
/// rewritten caller part and the conjunction.
const MAX_POLICY_Q_NODES: usize = MAX_NARROWED_Q_NODES - 3 * MAX_Q_NODES - 1;

/// Parse a `q` the broker wrote by joining the caller's own `q` (already
/// through [`parse_q`]) with a policy part that passed [`fits_policy_q`]
/// (threat V23). The caps are the sum of the two, so a caller `q` that
/// [`parse_q`] accepts is never refused here: the size of a predicate the
/// caller cannot see changes no answer. No byte cap: the text is rendered
/// from those two trees, and a rendered number is longer than the one
/// written (`1e300`), so bytes would count the renderer, not the caller.
pub fn parse_narrowed_q(input: &str) -> Result<QNode, NgsiError> {
    parse_capped(input, MAX_NARROWED_Q_DEPTH, MAX_NARROWED_Q_NODES)
}

/// Whether a policy `q` stays inside the share of the narrowed caps the
/// caller's part leaves it, so [`parse_narrowed_q`] accepts every join of
/// it with a caller `q` that [`parse_q`] accepts. Judged on the tree alone,
/// so the verdict is the same for every caller.
pub fn fits_policy_q(node: &QNode) -> bool {
    q_nodes(node) <= MAX_POLICY_Q_NODES && paren_depth(node) < MAX_NARROWED_Q_DEPTH
}

fn parse_capped(input: &str, depth: usize, nodes: usize) -> Result<QNode, NgsiError> {
    let mut p = Parser {
        rest: input.trim(),
        depth: 0,
        max_depth: depth,
    };
    let node = p.or_expr()?;
    if !p.rest.is_empty() {
        return Err(bad(input, "trailing input"));
    }
    if q_nodes(&node) > nodes {
        return Err(NgsiError::TooComplexQuery(format!(
            "q expression exceeds {nodes} nodes"
        )));
    }
    Ok(node)
}

/// 4.9 p.85: "`Number` shall be a number as mandated by the JSON
/// Specification, following the ABNF Grammar, production rule named `number`,
/// section 6 of IETF RFC 8259" — `[minus] int [frac] [exp]`, the int without
/// a leading zero, the fraction and the exponent with at least one digit each.
/// A float parse is much wider: `+5`, `01`, `.5`, `5.`, `NaN` and `inf` all
/// come back as numbers from it and none of them is a Number here, so each
/// would compare against a number where the term named text.
fn is_json_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
    let int = digits(s);
    if int == 0 || (int > 1 && s.starts_with('0')) {
        return false;
    }
    let rest = &s[int..];
    let rest = match rest.strip_prefix('.') {
        None => rest,
        Some(frac) => match digits(frac) {
            0 => return false,
            n => &frac[n..],
        },
    };
    match rest.strip_prefix(['e', 'E']) {
        None => rest.is_empty(),
        Some(exp) => {
            let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
            !exp.is_empty() && digits(exp) == exp.len()
        }
    }
}

fn q_nodes(n: &QNode) -> usize {
    match n {
        QNode::And(xs) | QNode::Or(xs) => xs.iter().map(q_nodes).fold(1, usize::saturating_add),
        _ => 1,
    }
}

/// The parentheses `n` renders with at its deepest, as `render.rs` writes
/// them: a group inside `;` and an `|` inside `|` are parenthesised.
fn paren_depth(n: &QNode) -> usize {
    let deepest = |xs: &[QNode], wrapped: fn(&QNode) -> bool| {
        xs.iter()
            .map(|x| paren_depth(x).saturating_add(usize::from(wrapped(x))))
            .max()
            .unwrap_or(0)
    };
    match n {
        QNode::And(xs) => deepest(xs, |x| matches!(x, QNode::And(_) | QNode::Or(_))),
        QNode::Or(xs) => deepest(xs, |x| matches!(x, QNode::Or(_))),
        _ => 0,
    }
}

fn bad(input: &str, why: &str) -> NgsiError {
    NgsiError::BadRequestData(format!("invalid q expression {input:?}: {why}"))
}

struct Parser<'a> {
    rest: &'a str,
    /// open parentheses currently on the stack (see `MAX_Q_DEPTH`)
    depth: usize,
    /// the ceiling on `depth`: `MAX_Q_DEPTH` for a caller's `q`
    max_depth: usize,
}

impl<'a> Parser<'a> {
    fn or_expr(&mut self) -> Result<QNode, NgsiError> {
        let first = self.and_expr()?;
        let mut rest = Vec::new();
        while self.eat('|') {
            rest.push(self.and_expr()?);
        }
        Ok(if rest.is_empty() {
            first
        } else {
            QNode::Or(std::iter::once(first).chain(rest).collect())
        })
    }

    fn and_expr(&mut self) -> Result<QNode, NgsiError> {
        let first = self.term()?;
        let mut rest = Vec::new();
        while self.eat(';') {
            rest.push(self.term()?);
        }
        Ok(if rest.is_empty() {
            first
        } else {
            QNode::And(std::iter::once(first).chain(rest).collect())
        })
    }

    fn term(&mut self) -> Result<QNode, NgsiError> {
        if self.eat('(') {
            // refuse BEFORE recursing — an overflow here aborts the process
            self.depth = self.depth.saturating_add(1);
            if self.depth > self.max_depth {
                return Err(NgsiError::TooComplexQuery(format!(
                    "q expression nests deeper than {}",
                    self.max_depth
                )));
            }
            let node = self.or_expr()?;
            self.depth = self.depth.saturating_sub(1);
            if !self.eat(')') {
                return Err(bad(self.rest, "expected ')'"));
            }
            return Ok(node);
        }
        let negated = self.eat('!');
        let path = self.qpath()?;
        if let Some(op) = self.cmp_op() {
            if negated {
                return Err(bad(self.rest, "'!' only prefixes an existence check"));
            }
            let value = self.value(op)?;
            Ok(QNode::Cmp { path, op, value })
        } else {
            Ok(QNode::Exists { path, negated })
        }
    }

    /// 4.9 Attribute: `attr{[T[,T]:]…}` linked-entity hops, then a dotted
    /// path, then at most one trailing `[member.path]` / `[lang]` / `[*]`.
    fn qpath(&mut self) -> Result<QPath, NgsiError> {
        let mut links = Vec::new();
        let mut braces = 0usize;
        let mut name = self.name()?;
        // LinkedEntityRelation: AttrName{ [EntityType(,EntityType)*:] … }
        while self.eat('{') {
            braces = braces.saturating_add(1);
            if braces > 8 {
                return Err(NgsiError::TooComplexQuery(
                    "q linked-entity path nests deeper than 8".into(),
                ));
            }
            let mut types = Vec::new();
            let mut inner = self.name()?;
            if self.rest.starts_with(',') || self.rest.starts_with(':') {
                types.push(inner);
                while self.eat(',') {
                    types.push(self.name()?);
                }
                if !self.eat(':') {
                    return Err(bad(self.rest, "expected ':' after EntityType hints"));
                }
                inner = self.name()?;
            }
            links.push(Link { attr: name, types });
            name = inner;
        }
        let mut path = vec![name];
        while self.eat('.') {
            path.push(self.name()?);
        }
        let bracket = if self.eat('[') {
            // spacing inside the bracket is insignificant, so `[ *a]` is
            // `[*a]` (refused) and never the member name `*a`
            self.rest = self.rest.trim_start();
            let b = if self.eat('*') {
                self.rest = self.rest.trim_start();
                vec!["*".to_owned()]
            } else {
                let mut b = vec![self.name_token()?];
                while self.eat('.') {
                    b.push(self.name_token()?);
                }
                b
            };
            if !self.eat(']') {
                return Err(bad(self.rest, "expected ']'"));
            }
            Some(b)
        } else {
            None
        };
        for _ in 0..braces {
            if !self.eat('}') {
                return Err(bad(self.rest, "expected '}'"));
            }
        }
        Ok(QPath {
            links,
            path,
            bracket,
        })
    }

    /// One path segment: everything up to a structural delimiter or a
    /// space of any kind.
    fn name_token(&mut self) -> Result<String, NgsiError> {
        self.rest = self.rest.trim_start();
        let end = self
            .rest
            .find(|c: char| c.is_whitespace() || "=!<>~;|(),.{}[]:".contains(c))
            .unwrap_or(self.rest.len());
        let (raw, rest) = self.rest.split_at(end);
        if raw.is_empty() {
            return Err(bad(rest, "expected attribute name"));
        }
        // spacing around the segment is insignificant (`a ==1`, `a ; b`)
        self.rest = rest.trim_start();
        Ok(raw.to_owned())
    }

    /// A segment that is a name in the 7.2.3.2 grammar (`AttrName`,
    /// `EntityType`): the 5.2.2.3 rule every stored name already obeys, so a
    /// query never names what no entity can hold.
    fn name(&mut self) -> Result<String, NgsiError> {
        let at = self.rest;
        let name = self.name_token()?;
        if !antares_jsonld::valid_name(&name) {
            return Err(bad(
                at,
                "an attribute or type name is a letter followed by letters, digits or '_'",
            ));
        }
        Ok(name)
    }

    fn cmp_op(&mut self) -> Option<CmpOp> {
        for (tok, op) in [
            ("==", CmpOp::Eq),
            // "!~=" before "!=" — the longer token must win the prefix race
            ("!~=", CmpOp::NotPattern),
            ("!=", CmpOp::Ne),
            ("~=", CmpOp::Pattern),
            (">=", CmpOp::Ge),
            ("<=", CmpOp::Le),
            (">", CmpOp::Gt),
            ("<", CmpOp::Lt),
        ] {
            if let Some(rest) = self.rest.strip_prefix(tok) {
                self.rest = rest;
                return Some(op);
            }
        }
        None
    }

    /// Query Term value for `op` — 4.9 p.84 pairs them precisely:
    /// `Operator ComparableValue` (ordering), `equal/unequal CompEqualityValue`
    /// (adds true/false, ValueList, Range, URI), `patternOp/notPatternOp
    /// RegExp`. Lists and ranges with an ordering or pattern operator are a
    /// grammar violation, not an empty result.
    fn value(&mut self, op: CmpOp) -> Result<QValue, NgsiError> {
        // 4.9: `patternOp`/`notPatternOp` take a `RegExp` (IEEE 1003.2), not
        // a `quotedStr`, so the operand is the pattern text as written — a
        // backslash in it belongs to the regular expression and is not an
        // RFC 8259 escape. Every other operand is a `quotedStr`, a Number, a
        // 4.6.3 dateTime/date/time or a URI.
        let regexp = matches!(op, CmpOp::Pattern | CmpOp::NotPattern);
        let first = self.scalar(regexp)?;
        let equality = matches!(op, CmpOp::Eq | CmpOp::Ne);
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix("..") {
            if !equality {
                return Err(bad(
                    self.rest,
                    "a Range is only valid with == or != (4.9 CompEqualityValue)",
                ));
            }
            self.rest = rest.trim_start();
            let hi = self.scalar(false)?;
            // Range = ComparableValue..ComparableValue: booleans excluded, and
            // an order relation needs both endpoints in one value space
            if std::mem::discriminant(&first) != std::mem::discriminant(&hi)
                || matches!(first, QValue::Bool(_))
            {
                return Err(bad(
                    self.rest,
                    "Range endpoints must be two comparable values of the same type",
                ));
            }
            return Ok(QValue::Range(Box::new(first), Box::new(hi)));
        }
        if self.rest.starts_with(',') {
            if !equality {
                return Err(bad(
                    self.rest,
                    "a ValueList is only valid with == or != (4.9 CompEqualityValue)",
                ));
            }
            let mut items = vec![first];
            while self.eat(',') {
                self.rest = self.rest.trim_start();
                items.push(self.scalar(false)?);
                self.rest = self.rest.trim_start();
            }
            return Ok(QValue::List(items));
        }
        if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Lt | CmpOp::Le)
            && matches!(first, QValue::Bool(_))
        {
            return Err(bad(
                self.rest,
                "true/false are only valid with == or != (4.9 OtherValue)",
            ));
        }
        Ok(first)
    }

    /// One scalar literal. Unquoted tokens stop at a delimiter or at `..`
    /// (the Range separator) — a decimal like `10.5` has no `..`, so
    /// `10.5..20.5` still splits at the right place.
    ///
    /// `regexp` selects the grammar the quoted form follows: a `RegExp`
    /// operand is taken as written, everything else is a `quotedStr`, i.e.
    /// "a text string as mandated by the JSON Specification, following the
    /// ABNF Grammar, production rule named String, section 7 of IETF
    /// RFC 8259" — escapes included, decoded to the text the term compares
    /// against, which is how the entity member reached the store too.
    fn scalar(&mut self, regexp: bool) -> Result<QValue, NgsiError> {
        // The clause writes its own examples with a space before the value
        // (`color!= "black", "red"`, p.90). Deciding quoted-vs-unquoted on an
        // untrimmed head read that as an unquoted token and kept the quotes
        // INSIDE the value, so the term matched a value literally spelled
        // with them.
        self.rest = self.rest.trim_start();
        if self.rest.starts_with('"') {
            if regexp {
                let rest = &self.rest[1..];
                let end = rest
                    .find('"')
                    .ok_or_else(|| bad(rest, "unterminated string"))?;
                let (s, rest) = rest.split_at(end);
                self.rest = &rest[1..];
                return Ok(QValue::Str(s.to_owned()));
            }
            let end = Self::json_string_end(self.rest)
                .ok_or_else(|| bad(self.rest, "unterminated string"))?;
            let (lit, rest) = self.rest.split_at(end);
            let s: String = serde_json::from_str(lit)
                .map_err(|_| bad(lit, "value is not an RFC 8259 String (4.9 quotedStr)"))?;
            self.rest = rest;
            return Ok(QValue::Str(s));
        }
        let stop = self
            .rest
            .find(|c: char| ";|(),".contains(c))
            .unwrap_or(self.rest.len());
        // the Range separator is looked for in this token only: across the
        // whole rest, every term rescanned the terms after it (T-3706)
        let end = self
            .rest
            .get(..stop)
            .and_then(|token| token.find(".."))
            .unwrap_or(stop);
        let (raw, rest) = self.rest.split_at(end);
        let raw = raw.trim();
        // The unquoted alternatives are `dateTime`/`date`/`time` (4.6.3), a
        // Number and a `URI` (RFC 3986). None of them admits a `\"`, so one
        // here is an unterminated `quotedStr`, not a value.
        if !regexp && raw.contains('"') {
            return Err(bad(raw, "unquoted value must not contain a quote (4.9)"));
        }
        self.rest = rest;
        match raw {
            "true" => Ok(QValue::Bool(true)),
            "false" => Ok(QValue::Bool(false)),
            // RFC 8259 section 6 lets a receiver limit the range: a Number
            // past f64 is refused, as serde_json refuses it in a body, so no
            // comparison runs against a value no Entity can hold
            _ if is_json_number(raw) => match raw.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(QValue::Num(n)),
                _ => Err(bad(raw, "Number is out of range (RFC 8259 section 6)")),
            },
            // the other unquoted alternatives: a URI, a dateTime, a date or a
            // time, all of them compared as text
            "" => Err(bad(raw, "expected value")),
            // A value that starts with an operator's character is a typo of
            // the operator (`speed>>`), never the text ">" (T-4084).
            _ if !regexp && raw.starts_with(['<', '>', '=', '!', '~']) => Err(bad(
                raw,
                "an unquoted value starts with no operator character (4.9); quote a text",
            )),
            _ => Ok(QValue::Str(raw.to_owned())),
        }
    }

    /// Byte index one past the closing quote of the RFC 8259 string at the head
    /// of `s`, or `None` when it never closes. A quote is the closing one only
    /// when it is not itself escaped, so the scan skips the byte after every
    /// backslash — both are ASCII, so a skip can never land mid-character in a
    /// way that reads as either.
    fn json_string_end(s: &str) -> Option<usize> {
        let b = s.as_bytes();
        let mut i = 1usize;
        while let Some(&c) = b.get(i) {
            match c {
                b'\\' => i = i.saturating_add(2),
                b'"' => return i.checked_add(1),
                _ => i = i.saturating_add(1),
            }
        }
        None
    }

    fn eat(&mut self, c: char) -> bool {
        if let Some(rest) = self.rest.strip_prefix(c) {
            self.rest = rest;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-4084: an unquoted value that starts with an operator's character is
    /// a typo of the operator, refused rather than compared as text.
    #[test]
    fn an_unquoted_value_starting_with_an_operator_is_refused() {
        for q in ["speed>>", "speed>=>", "speed==>1", "speed!=<"] {
            assert!(parse_q(q).is_err(), "{q}");
        }
        for q in [
            "speed>1",
            "speed>-1",
            "name==urn:ngsi-ld:Car:1",
            "observedAt>2026-01-01T00:00:00Z",
            "name==\">\"",
            "name~=>.*",
        ] {
            assert!(parse_q(q).is_ok(), "{q}");
        }
    }

    /// RFC 3986 2.1: "%" and exactly two hex digits; anything shorter or
    /// signed stays as written, at the end of the input too (T-3468: the
    /// decoder walks the bytes without indexing past them).
    #[test]
    fn percent_decode_takes_two_hex_digits_and_nothing_else() {
        for (input, want) in [
            ("", ""),
            ("%", "%"),
            ("%4", "%4"),
            ("a%41", "aA"),
            ("%41%", "A%"),
            ("%+1", "%+1"),
            ("%zz%2", "%zz%2"),
            ("%C3%A9", "\u{e9}"),
            ("\u{e9}%20x", "\u{e9} x"),
        ] {
            assert_eq!(percent_decode(input.as_bytes()), want, "{input:?}");
        }
    }

    /// 4.9 p.85: "`Number` shall be a number as mandated by the JSON
    /// Specification, following the ABNF Grammar, production rule named
    /// `number`, section 6 of IETF RFC 8259" — an optional minus, an int with
    /// no leading zero, a fraction of at least one digit, an exponent of at
    /// least one digit. A plain float parse is far wider than that: it takes
    /// `+5`, `01`, `.5`, `5.`, `NaN` and `inf`, none of which is a Number.
    /// The other unquoted alternatives of `ComparableValue` and
    /// `CompEqualityValue` are `dateTime`/`date`/`time` and `URI`, all of
    /// which compare as text, so a token that is not a Number is a String.
    #[test]
    fn an_unquoted_token_is_a_number_only_when_rfc_8259_says_so() {
        for (q, want) in [
            ("x==5", QValue::Num(5.0)),
            ("x==-5", QValue::Num(-5.0)),
            ("x==0", QValue::Num(0.0)),
            ("x==0.5", QValue::Num(0.5)),
            ("x==1e3", QValue::Num(1000.0)),
            ("x==1E+3", QValue::Num(1000.0)),
            ("x==-2.5e-2", QValue::Num(-0.025)),
            // every one of these a plain float parse accepts and RFC 8259
            // does not
            ("x==+5", QValue::Str("+5".into())),
            ("x==01", QValue::Str("01".into())),
            ("x==.5", QValue::Str(".5".into())),
            ("x==5.", QValue::Str("5.".into())),
            ("x==NaN", QValue::Str("NaN".into())),
            ("x==nan", QValue::Str("nan".into())),
            ("x==inf", QValue::Str("inf".into())),
            ("x==-inf", QValue::Str("-inf".into())),
            ("x==infinity", QValue::Str("infinity".into())),
            ("x==1e", QValue::Str("1e".into())),
            ("x==-", QValue::Str("-".into())),
        ] {
            let node = parse_q(q).expect(q);
            assert!(
                matches!(&node, QNode::Cmp { value, .. } if *value == want),
                "{q}: {node:?}"
            );
        }
    }

    /// RFC 8259 section 6: "This specification allows implementations to
    /// set limits on the range and precision of numbers accepted." A Number
    /// past f64 is refused, as an entity body holding it is (serde_json);
    /// read as infinity it would compare against values no Entity can hold
    /// and render as `inf`, which parses back as a String (found by the
    /// parse_q fuzz target, fuzz/regressions/parse_q).
    /// 4.9 `[*]` is the whole bracket, never the start of a member name,
    /// and spacing inside the bracket is insignificant: `m[ *a]` was read
    /// as the member `*a` and rendered as `m[*a]`, which is refused (found
    /// by the parse_q fuzz target, fuzz/regressions/parse_q).
    #[test]
    fn a_star_in_a_bracket_is_the_whole_bracket_whatever_the_spacing() {
        for q in ["m[*a]!=c", "m[ *a]!=c", "m[\t*a]!=c"] {
            assert!(parse_q(q).is_err(), "{q:?}");
        }
        for q in ["m[*]==\"x\"", "m[ * ]==\"x\"", "m[\t*\t]==\"x\""] {
            let node = parse_q(q).expect(q);
            assert_eq!(node.to_string(), "m[*]==\"x\"", "{q:?}");
        }
    }

    #[test]
    fn a_number_past_the_float_range_is_refused() {
        for q in ["A>8e44444", "x==-1e400", "x==1e309..2", "x==1..1E+999"] {
            assert!(
                matches!(parse_q(q), Err(NgsiError::BadRequestData(_))),
                "{q}: {:?}",
                parse_q(q)
            );
        }
        // below the range is not past it: an underflow is the nearest value
        assert_eq!(
            parse_q("x==1e-400").expect("underflow"),
            parse_q("x==0").expect("zero")
        );
    }

    #[test]
    fn simple_comparison() {
        let q = parse_q(r#"brandName=="Mercedes""#).expect("parse");
        assert_eq!(
            q,
            QNode::Cmp {
                path: QPath::dotted(vec!["brandName".into()]),
                op: CmpOp::Eq,
                value: QValue::Str("Mercedes".into())
            }
        );
    }

    #[test]
    fn and_or_precedence() {
        // `a==1;b==2|c==3` == (a AND b) OR c per grammar: OR binds looser
        let q = parse_q("a==1;b==2|c==3").expect("parse");
        assert!(
            matches!(&q, QNode::Or(items) if items.len() == 2 && matches!(items[0], QNode::And(_))),
            "{q:?}"
        );
    }

    #[test]
    fn dotted_path_and_numbers() {
        let q = parse_q("speed.value>=80.5").expect("parse");
        assert_eq!(
            q,
            QNode::Cmp {
                path: QPath::dotted(vec!["speed".into(), "value".into()]),
                op: CmpOp::Ge,
                value: QValue::Num(80.5)
            }
        );
    }

    /// TS 104 175 7.2.3.2 / 5.2.2.3: an attribute path, a linked-entity
    /// attribute and an EntityType hint are names (`unicodeLetter
    /// *(unicodeLetter / unicodeNumber / "_")`); anything else is
    /// BadRequestData rather than a name no entity can hold. Found by the
    /// parse_q fuzz target's render round-trip (T-3463):
    /// `(?\n<&3|$$\t)` parsed as the attributes "?\n" and "$$", and the
    /// tab before `)` was dropped where the one at the end was kept.
    #[test]
    fn attribute_names_follow_the_name_grammar() {
        for refused in [
            "(?\n<&3|$$\t)",
            "$$",
            "brand-name==1",
            "1st==1",
            "a\tb==1",
            "speed.$x==1",
            "rel{T-1:name}==1",
            "rel-x{name}==1",
        ] {
            assert!(
                matches!(parse_q(refused), Err(NgsiError::BadRequestData(_))),
                "{refused:?} parsed"
            );
        }
        for accepted in [
            "speed==1",
            "speed\t==1",
            "(speed\t)",
            "temperature_2==1",
            "größe>1",
            "owner{works{name}}==\"x\"",
            "rel{Building,Room:name}",
            "color[en-GB]==\"red\"",
            "address[city]==\"Berlin\"",
            "speed.observedAt>\"2020-01-01T00:00:00Z\"",
        ] {
            assert!(
                parse_q(accepted).is_ok(),
                "{accepted:?} refused: {:?}",
                parse_q(accepted)
            );
        }
    }

    #[test]
    fn existence_and_negation() {
        assert_eq!(
            parse_q("!temperature").expect("parse"),
            QNode::Exists {
                path: QPath::dotted(vec!["temperature".into()]),
                negated: true
            }
        );
    }

    #[test]
    fn parens_group() {
        let q = parse_q("(a==1|b==2);c==3").expect("parse");
        assert!(matches!(q, QNode::And(_)));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "==5", "a==\"unterminated", "a==1)"] {
            assert!(parse_q(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn value_list_parses_with_equality_ops_only() {
        // 4.9 p.85 ValueList = Value 1*(, Value); p.84 pairs it with ==/!= only
        let q = parse_q(r#"color=="black","red""#).expect("parse");
        assert_eq!(
            q,
            QNode::Cmp {
                path: QPath::dotted(vec!["color".into()]),
                op: CmpOp::Eq,
                value: QValue::List(vec![QValue::Str("black".into()), QValue::Str("red".into())])
            }
        );
        // spec's own spacing (`color!= "black", "red"`, p.90) must parse to
        // the same values — the space may not end up inside them, and neither
        // may the quotes that delimit them
        assert_eq!(
            parse_q(r#"color!= "black", "red""#).expect("parse"),
            QNode::Cmp {
                path: QPath::dotted(vec!["color".into()]),
                op: CmpOp::Ne,
                value: QValue::List(vec![QValue::Str("black".into()), QValue::Str("red".into())])
            }
        );
        assert_eq!(
            parse_q(r#"color==  "black""#).expect("parse"),
            QNode::Cmp {
                path: QPath::dotted(vec!["color".into()]),
                op: CmpOp::Eq,
                value: QValue::Str("black".into())
            },
            "a single spaced value keeps neither the space nor the quotes"
        );
        // mixed scalar kinds are legal (ValueList is over Value)
        assert!(parse_q("a==1,2,3").is_ok());
        // ordering + list is a grammar violation → 400, not empty result
        assert!(parse_q(r#"a>"x","y""#).is_err());
        assert!(parse_q("a>=1,2").is_err());
    }

    #[test]
    fn range_parses_with_equality_ops_only() {
        // 4.9 p.85 Range = ComparableValue dots ComparableValue
        let q = parse_q("temperature==10..20").expect("parse");
        assert_eq!(
            q,
            QNode::Cmp {
                path: QPath::dotted(vec!["temperature".into()]),
                op: CmpOp::Eq,
                value: QValue::Range(Box::new(QValue::Num(10.0)), Box::new(QValue::Num(20.0)))
            }
        );
        // decimals keep their fraction; `..` is not mistaken for `.`
        let q = parse_q("t!=10.5..20.5").expect("parse");
        assert!(matches!(
            q,
            QNode::Cmp {
                op: CmpOp::Ne,
                value: QValue::Range(_, _),
                ..
            }
        ));
        // DateTime endpoints (unquoted, per EXAMPLE 8 style literals)
        let q = parse_q("observedAt==2021-01-01T00:00:00Z..2021-02-01T00:00:00Z").expect("parse");
        assert!(matches!(
            q,
            QNode::Cmp {
                value: QValue::Range(_, _),
                ..
            }
        ));
        // ordering + range violates the grammar; bools are not ComparableValue
        assert!(parse_q("a>1..5").is_err());
        assert!(parse_q("a==true..false").is_err());
        assert!(parse_q("a==1..\"x\"").is_err(), "mixed-type endpoints");
    }

    #[test]
    fn not_pattern_op() {
        // 4.9 p.85 notPatternOp = !~=
        let q = parse_q(r#"name!~="^Merc""#).expect("parse");
        assert_eq!(
            q,
            QNode::Cmp {
                path: QPath::dotted(vec!["name".into()]),
                op: CmpOp::NotPattern,
                value: QValue::Str("^Merc".into())
            }
        );
    }

    /// 4.9: `quotedStr = String`, and `String` "shall be a text string as
    /// mandated by the JSON Specification, following the ABNF Grammar,
    /// production rule named String, section 7 of IETF RFC 8259" — whose
    /// `char` production is `unescaped / escape (…)`. So a Query Term value
    /// may carry an escaped quote, an escaped backslash, the two-character
    /// control escapes and `\uXXXX`, and the value the term compares against
    /// is the DECODED text: the entity member it is matched to was decoded by
    /// the JSON parser on the way in.
    #[test]
    fn a_quoted_string_is_an_rfc_8259_string() {
        for (q, want) in [
            (r#"a=="say \"hi\"""#, "say \"hi\""),
            (r#"a=="back\\slash""#, "back\\slash"),
            (r#"a=="line\nbreak""#, "line\nbreak"),
            (r#"a=="tab\there""#, "tab\there"),
            (r#"a=="caf\u00e9""#, "café"),
            (r#"a=="sl\/ash""#, "sl/ash"),
            (r#"a=="""#, ""),
        ] {
            let node = parse_q(q).unwrap_or_else(|e| panic!("{q}: {e:?}"));
            assert!(
                matches!(&node, QNode::Cmp { value: QValue::Str(s), .. } if s == want),
                "{q}: {node:?}"
            );
        }
    }

    /// The escape only ends the string when it is not itself escaped: a
    /// trailing `\"` continues the literal, and `\\` before the closing
    /// quote does not.
    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        assert!(
            parse_q(r#"a=="unterminated\""#).is_err(),
            "an escaped quote leaves the string open"
        );
        let node = parse_q(r#"a=="ends with a backslash\\";b"#).expect("parses");
        assert!(
            matches!(&node, QNode::And(items)
                if matches!(&items[0], QNode::Cmp { value: QValue::Str(s), .. } if s == "ends with a backslash\\")),
            "{node:?}"
        );
    }

    /// An escape RFC 8259 does not define is not a String, so the term is
    /// not a Query Term — refused rather than silently read as two
    /// characters.
    #[test]
    fn an_undefined_escape_is_not_a_string() {
        for q in [r#"a=="bad\x""#, r#"a=="short\u12""#, r#"a=="\u12zz""#] {
            assert!(parse_q(q).is_err(), "{q} must not parse");
        }
    }

    /// The unquoted alternatives of the grammar are `dateTime`/`date`/`time`
    /// (4.6.3) and `URI` (RFC 3986), and a raw `"` belongs to none of them.
    /// Accepting one produced a value that could not be written back as a
    /// Query Term at all.
    #[test]
    fn an_unquoted_value_may_not_carry_a_bare_quote() {
        for q in [r#"a==x"y"#, r#"a==urn:x:"y"#, r#"a>2020-01-01T00:00:"0Z"#] {
            assert!(parse_q(q).is_err(), "{q} must not parse");
        }
    }

    #[test]
    fn bool_with_ordering_op_is_a_grammar_violation() {
        // p.84: Operator (ordering) takes ComparableValue; true/false are
        // OtherValue, reachable only through ==/!=
        assert!(parse_q("a>true").is_err());
        assert!(parse_q("a==true").is_ok());
    }
}

#[cfg(test)]
mod complexity_tests {
    use super::*;

    #[test]
    fn q_complexity_cap_is_403_class() {
        // >512 nodes → TooComplexQuery, small trees untouched.
        let ok = "a==1;b==2|c==3";
        assert!(parse_q(ok).is_ok());
        let huge = (0..600)
            .map(|i| format!("a{i}==1"))
            .collect::<Vec<_>>()
            .join(";");
        let answer = parse_q(&huge);
        assert!(
            matches!(answer, Err(NgsiError::TooComplexQuery(_))),
            "{answer:?}"
        );
    }

    /// Regression: the parser once recursed per `(` with
    /// no depth counter. A Rust stack overflow is a guard-page ABORT, not a
    /// catchable panic — no tower layer can contain it — so ~4000 parens in a
    /// query string killed the whole broker process, and a percent-encoded
    /// copy stored in a subscription made that a restart-surviving crash loop.
    #[test]
    #[cfg_attr(miri, ignore)] // 50k parens: nine minutes under the interpreter
    fn deep_nesting_is_refused_before_it_can_overflow_the_stack() {
        let deep = format!("{}a==1{}", "(".repeat(50_000), ")".repeat(50_000));
        assert!(
            matches!(parse_q(&deep), Err(NgsiError::TooComplexQuery(_))),
            "deep nesting must be a 403, never an abort"
        );
        // the length cap fires first on that one; check depth alone too
        let deep = format!("{}a==1{}", "(".repeat(300), ")".repeat(300));
        assert!(deep.len() < MAX_Q_BYTES);
        assert!(matches!(parse_q(&deep), Err(NgsiError::TooComplexQuery(_))));
        // and ordinary grouping still parses
        assert!(parse_q("((a==1|b==2);c==3)").is_ok());
    }

    #[test]
    fn overlong_q_is_refused_at_the_entry_point() {
        // a POST query body carries `q` too, where the URI cap does not apply
        let long = format!("a=={}", "x".repeat(MAX_Q_BYTES));
        assert!(matches!(parse_q(&long), Err(NgsiError::TooComplexQuery(_))));
    }

    /// The depth ceiling is exact and counts open parentheses, not the
    /// parentheses seen: sibling groups close what they open, so a query may
    /// carry any number of them.
    #[test]
    fn depth_cap_is_exact_and_counts_only_open_parens() {
        let at_cap = format!("{}a==1{}", "(".repeat(MAX_Q_DEPTH), ")".repeat(MAX_Q_DEPTH));
        assert!(parse_q(&at_cap).is_ok(), "{MAX_Q_DEPTH} nested must parse");
        let over = format!(
            "{}a==1{}",
            "(".repeat(MAX_Q_DEPTH + 1),
            ")".repeat(MAX_Q_DEPTH + 1)
        );
        assert!(matches!(parse_q(&over), Err(NgsiError::TooComplexQuery(_))));
        let siblings = (0..200)
            .map(|i| format!("(a{i}==1)"))
            .collect::<Vec<_>>()
            .join(";");
        assert!(siblings.len() < MAX_Q_BYTES);
        assert!(parse_q(&siblings).is_ok(), "siblings are not cumulative");
    }

    /// The parser is a fuzz target: on any input it returns, and the error it
    /// returns is safe to hand back — the input is echoed Debug-escaped, so a
    /// rejected `q` cannot carry a raw CR/LF into a response or a log line.
    #[test]
    fn hostile_input_is_total_and_its_error_is_escaped() {
        for hostile in [
            "",
            " ",
            "\"",
            "\"\"",
            "a==\"",
            "..",
            "a==..",
            "a==1..",
            "a==..1",
            "a.",
            "a[",
            "a[]",
            "a{",
            "a{}",
            "a{b",
            "a{,:b}",
            ";",
            "|",
            "()",
            "(((",
            ")))",
            "!",
            "!!a",
            "a==1,",
            "a==,1",
            "~=",
            "a~=",
            "a!~=",
            "a==1)b",
            "\u{202e}==1",
            "ä==1",
            "a==\"ä",
            "温度.値>=1",
            "a\u{2028}==1",
            "a=={}",
            "a==1e999",
            "a==-0",
            "a==NaN..1",
            "a{b:c}{d:e}.f[g].h==1",
            &"{".repeat(64),
            &"[".repeat(64),
            &"a{".repeat(64),
            &".".repeat(64),
            &"!".repeat(64),
            &",".repeat(64),
        ] {
            match parse_q(hostile) {
                Ok(_) => {}
                Err(e) => {
                    let msg = e.to_string();
                    assert!(
                        !msg.chars().any(char::is_control),
                        "error text must stay escaped for {hostile:?}: {msg:?}"
                    );
                }
            }
        }
    }

    /// `!` is the existence-check prefix only (4.9): pairing it with a
    /// comparison is a grammar violation, not a silently ignored negation.
    #[test]
    fn negated_comparison_is_a_grammar_violation() {
        assert!(matches!(
            parse_q("!a==1"),
            Err(NgsiError::BadRequestData(_))
        ));
        assert!(matches!(
            parse_q("!a"),
            Ok(QNode::Exists { negated: true, .. })
        ));
        // `!=` after a path is the operator, not a negation prefix
        assert!(matches!(
            parse_q("a!=1"),
            Ok(QNode::Cmp { op: CmpOp::Ne, .. })
        ));
    }

    /// 4.17: `,`/`|` is OR, `(a;b)` is AND inside one alternative; names are
    /// expanded against the @context before they are compared.
    #[test]
    fn type_selection_or_and_and() {
        let ctx = crate::test_core();
        let d = |t: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{t}");
        let (car, bus) = (d("Car"), d("Bus"));
        let both = [car.as_str(), bus.as_str()];
        let car_only = [car.as_str()];
        assert!(type_selection_matches("Car", &car_only, &ctx));
        assert!(type_selection_matches("Bus,Car", &car_only, &ctx));
        assert!(type_selection_matches("Bus|Car", &car_only, &ctx));
        assert!(type_selection_matches("(Car;Bus)", &both, &ctx));
        assert!(!type_selection_matches("(Car;Bus)", &car_only, &ctx));
        assert!(!type_selection_matches("Bus", &[], &ctx));
    }

    /// 4.17 grammar: no empty EntityType, parentheses only around an
    /// and-group, `;` only inside one (Coraine query_entities_errors).
    #[test]
    fn type_selection_grammar() {
        for ok in [
            "Car",
            "Bus,Car",
            "Bus|Car",
            "(Car;Bus)",
            "(Home;Vehicle)|Motorhome",
            "https://e.org/T",
            "(A)",
        ] {
            assert!(check_type_selection(ok).is_ok(), "{ok}");
        }
        for bad in [
            "(A;)", "A;", ";A", "()", "A,", ",A", "A;;B", "(A;B", "A)", "((A))", "",
        ] {
            assert!(check_type_selection(bad).is_err(), "{bad}");
        }
    }

    /// 5.6.21.4: the referenced top-level names in source order, whether any
    /// path hops a Linked Entity, and the deepest hop chain.
    #[test]
    fn referenced_paths_and_link_depth() {
        let q = parse_q("speed>1;(brand==\"x\"|sensor{humidity}==2);!owner").expect("parse");
        assert_eq!(q.attribute_paths(), ["speed", "brand", "sensor", "owner"]);
        assert!(q.has_linked_paths());
        assert_eq!(q.max_link_depth(), 1);
        let flat = parse_q("speed>1|brand==\"x\"").expect("parse");
        assert!(!flat.has_linked_paths());
        assert_eq!(flat.max_link_depth(), 0);
        assert!(is_non_system_attr("speed"));
        for sys in SYSTEM_ATTRS {
            assert!(!is_non_system_attr(sys), "{sys}");
        }
    }

    /// The unquoted value: `false` is a Boolean, an empty value is refused,
    /// and a `..` past the next separator belongs to the next term.
    #[test]
    fn unquoted_scalars_and_where_a_range_stops() {
        assert!(matches!(
            parse_q("a==false").expect("parse"),
            QNode::Cmp {
                value: QValue::Bool(false),
                ..
            }
        ));
        assert!(parse_q("a==").is_err());
        assert!(parse_q("a==;b==1").is_err());
        let range = parse_q("a==1;b==2..3").expect("parse");
        assert_eq!(range, parse_q("(a==1);(b==2..3)").expect("parse"));
        assert!(
            matches!(&range, QNode::And(terms) if terms.len() == 2),
            "{range:?}"
        );
    }

    /// The byte ceiling is inclusive: MAX_Q_BYTES parses, one more is
    /// TooComplexQuery (T-3531 mutant: `>` → `>=`).
    #[test]
    fn a_query_of_exactly_the_byte_ceiling_parses() {
        let at = |n: usize| format!("a==\"{}\"", "x".repeat(n - 5));
        assert_eq!(at(MAX_Q_BYTES).len(), MAX_Q_BYTES);
        assert!(parse_q(&at(MAX_Q_BYTES)).is_ok());
        assert!(matches!(
            parse_q(&at(MAX_Q_BYTES + 1)),
            Err(NgsiError::TooComplexQuery(_))
        ));
    }

    /// The node budget: a flat chain within the depth limit but past
    /// MAX_Q_NODES is 400 TooComplexQuery, one node fewer parses.
    #[test]
    fn a_query_past_the_node_budget_is_too_complex() {
        let chain = |n: usize| vec!["a==1"; n].join(";");
        assert!(parse_q(&chain(MAX_Q_NODES - 1)).is_ok());
        assert!(matches!(
            parse_q(&chain(MAX_Q_NODES)),
            Err(NgsiError::TooComplexQuery(_))
        ));
    }

    /// Threat V23: the narrowed caps are the caller's and the policy's
    /// summed. The largest, the deepest and the longest-rendering `q` a
    /// caller may send, each term rewritten as a hidden one (`(a;!a)`) and
    /// joined with the largest and the deepest policy `q` that fits, still
    /// parse, so the policy's size decides no caller's answer.
    #[test]
    #[cfg_attr(miri, ignore)] // a cap-sized input: past the weekly Miri job's 90 min (T-3706)
    fn every_join_of_an_accepted_q_and_a_fitting_policy_parses() {
        fn hidden(n: QNode) -> QNode {
            match n {
                QNode::And(xs) => QNode::And(xs.into_iter().map(hidden).collect()),
                QNode::Or(xs) => QNode::Or(xs.into_iter().map(hidden).collect()),
                _ => QNode::And(vec![parse_q("a").expect("a"), parse_q("!a").expect("!a")]),
            }
        }
        let leaf = || parse_q("b==1").expect("leaf");
        let mut deep = "a==1".to_owned();
        for i in 0..MAX_Q_DEPTH {
            deep = format!("a==1{}({deep})", if i % 2 == 0 { ';' } else { '|' });
        }
        let callers = [
            vec!["a==1"; MAX_Q_NODES - 1].join(";"),
            deep,
            vec!["a==1e300"; MAX_Q_BYTES / 9].join(";"),
        ];
        let wide = QNode::Or(vec![leaf(); MAX_POLICY_Q_NODES - 1]);
        // bounded, so a `paren_depth` that stops counting fails here rather
        // than loop (T-3684: the mutants that did timed out)
        let mut tall = QNode::And(vec![leaf(), leaf()]);
        for _ in 0..MAX_NARROWED_Q_DEPTH {
            if paren_depth(&tall) >= MAX_NARROWED_Q_DEPTH - 1 {
                break;
            }
            tall = QNode::And(vec![leaf(), tall]);
        }
        assert_eq!(paren_depth(&tall), MAX_NARROWED_Q_DEPTH - 1);
        for policy in [&wide, &tall] {
            assert!(fits_policy_q(policy));
            for caller in &callers {
                let own = parse_q(caller).expect("an accepted q");
                for own in [hidden(own.clone()), own] {
                    let joined = QNode::And(vec![own, policy.clone()]).to_string();
                    assert!(parse_narrowed_q(&joined).is_ok(), "{}", &joined[..64]);
                }
            }
        }
        // the last rendering is past the caller's byte cap, which does not
        // apply to the broker's own text
        assert!(callers[2].len() <= MAX_Q_BYTES);
        assert!(parse_q(&callers[2]).expect("q").to_string().len() > MAX_Q_BYTES);
        // one node or one level more is a policy that does not fit
        assert!(!fits_policy_q(&QNode::Or(vec![leaf(); MAX_POLICY_Q_NODES])));
        assert!(!fits_policy_q(&QNode::And(vec![leaf(), tall])));
    }

    /// `paren_depth` counts the parentheses the renderer writes: a group
    /// inside `;`, an `|` inside `|`, never a `;` inside `|`.
    #[test]
    fn paren_depth_is_the_rendered_one() {
        for (q, depth) in [
            ("a", 0),
            ("a|b;c", 0),
            ("a;(b|c)", 1),
            ("a|(b|c)", 1),
            ("a;(b;(c|d))", 2),
        ] {
            let node = parse_q(q).expect("q");
            assert_eq!(paren_depth(&node), depth, "{q}");
            let rendered = node.to_string();
            let mut open = 0usize;
            let deepest = rendered.chars().fold(0, |deepest, c| {
                open = match c {
                    '(' => open + 1,
                    ')' => open - 1,
                    _ => open,
                };
                deepest.max(open)
            });
            assert_eq!(deepest, depth, "{rendered}");
        }
    }

    /// How the narrowed caps are made up (T-3684): the depth is the
    /// caller's twice over, half for the caller's part and its rewrite,
    /// half for the policy; the nodes are the caller's rewritten part (each
    /// term at most three), the conjunction, and the policy share, to the
    /// node. Each cap is used symbolically everywhere else, so only these
    /// equations notice one drifting.
    #[test]
    fn the_narrowed_caps_add_up() {
        assert_eq!(MAX_NARROWED_Q_DEPTH, 2 * MAX_Q_DEPTH);
        assert_eq!(
            3 * MAX_Q_NODES + 1 + MAX_POLICY_Q_NODES,
            MAX_NARROWED_Q_NODES
        );
    }

    /// Parsing grows with the `q`, not with its square: each unquoted value
    /// once looked for the Range `..` across the whole rest, so a chain of
    /// `n` comparisons scanned n²/2 terms (T-3706: the Miri job ran out of
    /// time inside one 511-term parse). Eight parses of a chain against one
    /// parse of a chain eight times as long: linear, the two take the same
    /// time (measured 1.04 to 1.24, also beside twelve busy cores); the old
    /// scan took 5.1 times as long. Both are timed alternately and of about
    /// the same length, so a loaded host slows both alike (T-4118: one small
    /// and one large parse drifted from 9 to 18 under load, against a bound
    /// of 20).
    #[test]
    #[cfg_attr(miri, ignore)] // a wall-clock ratio, measured natively
    fn parsing_a_chain_grows_linearly() {
        use std::time::{Duration, Instant};
        let chain = |n: usize| vec!["a==1"; n].join(";");
        let (small, large) = (chain(MAX_Q_NODES - 1), chain(MAX_NARROWED_Q_NODES - 1));
        let (mut eight_small, mut one_large) = (Duration::MAX, Duration::MAX);
        for _ in 0..31 {
            let started = Instant::now();
            for _ in 0..8 {
                assert!(parse_narrowed_q(&small).is_ok());
            }
            eight_small = eight_small.min(started.elapsed());
            let started = Instant::now();
            assert!(parse_narrowed_q(&large).is_ok());
            one_large = one_large.min(started.elapsed());
        }
        let ratio = one_large.as_secs_f64() / eight_small.as_secs_f64().max(1e-9);
        assert!(
            ratio < 2.5,
            "one 8x chain took {ratio:.2}x eight short ones"
        );
    }

    /// The narrowed caps are ceilings too: past them a narrowed `q` is
    /// TooComplexQuery, as a caller's is past [`MAX_Q_NODES`].
    #[test]
    #[cfg_attr(miri, ignore)] // a cap-sized input: past the weekly Miri job's 90 min (T-3706)
    fn a_narrowed_q_past_its_caps_is_too_complex() {
        let chain = |n: usize| vec!["a==1"; n].join(";");
        assert!(parse_narrowed_q(&chain(MAX_NARROWED_Q_NODES - 1)).is_ok());
        assert!(matches!(
            parse_narrowed_q(&chain(MAX_NARROWED_Q_NODES)),
            Err(NgsiError::TooComplexQuery(_))
        ));
        let nested = |d: usize| format!("{}a==1{}", "(".repeat(d), ")".repeat(d));
        assert!(parse_narrowed_q(&nested(MAX_NARROWED_Q_DEPTH)).is_ok());
        assert!(matches!(
            parse_narrowed_q(&nested(MAX_NARROWED_Q_DEPTH + 1)),
            Err(NgsiError::TooComplexQuery(_))
        ));
        assert!(matches!(
            parse_q(&nested(MAX_Q_DEPTH + 1)),
            Err(NgsiError::TooComplexQuery(_))
        ));
    }
}

/// Two refusals of the grammar no other test spells, `<=` written back, and a `scope` member
/// of neither shape 4.19 reads (T-3912).
#[cfg(test)]
mod edges {
    use super::*;

    #[test]
    fn grammar_edges_and_the_written_form() {
        let said = |q: &str| parse_q(q).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            said("sensor{Device,Car}==1").contains("expected ':'"),
            "{}",
            said("sensor{Device,Car}==1")
        );
        assert!(
            said(r#"a~="abc"#).contains("unterminated string"),
            "{}",
            said(r#"a~="abc"#)
        );
        assert_eq!(parse_q("a<=1").expect("parse").to_string(), "a<=1");
        assert!(!scope::scope_matches(
            "/a",
            &serde_json::json!({"scope": 5})
        ));
    }
}
