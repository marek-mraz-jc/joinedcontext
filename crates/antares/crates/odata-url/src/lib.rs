// SPDX-License-Identifier: MIT
//! OData 4.01 relative URLs to a typed AST: the resource path and the system query options
//! (ADR-0024 decision 1, row `odata-url`).
//!
//! A hand-written recursive-descent parser with one function per rule of
//! `odata-abnf-construction-rules.txt` at OASIS commit `a31c8db` (decision 3), bounded by the
//! caller's [`Limits`] (decision 4). The grammar depends on the model, so the parser asks a
//! [`Model`] about each name as it reads it. It implements the subset T-3441 maps (decision 5):
//! valid OData outside it answers [`UrlErrorKind::NotImplemented`]; a syntax error answers the
//! offset of the furthest byte any rule matched, which is the OASIS test cases' `FailAt`.
//!
//! The URL is first percent-encoding normalised (RFC 3986 6.2.2.2: percent-encoded unreserved
//! characters decoded), as the ABNF assumes; offsets index the normalised URL.
//!
//! [`ODataUri`] writes itself back in canonical form ([`std::fmt::Display`]), which parses to
//! the same tree.
//!
//! ```
//! use odata_url::{parse, Limits, Member, Model, ODataUri, Segment};
//!
//! struct Service;
//! impl Model for Service {
//!     fn entity_set(&self, name: &str) -> Option<&str> {
//!         (name == "Buildings").then_some("Building")
//!     }
//!     fn member(&self, _ty: &str, name: &str) -> Option<Member<'_>> {
//!         (name == "height").then_some(Member::Primitive { collection: false })
//!     }
//! }
//!
//! let uri = parse("Buildings('b1')?$filter=height gt 10&$top=5", &Service, &Limits::strict())?;
//! let ODataUri::Resource { path, options } = &uri else { unreachable!() };
//! assert_eq!(path[0], Segment::EntitySet("Buildings".into()));
//! assert_eq!(options.top, Some(5));
//! assert_eq!(uri.to_string(), "Buildings('b1')?$filter=height%20gt%2010&$top=5");
//! # Ok::<(), odata_url::UrlError>(())
//! ```

#![forbid(unsafe_code)]

mod apply;
mod ast;
mod cursor;
mod display;
#[cfg(feature = "edm")]
mod edm;
mod error;
mod expr;
mod limits;
mod literal;
mod model;
mod path;
mod query;
mod search;

use std::borrow::Cow;

pub use ast::{
    Aggregate, AggregateWith, ArithOp, CompareOp, Compute, ExpandItem, ExpandKind, ExpandPath,
    Expr, Format, Geography, Key, KeyValue, Levels, Literal, MemberPath, Method, ODataUri, OrderBy,
    PartitionBy, Position, QueryOptions, Segment, SelectItem, Shape, Transformation,
};
#[cfg(feature = "edm")]
pub use edm::EdmModel;
pub use error::{Feature, LimitKind, UrlError, UrlErrorKind};
pub use limits::Limits;
pub use literal::is_primitive_text;
pub use model::{Member, Model};
pub use search::Search;

use cursor::Cursor;
use expr::{Budget, Scope};
use query::{Allowed, Query};

/// Parses one relative OData URL (`odataRelativeUri`: what follows the service root, without a
/// leading `/`), asking `model` about every name, within `limits`.
///
/// # Errors
///
/// [`UrlError`] with the offset where parsing stopped: a syntax error, a limit, a duplicate
/// system option, a name `model` does not know, or valid OData outside the subset.
///
/// ```
/// # use odata_url::{parse, Feature, Limits, Member, Model, UrlErrorKind};
/// # struct Service;
/// # impl Model for Service {
/// #     fn entity_set(&self, name: &str) -> Option<&str> { (name == "Buildings").then_some("B") }
/// #     fn member(&self, _: &str, _: &str) -> Option<Member<'_>> { None }
/// # }
/// let refused = parse("Buildings?$index=1", &Service, &Limits::strict()).unwrap_err();
/// assert_eq!(refused.kind, UrlErrorKind::NotImplemented(Feature::Index));
/// assert_eq!(refused.offset, 10);
/// ```
pub fn parse<M: Model>(uri: &str, model: &M, limits: &Limits) -> Result<ODataUri, UrlError> {
    let path_len = uri.find('?').unwrap_or(uri.len());
    if path_len > limits.max_path_len {
        return Err(UrlError {
            kind: UrlErrorKind::LimitExceeded {
                limit: LimitKind::PathLength,
                max: limits.max_path_len,
            },
            offset: limits.max_path_len,
        });
    }
    let query_len = uri.len().saturating_sub(path_len).saturating_sub(1);
    if query_len > limits.max_query_len {
        return Err(UrlError {
            kind: UrlErrorKind::LimitExceeded {
                limit: LimitKind::QueryLength,
                max: limits.max_query_len,
            },
            offset: path_len
                .saturating_add(1)
                .saturating_add(limits.max_query_len),
        });
    }
    let (normalized, decoded) = normalized(uri);
    let mut c = Cursor::new(&normalized);
    // the offset a caller reads is into the `uri` it passed, not into its
    // normalized form: each decoded escape there is three bytes for one
    let raw = |mut e: UrlError| {
        let before = decoded.iter().take_while(|at| **at < e.offset).count();
        e.offset = e.offset.saturating_add(before.saturating_mul(2));
        e
    };
    let parsed = relative_uri(&mut c, model, limits).map_err(raw)?;
    if !c.at_end() {
        return Err(raw(c.syntax("end of URL")));
    }
    Ok(parsed)
}

