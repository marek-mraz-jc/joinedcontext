// SPDX-License-Identifier: EUPL-1.2
//! What a parse answers when it does not answer an [`ODataUri`](crate::ODataUri).

use std::fmt;

/// A refused URL: what is wrong, and the byte offset into the relative URL where the parser
/// stopped. For a syntax error the offset is the furthest byte any rule matched, which is what
/// the OASIS test cases call `FailAt`. No message repeats a byte of the request.
///
/// ```
/// # use odata_url::{Member, Model};
/// # struct Service;
/// # impl Model for Service {
/// #     fn entity_set(&self, name: &str) -> Option<&str> { (name == "Buildings").then_some("B") }
/// #     fn member(&self, _: &str, name: &str) -> Option<Member<'_>> {
/// #         (name == "height").then_some(Member::Primitive { collection: false })
/// #     }
/// # }
/// use odata_url::{parse, Limits, UrlErrorKind};
/// let e = parse("Buildings?$top=x", &Service, &Limits::strict()).unwrap_err();
/// assert_eq!((e.kind, e.offset), (UrlErrorKind::Syntax { expected: "DIGIT" }, 15));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlError {
    /// What is wrong.
    pub kind: UrlErrorKind,
    /// Byte offset into the relative URL.
    pub offset: usize,
}

/// The kinds of [`UrlError`], each with the HTTP status a service answers it with.
///
/// ```
/// # use odata_url::{Member, Model};
/// # struct Service;
/// # impl Model for Service {
/// #     fn entity_set(&self, name: &str) -> Option<&str> { (name == "Buildings").then_some("B") }
/// #     fn member(&self, _: &str, name: &str) -> Option<Member<'_>> {
/// #         (name == "height").then_some(Member::Primitive { collection: false })
/// #     }
/// # }
/// use odata_url::{parse, Limits, UrlErrorKind};
/// let e = parse("Towers", &Service, &Limits::strict()).unwrap_err();
/// assert_eq!(e.kind, UrlErrorKind::Unresolved);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlErrorKind {
    /// Not OData URL syntax (400); `expected` names the ABNF rule that could not continue.
    Syntax {
        /// The rule, as `odata-abnf-construction-rules.txt` names it.
        expected: &'static str,
    },
    /// A caller limit was exceeded (400, or 414 for a length).
    LimitExceeded {
        /// Which limit.
        limit: LimitKind,
        /// Its value.
        max: usize,
    },
    /// A system query option given twice (400, URL Conventions 5).
    DuplicateOption(&'static str),
    /// Valid OData, outside the implemented subset (501, ADR-0024 decision 5).
    NotImplemented(Feature),
    /// A name the model does not have at that position (404 in the resource path, 400 in a
    /// query option).
    Unresolved,
}

/// The limits of [`Limits`](crate::Limits), by name.
///
/// ```
/// # use odata_url::{Member, Model};
/// # struct Service;
/// # impl Model for Service {
/// #     fn entity_set(&self, name: &str) -> Option<&str> { (name == "Buildings").then_some("B") }
/// #     fn member(&self, _: &str, name: &str) -> Option<Member<'_>> {
/// #         (name == "height").then_some(Member::Primitive { collection: false })
/// #     }
/// # }
/// use odata_url::{parse, LimitKind, Limits, UrlErrorKind};
/// let limits = Limits { max_orderby_items: 1, ..Limits::strict() };
/// let e = parse("Buildings?$orderby=height,height", &Service, &limits).unwrap_err();
/// assert_eq!(e.kind, UrlErrorKind::LimitExceeded { limit: LimitKind::OrderbyItems, max: 1 });
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Bytes of the resource path.
    PathLength,
    /// Bytes of the query string.
    QueryLength,
    /// Segments of the resource path.
    PathSegments,
    /// Query options.
    QueryOptions,
    /// Nesting of an expression.
    ExprDepth,
    /// Expression nodes over the whole request.
    ExprNodes,
    /// Nested `$expand` levels, `$levels` included.
    ExpandDepth,
    /// `$expand` items over the whole request.
    ExpandItems,
    /// `$select` items over the whole request.
    SelectItems,
    /// `$orderby` items.
    OrderbyItems,
    /// Items of one `in` list.
    InListLength,
    /// Bytes of one literal.
    LiteralLength,
    /// An integer beyond what its target holds (`$top`, `$skip`, an `Int64` literal).
    Number,
}

