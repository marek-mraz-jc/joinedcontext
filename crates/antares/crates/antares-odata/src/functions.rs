// SPDX-License-Identifier: EUPL-1.2
//! The `$filter` methods this surface serves: each one's residual
//! function and argument families, and the `Capabilities.FilterFunctions`
//! list `$metadata` publishes from them. A leaf, so `edm` reads the list
//! without depending on the `$filter` translator in `filter`.

use antares_ops::residual::Func;
use odata_url::Method;

/// The families a numeric parameter takes.
pub(crate) const NUMBER: &[&str] = &["integer", "number"];
pub(crate) const STRING: &[&str] = &["Edm.String"];
const DATE: &[&str] = &["Edm.Date", "Edm.DateTimeOffset"];
const TIME: &[&str] = &["Edm.TimeOfDay", "Edm.DateTimeOffset"];
const INSTANT: &[&str] = &["Edm.DateTimeOffset"];

/// A method's residual function, the families each argument takes (the
/// last for the rest), and its result type; `None` for the result means
/// the first argument's (the rounding functions). `geo.intersects` is a
/// conjunct of its own (F29), never a value.
type Signature = (
    Func,
    &'static [&'static [&'static str]],
    Option<&'static str>,
);

/// `Capabilities.FilterFunctions`: every operator and function a `$filter`
/// translates to `q` or the residual, in the URL convention's names; a
/// method parsed without a [`signature`] is not served, `geo.intersects`
/// is a `georel`.
pub(crate) fn filter_functions() -> Vec<&'static str> {
    const OPERATORS: [&str; 18] = [
        "eq", "ne", "gt", "ge", "lt", "le", "and", "or", "not", "in", "add", "sub", "mul", "div",
        "divby", "mod", "any", "all",
    ];
    OPERATORS
        .into_iter()
        .chain(
            Method::all()
                .filter(|m| *m == Method::GeoIntersects || signature(*m).is_some())
                .map(Method::name),
        )
        .collect()
}

pub(crate) fn signature(method: Method) -> Option<Signature> {
    const BOOL: Option<&str> = Some("Edm.Boolean");
    const INT: Option<&str> = Some("Edm.Int32");
    const STR: Option<&str> = Some("Edm.String");
    Some(match method {
        Method::Contains => (Func::Contains, &[STRING], BOOL),
        Method::StartsWith => (Func::StartsWith, &[STRING], BOOL),
        Method::EndsWith => (Func::EndsWith, &[STRING], BOOL),
        Method::MatchesPattern => (Func::MatchesPattern, &[STRING], BOOL),
        Method::Concat => (Func::Concat, &[STRING], STR),
        Method::IndexOf => (Func::IndexOf, &[STRING], INT),
        Method::Length => (Func::Length, &[STRING], INT),
        Method::Substring => (Func::Substring, &[STRING, NUMBER], STR),
        Method::ToLower => (Func::ToLower, &[STRING], STR),
        Method::ToUpper => (Func::ToUpper, &[STRING], STR),
        Method::Trim => (Func::Trim, &[STRING], STR),
        Method::Year => (Func::Year, &[DATE], INT),
        Method::Month => (Func::Month, &[DATE], INT),
        Method::Day => (Func::Day, &[DATE], INT),
        Method::Hour => (Func::Hour, &[TIME], INT),
        Method::Minute => (Func::Minute, &[TIME], INT),
        Method::Second => (Func::Second, &[TIME], INT),
        Method::FractionalSeconds => (Func::FractionalSeconds, &[TIME], Some("Edm.Decimal")),
        Method::TotalOffsetMinutes => (Func::TotalOffsetMinutes, &[INSTANT], INT),
        Method::Date => (Func::Date, &[INSTANT], Some("Edm.Date")),
        Method::Time => (Func::Time, &[INSTANT], Some("Edm.TimeOfDay")),
        Method::Now => (Func::Now, &[], Some("Edm.DateTimeOffset")),
        Method::MinDateTime => (Func::MinDateTime, &[], Some("Edm.DateTimeOffset")),
        Method::MaxDateTime => (Func::MaxDateTime, &[], Some("Edm.DateTimeOffset")),
        Method::Round => (Func::Round, &[NUMBER], None),
        Method::Floor => (Func::Floor, &[NUMBER], None),
        Method::Ceiling => (Func::Ceiling, &[NUMBER], None),
        Method::GeoIntersects => return None,
    })
}
