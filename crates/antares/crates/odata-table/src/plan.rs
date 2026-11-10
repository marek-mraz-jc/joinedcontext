// SPDX-License-Identifier: MIT
//! Query options to one [`Plan`] every source executes: what is supported is decided here.

use odata_url::{CompareOp, EdmModel, Expr, Literal, Member, Method, Model as _, SelectItem};
use serde_json::{Number, Value};

use crate::{QueryOptions, TableError, TableInfo};

/// A filter condition over a row, in the small vocabulary every source evaluates the same way.
/// Comparisons are two-valued: a comparison with a missing or differently typed value is
/// `false`, never unknown, so `not` of it is `true` (OData 4.01 URL Conventions 5.1.1.1).
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    /// Always this.
    Const(bool),
    /// Both.
    And(Box<Cond>, Box<Cond>),
    /// Either.
    Or(Box<Cond>, Box<Cond>),
    /// Not.
    Not(Box<Cond>),
    /// The value at `path` compared with `value` (a string, number or boolean): `false` when
    /// the value there is missing, `null` or of another JSON type.
    Compare {
        /// The property path.
        path: Vec<String>,
        /// The operator, the row's value on its left.
        op: CompareOp,
        /// The literal on its right.
        value: Value,
    },
    /// The value at `path` is missing or `null`.
    IsNull {
        /// The property path.
        path: Vec<String>,
    },
    /// The string at `path` contains, starts or ends with `needle`; `false` for a non-string.
    Text {
        /// The property path.
        path: Vec<String>,
        /// Which test.
        op: TextOp,
        /// The text looked for.
        needle: String,
    },
}

/// The string tests of [`Cond::Text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOp {
    /// `contains`.
    Contains,
    /// `startswith`.
    StartsWith,
    /// `endswith`.
    EndsWith,
}

/// One ordering key: a primitive property path, ascending unless `descending`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    /// The property path.
    pub path: Vec<String>,
    /// Descending.
    pub descending: bool,
}

/// What a source executes: the filter, the total order, the window and the count.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// `$filter`, applied after the scope.
    pub filter: Cond,
    /// `$orderby`, then the key ascending, so every page order is total.
    pub order: Vec<SortKey>,
    /// `$select`: the top-level properties answered; `None` answers every declared one.
    pub select: Option<Vec<String>>,
    /// Rows skipped: `$skip` plus the `$skiptoken`.
    pub offset: u64,
    /// Rows this page answers.
    pub limit: u64,
    /// Rows a source fetches: `limit`, plus one to learn whether another page follows.
    pub fetch: u64,
    /// The `$skiptoken` this page started at: rows earlier pages answered.
    pub token: u64,
    /// `$count=true`.
    pub count: bool,
}

/// The plan of `options` on the set `info`, pages of at most `max_page` rows.
///
/// # Errors
///
/// [`TableError::NotImplemented`] for a construct outside the subset (the crate docs list it),
/// [`TableError::UnknownProperty`] for a path the entity type lacks,
/// [`TableError::BadRequest`] for a `$skiptoken` this service did not write.
pub fn plan(info: &TableInfo, options: &QueryOptions, max_page: u64) -> Result<Plan, TableError> {
    let unsupported = |what: &str| Err(TableError::NotImplemented(what.to_owned()));
    if options.search.is_some() {
        return unsupported("$search");
    }
    if options.apply.is_some() {
        return unsupported("$apply");
    }
    if options.compute.is_some() {
        return unsupported("$compute");
    }
    if options.expand.is_some() {
        return unsupported("$expand");
    }
    let planner = Planner { info, options };
    let filter = match &options.filter {
        Some(expr) => planner.cond(expr)?,
        None => Cond::Const(true),
    };
    let mut order = Vec::new();
    for item in options.orderby.iter().flatten() {
        let Expr::Member(member) = &item.expr else {
            return unsupported("$orderby on an expression");
        };
        order.push(SortKey {
            path: planner.path(member)?,
            descending: item.descending,
        });
    }
    let key = vec![info.key.clone()];
    if !order.iter().any(|k| k.path == key) {
        order.push(SortKey {
            path: key,
            descending: false,
        });
    }
    let select = match &options.select {
        None => None,
        Some(items) if items.contains(&SelectItem::Star) => None,
        Some(items) => Some(planner.select(items)?),
    };
    let token = match &options.skiptoken {
        None => 0,
        Some(text) => text.parse::<u64>().map_err(|_| {
            TableError::BadRequest("the $skiptoken is not one this service wrote".into())
        })?,
    };
    let remaining = options
        .top
        .map_or(u64::MAX, |top| top.saturating_sub(token));
    let limit = remaining.min(max_page);
    Ok(Plan {
        filter,
        order,
        select,
        offset: options.skip.unwrap_or(0).saturating_add(token),
        limit,
        // one more row tells whether a next page exists; none when `$top` ends at this page
        fetch: if remaining > limit {
            limit.saturating_add(1)
        } else {
            limit
        },
        token,
        count: options.count == Some(true),
    })
}

