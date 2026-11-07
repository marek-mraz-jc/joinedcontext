// SPDX-License-Identifier: EUPL-1.2
//! The query options and the resource path of a SensorThings request, bounded before they are
//! read: `$filter` compiled to the NGSI-LD `q` (and, on a series, the temporal window), the page
//! options, and the path split without cutting a key literal.

use serde_json::{json, Map, Value};

/// The longest `$filter` read, in bytes.
pub const MAX_FILTER: usize = 4096;

/// The default and the largest `$top`.
pub const MAX_TOP: usize = 100;

/// The largest `$skip`: what one view request may offset to.
pub const MAX_SKIP: usize = 10_000;

/// The serialized size a page is cut at, between whole resources.
pub const PAGE_BYTES: usize = 1 << 20;

/// The deepest `$expand` path.
const MAX_EXPAND: usize = 2;

pub use crate::residual::Bad;

/// The query options of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// `$top`, 0 to [`MAX_TOP`]; 0 answers the count alone.
    pub top: usize,
    /// `$skip`.
    pub skip: usize,
    /// `$count=true`.
    pub count: bool,
    /// `$select`: the members each resource keeps; empty keeps every one (`id` is `@iot.id`).
    pub select: Vec<String>,
    /// Each `$expand` path, its nested options left out.
    pub expand: Vec<String>,
    /// `$orderby=phenomenonTime desc`, the order a Datastream's Observations are read in.
    pub ordered: bool,
    /// `$orderby`: member paths, each with `true` for `desc`; empty without.
    pub orderby: Vec<(Vec<String>, bool)>,
    /// `$filter`, as sent.
    pub filter: Option<String>,
    /// `$skiptoken` of a set whose Entity holds several resources, as its `@iot.nextLink` writes
    /// it: the Entity offset and how many of that Entity's resources were served.
    pub resume: Option<(usize, usize)>,
}

