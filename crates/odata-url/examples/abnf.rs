// SPDX-License-Identifier: EUPL-1.2
//! `odata_url_abnf_testcases`: the OASIS ABNF test cases (`odata-abnf-testcases.yaml` at
//! `a31c8db`, converted to JSON by `dev/odata-url-abnf.py`, which checks its digest) run
//! through [`odata_url::parse`].
//!
//! Each case is parsed as a URL: a rule inside a query option is wrapped in the URL around it
//! and the offset shifted back. The names are those of the file's `Constraints`, every member
//! valid on every type, as the ABNF's own runner checks them. A case passes when:
//!
//! - it should match: the parser accepts it, or refuses it as outside the subset
//!   (`NotImplemented`) or as a name the flat model cannot hold (`Unresolved`);
//! - it should fail at `FailAt`: a syntax error at exactly that offset, or any other refusal;
//! - its rule lies wholly outside the subset: anything but acceptance.
//!
//! A literal rule's match must be a literal of its kind, and a one-option rule's match one
//! option: `$filter=1` accepted is no `boolean`. `DuplicateOption` counts as a refusal, as the
//! ABNF does not see duplicates.
//!
//! Payload, header and service-root rules are counted as another crate's (odata-json, the
//! surface) and not run. The report gives the count per rule and outcome; the exit code is 1
//! when any case fails.
//!
//! ```text
//! cargo run -p odata-url --example abnf -- cases.json [--show <outcome prefix>]
//! ```

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::process::ExitCode;

use odata_url::{
    parse, Expr, Geography, Limits, Literal, Member, Model, ODataUri, QueryOptions, Shape,
    UrlErrorKind,
};
use serde_json::Value;

/// Every name of the file's `Constraints`, by category.
struct Flat {
    sets: HashSet<String>,
    primitive: HashSet<String>,
    primitives: HashSet<String>,
    complex: HashSet<String>,
    complexes: HashSet<String>,
    entity: HashSet<String>,
    entities: HashSet<String>,
}

impl Model for Flat {
    fn entity_set(&self, name: &str) -> Option<&str> {
        self.sets.contains(name).then_some("Any")
    }

    fn member(&self, _ty: &str, name: &str) -> Option<Member<'_>> {
        if self.primitive.contains(name) {
            Some(Member::Primitive { collection: false })
        } else if self.primitives.contains(name) {
            Some(Member::Primitive { collection: true })
        } else if self.complex.contains(name) {
            Some(Member::Complex {
                ty: "Any",
                collection: false,
            })
        } else if self.complexes.contains(name) {
            Some(Member::Complex {
                ty: "Any",
                collection: true,
            })
        } else if self.entity.contains(name) {
            Some(Member::Navigation {
                ty: "Any",
                collection: false,
            })
        } else if self.entities.contains(name) {
            Some(Member::Navigation {
                ty: "Any",
                collection: true,
            })
        } else {
            None
        }
    }
}

fn names(constraints: &Value, categories: &[&str]) -> HashSet<String> {
    categories
        .iter()
        .filter_map(|c| constraints.get(c).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Rules of another crate: JSON payload values, headers, the service root.
fn elsewhere(rule: &str) -> Option<&'static str> {
    let payload = (rule.ends_with("Value") && rule != "dateTimeOffsetValueInUrl")
        || ["context", "primitiveValue", "stringInUrl"].contains(&rule);
    if payload {
        Some("odata-json")
    } else if ["header", "preference", "prefer", "request-id"].contains(&rule)
        || rule.ends_with("Preference")
    {
        Some("headers (surface)")
    } else if rule == "odataUri" {
        Some("service root (surface)")
    } else {
        None
    }
}

/// Rules that lie wholly outside the subset (T-3441): never accepted.
const OUTSIDE: &[&str] = &[
    "deltatoken",
    "isofExpr",
    "functionParameter",
    "enumLiteral",
    "binaryLiteral",
    "durationLiteral",
    "geographyCollection",
    "geographyLineString",
    "geographyMultiLineString",
    "geographyMultiPoint",
    "geographyMultiPolygon",
    "geometryCollection",
    "geometryLineString",
    "geometryMultiLineString",
    "geometryMultiPoint",
    "geometryMultiPolygon",
    "geometryPoint",
    "geometryPolygon",
];

/// Cases the parser answers otherwise than the file, by input: whether it accepts them, and why
/// that answer is the right one.
const DEVIATIONS: &[(&str, bool, &str)] = &[
    (
        // `customQueryOption = customName [ EQ customValue ]` matches the second `more`
        "$search=more&more",
        true,
        "accepted: `&more` is a custom option",
    ),
    (
        // the file's constraints make the computed name a complex collection property of every
        // type, the flat model's too: a `$compute` name must differ from the type's names
        "$compute=@Core.Messages as _at_Core_dot_Messages&$select=_at_Core_dot_Messages($top=5)",
        false,
        "refused: the computed name is a declared property",
    ),
];

/// The URL around a case of `rule`.
fn prefix(rule: &str) -> &'static str {
    match rule {
        "odataRelativeUri" | "resourcePath" | "entitySetName" | "odataIdentifier" => "",
        "queryOptions" | "systemQueryOption" | "customQueryOption" | "filter" | "select"
        | "expand" | "orderby" | "orderBy" | "skiptoken" | "deltatoken" | "compute" | "search" => {
            "Customers?"
        }
        "anyExpr" => "Customers?$filter=Items/",
        "searchExpr" => "Customers?$search=",
        _ => "Customers?$filter=",
    }
}