struct Planner<'a> {
    info: &'a TableInfo,
    options: &'a QueryOptions,
}

impl Planner<'_> {
    fn cond(&self, expr: &Expr) -> Result<Cond, TableError> {
        Ok(match expr {
            Expr::And(a, b) => Cond::And(Box::new(self.cond(a)?), Box::new(self.cond(b)?)),
            Expr::Or(a, b) => Cond::Or(Box::new(self.cond(a)?), Box::new(self.cond(b)?)),
            Expr::Not(a) => Cond::Not(Box::new(self.cond(a)?)),
            Expr::Literal(Literal::Bool(b)) => Cond::Const(*b),
            Expr::Compare(left, op, right) => self.compare(left, *op, right)?,
            Expr::In(left, list) => {
                let path = self.member(left)?;
                let mut out = Cond::Const(false);
                for literal in list {
                    let one = equal(path.clone(), value(literal)?);
                    out = Cond::Or(Box::new(out), Box::new(one));
                }
                out
            }
            Expr::Call(method, args) => {
                let op = match method {
                    Method::Contains => TextOp::Contains,
                    Method::StartsWith => TextOp::StartsWith,
                    Method::EndsWith => TextOp::EndsWith,
                    other => return Err(TableError::NotImplemented(other.name().to_owned())),
                };
                let [haystack, needle] = args.as_slice() else {
                    return Err(TableError::NotImplemented(method.name().to_owned()));
                };
                let path = self.member(haystack)?;
                match self.literal(needle)? {
                    Some(Value::String(needle)) => Cond::Text { path, op, needle },
                    // contains(x, null) is null: no row
                    None => Cond::Const(false),
                    Some(_) => {
                        return Err(TableError::BadRequest(format!(
                            "{} takes a string",
                            method.name()
                        )))
                    }
                }
            }
            Expr::Member(_) => {
                self.compare(expr, CompareOp::Eq, &Expr::Literal(Literal::Bool(true)))?
            }
            _ => return Err(TableError::NotImplemented(construct(expr).to_owned())),
        })
    }

    /// A comparison of a property with a literal, in either order.
    fn compare(&self, left: &Expr, op: CompareOp, right: &Expr) -> Result<Cond, TableError> {
        let (path, op, literal) = match (left, right) {
            (Expr::Member(_), _) => (self.member(left)?, op, right),
            (_, Expr::Member(_)) => (self.member(right)?, flipped(op), left),
            _ => {
                return Err(TableError::NotImplemented(
                    "a comparison without a property".into(),
                ))
            }
        };
        Ok(match (self.literal(literal)?, op) {
            (None, CompareOp::Eq) => Cond::IsNull { path },
            (None, CompareOp::Ne) => Cond::Not(Box::new(Cond::IsNull { path })),
            (None, _) => Cond::Const(false),
            (Some(value), CompareOp::Ne) => Cond::Not(Box::new(equal(path, value))),
            (Some(value), op) => Cond::Compare { path, op, value },
        })
    }

    fn member(&self, expr: &Expr) -> Result<Vec<String>, TableError> {
        match expr {
            Expr::Member(member) => self.path(member),
            other => Err(TableError::NotImplemented(construct(other).to_owned())),
        }
    }

    /// A member path checked against the model: primitive at its end, single-valued complex
    /// properties on the way.
    fn path(&self, member: &odata_url::MemberPath) -> Result<Vec<String>, TableError> {
        if member.variable.is_some() {
            return Err(TableError::NotImplemented("lambda variables".into()));
        }
        let model = EdmModel(&self.info.model);
        let mut ty = self.info.entity_type.clone();
        let mut walked = member.path.iter().peekable();
        while let Some(name) = walked.next() {
            let last = walked.peek().is_none();
            match model.member(&ty, name) {
                None => return Err(TableError::UnknownProperty(member.path.join("/"))),
                Some(Member::Primitive { collection: false }) if last => {}
                Some(Member::Complex {
                    ty: next,
                    collection: false,
                }) if !last => ty = next.to_owned(),
                Some(Member::Navigation { .. }) => {
                    return Err(TableError::NotImplemented("navigation properties".into()))
                }
                Some(
                    Member::Primitive { collection: true }
                    | Member::Complex {
                        collection: true, ..
                    },
                ) => {
                    return Err(TableError::NotImplemented(
                        "collection-valued properties".into(),
                    ))
                }
                Some(_) => {
                    return Err(TableError::NotImplemented(
                        "a path ending in a complex property".into(),
                    ))
                }
            }
        }
        Ok(member.path.clone())
    }

    fn select(&self, items: &[SelectItem]) -> Result<Vec<String>, TableError> {
        let mut names = Vec::new();
        for item in items {
            match item {
                SelectItem::Path(path) => match path.as_slice() {
                    [name] if self.info.properties.contains(name) => {
                        if !names.contains(name) {
                            names.push(name.clone());
                        }
                    }
                    [_] => return Err(TableError::UnknownProperty(path.join("/"))),
                    _ => {
                        return Err(TableError::NotImplemented(
                            "$select of a nested path".into(),
                        ))
                    }
                },
                SelectItem::Annotation(_) => {
                    return Err(TableError::NotImplemented(
                        "$select of an annotation".into(),
                    ))
                }
                SelectItem::Star => {}
            }
        }
        Ok(names)
    }

    /// A literal or parameter alias as a JSON value; `None` for `null`.
    fn literal(&self, expr: &Expr) -> Result<Option<Value>, TableError> {
        let literal = match expr {
            Expr::Literal(literal) => literal,
            Expr::Alias(name) => self
                .options
                .aliases
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, l)| l)
                .ok_or_else(|| {
                    TableError::BadRequest(format!("the parameter alias {name} has no value"))
                })?,
            other => return Err(TableError::NotImplemented(construct(other).to_owned())),
        };
        if *literal == Literal::Null {
            return Ok(None);
        }
        value(literal).map(Some)
    }
}