/// The query options of `pairs`.
///
/// # Errors
/// [`Bad`] naming the option that is no number, too deep, too large or an order not served.
pub fn options(pairs: &[(String, String)]) -> Result<Options, Bad> {
    let mut out = Options {
        top: MAX_TOP,
        skip: 0,
        count: false,
        select: Vec::new(),
        expand: Vec::new(),
        ordered: false,
        orderby: Vec::new(),
        filter: None,
        resume: None,
    };
    for (name, value) in pairs {
        match name.as_str() {
            "$top" => out.top = number("$top", value, usize::MAX)?.min(MAX_TOP),
            "$skip" => out.skip = number("$skip", value, MAX_SKIP)?,
            "$count" => out.count = value == "true",
            "$select" => {
                out.select = list(value)
                    .map(|s| if s == "id" { "@iot.id" } else { s }.to_owned())
                    .collect();
            }
            "$expand" => {
                // `Observations($top=1)`: the path before its nested options.
                let paths: Vec<&str> = list(value).filter_map(|p| p.split('(').next()).collect();
                if paths.iter().any(|p| p.split('/').count() > MAX_EXPAND) {
                    return Err(Bad(
                        "$expand",
                        format!("a path is at most {MAX_EXPAND} deep"),
                    ));
                }
                out.expand = paths.into_iter().map(str::to_owned).collect();
            }
            "$orderby" => {
                out.orderby = crate::residual::orderby(value)?;
                out.ordered = out.orderby.len() == 1
                    && out
                        .orderby
                        .first()
                        .is_some_and(|(p, desc)| *desc && *p == ["phenomenonTime"]);
            }
            "$filter" if value.len() > MAX_FILTER => {
                return Err(Bad("$filter", format!("at most {MAX_FILTER} bytes")));
            }
            "$filter" => out.filter = Some(value.clone()),
            "$skiptoken" => {
                let usage = || Bad("$skiptoken", "the value an @iot.nextLink carries".into());
                let (entity, served) = value.split_once('.').ok_or_else(usage)?;
                out.resume = Some((
                    number("$skiptoken", entity, MAX_SKIP)?,
                    number("$skiptoken", served, MAX_SKIP)?,
                ));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The comma-separated items of an option, trimmed, empty ones left out.
fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// A whole number of at most `max`.
fn number(name: &'static str, value: &str, max: usize) -> Result<usize, Bad> {
    value
        .parse::<usize>()
        .ok()
        .filter(|n| *n <= max)
        .ok_or_else(|| Bad(name, format!("a whole number, at most {max}")))
}

/// What a `$filter` property path of a set names: the NGSI-LD Attribute, or a refusal.
pub type Names<'a> = &'a dyn Fn(&str) -> Result<String, Bad>;

/// The path of a set's resource to its Thing: a Thing is its Entity, a Datastream's Thing and an
/// Observation's Datastream's Thing are too.
fn thing_path<'a>(set: &str, path: &'a str) -> Option<&'a str> {
    match set {
        "Things" => Some(path),
        "Datastreams" => path.strip_prefix("Thing/"),
        "Observations" => path.strip_prefix("Datastream/Thing/"),
        _ => None,
    }
}

/// The NGSI-LD Attribute an STA property path of `set` reads (T-5205 finding 8): a Thing's
/// `name`, `description` and `properties/{attribute}`, reached from a Datastream through `Thing/`
/// and from an Observation through `Datastream/Thing/`. Every other path is refused by name, so no
/// filter silently reads an Attribute the client did not mean.
///
/// # Errors
/// [`Bad`] naming the path.
pub fn property(set: &str, path: &str) -> Result<String, Bad> {
    let attribute = thing_path(set, path).and_then(|p| match p {
        "name" | "description" => Some(p),
        _ => p
            .strip_prefix("properties/")
            .filter(|a| a.chars().all(name_char) && !a.is_empty() && *a != "type"),
    });
    attribute.map(str::to_owned).ok_or_else(|| {
        Bad(
            "$filter",
            format!("{path} is not a property {set} are filtered on"),
        )
    })
}

/// A character of an NGSI-LD `q` attribute name (CIM 009 4.9 nameChar): a letter, a digit or `_`.
/// Every `q` operator lies outside it, so a path cannot carry one into the query.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `$filter` as the NGSI-LD `q`: comparisons, `and`, `or`, parentheses and `substringof`, each
/// property read through `names`; anything else refused by name, since a filter dropped silently
/// answers rows the caller asked not to see.
///
/// # Errors
/// [`Bad`] naming the construct.
pub fn filter_to_q(filter: &str, names: Names<'_>) -> Result<String, Bad> {
    let mut out = String::new();
    let mut rest = filter.trim();
    let mut operand = true;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('(') {
            out.push('(');
            (rest, operand) = (tail.trim_start(), true);
            continue;
        }
        if let Some(tail) = rest.strip_prefix(')') {
            out.push(')');
            (rest, operand) = (tail.trim_start(), false);
            continue;
        }
        if operand && rest.starts_with('\'') {
            let text = literal(&mut rest)?;
            out.push_str(&quoted(&text));
            (rest, operand) = (rest.trim_start(), false);
            continue;
        }
        let (token, tail) = match rest.find([' ', '(', ')']) {
            Some(at) => rest.split_at(at),
            None => (rest, ""),
        };
        rest = tail.trim_start();
        if !operand {
            out.push_str(connective(token)?);
            operand = true;
        } else if token.eq_ignore_ascii_case("substringof") {
            out.push_str(&substring(&mut rest, names)?);
            operand = false;
        } else if token.eq_ignore_ascii_case("not") {
            return Err(bad(
                "not is not supported; write the negated comparison instead",
            ));
        } else if tail.starts_with('(') {
            return Err(Bad("$filter", format!("{token}() is not supported")));
        } else {
            out.push_str(&term(token, names)?);
            operand = false;
        }
    }
    if out.is_empty() {
        return Err(bad("the filter is empty"));
    }
    Ok(out)
}

/// A `$filter` refusal.
fn bad(detail: &str) -> Bad {
    Bad("$filter", detail.to_owned())
}

/// A comparison or logical operator as NGSI-LD writes it.
fn connective(token: &str) -> Result<&'static str, Bad> {
    Ok(match token.to_ascii_lowercase().as_str() {
        "eq" => "==",
        "ne" => "!=",
        "gt" => ">",
        "ge" => ">=",
        "lt" => "<",
        "le" => "<=",
        "and" => ";",
        "or" => "|",
        other => return Err(Bad("$filter", format!("{other} is not supported"))),
    })
}

/// The OData string literal `rest` starts with (`'…'`, a quote written twice), unquoted; `rest`
/// moves past it.
fn literal(rest: &mut &str) -> Result<String, Bad> {
    let mut text = String::new();
    let mut chars = rest.char_indices().skip(1).peekable();
    while let Some((at, c)) = chars.next() {
        if c != '\'' {
            text.push(c);
        } else if chars.next_if(|(_, n)| *n == '\'').is_some() {
            text.push('\'');
        } else {
            *rest = rest.get(at.saturating_add(1)..).unwrap_or_default();
            return Ok(text);
        }
    }
    Err(bad("a text literal is not closed with '"))
}