/// OData constructs outside the implemented subset (ADR-0024 decision 5, by T-3441 row).
///
/// ```
/// # use odata_url::{Member, Model};
/// # struct Service;
/// # impl Model for Service {
/// #     fn entity_set(&self, name: &str) -> Option<&str> { (name == "Buildings").then_some("B") }
/// #     fn member(&self, _: &str, name: &str) -> Option<Member<'_>> {
/// #         (name == "height").then_some(Member::Primitive { collection: false })
/// #     }
/// # }
/// use odata_url::{parse, Feature, Limits, UrlErrorKind};
/// let e = parse("Buildings?$filter=height has 1", &Service, &Limits::strict()).unwrap_err();
/// assert_eq!((e.kind, e.offset), (UrlErrorKind::NotImplemented(Feature::Has), 25));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `$index` (Q17).
    Index,
    /// A `$apply` transformation other than `identity`, `filter`, `orderby`, `top`, `skip`,
    /// `aggregate` and `groupby` by single properties; custom aggregates and methods (Q19,
    /// T-3985).
    Apply,
    /// `$deltatoken` (Q21).
    DeltaToken,
    /// `$root` (Q24).
    Root,
    /// `$it`, `$this` (Q25).
    ImplicitVariable,
    /// A parameter alias bound to anything but a literal (Q26).
    AliasExpression,
    /// The `/$filter(...)` path segment (Q28).
    FilterSegment,
    /// `$crossjoin`, `$all` (Q36).
    CrossJoin,
    /// Type casts in a path, an expand or a select (Q37), and `$entity/<type>`.
    TypeCast,
    /// Members of an ordered collection by position (Q39).
    Ordinal,
    /// Functions and actions, bound or imported (W27 to W31).
    Operation,
    /// `/$each` (W22).
    Each,
    /// `/$value` of an entity, the media resource (W12).
    MediaValue,
    /// Options inside `$expand(...)` other than `$select`, `$expand`, `$levels` (Q5), and
    /// options on a selected collection.
    NestedOption,
    /// `$count` inside `$expand` or an expression path (Q13).
    NestedCount,
    /// Key predicates and `$filter` segments inside an expression path.
    PathInExpression,
    /// `has` (F13).
    Has,
    /// `totalseconds`, `geo.distance`, `geo.length`, `hassubset`, `hassubsequence`, `case`
    /// (F18, F19, F30).
    Method,
    /// `cast`, `isof` (F19).
    CastIsOf,
    /// `in` against anything but a literal list.
    InExpression,
    /// JSON arrays and objects in a URL.
    JsonLiteral,
    /// Literals without an NGSI-LD value type: enum, geometry, and geography other than
    /// points and polygons (F27).
    Literal,
    /// Identifiers with percent-encoded non-ASCII characters.
    NonAsciiIdentifier,
    /// Annotations in `$expand` and `$select`, a path on from an annotation's value, and
    /// `$expand=$value`.
    AnnotationPath,
    /// `*` after a complex property in `$expand`.
    PathStar,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            UrlErrorKind::Syntax { expected } => {
                write!(
                    f,
                    "syntax error at byte {}: expected {expected}",
                    self.offset
                )
            }
            UrlErrorKind::LimitExceeded { limit, max } => {
                write!(
                    f,
                    "limit exceeded at byte {}: {} (max {max})",
                    self.offset,
                    limit.as_str()
                )
            }
            UrlErrorKind::DuplicateOption(name) => {
                write!(f, "system query option {name} given twice")
            }
            UrlErrorKind::NotImplemented(feature) => {
                write!(
                    f,
                    "not implemented at byte {}: {}",
                    self.offset,
                    feature.as_str()
                )
            }
            UrlErrorKind::Unresolved => write!(f, "unknown name at byte {}", self.offset),
        }
    }
}

impl std::error::Error for UrlError {}

impl LimitKind {
    /// The limit's fixed name.
    ///
    /// ```
    /// assert_eq!(odata_url::LimitKind::ExprDepth.as_str(), "expression depth");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathLength => "path length",
            Self::QueryLength => "query length",
            Self::PathSegments => "path segments",
            Self::QueryOptions => "query options",
            Self::ExprDepth => "expression depth",
            Self::ExprNodes => "expression nodes",
            Self::ExpandDepth => "expand depth",
            Self::ExpandItems => "expand items",
            Self::SelectItems => "select items",
            Self::OrderbyItems => "orderby items",
            Self::InListLength => "in list length",
            Self::LiteralLength => "literal length",
            Self::Number => "number range",
        }
    }
}

impl Feature {
    /// The construct's fixed name.
    ///
    /// ```
    /// assert_eq!(odata_url::Feature::Index.as_str(), "$index");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Index => "$index",
            Self::Apply => "$apply transformation",
            Self::DeltaToken => "$deltatoken",
            Self::Root => "$root",
            Self::ImplicitVariable => "$it and $this",
            Self::AliasExpression => "parameter alias bound to an expression",
            Self::FilterSegment => "$filter path segment",
            Self::CrossJoin => "$crossjoin and $all",
            Self::TypeCast => "type cast or bound operation",
            Self::Ordinal => "ordinal index",
            Self::Operation => "function or action",
            Self::Each => "$each",
            Self::MediaValue => "media resource",
            Self::NestedOption => "nested query option",
            Self::NestedCount => "nested $count",
            Self::PathInExpression => "key or filter in an expression path",
            Self::Has => "has operator",
            Self::Method => "method call",
            Self::CastIsOf => "cast and isof",
            Self::InExpression => "in over a non-literal",
            Self::JsonLiteral => "JSON literal",
            Self::Literal => "literal type",
            Self::NonAsciiIdentifier => "non-ASCII identifier",
            Self::AnnotationPath => "annotation path",
            Self::PathStar => "* after a path in $expand",
        }
    }
}