/// A non-null literal as a JSON value. Date, time, duration and GUID literals compare as
/// their text, as a source stores them.
// ponytail: temporal literals compare as text, which orders ISO 8601 values written with
// one offset; typed temporal comparison when a source stores mixed offsets.
fn value(literal: &Literal) -> Result<Value, TableError> {
    Ok(match literal {
        Literal::Null => Value::Null,
        Literal::Bool(b) => Value::Bool(*b),
        Literal::Int(i) => Value::Number((*i).into()),
        Literal::Decimal(text) => text
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| TableError::BadRequest(format!("{text} is not a finite number")))?,
        Literal::String(s)
        | Literal::Date(s)
        | Literal::DateTimeOffset(s)
        | Literal::TimeOfDay(s)
        | Literal::Guid(s)
        | Literal::Duration(s) => Value::String(s.clone()),
        Literal::Geography(_) | Literal::Geometry(_) => {
            return Err(TableError::NotImplemented("geography literals".into()))
        }
        Literal::Binary(_) => return Err(TableError::NotImplemented("binary literals".into())),
    })
}

/// `path eq value`, with `null` in an `in` list meaning "is null".
fn equal(path: Vec<String>, value: Value) -> Cond {
    if value.is_null() {
        Cond::IsNull { path }
    } else {
        Cond::Compare {
            path,
            op: CompareOp::Eq,
            value,
        }
    }
}

fn flipped(op: CompareOp) -> CompareOp {
    match op {
        CompareOp::Gt => CompareOp::Lt,
        CompareOp::Ge => CompareOp::Le,
        CompareOp::Lt => CompareOp::Gt,
        CompareOp::Le => CompareOp::Ge,
        same => same,
    }
}

/// The name a 501 answer gives an unsupported expression.
fn construct(expr: &Expr) -> &'static str {
    match expr {
        Expr::Arith(..) | Expr::Neg(_) => "arithmetic operators",
        Expr::Any { .. } | Expr::All { .. } => "lambda operators",
        Expr::Call(..) => "this function",
        _ => "this expression",
    }
}