/// A text as an NGSI-LD `quotedStr` (an RFC 8259 String: `"` and `\` escaped).
fn quoted(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// `substringof('text',property)` as a pattern match, the text matched literally; `rest` starts
/// at its parenthesis.
fn substring(rest: &mut &str, names: Names<'_>) -> Result<String, Bad> {
    let usage = || bad("substringof takes ('text', property)");
    let mut inner = rest.strip_prefix('(').ok_or_else(usage)?.trim_start();
    if !inner.starts_with('\'') {
        return Err(usage());
    }
    let needle = literal(&mut inner)?;
    let (property, after) = inner
        .trim_start()
        .strip_prefix(',')
        .and_then(|p| p.split_once(')'))
        .ok_or_else(usage)?;
    let property = property.trim();
    if ID.contains(&property) {
        return Err(bad("id takes eq or ne on the top-level and"));
    }
    if needle.is_empty() || property.is_empty() {
        return Err(usage());
    }
    // A pattern ends at its first `"` (NGSI-LD 4.9 RegExp), so one cannot be matched.
    if needle.contains('"') {
        return Err(bad("substringof cannot match a text holding \""));
    }
    let mut pattern = String::new();
    for c in needle.chars() {
        if r".^$*+?()[]{}|\".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    *rest = after.trim_start();
    Ok(format!("{}~=\".*{pattern}.*\"", names(property)?))
}

/// The names a resource's id goes by: no Attribute, so never a `q` operand.
const ID: [&str; 2] = ["id", "@iot.id"];

/// An unquoted operand: a number, `true`, `false` or an instant as written, else a property path
/// read through `names`. The id is refused: [`by_id`] reads its top-level comparisons. Any other
/// token that starts like a value is refused, so none carries a `q` operator past [`property`].
fn term(token: &str, names: Names<'_>) -> Result<String, Bad> {
    if ID.contains(&token) {
        return Err(bad("id takes eq or ne on the top-level and"));
    }
    if is_value(token) {
        return Ok(token.to_owned());
    }
    if token.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        return Err(Bad(
            "$filter",
            format!("{token} is not a number, true, false or an instant such as 2026-01-01T00:00:00Z; quote a text"),
        ));
    }
    names(token)
}

/// Whether `token` is an unquoted value: `true`, `false`, a finite number or an RFC 3339 instant
/// (`2026-01-01T00:00:00Z`, `2026-01-01T00:00:00.5+02:00`). Each is digits and fixed punctuation.
fn is_value(token: &str) -> bool {
    let shape: String = token
        .chars()
        .map(|c| if c.is_ascii_digit() { '9' } else { c })
        .collect();
    let number = shape.chars().all(|c| "9-+.eE".contains(c))
        && token.parse::<f64>().is_ok_and(f64::is_finite);
    let (clock, zone) = (shape.get(..19), shape.get(19..).unwrap_or_default());
    let zone = match zone.strip_prefix('.') {
        Some(fraction) if fraction.starts_with('9') => fraction.trim_start_matches('9'),
        Some(_) => "",
        None => zone,
    };
    let instant = clock == Some("9999-99-99T99:99:99") && matches!(zone, "Z" | "+99:99" | "-99:99");
    matches!(token, "true" | "false") || number || instant
}

/// A set's `$filter` split (T-4928): the resource ids its top-level `and` compares with `eq`
/// and `ne`, which no `q` can name, the types `properties/type eq` names (the view's `type`
/// selector), and the `q` of the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ById {
    /// `id eq` literals: the resource is each of them.
    pub is: Vec<String>,
    /// `id ne` literals: the resource is none of them.
    pub not: Vec<String>,
    /// `properties/type eq` literals: the Entity is of each of them.
    pub types: Vec<String>,
    /// The NGSI-LD `q` of every other conjunct.
    pub q: Option<String>,
}

impl ById {
    /// Whether the resource of `id` passes the id comparisons.
    #[must_use]
    pub fn admits(&self, id: Option<&str>) -> bool {
        self.is.iter().all(|i| Some(i.as_str()) == id)
            && !self.not.iter().any(|i| Some(i.as_str()) == id)
    }
}

