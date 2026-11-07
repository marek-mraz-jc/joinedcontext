// SPDX-License-Identifier: EUPL-1.2
//! Format parameters, context URLs and the request headers that choose a response's shape
//! (JSON Format 4.01 sections 3 and 10; Protocol 4.01 sections 8.2.6 to 8.2.8).

use std::fmt::{self, Display};

/// `metadata` (`odata.metadata` in 4.0) of the JSON media type (JSON Format 4.01 section 3.1):
/// how much control information a response carries.
///
/// ```
/// use odata_json::MetadataLevel;
/// assert_eq!(MetadataLevel::parse("FULL"), Some(MetadataLevel::Full));
/// assert_eq!(MetadataLevel::parse("most"), None);
/// assert_eq!(MetadataLevel::default(), MetadataLevel::Minimal);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataLevel {
    /// No control information beyond `count` and `nextLink`.
    None,
    /// The context URL and what a client cannot compute from it (the default).
    #[default]
    Minimal,
    /// Also entity ids, types, navigation links and the types of non-string primitives.
    Full,
}

impl MetadataLevel {
    /// The level a parameter value names, ASCII case-insensitive; `None` for any other value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            ("none", Self::None),
            ("minimal", Self::Minimal),
            ("full", Self::Full),
        ]
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
        .map(|(_, level)| level)
    }
}

/// The OData version a response is written in: it names the control information (`@odata.id`
/// in 4.0, `@id` in 4.01, JSON Format 4.01 section 4.5).
///
/// ```
/// use odata_json::ODataVersion;
/// assert_eq!(ODataVersion::V4_01.as_str(), "4.01");
/// assert_eq!(ODataVersion::V4_0.to_string(), "4.0");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ODataVersion {
    /// `4.0`.
    V4_0,
    /// `4.01`.
    V4_01,
}

impl ODataVersion {
    /// The `OData-Version` header value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V4_0 => "4.0",
            Self::V4_01 => "4.01",
        }
    }

    /// The prefix of control information: `@odata.` in 4.0, `@` in 4.01 ("this prefix SHOULD be
    /// omitted").
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::V4_0 => "@odata.",
            Self::V4_01 => "@",
        }
    }
}

impl Display for ODataVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What every writer needs to know about the response.
///
/// ```
/// use odata_json::{Ctx, MetadataLevel, ODataVersion};
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_01, metadata_etag: None };
/// assert!(ctx.service_root.ends_with('/'));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx<'a> {
    /// The service root URL, ending in `/`; context URLs and absolute ids start with it.
    pub service_root: &'a str,
    /// The negotiated metadata level.
    pub level: MetadataLevel,
    /// The negotiated version ([`negotiate_version`]).
    pub version: ODataVersion,
    /// The metadata document's `ETag`, written after the context URL as
    /// `@odata.metadataEtag` (JSON Format 4.01 section 4.5.2); `None` writes none.
    pub metadata_etag: Option<&'a str>,
}

/// What a context URL describes (JSON Format 4.01 section 10).
///
/// ```
/// use odata_json::ContextTarget;
/// let target = ContextTarget::Collection { set: "Buildings", select: Some(&["id", "height"]) };
/// assert_eq!(target.to_string(), "Buildings(id,height)");
/// assert_eq!(ContextTarget::References.to_string(), "Collection($ref)");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTarget<'a> {
    /// The service document (section 10.1).
    ServiceDocument,
    /// A collection of entities of a set, projected by `$select` when given (10.2, 10.9).
    Collection {
        /// The entity set.
        set: &'a str,
        /// The `$select` list.
        select: Option<&'a [&'a str]>,
    },
    /// One entity of a set, projected by `$select` when given (10.3, 10.10).
    Entity {
        /// The entity set.
        set: &'a str,
        /// The `$select` list.
        select: Option<&'a [&'a str]>,
    },
    /// One entity reference (10.18).
    Reference,
    /// A collection of entity references (10.18).
    References,
}