/// The literal kind a literal rule names; `None` for a rule that is no literal rule.
fn literal_rule(rule: &str) -> Option<fn(&Literal) -> bool> {
    Some(match rule {
        "date" => |l| matches!(l, Literal::Date(_)),
        "boolean" => |l| matches!(l, Literal::Bool(_)),
        "dateTimeOffsetLiteral" | "dateTimeOffsetValueInUrl" => {
            |l| matches!(l, Literal::DateTimeOffset(_))
        }
        "timeOfDayLiteral" => |l| matches!(l, Literal::TimeOfDay(_)),
        "guid" => |l| matches!(l, Literal::Guid(_)),
        "stringLiteral" => |l| matches!(l, Literal::String(_)),
        "decimalLiteral" | "doubleLiteral" | "singleLiteral" => {
            |l| matches!(l, Literal::Int(_) | Literal::Decimal(_))
        }
        "int16Literal" | "int32Literal" | "int64Literal" | "sbyteLiteral" => {
            |l| matches!(l, Literal::Int(_))
        }
        "null" => |l| matches!(l, Literal::Null),
        "geographyPoint" => |l| {
            matches!(
                l,
                Literal::Geography(Geography {
                    shape: Shape::Point(_),
                    ..
                })
            )
        },
        "geographyPolygon" => |l| {
            matches!(
                l,
                Literal::Geography(Geography {
                    shape: Shape::Polygon(_),
                    ..
                })
            )
        },
        "primitiveLiteral" => |_| true,
        r if OUTSIDE.contains(&r)
            && !r.ends_with("Expr")
            && !["deltatoken", "functionParameter"].contains(&r) =>
        {
            |_| false
        }
        _ => return None,
    })
}

/// Rules of one query option: a match is one option, not several.
const ONE_OPTION: &[&str] = &[
    "systemQueryOption",
    "customQueryOption",
    "filter",
    "select",
    "expand",
    "orderby",
    "orderBy",
    "skiptoken",
    "search",
    "compute",
];

fn option_count(o: &QueryOptions) -> usize {
    let system = [
        o.filter.is_some(),
        o.search.is_some(),
        o.compute.is_some(),
        o.select.is_some(),
        o.expand.is_some(),
        o.orderby.is_some(),
        o.top.is_some(),
        o.skip.is_some(),
        o.count.is_some(),
        o.format.is_some(),
        o.skiptoken.is_some(),
        o.schemaversion.is_some(),
        o.id.is_some(),
    ];
    system
        .iter()
        .filter(|set| **set)
        .count()
        .saturating_add(o.aliases.len())
        .saturating_add(o.custom.len())
}

/// Whether an accepted URL is a match of `rule` itself, not of another rule around it.
fn matches_rule(rule: &str, uri: &ODataUri) -> bool {
    let options = match uri {
        ODataUri::Resource { options, .. } => Some(options),
        _ => None,
    };
    if let Some(kind) = literal_rule(rule) {
        return options
            .and_then(|o| o.filter.as_ref())
            .is_some_and(|f| match f {
                Expr::Literal(l) => kind(l),
                _ => false,
            });
    }
    if ONE_OPTION.contains(&rule) {
        return options.is_some_and(|o| option_count(o) == 1);
    }
    true
}