/// The `$filter` of the entity set `set` as [`ById`].
///
/// # Errors
/// [`Bad`] naming the construct: an id or a type compared other than by `eq` (an id also `ne`)
/// with a quoted literal, or anything [`filter_to_q`] refuses.
pub fn by_id(filter: &str, set: &str) -> Result<ById, Bad> {
    let mut out = ById::default();
    let mut rest = Vec::new();
    for conjunct in conjuncts(filter) {
        let mut words = conjunct.splitn(3, ' ');
        let (first, operator, literal) = (words.next(), words.next(), words.next());
        let first = first.unwrap_or_default();
        let kind = first == "properties/type" || thing_path(set, first) == Some("properties/type");
        if !kind && !ID.contains(&first) {
            rest.push(conjunct);
            continue;
        }
        let literal = literal
            .map(str::trim)
            .and_then(|l| l.strip_prefix('\'')?.strip_suffix('\''))
            .filter(|l| !l.is_empty())
            .ok_or_else(|| bad("id and properties/type compare with a quoted literal"))?
            .replace("''", "'");
        match (kind, operator.map(str::to_ascii_lowercase).as_deref()) {
            (true, Some("eq")) => out.types.push(literal),
            (false, Some("eq")) => out.is.push(literal),
            (false, Some("ne")) => out.not.push(literal),
            _ => {
                return Err(bad(
                    "id takes eq or ne, properties/type takes eq, on the top-level and",
                ))
            }
        }
    }
    if !rest.is_empty() {
        let names = |p: &str| property(set, p);
        out.q = Some(filter_to_q(&rest.join(" and "), &names)?);
    }
    Ok(out)
}

/// The `$filter` of a Datastream's Observations as the temporal window (`timerel`, `timeAt`,
/// `endTimeAt`) of its `phenomenonTime` predicates. Retrieve Temporal Evolution takes no `q`
/// (NGSI-LD 6.19.3.1), so a predicate on anything else, `result` too, is refused by name
/// (`docs/src/sta.md`, not served). `phenomenonTime` is an instance's instant, read by NGSI-LD as a window on the whole request,
/// so a predicate on it stands on the top-level `and` alone; inside a parenthesis or an `or` it
/// is refused. The window is exact: NGSI-LD's `after` holds its instant and `before` does not
/// (4.11), so `gt` and `le` move it one millisecond on.
///
/// # Errors
/// [`Bad`] naming the predicate.
pub fn series_filter(filter: &str) -> Result<Vec<(&'static str, String)>, Bad> {
    const TIME: &str = "phenomenontime";
    let usage = || bad("phenomenonTime takes `phenomenonTime <op> <instant>` on the top-level and");
    let (mut start, mut end) = (None, None);
    let mut rest = Vec::new();
    for conjunct in conjuncts(filter) {
        let lowered = conjunct.to_ascii_lowercase();
        if !lowered.contains(TIME) {
            rest.push(conjunct);
            continue;
        }
        let words: Vec<&str> = conjunct.split_whitespace().collect();
        let [first, operator, instant] = words.as_slice() else {
            return Err(usage());
        };
        if !first.eq_ignore_ascii_case(TIME) {
            return Err(usage());
        }
        let at = crate::mapping::instant(instant.trim_matches('\''));
        let after = || {
            crate::mapping::next_milli(&at).ok_or_else(|| {
                bad("phenomenonTime compares with a UTC instant such as 2026-01-01T00:00:00Z")
            })
        };
        match operator.to_ascii_lowercase().as_str() {
            "ge" => start = Some(at),
            "gt" => start = Some(after()?),
            "lt" => end = Some(at),
            "le" => end = Some(after()?),
            "eq" => (start, end) = (Some(at.clone()), Some(after()?)),
            other => {
                return Err(Bad(
                    "$filter",
                    format!("{other} is not supported on phenomenonTime"),
                ))
            }
        }
    }
    let window = match (start, end) {
        (Some(s), Some(e)) if s >= e => return Err(bad("the window ends before it starts")),
        (Some(s), Some(e)) => vec![
            ("timerel", "between".into()),
            ("timeAt", s),
            ("endTimeAt", e),
        ],
        (Some(s), None) => vec![("timerel", "after".into()), ("timeAt", s)],
        (None, Some(e)) => vec![("timerel", "before".into()), ("timeAt", e)],
        (None, None) => Vec::new(),
    };
    if !rest.is_empty() {
        let names = |p: &str| {
            Err(Bad(
                "$filter",
                format!("{p}: a Datastream's Observations are filtered on phenomenonTime only"),
            ))
        };
        filter_to_q(&rest.join(" and "), &names)?;
        return Err(bad(
            "a Datastream's Observations are filtered on phenomenonTime only",
        ));
    }
    Ok(window)
}