/// The fragment after `$metadata`; empty for the service document.
impl Display for ContextTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let projected = |f: &mut fmt::Formatter<'_>, set: &str, select: Option<&[&str]>| {
            f.write_str(set)?;
            match select {
                Some(names) => write!(f, "({})", names.join(",")),
                None => Ok(()),
            }
        };
        match *self {
            Self::ServiceDocument => Ok(()),
            Self::Collection { set, select } => projected(f, set, select),
            Self::Entity { set, select } => {
                projected(f, set, select)?;
                f.write_str("/$entity")
            }
            Self::Reference => f.write_str("$ref"),
            Self::References => f.write_str("Collection($ref)"),
        }
    }
}

/// The context URL of a response (JSON Format 4.01 section 10): `{service root}$metadata`, then
/// `#` and the target unless it is the service document.
///
/// ```
/// use odata_json::{context_url, ContextTarget, Ctx, MetadataLevel, ODataVersion};
/// let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_0, metadata_etag: None };
/// assert_eq!(
///     context_url(&ctx, &ContextTarget::Entity { set: "Buildings", select: None }),
///     "https://example.org/odata/$metadata#Buildings/$entity"
/// );
/// assert_eq!(context_url(&ctx, &ContextTarget::ServiceDocument), "https://example.org/odata/$metadata");
/// ```
#[must_use]
pub fn context_url(ctx: &Ctx<'_>, target: &ContextTarget<'_>) -> String {
    match target {
        ContextTarget::ServiceDocument => format!("{}$metadata", ctx.service_root),
        _ => format!("{}$metadata#{target}", ctx.service_root),
    }
}

/// Why a request header was refused.
///
/// ```
/// use odata_json::{negotiate_version, HeaderError};
/// assert_eq!(negotiate_version(Some("5.0"), None), Err(HeaderError::UnsupportedVersion));
/// assert_eq!(HeaderError::TooLong.to_string(), "the header is longer than this service reads");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    /// Longer than the caller's bound.
    TooLong,
    /// Not a version number.
    Malformed,
    /// A version this service does not speak: `OData-Version` other than 4.0 or 4.01, or an
    /// `OData-MaxVersion` below 4.0 (Protocol 4.01 section 8.2.7).
    UnsupportedVersion,
}

impl Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLong => "the header is longer than this service reads",
            Self::Malformed => "the header is not a version number",
            Self::UnsupportedVersion => "the requested OData version is not supported",
        })
    }
}

impl std::error::Error for HeaderError {}

/// `major.minor` as (major, hundredths): `4.01` is (4, 1), `4.1` is (4, 10).
fn version(text: &str) -> Result<(u8, u8), HeaderError> {
    let (major, minor) = text.trim().split_once('.').ok_or(HeaderError::Malformed)?;
    let digits =
        |s: &str, max: usize| (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major, 2) || !digits(minor, 2) {
        return Err(HeaderError::Malformed);
    }
    let number = |s: &str| s.parse::<u8>().map_err(|_| HeaderError::Malformed);
    let hundredths = if minor.len() == 1 {
        number(minor)?.saturating_mul(10)
    } else {
        number(minor)?
    };
    Ok((number(major)?, hundredths))
}

/// The version of the response, from the request's `OData-Version` and `OData-MaxVersion`
/// headers (Protocol 4.01 sections 8.2.6, 8.2.7): the highest version this service speaks that
/// is at most `OData-MaxVersion`; without it, the request's own version; without either, 4.0.
///
/// ```
/// use odata_json::{negotiate_version, HeaderError, ODataVersion};
/// assert_eq!(negotiate_version(None, Some("4.01")), Ok(ODataVersion::V4_01));
/// assert_eq!(negotiate_version(Some("4.0"), Some("4.02")), Ok(ODataVersion::V4_01));
/// assert_eq!(negotiate_version(Some("4.01"), None), Ok(ODataVersion::V4_01));
/// assert_eq!(negotiate_version(None, None), Ok(ODataVersion::V4_0));
/// assert_eq!(negotiate_version(None, Some("3.0")), Err(HeaderError::UnsupportedVersion));
/// ```
///
/// # Errors
///
/// [`HeaderError::Malformed`] for a value that is not `major.minor`, and
/// [`HeaderError::UnsupportedVersion`] as that variant says.
pub fn negotiate_version(
    odata_version: Option<&str>,
    max_version: Option<&str>,
) -> Result<ODataVersion, HeaderError> {
    let request = match odata_version.map(version).transpose()? {
        None | Some((4, 0)) => ODataVersion::V4_0,
        Some((4, 1)) => ODataVersion::V4_01,
        Some(_) => return Err(HeaderError::UnsupportedVersion),
    };
    match max_version.map(version).transpose()? {
        None => Ok(request),
        Some(max) if max < (4, 0) => Err(HeaderError::UnsupportedVersion),
        Some(max) if max < (4, 1) => Ok(ODataVersion::V4_0),
        Some(_) => Ok(ODataVersion::V4_01),
    }
}