/// `pass` or `fail`, and the outcome's name.
fn judge(case: &Value, model: &Flat, limits: &Limits) -> Option<(bool, String)> {
    let rule = case.get("Rule")?.as_str()?;
    let input = case.get("Input")?.as_str()?;
    if let Some((_, accepted, why)) = DEVIATIONS.iter().find(|(i, _, _)| *i == input) {
        let parsed = parse(&format!("{}{input}", prefix(rule)), model, limits);
        if parsed.is_ok() == *accepted {
            return Some((true, format!("deviation, {why}")));
        }
    }
    // The YAML is read without type inference, so `FailAt` may arrive as a string.
    let fail_at = case.get("FailAt").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    let outside = OUTSIDE.contains(&rule);
    let wrap = prefix(rule);
    Some(match parse(&format!("{wrap}{input}"), model, limits) {
        Ok(uri) => match (matches_rule(rule, &uri), fail_at, outside) {
            (true, None, false) => (true, "accepted".into()),
            (true, _, true) => (false, "accepted, rule outside the subset".into()),
            (true, Some(_), false) => (false, "accepted, should fail".into()),
            (false, Some(_), _) | (false, None, true) => (true, "another rule's match".into()),
            (false, None, false) => (false, "accepted as another rule".into()),
        },
        Err(e) => {
            let at = u64::try_from(e.offset.checked_sub(wrap.len())?).ok()?;
            match (e.kind, fail_at) {
                (_, _) if outside => (true, "refused, outside the subset".into()),
                (UrlErrorKind::NotImplemented(_), _) => (true, "not implemented".into()),
                (UrlErrorKind::Unresolved, _) => (true, "unresolved".into()),
                (UrlErrorKind::DuplicateOption(_), _) => (true, "refused: duplicate option".into()),
                (UrlErrorKind::Syntax { .. }, Some(want)) if want == at => {
                    (true, "fails at FailAt".into())
                }
                (UrlErrorKind::Syntax { expected }, Some(want)) => (
                    false,
                    format!("fails at {at} (wants {want}), expected {expected}"),
                ),
                (kind, Some(_)) => (true, format!("refused: {kind:?}")),
                (kind, None) => (false, format!("fails at {at}: {kind:?}")),
            }
        }
    })
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        return report("usage: abnf <cases.json>\n", false);
    };
    let Ok(data) = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()))
    else {
        return report(&format!("cannot read {path}\n"), false);
    };
    let constraints = data.get("Constraints").cloned().unwrap_or_default();
    let model = Flat {
        sets: names(&constraints, &["entitySetName", "singletonEntity"]),
        // A stream property is no member of the subset's models: `Unresolved`.
        primitive: names(
            &constraints,
            &["primitiveKeyProperty", "primitiveNonKeyProperty"],
        ),
        primitives: names(&constraints, &["primitiveColProperty"]),
        complex: names(&constraints, &["complexProperty"]),
        complexes: names(&constraints, &["complexColProperty"]),
        entity: names(&constraints, &["entityNavigationProperty"]),
        entities: names(&constraints, &["entityColNavigationProperty"]),
    };
    // `$levels=4` in the cases: deeper than the strict default allows.
    let limits = Limits {
        max_expand_depth: 8,
        ..Limits::strict()
    };
    let cases = data
        .get("TestCases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut shown = Vec::new();
    // `--show <outcome prefix>`: list the passing cases of that outcome too.
    let show = std::env::args()
        .nth(3)
        .filter(|_| std::env::args().nth(2).as_deref() == Some("--show"));
    for case in &cases {
        let rule = case.get("Rule").and_then(Value::as_str).unwrap_or("?");
        let input = case
            .get("Input")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // A fragment never reaches a server: these are context URLs, odata-json's.
        let owner =
            elsewhere(rule).or_else(|| input.contains('#').then_some("odata-json (context URL)"));
        let (pass, outcome) = match owner {
            Some(owner) => (true, format!("not run: {owner}")),
            None => judge(case, &model, &limits).unwrap_or((false, "malformed case".into())),
        };
        if !pass
            || show
                .as_deref()
                .is_some_and(|wanted| outcome.starts_with(wanted))
        {
            let name = case.get("Name").and_then(Value::as_str).unwrap_or("?");
            let line = format!(
                "{} {rule} {name:?} {input:?}: {outcome}",
                if pass { "SHOW" } else { "FAIL" }
            );
            if pass {
                shown.push(line);
            } else {
                failures.push(line);
            }
        }
        let entry = counts
            .entry((rule.to_owned(), if pass { outcome } else { "FAIL".into() }))
            .or_default();
        *entry = entry.saturating_add(1);
    }
    let mut lines: Vec<String> = counts
        .iter()
        .map(|((rule, outcome), n)| format!("{rule:28} {outcome:40} {n}"))
        .collect();
    lines.extend(shown.iter().chain(&failures).cloned());
    lines.push(format!(
        "{} cases, {} failed\n",
        cases.len(),
        failures.len()
    ));
    let text = lines.join("\n");
    report(&text, failures.is_empty() && !cases.is_empty())
}

fn report(text: &str, ok: bool) -> ExitCode {
    let written = std::io::stdout().lock().write_all(text.as_bytes());
    if ok && written.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
