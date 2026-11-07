// SPDX-License-Identifier: EUPL-1.2
//! `commonExpr` and `boolCommonExpr` (ABNF section 4) for `$filter` and `$orderby`.
//!
//! The ABNF lists operators without precedence; the tree follows URL Conventions 5.1.1.16:
//! `not` and negation bind tightest, then `mul div divby mod`, then `add sub`, then
//! `gt ge lt le`, then `eq ne in`, then `and`, then `or`, each binary one left-associative.

use crate::cursor::{count, Cursor};
use crate::literal::literal;
use crate::model::{Member, Model};
use crate::{
    ArithOp, CompareOp, Expr, Feature, LimitKind, Limits, Literal, MemberPath, Method, UrlError,
    UrlErrorKind,
};

/// Counters shared by every expression of one request.
pub(crate) struct Budget<'l> {
    pub(crate) limits: &'l Limits,
    pub(crate) nodes: usize,
    /// `$apply` sequences open around the one being parsed: a `concat` branch is one level
    /// deeper than the sequence holding it (T-4017).
    pub(crate) apply_depth: usize,
    /// The `$compute` names, which resolve like primitive members of the resource.
    pub(crate) computed: Vec<String>,
}

/// What a path can name from where it starts: the members of a structured type, or nothing
/// (a primitive, or a collection of primitives).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'m> {
    Type(&'m str),
    Primitive,
}

/// Lambda variables in scope, innermost last, with what each ranges over.
type Variables<'m> = Vec<(String, Scope<'m>)>;

struct Parser<'c, 'a, 'm, M: Model> {
    c: &'c mut Cursor<'a>,
    model: &'m M,
    limits: &'c Limits,
    nodes: &'c mut usize,
    computed: &'c [String],
    /// What names without a lambda variable resolve against, inside lambdas too.
    root: Scope<'m>,
    variables: Variables<'m>,
    depth: usize,
}

/// One `commonExpr` over the members of `scope`.
pub(crate) fn common_expr<'m, M: Model>(
    c: &mut Cursor<'_>,
    model: &'m M,
    scope: Scope<'m>,
    budget: &mut Budget<'_>,
) -> Result<Expr, UrlError> {
    let mut p = Parser {
        c,
        model,
        limits: budget.limits,
        nodes: &mut budget.nodes,
        computed: &budget.computed,
        root: scope,
        variables: Vec::new(),
        depth: 0,
    };
    p.or_expr(scope)
}

const RELATIONAL: [(&str, CompareOp); 4] = [
    ("gt", CompareOp::Gt),
    ("ge", CompareOp::Ge),
    ("lt", CompareOp::Lt),
    ("le", CompareOp::Le),
];
const EQUALITY: [(&str, CompareOp); 2] = [("eq", CompareOp::Eq), ("ne", CompareOp::Ne)];
const ADDITIVE: [ArithOp; 2] = [ArithOp::Add, ArithOp::Sub];
/// `divby` before `div`: the operator word must be followed by white space, so the order only
/// saves a failed attempt.
const MULTIPLICATIVE: [ArithOp; 4] = [ArithOp::Mul, ArithOp::DivBy, ArithOp::Div, ArithOp::Mod];

/// Method names of `methodCallExpr` the parser does not implement: `totalseconds` reads a
/// duration, which has no NGSI-LD value type (F27); `geo.distance` and `geo.length` measure,
/// which ADR-0028 rule 8 keeps out of the residual; `hassubset`, `hassubsequence` and `case`
/// have no evaluator.
const OTHER_METHODS: [&str; 6] = [
    "totalseconds",
    "geo.distance",
    "geo.length",
    "hassubset",
    "hassubsequence",
    "case",
];

impl<'m, M: Model> Parser<'_, '_, 'm, M> {
    fn node(&mut self) -> Result<(), UrlError> {
        count(
            self.nodes,
            self.limits.max_expr_nodes,
            LimitKind::ExprNodes,
            self.c,
        )
    }