/// The preferences of a `Prefer` header this crate maps (ADR-0024 decision 5); every other
/// preference is ignored, as RFC 7240 section 2 asks.
///
/// ```
/// use odata_json::{parse_prefer, Prefer};
/// let prefer = parse_prefer("return=minimal, odata.maxpagesize=50", 256)?;
/// assert_eq!(prefer, Prefer { maxpagesize: Some(50), continue_on_error: false });
/// # Ok::<(), odata_json::HeaderError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prefer {
    /// `odata.maxpagesize` (`maxpagesize` in 4.01), a positive integer (Protocol 4.01 8.2.8.3).
    pub maxpagesize: Option<u64>,
    /// `odata.continue-on-error` (`continue-on-error`), for `$batch` (8.2.8.4).
    pub continue_on_error: bool,
}

/// `text` split at `delimiter` outside double-quoted strings.
fn split_unquoted(text: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    for (at, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if c == delimiter && !quoted => {
                parts.push(text.get(start..at).unwrap_or_default());
                start = at.saturating_add(1);
            }
            _ => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default());
    parts
}

/// A preference value without its optional double quotes and escapes.
fn unquoted(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for c in inner.chars() {
        if c == '\\' && !escaped {
            escaped = true;
        } else {
            out.push(c);
            escaped = false;
        }
    }
    out
}

/// The `Prefer` header (all of them joined by `,`): each preference's name, ASCII
/// case-insensitive, with or without the `odata.` prefix; the first instance of a preference
/// counts (RFC 7240 section 2); a value this crate cannot read leaves the preference unset.
///
/// ```
/// use odata_json::{parse_prefer, HeaderError};
/// let prefer = parse_prefer("MaxPageSize=\"10\"; x=y, odata.maxpagesize=20, continue-on-error", 256)?;
/// assert_eq!(prefer.maxpagesize, Some(10));
/// assert!(prefer.continue_on_error);
/// assert_eq!(parse_prefer("odata.maxpagesize=0", 256)?.maxpagesize, None);
/// assert_eq!(parse_prefer(&"x".repeat(300), 256), Err(HeaderError::TooLong));
/// # Ok::<(), HeaderError>(())
/// ```
///
/// # Errors
///
/// [`HeaderError::TooLong`] when the header is longer than `max_len` bytes.
pub fn parse_prefer(header: &str, max_len: usize) -> Result<Prefer, HeaderError> {
    if header.len() > max_len {
        return Err(HeaderError::TooLong);
    }
    let mut prefer = Prefer::default();
    let (mut page_seen, mut continue_seen) = (false, false);
    for preference in split_unquoted(header, ',') {
        let head = split_unquoted(preference, ';')
            .into_iter()
            .next()
            .unwrap_or_default();
        let (name, value) = match head.split_once('=') {
            Some((name, value)) => (name.trim(), Some(unquoted(value.trim()))),
            None => (head.trim(), None),
        };
        let name = name.to_ascii_lowercase();
        match name.strip_prefix("odata.").unwrap_or(&name) {
            "maxpagesize" if !page_seen => {
                page_seen = true;
                prefer.maxpagesize = value
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|size| *size > 0);
            }
            "continue-on-error" if !continue_seen => {
                continue_seen = true;
                prefer.continue_on_error = match value.as_deref() {
                    None => true,
                    Some(v) => v.eq_ignore_ascii_case("true"),
                };
            }
            _ => {}
        }
    }
    Ok(prefer)
}