/// The top-level ` and `-separated terms of a filter, what a parenthesis or a text literal holds
/// left whole; split on character boundaries only (T-4323).
fn conjuncts(filter: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let (mut depth, mut start, mut resume, mut text) = (0usize, 0usize, 0usize, false);
    for (at, c) in filter.char_indices() {
        if at < resume {
            continue;
        }
        match c {
            // A doubled quote inside a literal closes and reopens it: the same state after both.
            '\'' => text = !text,
            _ if text => {}
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ' ' if depth == 0
                && filter
                    .get(at..at.saturating_add(5))
                    .is_some_and(|w| w.eq_ignore_ascii_case(" and ")) =>
            {
                terms.extend(filter.get(start..at));
                (start, resume) = (at.saturating_add(5), at.saturating_add(5));
            }
            _ => {}
        }
    }
    terms.extend(filter.get(start..));
    terms
        .into_iter()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// One step of a resource path: the set and the key it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The set name.
    pub set: String,
    /// The key literal, unquoted.
    pub key: Option<String>,
}

/// The steps of a resource path, a slash inside a quoted key left in it (a Datastream id is
/// `{urn}/{attribute}`).
#[must_use]
pub fn steps(path: &str) -> Vec<Step> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in path.chars() {
        match c {
            '/' if !quoted => out.extend(step(&std::mem::take(&mut current))),
            '\'' => {
                quoted = !quoted;
                current.push(c);
            }
            _ => current.push(c),
        }
    }
    out.extend(step(&current));
    out
}

/// One step: `Set` or `Set('key')`, a doubled quote in the key read as one.
fn step(raw: &str) -> Option<Step> {
    if raw.is_empty() {
        return None;
    }
    Some(match raw.split_once('(') {
        Some((set, rest)) => {
            let key = rest.strip_suffix(')').unwrap_or(rest);
            let key = key.strip_prefix('\'').unwrap_or(key);
            let key = key.strip_suffix('\'').unwrap_or(key);
            Step {
                set: set.to_owned(),
                key: Some(key.replace("''", "'")),
            }
        }
        None => Step {
            set: raw.to_owned(),
            key: None,
        },
    })
}

/// The resources of a page cut at [`PAGE_BYTES`] between Entities, never inside one; the
/// Entities served, for the next page's `$skip`. A first Entity larger than the budget is
/// served alone.
#[must_use]
pub fn fit(groups: Vec<Vec<Value>>) -> (Vec<Value>, usize) {
    let mut out = Vec::new();
    let (mut bytes, mut served) = (0usize, 0usize);
    for group in groups {
        let size = group
            .iter()
            .map(|v| v.to_string().len().saturating_add(1))
            .fold(0usize, usize::saturating_add);
        bytes = bytes.saturating_add(size);
        if bytes > PAGE_BYTES && served > 0 {
            break;
        }
        out.extend(group);
        served = served.saturating_add(1);
    }
    (out, served)
}

/// A collection: `@iot.count` when known, `value`, `@iot.nextLink` when more remain. `select`
/// keeps those members of each resource.
#[must_use]
pub fn collection(
    items: Vec<Value>,
    select: &[String],
    count: Option<usize>,
    next: Option<String>,
) -> Value {
    let mut out = Map::new();
    if let Some(count) = count {
        out.insert("@iot.count".into(), json!(count));
    }
    let items = items.into_iter().map(|i| selected(i, select)).collect();
    out.insert("value".into(), Value::Array(items));
    if let Some(next) = next {
        out.insert("@iot.nextLink".into(), json!(next));
    }
    Value::Object(out)
}