    fn enter(&mut self) -> Result<(), UrlError> {
        count(
            &mut self.depth,
            self.limits.max_expr_depth,
            LimitKind::ExprDepth,
            self.c,
        )
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// `RWS word RWS` at the cursor, consumed when it is there; the cursor stays otherwise.
    fn operator(&mut self, word: &str) -> bool {
        let start = self.c.pos();
        if self.c.rws() && self.c.eat_ci(word) && self.c.rws() {
            return true;
        }
        self.c.reset(start);
        false
    }

    fn or_expr(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.and_expr(scope)?;
        while self.operator("or") {
            let right = self.and_expr(scope)?;
            self.node()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and_expr(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.equality(scope)?;
        while self.operator("and") {
            let right = self.equality(scope)?;
            self.node()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.relational(scope)?;
        loop {
            if let Some(op) = EQUALITY.into_iter().find(|(w, _)| self.operator(w)) {
                let right = self.relational(scope)?;
                self.node()?;
                left = Expr::Compare(Box::new(left), op.1, Box::new(right));
            } else if self.operator("in") {
                let list = self.in_list(scope)?;
                self.node()?;
                left = Expr::In(Box::new(left), list);
            } else {
                self.refuse_operators(&["has"], Feature::Has)?;
                return Ok(left);
            }
        }
    }

    fn relational(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.additive(scope)?;
        while let Some(op) = RELATIONAL.into_iter().find(|(w, _)| self.operator(w)) {
            let right = self.additive(scope)?;
            self.node()?;
            left = Expr::Compare(Box::new(left), op.1, Box::new(right));
        }
        Ok(left)
    }

    fn additive(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.multiplicative(scope)?;
        while let Some(op) = ADDITIVE.into_iter().find(|op| self.operator(op.word())) {
            let right = self.multiplicative(scope)?;
            self.node()?;
            left = Expr::Arith(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let mut left = self.unary(scope)?;
        while let Some(op) = MULTIPLICATIVE
            .into_iter()
            .find(|op| self.operator(op.word()))
        {
            let right = self.unary(scope)?;
            self.node()?;
            left = Expr::Arith(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let start = self.c.pos();
        if self.c.eat_ci("not") && self.c.rws() {
            self.enter()?;
            let operand = self.unary(scope)?;
            self.leave();
            self.node()?;
            return Ok(Expr::Not(Box::new(operand)));
        }
        self.c.reset(start);
        // `negateExpr = "-" BWS commonExpr`, unless the `-` starts a numeric literal.
        if self.c.eat("-") {
            self.c.reset(start);
            if let Some(value) = literal(self.c, self.limits)? {
                self.node()?;
                return Ok(Expr::Literal(value));
            }
            self.c.reset(start);
            self.c.eat("-");
            self.c.bws();
            self.enter()?;
            let operand = self.unary(scope)?;
            self.leave();
            self.node()?;
            return Ok(Expr::Neg(Box::new(operand)));
        }
        self.primary(scope)
    }

    /// `feature` refused at its operator word when one of `words` follows.
    fn refuse_operators(&mut self, words: &[&str], feature: Feature) -> Result<(), UrlError> {
        let start = self.c.pos();
        if words.iter().any(|w| self.operator(w)) {
            self.c.reset(start);
            self.c.rws();
            return Err(self.c.not_implemented(feature));
        }
        Ok(())
    }

    /// `listExpr`, or any other right side of `in`, which is not implemented.
    fn in_list(&mut self, scope: Scope<'m>) -> Result<Vec<Literal>, UrlError> {
        let start = self.c.pos();
        if let Some(list) = self.literal_list()? {
            return Ok(list);
        }
        self.c.reset(start);
        self.primary(scope)?;
        self.c.reset(start);
        Err(self.c.not_implemented(Feature::InExpression))
    }

    /// `OPEN BWS [ primitiveLiteral BWS *( COMMA BWS primitiveLiteral BWS ) ] CLOSE`, `None`
    /// when an item is not a literal.
    fn literal_list(&mut self) -> Result<Option<Vec<Literal>>, UrlError> {
        if !self.c.open() {
            return Ok(None);
        }
        self.c.bws();
        let mut items = Vec::new();
        if self.c.close() {
            return Ok(Some(items));
        }
        loop {
            // Counted before the item is read.
            if items.len() >= self.limits.max_in_list_len {
                return Err(self
                    .c
                    .limit(LimitKind::InListLength, self.limits.max_in_list_len));
            }
            let Some(item) = literal(self.c, self.limits)? else {
                return Ok(None);
            };
            items.push(item);
            self.c.bws();
            if self.c.close() {
                return Ok(Some(items));
            }
            if !self.c.comma() {
                return Ok(None);
            }
            self.c.bws();
        }
    }

    fn primary(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        self.node()?;
        let start = self.c.pos();
        if let Some(value) = literal(self.c, self.limits)? {
            return Ok(Expr::Literal(value));
        }
        if self.c.at() {
            return match self.c.identifier()? {
                Some(name) if !matches!(self.c.peek(), Some(b'.' | b'/')) => {
                    Ok(Expr::Alias(name.to_owned()))
                }
                _ => {
                    self.c.reset(start);
                    self.member_path(scope)
                }
            };
        }
        if self.c.eat("$root") {
            return Err(self.not_implemented_at(start, Feature::Root));
        }
        if self.c.eat("$it") || self.c.eat("$this") {
            return Err(self.not_implemented_at(start, Feature::ImplicitVariable));
        }
        if self.c.open() {
            self.enter()?;
            self.c.bws();
            let inner = self.or_expr(scope)?;
            self.c.bws();
            if !self.c.close() {
                return Err(self.c.syntax("parenExpr"));
            }
            self.leave();
            return Ok(inner);
        }
        if let Some(call) = self.method_call(scope)? {
            return Ok(call);
        }
        // `arrayOrObject` starts with `BWS`, which is why a stray space is consumed here.
        self.c.bws();
        if self.c.eat("[") || self.c.eat_ci("%5B") || self.c.eat("{") || self.c.eat_ci("%7B") {
            return Err(self.not_implemented_at(start, Feature::JsonLiteral));
        }
        self.c.reset(start);
        self.member_path(scope)
    }

    fn not_implemented_at(&mut self, at: usize, feature: Feature) -> UrlError {
        self.c.reset(at);
        self.c.not_implemented(feature)
    }

    /// A method name directly followed by `OPEN`: the methods of [`Method`] parsed with their
    /// arity checked, the others not implemented. `None` (cursor unmoved) when no method call
    /// starts here.
    fn method_call(&mut self, scope: Scope<'m>) -> Result<Option<Expr>, UrlError> {
        let start = self.c.pos();
        for method in Method::all() {
            if self.c.eat_ci(method.name()) && self.c.open() {
                self.enter()?;
                let args = self.arguments(scope, method.arity())?;
                self.leave();
                return Ok(Some(Expr::Call(method, args)));
            }
            self.c.reset(start);
        }
        for name in OTHER_METHODS {
            if self.c.eat_ci(name) && self.c.open() {
                return Err(self.not_implemented_at(start, Feature::Method));
            }
            self.c.reset(start);
        }
        for name in ["cast", "isof"] {
            if self.c.eat_ci(name) && self.c.open() {
                return Err(self.not_implemented_at(start, Feature::CastIsOf));
            }
            self.c.reset(start);
        }
        Ok(None)
    }

    /// The arguments after a method's `OPEN`, through its `CLOSE`: `BWS [ commonExpr BWS
    /// *( COMMA BWS commonExpr BWS ) ] CLOSE`, between `least` and `most` of them. A missing
    /// argument fails where its `COMMA` was due, an extra one where the `CLOSE` was.
    fn arguments(
        &mut self,
        scope: Scope<'m>,
        (least, most): (usize, usize),
    ) -> Result<Vec<Expr>, UrlError> {
        self.c.bws();
        let mut args = Vec::new();
        if most == 0 {
            return if self.c.close() {
                Ok(args)
            } else {
                Err(self.c.syntax("CLOSE"))
            };
        }
        loop {
            args.push(self.or_expr(scope)?);
            self.c.bws();
            if args.len() >= least && self.c.close() {
                return Ok(args);
            }
            if args.len() >= most {
                return Err(self.c.syntax("CLOSE"));
            }
            if !self.c.comma() {
                return Err(self.c.syntax("COMMA"));
            }
            self.c.bws();
        }
    }

    /// `firstMemberExpr`: a lambda variable or a member of `scope`, then the path on from it.
    fn member_path(&mut self, scope: Scope<'m>) -> Result<Expr, UrlError> {
        let start = self.c.pos();
        let mut path = MemberPath {
            variable: None,
            path: Vec::new(),
        };
        let mut here = scope;
        if let Some(name) = self.c.identifier()? {
            if let Some((_, ranges)) = self.variables.iter().rev().find(|(v, _)| v == name) {
                path.variable = Some(name.to_owned());
                here = *ranges;
                if !self.slash_continues() {
                    return Ok(Expr::Member(path));
                }
            } else {
                self.c.reset(start);
            }
        }
        self.member_expr(here, path)
    }

    fn slash_continues(&mut self) -> bool {
        self.c.slash()
    }

    /// `OPEN` next, not consumed.
    fn at_open(&self) -> bool {
        let rest = self.c.rest();
        rest.starts_with('(') || rest.get(..3).is_some_and(|h| h.eq_ignore_ascii_case("%28"))
    }

    /// `memberExpr` over `scope`, appended to `path`. A path through single-valued complex or
    /// navigation properties is walked in a loop, each segment one level of `max_expr_depth`
    /// (F-ANT5-001: a self-referencing navigation repeated thousands of times must not recurse).
    fn member_expr(&mut self, scope: Scope<'m>, path: MemberPath) -> Result<Expr, UrlError> {
        let depth = self.depth;
        let walked = self.member_walk(scope, path);
        self.depth = depth;
        walked
    }

    fn member_walk(
        &mut self,
        mut scope: Scope<'m>,
        mut path: MemberPath,
    ) -> Result<Expr, UrlError> {
        loop {
            let start = self.c.pos();
            if self.c.at() {
                return self.annotated(path, start);
            }
            let Some(name) = self.c.identifier()? else {
                return Err(self.c.syntax("memberExpr"));
            };
            if self.c.peek() == Some(b'.') {
                return self.qualified(start);
            }
            let member = match scope {
                Scope::Type(ty) => self.model.member(ty, name),
                Scope::Primitive => None,
            };
            // a `$compute` name, from the resource itself only
            let root = path.variable.is_none() && path.path.is_empty();
            let member = member.or_else(|| {
                (root && self.computed.iter().any(|c| c == name))
                    .then_some(Member::Primitive { collection: false })
            });
            let Some(member) = member else {
                if self.at_open() {
                    // An unqualified function call; no function is in the subset's model.
                    return Err(self.c.syntax("functionExprParameters"));
                }
                self.c.reset(start);
                return Err(self.c.error(UrlErrorKind::Unresolved));
            };
            path.path.push(name.to_owned());
            return match member {
                Member::Primitive { collection: false } => {
                    if self.slash_continues() {
                        let at = self.c.pos();
                        if self.c.at() {
                            return self.annotated(path, at);
                        }
                        if self.c.identifier()?.is_some() && self.c.peek() == Some(b'.') {
                            return self.qualified(at);
                        }
                        return Err(self.c.syntax("primitivePathExpr"));
                    }
                    Ok(Expr::Member(path))
                }
                Member::Primitive { collection: true } => self.collection(Scope::Primitive, path),
                Member::Complex {
                    ty,
                    collection: false,
                }
                | Member::Navigation {
                    ty,
                    collection: false,
                } => {
                    if self.slash_continues() {
                        self.enter()?;
                        scope = Scope::Type(ty);
                        continue;
                    }
                    Ok(Expr::Member(path))
                }
                Member::Complex {
                    ty,
                    collection: true,
                } => self.collection(Scope::Type(ty), path),
                Member::Navigation {
                    ty,
                    collection: true,
                } => {
                    if self.at_open() {
                        return Err(self.c.not_implemented(Feature::PathInExpression));
                    }
                    self.collection(Scope::Type(ty), path)
                }
            };
        }
    }

    /// A dotted name in a member position: a function or a type cast, neither implemented.
    fn qualified(&mut self, start: usize) -> Result<Expr, UrlError> {
        while self.c.eat(".") {
            if self.c.identifier()?.is_none() {
                return Err(self.c.syntax("odataIdentifier"));
            }
        }
        let feature = if self.at_open() {
            Feature::Operation
        } else {
            Feature::TypeCast
        };
        Err(self.not_implemented_at(start, feature))
    }

    /// `collectionPathExpr` after a collection-valued `path` whose items are `items`.
    fn collection(&mut self, items: Scope<'m>, path: MemberPath) -> Result<Expr, UrlError> {
        let start = self.c.pos();
        if !self.slash_continues() {
            return Ok(Expr::Member(path));
        }
        if self.c.eat("$count") {
            return Err(self.not_implemented_at(start, Feature::NestedCount));
        }
        if self.c.eat("$filter") {
            return Err(self.not_implemented_at(start, Feature::PathInExpression));
        }
        let at = self.c.pos();
        if self.c.at() {
            return self.annotated(path, at);
        }
        if self.c.eat_ci("any") && self.c.open() {
            return self.any(items, path);
        }
        self.c.reset(at);
        if self.c.eat_ci("all") && self.c.open() {
            return self.all(items, path);
        }
        self.c.reset(at);
        if self.c.identifier()?.is_some() {
            if self.c.peek() == Some(b'.') {
                return self.qualified(at);
            }
            if self.at_open() {
                // A bound function without its namespace (4.01 default namespaces).
                return Err(self.not_implemented_at(at, Feature::Operation));
            }
        }
        Err(self.c.syntax("collectionPathExpr"))
    }

    /// The annotation whose `AT` is at `at` ends `path`; a path on from its value is not
    /// implemented.
    fn annotated(&mut self, mut path: MemberPath, at: usize) -> Result<Expr, UrlError> {
        path.path.push(annotation(self.c, at)?);
        if self.c.peek() == Some(b'/') {
            return Err(self.not_implemented_at(at, Feature::AnnotationPath));
        }
        Ok(Expr::Member(path))
    }

    /// The rest of `anyExpr` after `any(`.
    fn any(&mut self, items: Scope<'m>, collection: MemberPath) -> Result<Expr, UrlError> {
        self.c.bws();
        if self.c.close() {
            return Ok(Expr::Any {
                collection,
                lambda: None,
            });
        }
        let lambda = self.lambda(items)?;
        Ok(Expr::Any {
            collection,
            lambda: Some(lambda),
        })
    }

    /// The rest of `allExpr` after `all(`: `all` has no empty form.
    fn all(&mut self, items: Scope<'m>, collection: MemberPath) -> Result<Expr, UrlError> {
        self.c.bws();
        if self.c.close() {
            // past the `)`, where the ABNF runner stops reading `all()` as a function call
            return Err(self.c.syntax("lambdaVariableExpr"));
        }
        let lambda = self.lambda(items)?;
        Ok(Expr::All { collection, lambda })
    }

    /// `lambdaVariableExpr BWS COLON BWS lambdaPredicateExpr BWS CLOSE`, the variable ranging
    /// over `items`.
    fn lambda(&mut self, items: Scope<'m>) -> Result<(String, Box<Expr>), UrlError> {
        let Some(variable) = self.c.identifier()? else {
            return Err(self.c.syntax("lambdaVariableExpr"));
        };
        let variable = variable.to_owned();
        self.c.bws();
        if !self.c.colon() {
            return Err(self.c.syntax("COLON"));
        }
        self.c.bws();
        self.enter()?;
        self.variables.push((variable.clone(), items));
        let body = self.or_expr(self.root);
        self.variables.pop();
        let body = body?;
        self.leave();
        self.c.bws();
        if !self.c.close() {
            return Err(self.c.syntax("CLOSE"));
        }
        Ok((variable, Box::new(body)))
    }
}

/// `annotationInQuery` with its `AT` at `start` already consumed: `@namespace.term`, an
/// optional `#qualifier` (written `%23`), returned with the `@`.
pub(crate) fn annotation(c: &mut Cursor<'_>, start: usize) -> Result<String, UrlError> {
    let mut name = String::from("@");
    let mut parts = 0_u8;
    loop {
        let Some(part) = c.identifier()? else {
            return Err(c.syntax("annotationInQuery"));
        };
        name.push_str(part);
        parts = parts.saturating_add(1);
        if !c.eat(".") {
            break;
        }
        name.push('.');
    }
    if parts < 2 {
        // A term without its namespace cannot be told apart; refused like an unknown name.
        c.reset(start);
        return Err(c.error(UrlErrorKind::Unresolved));
    }
    if c.eat_ci("%23") {
        let Some(qualifier) = c.identifier()? else {
            return Err(c.syntax("annotationQualifier"));
        };
        name.push('#');
        name.push_str(qualifier);
    }
    Ok(name)
}
