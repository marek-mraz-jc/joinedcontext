// SPDX-License-Identifier: EUPL-1.2
//! The bounds a parse runs under (ADR-0024 decision 4). The caller passes them; there is no
//! default on a request path.

/// Every bound of one parse. A count past its bound answers
/// [`LimitExceeded`](crate::UrlErrorKind::LimitExceeded); no counter wraps.
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
/// use odata_url::{parse, Limits};
/// let tight = Limits { max_path_segments: 1, ..Limits::strict() };
/// assert!(parse("Buildings", &Service, &tight).is_ok());
/// assert!(parse("Buildings/$count", &Service, &tight).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of the resource path (before `?`).
    pub max_path_len: usize,
    /// Bytes of the query string (after `?`).
    pub max_query_len: usize,
    /// Segments of the resource path, keys and `$`-segments included.
    pub max_path_segments: usize,
    /// `name=value` pairs of the query string.
    pub max_query_options: usize,
    /// Nesting of one expression: parentheses, `not`, lambdas, method calls.
    pub max_expr_depth: usize,
    /// Expression nodes over the whole request.
    pub max_expr_nodes: usize,
    /// Nested `$expand` levels; `$levels` counts as its number of levels, `max` as this bound.
    pub max_expand_depth: usize,
    /// `$expand` items over the whole request.
    pub max_expand_items: usize,
    /// `$select` items over the whole request.
    pub max_select_items: usize,
    /// `$orderby` items.
    pub max_orderby_items: usize,
    /// Items of one `in (...)` list.
    pub max_in_list_len: usize,
    /// Bytes of one literal as written in the URL.
    pub max_literal_len: usize,
}

impl Limits {
    /// The defaults T-3441 decided (`odata-linkml-mapping.md` section 10, S1 and S2): 2048-byte
    /// path, 8192-byte query (`MAX_URI_BYTES`), 32 segments, 32 options, expression depth 32
    /// with 256 nodes, expand depth 3 with 8 items, 64 select and 8 orderby items, 100-item
    /// lists, 1024-byte literals.
    ///
    /// ```
    /// let limits = odata_url::Limits::strict();
    /// assert_eq!(limits.max_expand_depth, 3);
    /// ```
    #[must_use]
    pub fn strict() -> Self {
        Limits {
            max_path_len: 2048,
            max_query_len: 8192,
            max_path_segments: 32,
            max_query_options: 32,
            max_expr_depth: 32,
            max_expr_nodes: 256,
            max_expand_depth: 3,
            max_expand_items: 8,
            max_select_items: 64,
            max_orderby_items: 8,
            max_in_list_len: 100,
            max_literal_len: 1024,
        }
    }
}