/// The members of `item` that `select` names; every one for an empty `select`.
#[must_use]
pub fn selected(mut item: Value, select: &[String]) -> Value {
    if let (Some(map), false) = (item.as_object_mut(), select.is_empty()) {
        map.retain(|k, _| select.iter().any(|s| s == k));
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self link's key, percent-encoded by `mapping::self_link`, is the
    /// key the route reads back: decode the path once, then split it.
    #[test]
    fn a_self_links_key_reads_back_through_the_route_steps() {
        const BASE: &str = "http://h/x/views/v/sta/v1.1";
        for (id, key) in [
            ("urn:a/NO2#ds", "urn:a%2FNO2%23ds"),
            ("urn:a%2FNO2", "urn:a%252FNO2"),
            ("urn:a?b", "urn:a%3Fb"),
            ("urn:a b", "urn:a%20b"),
            ("urn:á", "urn:%C3%A1"),
            ("urn:a')/../x", "urn:a''%29%2F..%2Fx"),
        ] {
            let link = crate::mapping::self_link(BASE, "Datastreams", id);
            assert_eq!(link, format!("{BASE}/Datastreams('{key}')"), "{id}");
            // what the route does with it: decode the path once, then read the key
            let path = link.strip_prefix(BASE).unwrap_or_default();
            let decoded = antares_ops::percent::decoded(path).unwrap_or_default();
            let steps = steps(&decoded);
            let keys: Vec<_> = steps.iter().map(|s| s.key.as_deref()).collect();
            assert_eq!(keys, [Some(id)], "{id}: {decoded}");
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn options_are_bounded_and_named_when_refused() {
        let o = options(&pairs(&[
            ("$top", "500"),
            ("$skip", "3"),
            ("$count", "true"),
            ("$select", "id, name,"),
            ("$expand", "Locations,Datastreams/Observations($top=1)"),
            ("$orderby", "phenomenonTime DESC"),
            ("$filter", "a eq 1"),
            ("other", "x"),
        ]));
        assert_eq!(
            o,
            Ok(Options {
                top: 100,
                skip: 3,
                count: true,
                select: vec!["@iot.id".into(), "name".into()],
                expand: vec!["Locations".into(), "Datastreams/Observations".into()],
                ordered: true,
                orderby: vec![(vec!["phenomenonTime".into()], true)],
                filter: Some("a eq 1".into()),
                resume: None,
            })
        );
        let nested = options(&pairs(&[
            ("$expand", "Observations($top=1)"),
            ("$skiptoken", "7.2"),
        ]));
        assert_eq!(
            nested.as_ref().map(|o| (o.expand.clone(), o.resume)),
            Ok((vec!["Observations".to_owned()], Some((7, 2))))
        );
        for token in ["7", "x.1", "10001.0", "1.-1"] {
            assert_eq!(
                options(&pairs(&[("$skiptoken", token)])).map_err(|b| b.0),
                Err("$skiptoken"),
                "{token}"
            );
        }
        // T-5205 finding 19: `$top=0` is the count alone. Any other order is read over the
        // collection (T-5918); one that names no property is refused.
        assert_eq!(options(&pairs(&[("$top", "0")])).map(|o| o.top), Ok(0));
        for order in [
            "name",
            "resultTime desc,phenomenonTime asc",
            "phenomenonTime asc",
        ] {
            let ordered = options(&pairs(&[("$orderby", order)])).map(|o| o.ordered);
            assert_eq!(ordered, Ok(false), "{order}");
        }
        assert_eq!(
            options(&pairs(&[("$orderby", "name sideways")])).map_err(|b| b.0),
            Err("$orderby")
        );
        assert!(options(&pairs(&[("$top", "x")])).is_err());
        assert_eq!(
            options(&pairs(&[("$skip", "10001")])).map_err(|b| b.0),
            Err("$skip")
        );
        assert_eq!(
            options(&pairs(&[("$expand", "A/B/C")])).map_err(|b| b.0),
            Err("$expand")
        );
        let huge = "a".repeat(MAX_FILTER + 1);
        assert_eq!(
            options(&pairs(&[("$filter", &huge)])).map_err(|b| b.0),
            Err("$filter")
        );
    }

    /// T-4928: a resource id is no Attribute; a `q` naming it scanned the whole store on prod.
    #[test]
    fn an_id_comparison_is_read_by_id_and_never_sent_as_q() {
        assert_eq!(
            by_id(
                "id eq 'urn:a/x/2026' and properties/speed gt 3 and @iot.id ne 'it''s'",
                "Things"
            ),
            Ok(ById {
                is: vec!["urn:a/x/2026".into()],
                not: vec!["it's".into()],
                types: Vec::new(),
                q: Some("speed>3".into()),
            })
        );
        assert_eq!(
            by_id("properties/speed gt 3", "Things").map(|b| b.q),
            Ok(Some("speed>3".into()))
        );
        for refused in [
            "id gt 'a'",
            "id eq 3",
            "id eq ''",
            "(id eq 'a')",
            "name gt 3 or id eq 'a'",
            "substringof('a', id)",
            "properties/type ne 'Car'",
        ] {
            assert_eq!(
                by_id(refused, "Things").map_err(|b| b.0),
                Err("$filter"),
                "{refused}"
            );
        }
        let split = by_id("id eq 'a' and id ne 'b'", "Things").unwrap_or_default();
        assert!(split.admits(Some("a")));
        assert!(!split.admits(Some("b")));
        assert!(!split.admits(None));
        assert!(ById::default().admits(None));
    }

    /// T-5205 finding 9: the catalogue's link names its type, which narrows the read.
    #[test]
    fn a_type_comparison_is_the_views_type_selector() {
        let f = "Thing/properties/type eq 'Accident' and Thing/properties/isPartOf eq 'bbsk-x'";
        assert_eq!(
            by_id(f, "Datastreams"),
            Ok(ById {
                types: vec!["Accident".into()],
                q: Some("isPartOf==\"bbsk-x\"".into()),
                ..ById::default()
            })
        );
        assert_eq!(
            by_id("properties/type eq 'Car'", "Things").map(|b| b.types),
            Ok(vec!["Car".to_owned()])
        );
    }

    /// T-5205 finding 8: STA property names, never an Attribute of the same last segment.
    #[test]
    fn a_property_is_read_per_set_or_refused_by_name() {
        for (set, path, want) in [
            ("Things", "name", Some("name")),
            ("Things", "properties/isPartOf", Some("isPartOf")),
            ("Datastreams", "Thing/description", Some("description")),
            ("Observations", "Datastream/Thing/properties/x", Some("x")),
            ("Datastreams", "name", None),
            ("Observations", "result", None),
            ("Things", "properties/a/b", None),
            ("Things", "properties/", None),
            ("Things", "properties/type", None),
            ("Sensors", "name", None),
        ] {
            assert_eq!(
                property(set, path).ok(),
                want.map(str::to_owned),
                "{set} {path}"
            );
        }
        assert_eq!(
            property("Datastreams", "name").map_err(|b| b.1),
            Err("name is not a property Datastreams are filtered on".into())
        );
    }

    #[test]
    fn a_filter_compiles_to_q_or_names_what_it_refuses() {
        let names = |p: &str| property("Things", p);
        assert_eq!(
            filter_to_q(
                "(name eq 'Žilina' or properties/speed gt 5) and properties/x le -2",
                &names
            ),
            Ok("(name==\"Žilina\"|speed>5);x<=-2".into())
        );
        assert_eq!(
            filter_to_q(
                "properties/a ne true and properties/b ge 2026-01-01T00:00:00Z and name lt 'b'",
                &names
            ),
            Ok("a!=true;b>=2026-01-01T00:00:00Z;name<\"b\"".into())
        );
        assert_eq!(
            filter_to_q("substringof('Ži', name)", &names),
            Ok("name~=\".*Ži.*\"".into())
        );
        // T-5205 finding 10: a literal with a space, a parenthesis or a doubled quote.
        assert_eq!(
            filter_to_q("name eq 'Banská Bystrica (mesto) and ''okolie'''", &names),
            Ok("name==\"Banská Bystrica (mesto) and 'okolie'\"".into())
        );
        // T-5205 finding 11: a quote ends no string, a dot matches only a dot.
        assert_eq!(
            filter_to_q(r#"name eq 'a"b\'"#, &names),
            Ok(r#"name=="a\"b\\""#.into())
        );
        assert_eq!(
            filter_to_q("substringof('a.b*(c)', name)", &names),
            Ok(r#"name~=".*a\.b\*\(c\).*""#.into())
        );
        for refused in [
            "substringof name",
            "substringof('x')",
            "substringof(x, name)",
            "substringof('', name)",
            "substringof('x', )",
            "substringof('a\"', name)",
            "substringof('x', Datastreams/x)",
            "not name eq 1",
            "name eq 'open",
            "Datastreams/x le 2",
        ] {
            assert!(filter_to_q(refused, &names).is_err(), "{refused}");
        }
        assert_eq!(
            filter_to_q("name has 1", &names).map_err(|b| b.1),
            Err("has is not supported".into())
        );
        // T-5205 finding 23: a function is named.
        assert_eq!(
            filter_to_q("startswith(name,'B')", &names).map_err(|b| b.1),
            Err("startswith() is not supported".into())
        );
        assert!(filter_to_q("  ", &names).is_err());
    }

    /// The integrator's review of T-5205: an unquoted operand carries no `q` operator, as a
    /// value or as a property path.
    #[test]
    fn an_unquoted_operand_is_a_value_or_a_name_and_never_a_query() {
        let names = |p: &str| property("Things", p);
        for value in [
            "-2",
            "1.5e3",
            "true",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00.25Z",
            "2026-01-01T00:00:00+02:00",
            "2026-01-01T00:00:00.5-01:30",
        ] {
            let filter = format!("properties/x eq {value}");
            assert_eq!(filter_to_q(&filter, &names), Ok(format!("x=={value}")));
        }
        for injected in [
            "1|refDevice==\"x\"",
            "1;refDevice==\"x\"",
            "-1e999",
            "1-2",
            "2026-01-01",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00.5",
            "2026-01-01T00:00:00Z|a==1",
        ] {
            let filter = format!("properties/x eq {injected}");
            assert_eq!(
                filter_to_q(&filter, &names).map_err(|b| b.1),
                Err(format!("{injected} is not a number, true, false or an instant such as 2026-01-01T00:00:00Z; quote a text"))
            );
        }
        for path in [
            "properties/a|b",
            "properties/a==1",
            "properties/a.observedAt",
            "name;x",
        ] {
            assert_eq!(
                filter_to_q(&format!("{path} eq 1"), &names).map_err(|b| b.1),
                Err(format!("{path} is not a property Things are filtered on"))
            );
        }
        assert_eq!(
            filter_to_q("properties/x eq 1 or properties/größe lt 2", &names),
            Ok("x==1|größe<2".into())
        );
    }

    /// T-5205 finding 12: each operator's own edge.
    #[test]
    fn a_series_filter_is_its_exact_window() {
        assert_eq!(
            series_filter(
                "phenomenonTime ge '2026-01-01T00:00:00Z' and phenomenonTime lt 2026-02-01T00:00:00Z"
            ),
            Ok(vec![
                ("timerel", "between".into()),
                ("timeAt", "2026-01-01T00:00:00.000Z".into()),
                ("endTimeAt", "2026-02-01T00:00:00.000Z".into())
            ])
        );
        assert_eq!(
            series_filter("phenomenonTime gt 2026-01-01T00:00:00Z"),
            Ok(vec![
                ("timerel", "after".into()),
                ("timeAt", "2026-01-01T00:00:00.001Z".into())
            ])
        );
        assert_eq!(
            series_filter("phenomenonTime le 2026-01-01T00:00:00Z"),
            Ok(vec![
                ("timerel", "before".into()),
                ("timeAt", "2026-01-01T00:00:00.001Z".into())
            ])
        );
        assert_eq!(
            series_filter("phenomenonTime eq 2026-01-01T00:00:00Z"),
            Ok(vec![
                ("timerel", "between".into()),
                ("timeAt", "2026-01-01T00:00:00.000Z".into()),
                ("endTimeAt", "2026-01-01T00:00:00.001Z".into())
            ])
        );
        assert_eq!(
            series_filter("result gt 3").map_err(|b| b.1),
            Err("result: a Datastream's Observations are filtered on phenomenonTime only".into())
        );
        for refused in [
            "result eq 1 or phenomenonTime gt 2026-01-01T00:00:00Z",
            "phenomenonTime gt",
            "result eq phenomenonTime",
            "phenomenonTime ne 2026-01-01T00:00:00Z",
            "phenomenonTime gt yesterday",
            "phenomenonTime gt 2026-02-01T00:00:00Z and phenomenonTime lt 2026-01-01T00:00:00Z",
            "phenomenonTime ge 2026-01-01T00:00:00Z and phenomenonTime lt 2026-01-01T00:00:00Z",
            "1 eq 1",
        ] {
            assert!(series_filter(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_path_keeps_the_slash_inside_a_key() {
        assert_eq!(
            steps("Datastreams('urn:a/speed')/Observations/"),
            vec![
                Step {
                    set: "Datastreams".into(),
                    key: Some("urn:a/speed".into())
                },
                Step {
                    set: "Observations".into(),
                    key: None
                }
            ]
        );
        assert_eq!(
            steps("Things('urn:a''b')")
                .first()
                .and_then(|s| s.key.clone()),
            Some("urn:a'b".into())
        );
        assert_eq!(
            steps("Things(urn:a)").first().and_then(|s| s.key.clone()),
            Some("urn:a".into())
        );
        assert_eq!(steps("").len(), 0);
    }

    #[test]
    fn a_page_is_cut_between_entities() {
        let big = json!("x".repeat(PAGE_BYTES / 2));
        let (items, served) = fit(vec![
            vec![big.clone()],
            vec![big.clone(), json!(1)],
            vec![json!(2)],
        ]);
        assert_eq!((items.len(), served), (1, 1));
        let (items, served) = fit(vec![
            vec![json!("x".repeat(PAGE_BYTES * 2))],
            vec![json!(1)],
        ]);
        assert_eq!((items.len(), served), (1, 1));
        let (items, served) = fit(vec![vec![json!(1), json!(2)], vec![]]);
        assert_eq!((items.len(), served), (2, 2));
    }

    #[test]
    fn a_collection_selects_counts_and_links() {
        let c = collection(
            vec![json!({"a": 1, "b": 2}), json!(3)],
            &["a".into()],
            Some(2),
            Some("n".into()),
        );
        assert_eq!(
            c,
            json!({"@iot.count": 2, "value": [{"a": 1}, 3], "@iot.nextLink": "n"})
        );
        assert_eq!(collection(vec![], &[], None, None), json!({"value": []}));
    }
}