/// `odataRelativeUri`.
fn relative_uri<M: Model>(
    c: &mut Cursor<'_>,
    model: &M,
    limits: &Limits,
) -> Result<ODataUri, UrlError> {
    let start = c.pos();
    if c.at_end() {
        return Ok(ODataUri::ServiceDocument);
    }
    let mut query = Query {
        c,
        model,
        budget: Budget {
            limits,
            nodes: 0,
            apply_depth: 0,
            computed: Vec::new(),
        },
        expand_items: 0,
        select_items: 0,
    };
    if query.c.eat("$batch") {
        let options = optional_query(&mut query, Scope::Primitive, Allowed::Format)?;
        return Ok(ODataUri::Batch { options });
    }
    if query.c.eat("$entity") {
        if query.c.eat("/") {
            query.c.reset(start);
            return Err(query.c.not_implemented(Feature::TypeCast));
        }
        if !query.c.eat("?") {
            return Err(query.c.syntax("entityOptions"));
        }
        let options = query.options(Scope::Primitive, Allowed::EntityId)?;
        return Ok(ODataUri::Entity { options });
    }
    if query.c.eat("$metadata") {
        let options = optional_query(&mut query, Scope::Primitive, Allowed::Format)?;
        return Ok(ODataUri::Metadata { options });
    }
    if query.c.eat("$crossjoin") || query.c.eat("$all") {
        query.c.reset(start);
        return Err(query.c.not_implemented(Feature::CrossJoin));
    }
    let (path, scope) = path::resource_path(query.c, model, limits)?;
    let options = optional_query(&mut query, scope, Allowed::All)?;
    Ok(ODataUri::Resource { path, options })
}

/// `[ "?" [ options ] ]`.
fn optional_query<'m, M: Model>(
    query: &mut Query<'_, '_, '_, 'm, M>,
    scope: Scope<'m>,
    allowed: Allowed,
) -> Result<QueryOptions, UrlError> {
    if !query.c.eat("?") || query.c.at_end() {
        return Ok(QueryOptions::default());
    }
    query.options(scope, allowed)
}

/// RFC 3986 6.2.2.2: percent-encoded unreserved characters decoded, everything else kept.
#[cfg(test)]
fn normalize(uri: &str) -> Cow<'_, str> {
    normalized(uri).0
}

/// `uri` normalized (RFC 3986 6.2.2.2) and the offsets in it where an escape was decoded, ascending.
fn normalized(uri: &str) -> (Cow<'_, str>, Vec<usize>) {
    let bytes = uri.as_bytes();
    let decoded = |i: usize| -> Option<u8> {
        let (Some(b'%'), Some(&hi), Some(&lo)) = (
            bytes.get(i),
            bytes.get(i.saturating_add(1)),
            bytes.get(i.saturating_add(2)),
        ) else {
            return None;
        };
        let value = char::from(hi)
            .to_digit(16)?
            .checked_mul(16)?
            .checked_add(char::from(lo).to_digit(16)?)?;
        let byte = u8::try_from(value).ok()?;
        (byte.is_ascii_alphanumeric() || b"-._~".contains(&byte)).then_some(byte)
    };
    if !(0..bytes.len()).any(|i| decoded(i).is_some()) {
        return (Cow::Borrowed(uri), Vec::new());
    }
    let mut out = String::with_capacity(uri.len());
    let mut at = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(plain) = decoded(i).filter(|plain| !escapes(&out, *plain)) {
            at.push(out.len());
            out.push(char::from(plain));
            i = i.saturating_add(3);
        } else {
            // `i` stays on a character boundary: only three ASCII bytes are ever skipped.
            let next = uri
                .get(i..)
                .and_then(|rest| rest.chars().next())
                .unwrap_or_default();
            out.push(next);
            i = i.saturating_add(next.len_utf8());
        }
    }
    (Cow::Owned(out), at)
}

/// Whether `plain` after `out` would spell a new `%XX` (`%6%65` is not `%6e`): a hex digit after
/// `%`, or after `%` and a hex digit.
fn escapes(out: &str, plain: u8) -> bool {
    let tail = out.as_bytes();
    let last = tail.len().checked_sub(1).and_then(|i| tail.get(i));
    let before = tail.len().checked_sub(2).and_then(|i| tail.get(i));
    plain.is_ascii_hexdigit()
        && (last == Some(&b'%')
            || (before == Some(&b'%') && last.is_some_and(u8::is_ascii_hexdigit)))
}

/// `text` with each `%XX` decoded: a value as written in a URL, read as meant (a `$format`
/// media type, T-3742). `None` for a `%` without two hex digits after it, or bytes that are no
/// UTF-8.
///
/// ```
/// use odata_url::percent_decoded;
/// assert_eq!(percent_decoded("application%2Fjson%3Bodata.metadata%3Dnone").as_deref(),
///            Some("application/json;odata.metadata=none"));
/// assert_eq!(percent_decoded("a%2"), None);
/// assert_eq!(percent_decoded("%FF"), None);
/// ```
#[must_use]
pub fn percent_decoded(text: &str) -> Option<String> {
    let mut c = cursor::Cursor::new(text);
    let mut out = Vec::with_capacity(text.len());
    while let Some(b) = c.peek() {
        if b == b'%' {
            out.push(literal::pct_encoded(&mut c)?);
        } else {
            out.push(b);
            c.advance(1);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests;
