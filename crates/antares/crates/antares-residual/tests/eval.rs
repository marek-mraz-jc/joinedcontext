// SPDX-License-Identifier: EUPL-1.2
//! `antares-residual` over a normalized NGSI-LD Entity: every path, operator and function, each
//! with its unknown and mistyped rows (ADR-0028 rules 2 and 7).

use ngsild_residual::{
    compile, eval, Cmp, Date, DateTime, Duration, Error, Expr, Func, Lit, Member, Path, Program,
    Time, Truth, MAX_DEPTH, MAX_NODES, MAX_STRING_BYTES,
};
use serde_json::{json, Value};

const NOW: &str = "2026-10-11T15:00:00Z";

fn now() -> DateTime {
    DateTime::parse(NOW).unwrap_or(DateTime::MIN)
}

fn entity() -> Value {
    json!({
        "id": "urn:ngsi-ld:Station:1",
        "type": "Station",
        "createdAt": "2026-01-01T00:00:00Z",
        "modifiedAt": "2026-02-01T00:00:00Z",
        "name": {
            "type": "Property", "value": "Ångström Station",
            "observedAt": "2026-10-11T10:00:00Z", "unitCode": "C62",
            "createdAt": "2026-03-01T00:00:00Z", "modifiedAt": "2026-04-01T00:00:00Z",
            "accuracy": {"type": "Property", "value": 0.5}
        },
        "padded": {"type": "Property", "value": "  x  "},
        "pm10": {"type": "Property", "value": 12},
        "ratio": {"type": "Property", "value": 2.5},
        "huge": {"type": "Property", "value": 1e308},
        "flag": {"type": "Property", "value": true},
        "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:1"},
        "label": {"type": "LanguageProperty", "languageMap": {"en": "north"}},
        "tags": {"type": "ListProperty", "valueList": ["a", "b", 3]},
        "none": {"type": "ListProperty", "valueList": []},
        "links": {"type": "ListRelationship", "objectList": ["urn:ngsi-ld:Person:2"]},
        "kind": {"type": "VocabProperty", "vocab": "Car"},
        "doc": {"type": "JsonProperty", "json": {"a": {"b": 7}}},
        "address": {"type": "Property", "value": {"city": "Bonn", "rooms": [{"n": 1}, {"n": 5}]}},
        "when": {"type": "Property", "value": {"@type": "DateTime", "@value": "2026-10-11T12:30:15.25+02:00"}},
        "day": {"type": "Property", "value": {"@type": "Date", "@value": "2026-10-11"}},
        "at": {"type": "Property", "value": {"@type": "Time", "@value": "08:15:00"}},
        "broken": {"type": "Property", "value": {"@type": "DateTime", "@value": "soon"}},
        "count": {"type": "Property", "value": {"@type": "xsd:integer", "@value": 4}},
        "plain": {"type": "Property", "value": "2026-10-11T10:00:00Z"},
        "speed": [
            {"type": "Property", "value": 10, "datasetId": "urn:ngsi-ld:Dataset:a"},
            {"type": "Property", "value": 20}
        ],
        "only": [{"type": "Property", "value": 10, "datasetId": "urn:ngsi-ld:Dataset:a"}],
        "nothing": {"type": "Property", "value": null},
        "bare": 5
    })
}

fn program(e: &Expr) -> Program {
    compile(e, now()).unwrap_or_else(|err| unreachable!("{e:?}: {err}"))
}

fn on(e: &Expr, entity: &Value) -> Truth {
    eval(&program(e), entity).unwrap_or_else(|err| unreachable!("{e:?}: {err}"))
}

fn truth(e: &Expr) -> Truth {
    on(e, &entity())
}

fn refused(e: &Expr) -> Error {
    compile(e, now())
        .err()
        .unwrap_or_else(|| unreachable!("{e:?} compiled"))
}

fn p(attr: &str) -> Expr {
    Expr::Path(Path::value(attr))
}

fn at(attrs: &[&str], member: Member, keys: &[&str]) -> Expr {
    Expr::Path(Path::Attribute {
        attrs: attrs.iter().map(|s| (*s).to_owned()).collect(),
        member,
        keys: keys.iter().map(|s| (*s).to_owned()).collect(),
    })
}

fn s(v: &str) -> Expr {
    Expr::Lit(Lit::Str(v.into()))
}

fn i(v: i64) -> Expr {
    Expr::Lit(Lit::Int(v))
}

fn n(v: f64) -> Expr {
    Expr::Lit(Lit::Num(v))
}

fn b(v: bool) -> Expr {
    Expr::Lit(Lit::Bool(v))
}

fn dt(v: &str) -> Expr {
    Expr::Lit(Lit::DateTime(DateTime::parse(v).unwrap_or(DateTime::MIN)))
}

fn date(v: &str) -> Expr {
    Expr::Lit(Lit::Date(
        Date::parse(v).unwrap_or_else(|| unreachable!("{v}")),
    ))
}

fn time(v: &str) -> Expr {
    Expr::Lit(Lit::Time(
        Time::parse(v).unwrap_or_else(|| unreachable!("{v}")),
    ))
}

fn cmp(op: Cmp, a: Expr, b: Expr) -> Expr {
    Expr::Cmp(op, Box::new(a), Box::new(b))
}

fn eq(a: Expr, b: Expr) -> Expr {
    cmp(Cmp::Eq, a, b)
}

fn call(f: Func, args: Vec<Expr>) -> Expr {
    Expr::Call(f, args)
}

fn var(up: usize, keys: &[&str]) -> Expr {
    Expr::Var {
        up,
        keys: keys.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn not(e: Expr) -> Expr {
    Expr::Not(Box::new(e))
}

fn is_null(e: Expr) -> Expr {
    Expr::IsNull(Box::new(e))
}

fn any(list: Expr, body: Option<Expr>) -> Expr {
    Expr::Any(Box::new(list), body.map(Box::new))
}

fn all(list: Expr, body: Expr) -> Expr {
    Expr::All(Box::new(list), Box::new(body))
}

use Truth::{False as F, True as T, Unknown as U};

fn check(rows: &[(Expr, Truth)]) {
    let e = entity();
    for (expr, want) in rows {
        assert_eq!(on(expr, &e), *want, "{expr:?}");
    }
}

#[test]
fn comparison_promotes_numbers_and_reads_text_as_dates() {
    check(&[
        (eq(p("pm10"), i(12)), T),
        (cmp(Cmp::Ne, p("pm10"), i(12)), F),
        (cmp(Cmp::Lt, p("pm10"), i(13)), T),
        (cmp(Cmp::Le, p("pm10"), i(12)), T),
        (cmp(Cmp::Gt, p("pm10"), i(12)), F),
        (cmp(Cmp::Ge, p("pm10"), i(12)), T),
        (eq(p("pm10"), n(12.0)), T),
        (cmp(Cmp::Gt, p("ratio"), i(2)), T),
        (cmp(Cmp::Lt, n(f64::NAN), p("ratio")), U),
        (cmp(Cmp::Lt, s("a"), s("b")), T),
        (eq(p("flag"), b(true)), T),
        (cmp(Cmp::Ne, b(false), p("flag")), T),
        (cmp(Cmp::Lt, p("flag"), b(true)), U),
        (cmp(Cmp::Gt, b(true), p("flag")), U),
        (eq(p("pm10"), s("12")), U),
        (eq(p("missing"), i(1)), U),
        (eq(p("nothing"), i(1)), U),
        (eq(p("address"), i(1)), U),
        (eq(p("when"), dt("2026-10-11T10:30:15.25Z")), T),
        (eq(p("plain"), dt("2026-10-11T12:00:00+02:00")), T),
        (cmp(Cmp::Lt, dt("2026-10-11T09:00:00Z"), p("plain")), T),
        (eq(p("when"), i(1)), U),
        (eq(p("day"), date("2026-10-11")), T),
        (eq(p("day"), s("2026-10-11")), T),
        (cmp(Cmp::Gt, s("2026-10-12"), p("day")), T),
        (eq(p("day"), i(1)), U),
        (cmp(Cmp::Lt, p("at"), time("09:00")), T),
        (cmp(Cmp::Gt, s("09:00"), p("at")), T),
        (eq(p("at"), i(8)), U),
        (eq(p("count"), i(4)), T),
        (is_null(p("broken")), T),
    ]);
}

/// A date and a date-time compare by the date-time's UTC day: a date is the whole day it names.
#[test]
fn a_date_and_a_date_time_compare_by_utc_day() {
    check(&[
        // `when` is 2026-10-11T10:30:15.25Z
        (eq(p("when"), date("2026-10-11")), T),
        (cmp(Cmp::Gt, p("when"), date("2026-10-10")), T),
        (cmp(Cmp::Lt, p("when"), date("2026-10-11")), F),
        (cmp(Cmp::Le, p("day"), dt("2026-10-11T00:00:00Z")), T),
        (cmp(Cmp::Ge, p("day"), dt("2026-10-11T23:59:59Z")), T),
        (cmp(Cmp::Ge, p("day"), dt("2026-10-12T00:30:00+01:00")), T),
        (cmp(Cmp::Ge, p("day"), dt("2026-10-12T00:00:00Z")), F),
        (eq(s("2026-10-11T23:30:00-01:00"), date("2026-10-12")), T),
        (eq(p("broken"), date("2026-10-11")), U),
    ]);
}

#[test]
fn logic_is_three_valued_and_not_is_exact() {
    let unknown = || eq(p("missing"), i(1));
    check(&[
        (Expr::And(vec![]), T),
        (Expr::Or(vec![]), F),
        (Expr::And(vec![b(true), unknown()]), U),
        (Expr::And(vec![b(false), unknown()]), F),
        (Expr::And(vec![unknown(), b(false)]), F),
        (Expr::Or(vec![b(true), unknown()]), T),
        (Expr::Or(vec![unknown(), b(true)]), T),
        (Expr::Or(vec![b(false), unknown()]), U),
        (not(unknown()), U),
        (not(eq(p("pm10"), i(12))), F),
        (not(not(eq(p("pm10"), i(12)))), T),
        (is_null(p("missing")), T),
        (is_null(p("nothing")), T),
        (is_null(p("pm10")), F),
        (is_null(unknown()), T),
        (p("flag"), T),
        (p("pm10"), U),
    ]);
}

#[test]
fn in_lists_match_or_stay_unknown() {
    let isin = |e: Expr, lits: Vec<Lit>| Expr::In(Box::new(e), lits);
    check(&[
        (isin(p("pm10"), vec![Lit::Int(1), Lit::Int(12)]), T),
        (isin(p("pm10"), vec![Lit::Int(1), Lit::Int(2)]), F),
        (isin(p("pm10"), vec![Lit::Str("x".into()), Lit::Int(1)]), U),
        (isin(p("pm10"), vec![Lit::Str("x".into()), Lit::Int(12)]), T),
        (isin(p("pm10"), vec![]), F),
        (isin(p("missing"), vec![Lit::Int(1)]), U),
    ]);
}

#[test]
fn lambdas_over_lists_nest_and_read_object_members() {
    let tags = || p("tags");
    check(&[
        (any(tags(), Some(eq(var(0, &[]), s("b")))), T),
        (any(tags(), Some(eq(var(0, &[]), s("z")))), U),
        (any(tags(), Some(cmp(Cmp::Gt, var(0, &[]), i(5)))), U),
        (any(tags(), Some(eq(var(0, &[]), i(3)))), T),
        (any(tags(), None), T),
        (any(p("none"), None), F),
        (any(p("none"), Some(b(true))), F),
        (all(tags(), cmp(Cmp::Ne, var(0, &[]), s("z"))), U),
        (all(tags(), eq(var(0, &[]), s("a"))), F),
        (all(p("none"), b(false)), T),
        (
            all(
                at(&["address"], Member::Value, &["rooms"]),
                cmp(Cmp::Gt, var(0, &["n"]), i(0)),
            ),
            T,
        ),
        (
            any(
                at(&["address"], Member::Value, &["rooms"]),
                Some(cmp(Cmp::Gt, var(0, &["n"]), i(4))),
            ),
            T,
        ),
        (
            any(
                at(&["address"], Member::Value, &["rooms"]),
                Some(eq(var(0, &["missing"]), i(4))),
            ),
            U,
        ),
        (
            any(
                at(&["address"], Member::Value, &["rooms"]),
                Some(any(
                    tags(),
                    Some(Expr::And(vec![
                        eq(var(1, &["n"]), i(5)),
                        eq(var(0, &[]), s("a")),
                    ])),
                )),
            ),
            T,
        ),
        (any(p("pm10"), None), U),
        (any(p("missing"), Some(b(true))), U),
        (all(p("pm10"), b(true)), U),
        (
            any(p("links"), Some(eq(var(0, &[]), s("urn:ngsi-ld:Person:2")))),
            T,
        ),
    ]);
    assert!(matches!(refused(&eq(var(0, &[]), i(1))), Error::Invalid(_)));
    assert!(matches!(
        refused(&any(p("tags"), Some(eq(var(1, &[]), i(1))))),
        Error::Invalid(_)
    ));
}

#[test]
fn paths_reach_every_member_kind_and_the_default_instance() {
    let typed = json!({"id": "urn:ngsi-ld:X:1", "type": ["A", "B"]});
    assert_eq!(
        on(
            &any(Expr::Path(Path::Type), Some(eq(var(0, &[]), s("B")))),
            &typed
        ),
        T
    );
    assert_eq!(on(&eq(Expr::Path(Path::Type), s("A")), &typed), U);
    assert_eq!(on(&is_null(Expr::Path(Path::CreatedAt)), &typed), T);
    check(&[
        (eq(Expr::Path(Path::Id), s("urn:ngsi-ld:Station:1")), T),
        (eq(Expr::Path(Path::Type), s("Station")), T),
        (
            cmp(
                Cmp::Lt,
                Expr::Path(Path::CreatedAt),
                Expr::Path(Path::ModifiedAt),
            ),
            T,
        ),
        (
            eq(
                at(&["name"], Member::ObservedAt, &[]),
                dt("2026-10-11T10:00:00Z"),
            ),
            T,
        ),
        (eq(at(&["name"], Member::UnitCode, &[]), s("C62")), T),
        (
            eq(
                at(&["name"], Member::CreatedAt, &[]),
                dt("2026-03-01T00:00:00Z"),
            ),
            T,
        ),
        (
            eq(
                at(&["name"], Member::ModifiedAt, &[]),
                dt("2026-04-01T00:00:00Z"),
            ),
            T,
        ),
        (is_null(at(&["pm10"], Member::ObservedAt, &[])), T),
        (eq(at(&["name", "accuracy"], Member::Value, &[]), n(0.5)), T),
        (is_null(at(&["name", "missing"], Member::Value, &[])), T),
        (eq(p("owner"), s("urn:ngsi-ld:Person:1")), T),
        (eq(at(&["label"], Member::Value, &["en"]), s("north")), T),
        (eq(p("kind"), s("Car")), T),
        (eq(at(&["doc"], Member::Value, &["a", "b"]), i(7)), T),
        (is_null(at(&["doc"], Member::Value, &["a", "c"])), T),
        (eq(p("speed"), i(20)), T),
        (is_null(p("only")), T),
        (is_null(p("bare")), T),
    ]);
    assert!(matches!(
        refused(&at(&[], Member::Value, &[])),
        Error::Invalid(_)
    ));
}

#[test]
fn string_functions() {
    let name = || p("name");
    check(&[
        (call(Func::Contains, vec![name(), s("Station")]), T),
        (call(Func::StartsWith, vec![name(), s("Å")]), T),
        (call(Func::EndsWith, vec![name(), s("North")]), F),
        (call(Func::Contains, vec![p("pm10"), s("1")]), U),
        (call(Func::Contains, vec![name(), p("missing")]), U),
        (eq(call(Func::Length, vec![name()]), i(16)), T),
        (eq(call(Func::IndexOf, vec![name(), s("Station")]), i(9)), T),
        (eq(call(Func::IndexOf, vec![name(), s("zzz")]), i(-1)), T),
        (
            eq(call(Func::Substring, vec![name(), i(9)]), s("Station")),
            T,
        ),
        (
            eq(call(Func::Substring, vec![name(), i(0), i(3)]), s("Ång")),
            T,
        ),
        (eq(call(Func::Substring, vec![name(), i(99)]), s("")), T),
        (is_null(call(Func::Substring, vec![name(), i(-1)])), T),
        (is_null(call(Func::Substring, vec![name(), i(0), i(-1)])), T),
        (
            is_null(call(Func::Substring, vec![name(), i(0), s("3")])),
            T,
        ),
        (is_null(call(Func::Substring, vec![name(), s("0")])), T),
        (
            eq(call(Func::ToLower, vec![name()]), s("ångström station")),
            T,
        ),
        (
            eq(call(Func::ToUpper, vec![name()]), s("ÅNGSTRÖM STATION")),
            T,
        ),
        (eq(call(Func::Trim, vec![p("padded")]), s("x")), T),
        (
            eq(call(Func::Concat, vec![s("a"), p("kind")]), s("aCar")),
            T,
        ),
        (
            eq(
                call(Func::CaseI, vec![name()]),
                call(Func::CaseI, vec![s("ÅNGSTRÖM station")]),
            ),
            T,
        ),
        (
            eq(call(Func::AccentI, vec![name()]), s("Angstrom Station")),
            T,
        ),
        (
            eq(
                call(Func::CaseI, vec![call(Func::AccentI, vec![name()])]),
                call(
                    Func::AccentI,
                    vec![call(Func::CaseI, vec![s("ANGSTRÖM STATION")])],
                ),
            ),
            T,
        ),
    ]);
}

#[test]
fn strings_past_the_bound_are_unknown() {
    let half = "x".repeat(MAX_STRING_BYTES / 2 + 1);
    let whole = "ΐ".repeat(MAX_STRING_BYTES / 4);
    let big = json!({"id": "urn:ngsi-ld:X:1", "type": "X",
        "half": {"type": "Property", "value": half}, "whole": {"type": "Property", "value": whole}});
    assert_eq!(
        on(
            &is_null(call(Func::Concat, vec![p("half"), p("half")])),
            &big
        ),
        T
    );
    assert_eq!(
        on(&is_null(call(Func::Concat, vec![p("half"), s("")])), &big),
        F
    );
    assert_eq!(
        on(&is_null(call(Func::ToUpper, vec![p("whole")])), &big),
        T,
        "ΐ upper-cases to three characters"
    );
}

#[test]
fn patterns_match_like_their_standards() {
    let name = || p("name");
    check(&[
        (call(Func::MatchesPattern, vec![name(), s(r"^\w")]), F),
        (call(Func::MatchesPattern, vec![name(), s("^Å.*n$")]), T),
        (call(Func::MatchesPattern, vec![p("pm10"), s("1")]), U),
        (call(Func::Like, vec![name(), s("%Station")]), T),
        (call(Func::Like, vec![name(), s("Station")]), F),
        (
            call(
                Func::Like,
                vec![
                    call(Func::CaseI, vec![name()]),
                    call(Func::CaseI, vec![s("%STATION")]),
                ],
            ),
            T,
        ),
        (
            call(
                Func::Like,
                vec![
                    call(Func::AccentI, vec![name()]),
                    call(Func::AccentI, vec![s("Ångs%")]),
                ],
            ),
            T,
        ),
    ]);
    assert!(matches!(
        refused(&call(Func::MatchesPattern, vec![name(), s("a(?=b)")])),
        Error::Unsupported(_)
    ));
    assert!(matches!(
        refused(&call(Func::Like, vec![name(), p("kind")])),
        Error::Unsupported(_)
    ));
    assert!(matches!(
        refused(&call(Func::Like, vec![name(), s("50\\")])),
        Error::Invalid(_)
    ));
    assert_eq!(
        refused(&call(
            Func::MatchesPattern,
            vec![name(), s("(?:[a-z]{100}){100}")]
        )),
        Error::TooComplex
    );
}

#[test]
fn date_and_time_functions_read_the_written_offset() {
    let when = || p("when");
    check(&[
        (eq(call(Func::Year, vec![when()]), i(2026)), T),
        (eq(call(Func::Month, vec![when()]), i(10)), T),
        (eq(call(Func::Day, vec![when()]), i(11)), T),
        (eq(call(Func::Hour, vec![when()]), i(12)), T),
        (eq(call(Func::Minute, vec![when()]), i(30)), T),
        (eq(call(Func::Second, vec![when()]), i(15)), T),
        (eq(call(Func::FractionalSeconds, vec![when()]), n(0.25)), T),
        (eq(call(Func::TotalOffsetMinutes, vec![when()]), i(120)), T),
        (eq(call(Func::Date, vec![when()]), date("2026-10-11")), T),
        (eq(call(Func::Time, vec![when()]), time("12:30:15.25")), T),
        (eq(call(Func::Year, vec![p("day")]), i(2026)), T),
        (eq(call(Func::Hour, vec![p("at")]), i(8)), T),
        (eq(call(Func::Hour, vec![p("plain")]), i(10)), T),
        (eq(call(Func::Month, vec![s("2026-03-04")]), i(3)), T),
        (eq(call(Func::Minute, vec![s("08:15")]), i(15)), T),
        (is_null(call(Func::Year, vec![p("pm10")])), T),
        (is_null(call(Func::Year, vec![p("missing")])), T),
        (is_null(call(Func::Hour, vec![p("day")])), T),
        (is_null(call(Func::TotalOffsetMinutes, vec![p("day")])), T),
        (is_null(call(Func::Date, vec![p("pm10")])), T),
        (is_null(call(Func::Time, vec![p("day")])), T),
        (is_null(call(Func::FractionalSeconds, vec![p("pm10")])), T),
        (eq(call(Func::Now, vec![]), dt(NOW)), T),
        (
            cmp(
                Cmp::Lt,
                call(Func::MinDateTime, vec![]),
                call(Func::Now, vec![]),
            ),
            T,
        ),
        (
            cmp(
                Cmp::Gt,
                call(Func::MaxDateTime, vec![]),
                call(Func::Now, vec![]),
            ),
            T,
        ),
    ]);
}

#[test]
fn arithmetic_is_checked_and_rounding_keeps_integers() {
    let f2 = |f: Func, a: Expr, b: Expr| call(f, vec![a, b]);
    check(&[
        (eq(call(Func::Round, vec![n(2.5)]), n(3.0)), T),
        (eq(call(Func::Round, vec![n(-2.5)]), n(-3.0)), T),
        (eq(call(Func::Floor, vec![p("ratio")]), n(2.0)), T),
        (eq(call(Func::Ceiling, vec![p("ratio")]), i(3)), T),
        (eq(call(Func::Round, vec![p("pm10")]), i(12)), T),
        (is_null(call(Func::Floor, vec![s("x")])), T),
        (eq(f2(Func::Add, p("pm10"), i(1)), i(13)), T),
        (is_null(f2(Func::Add, i(i64::MAX), i(1))), T),
        (eq(f2(Func::Sub, p("pm10"), n(0.5)), n(11.5)), T),
        (is_null(f2(Func::Sub, i(i64::MIN), i(1))), T),
        (eq(f2(Func::Mul, p("pm10"), i(2)), i(24)), T),
        (is_null(f2(Func::Mul, p("huge"), n(10.0))), T),
        (eq(f2(Func::Div, i(7), i(2)), i(3)), T),
        (eq(f2(Func::Div, i(-7), i(2)), i(-3)), T),
        (eq(f2(Func::Div, n(7.0), i(2)), n(3.5)), T),
        (is_null(f2(Func::Div, i(1), i(0))), T),
        (is_null(f2(Func::Div, n(1.0), i(0))), T),
        (is_null(f2(Func::Div, i(i64::MIN), i(-1))), T),
        (eq(f2(Func::DivBy, i(7), i(2)), n(3.5)), T),
        (is_null(f2(Func::DivBy, i(0), i(0))), T),
        (eq(f2(Func::Mod, i(7), i(3)), i(1)), T),
        (eq(f2(Func::Mod, i(-7), i(3)), i(-1)), T),
        (is_null(f2(Func::Mod, i(7), i(0))), T),
        (eq(f2(Func::Mod, n(7.5), i(2)), n(1.5)), T),
        (eq(f2(Func::Pow, p("pm10"), i(2)), i(144)), T),
        (eq(f2(Func::Pow, i(2), i(-1)), n(0.5)), T),
        (eq(f2(Func::Pow, n(2.0), n(0.5)), n(2.0_f64.sqrt())), T),
        // exact on every platform, Miri included: a division and a square root
        (eq(f2(Func::Pow, i(2), i(-2)), n(0.25)), T),
        (eq(f2(Func::Pow, n(4.0), n(0.5)), n(2.0)), T),
        (is_null(f2(Func::Pow, i(0), i(-1))), T),
        // past i64 the integer route has no answer, and powf still gives one
        (cmp(Cmp::Gt, f2(Func::Pow, i(10), i(-30)), n(0.0)), T),
        (is_null(f2(Func::Pow, i(i64::MAX), i(2))), T),
        (is_null(f2(Func::Pow, i(-8), n(0.5))), T),
        (is_null(f2(Func::Pow, s("x"), i(2))), T),
        (eq(call(Func::Neg, vec![p("pm10")]), i(-12)), T),
        (eq(call(Func::Neg, vec![p("ratio")]), n(-2.5)), T),
        (is_null(call(Func::Neg, vec![i(i64::MIN)])), T),
        (is_null(call(Func::Neg, vec![s("x")])), T),
        (is_null(f2(Func::Add, s("x"), i(1))), T),
        (is_null(f2(Func::Add, i(1), p("missing"))), T),
    ]);
}

#[test]
fn wrong_argument_counts_are_refused() {
    for e in [
        call(Func::Contains, vec![s("a")]),
        call(Func::Substring, vec![s("a"), i(1), i(1), i(1)]),
        call(Func::Now, vec![i(1)]),
    ] {
        assert!(matches!(refused(&e), Error::Invalid(_)), "{e:?}");
    }
}

#[test]
fn bounds_refuse_deep_and_wide() {
    let deep = |levels: usize| (0..levels).fold(b(true), |e, _| not(e));
    assert_eq!(truth(&deep(MAX_DEPTH - 1)), F, "63 negations of true");
    assert_eq!(refused(&deep(MAX_DEPTH)), Error::TooComplex);
    assert_eq!(
        refused(&deep(1_000)),
        Error::TooComplex,
        "refused before it recurses that far"
    );

    let wide = |members: usize| Expr::And(vec![b(true); members]);
    assert_eq!(truth(&wide(MAX_NODES - 1)), T);
    assert_eq!(refused(&wide(MAX_NODES)), Error::TooComplex);
    let list = Expr::In(Box::new(p("pm10")), vec![Lit::Int(0); MAX_NODES - 1]);
    assert_eq!(refused(&list), Error::TooComplex);
    assert_eq!(refused(&wide(10 * MAX_NODES)), Error::TooComplex);
}

#[test]
fn nested_lambdas_over_long_lists_hit_the_step_budget() {
    let long = json!({"id": "urn:ngsi-ld:X:1", "type": "X",
        "l": {"type": "ListProperty", "valueList": vec![0; 1100]}});
    let never = || eq(var(0, &[]), i(-1));
    let one = any(p("l"), Some(never()));
    assert_eq!(on(&one, &long), F);
    let nested = any(p("l"), Some(any(p("l"), Some(never()))));
    assert_eq!(eval(&program(&nested), &long), Err(Error::TooComplex));
    assert_eq!(program(&one).eval(&long), Ok(F));
}

#[test]
fn a_program_names_the_attributes_and_system_members_it_reads() -> Result<(), Error> {
    use ngsild_residual::Spatial;
    let at = |attrs: &[&str], member| Path::Attribute {
        attrs: attrs.iter().map(|a| (*a).to_owned()).collect(),
        member,
        keys: Vec::new(),
    };
    let e = Expr::And(vec![
        Expr::IsNull(Box::new(Expr::Path(Path::value("speed")))),
        Expr::Any(Box::new(Expr::Path(Path::value("speed"))), None),
        Expr::IsNull(Box::new(Expr::Path(Path::Id))),
        Expr::Any(
            Box::new(Expr::Path(Path::value("tags"))),
            Some(Box::new(Expr::Cmp(
                Cmp::Eq,
                Box::new(Expr::Var {
                    up: 0,
                    keys: Vec::new(),
                }),
                Box::new(Expr::Path(at(&["speed", "accuracy"], Member::Value))),
            ))),
        ),
        Expr::Spatial(
            Spatial::Within,
            Box::new(Expr::Path(Path::value("location"))),
            Box::new(Expr::Lit(Lit::Geometry(
                serde_json::json!({"type": "Point", "coordinates": [1, 1]}),
            ))),
        ),
        Expr::All(
            Box::new(Expr::Path(Path::value("links"))),
            Box::new(Expr::IsNull(Box::new(Expr::Path(Path::value("tags"))))),
        ),
    ]);
    let program = compile(&e, DateTime::MIN)?;
    assert_eq!(program.attributes(), ["speed", "tags", "location", "links"]);
    assert!(!program.reads_system_members());
    for system in [
        Expr::Path(Path::ModifiedAt),
        Expr::Path(Path::CreatedAt),
        Expr::Path(at(&["speed"], Member::CreatedAt)),
        Expr::Not(Box::new(Expr::Path(at(
            &["speed", "x"],
            Member::ModifiedAt,
        )))),
    ] {
        assert!(
            compile(&system, DateTime::MIN)?.reads_system_members(),
            "{system:?}"
        );
    }
    let observed = compile(
        &Expr::Path(at(&["speed"], Member::ObservedAt)),
        DateTime::MIN,
    )?;
    assert!(!observed.reads_system_members());
    assert_eq!(observed.attributes(), ["speed"]);
    Ok(())
}

#[test]
fn a_value_is_json_of_its_kind_and_unknown_is_none() {
    let value = |e: Expr| program(&e).value(&entity());
    let read = |attr: &str| value(Expr::Path(Path::value(attr)));
    assert_eq!(read("flag"), Ok(Some(json!(true))));
    assert_eq!(read("pm10"), Ok(Some(json!(12))));
    assert_eq!(read("ratio"), Ok(Some(json!(2.5))));
    assert_eq!(read("padded"), Ok(Some(json!("  x  "))));
    assert_eq!(
        read("when"),
        Ok(Some(json!("2026-10-11T12:30:15.25+02:00")))
    );
    assert_eq!(read("day"), Ok(Some(json!("2026-10-11"))));
    assert_eq!(read("at"), Ok(Some(json!("08:15:00"))));
    assert_eq!(read("tags"), Ok(Some(json!(["a", "b", 3]))));
    assert_eq!(
        read("address"),
        Ok(Some(json!({"city": "Bonn", "rooms": [{"n": 1}, {"n": 5}]})))
    );
    // missing, null, mistyped and overflowing are unknown, never a default
    assert_eq!(read("absent"), Ok(None));
    assert_eq!(read("nothing"), Ok(None));
    assert_eq!(read("broken"), Ok(None));
    let twice = |l: Lit| {
        Expr::Call(
            Func::Mul,
            vec![Expr::Path(Path::value("huge")), Expr::Lit(l)],
        )
    };
    assert_eq!(value(twice(Lit::Int(10))), Ok(None));
    // a computed value: a function over two members, a comparison
    let concat = Expr::Call(
        Func::Concat,
        vec![Expr::Path(Path::value("kind")), Expr::Path(Path::Id)],
    );
    assert_eq!(value(concat), Ok(Some(json!("Carurn:ngsi-ld:Station:1"))));
    let over = Expr::Cmp(
        Cmp::Gt,
        Box::new(Expr::Path(Path::value("pm10"))),
        Box::new(Expr::Lit(Lit::Int(10))),
    );
    assert_eq!(value(over), Ok(Some(json!(true))));
}

/// T-5905: a duration, bytes and a time NGSI-LD stores as text compare as the values they spell
/// (`OData` `Edm.Duration`, `Edm.Binary`, `Edm.TimeOfDay`), and `InRange` is a narrowing cast.
#[test]
fn durations_bytes_and_times_stored_as_text_compare_as_values() {
    let stored = json!({
        "id": "urn:ngsi-ld:Product:1", "type": "Product",
        "ship": {"type": "Property", "value": "P1DT2H30M"},
        "back": {"type": "Property", "value": "-P1D"},
        "data": {"type": "Property", "value": "dGVzdA=="},
        "opens": {"type": "Property", "value": "09:30:00Z"},
        "junk": {"type": "Property", "value": "soon"},
        "status": {"type": "Property", "value": 300}
    });
    let d = |t: &str| Lit::Duration(Duration::parse(t).unwrap_or_else(|| unreachable!("{t}")));
    let cmp = |op, attr: &str, lit: Lit| Expr::Cmp(op, Box::new(p(attr)), Box::new(Expr::Lit(lit)));
    for (e, want) in [
        (cmp(Cmp::Eq, "ship", d("PT95400S")), Truth::True),
        (cmp(Cmp::Gt, "ship", d("P1D")), Truth::True),
        (cmp(Cmp::Lt, "ship", d("PT1H")), Truth::False),
        (cmp(Cmp::Eq, "back", d("-PT24H")), Truth::True),
        (cmp(Cmp::Eq, "junk", d("P1D")), Truth::Unknown),
        (
            cmp(Cmp::Eq, "data", Lit::Binary(b"test".to_vec())),
            Truth::True,
        ),
        (
            cmp(Cmp::Ne, "data", Lit::Binary(vec![1, 2, 3])),
            Truth::True,
        ),
        (cmp(Cmp::Eq, "opens", Lit::Binary(vec![])), Truth::Unknown),
        (
            cmp(
                Cmp::Eq,
                "opens",
                Lit::Time(Time::parse("09:30:00").unwrap_or_else(|| unreachable!())),
            ),
            Truth::True,
        ),
    ] {
        assert_eq!(on(&e, &stored), want, "{e:?}");
    }
    let narrowed = |low, high| {
        Expr::Call(
            Func::InRange,
            vec![
                p("status"),
                Expr::Lit(Lit::Int(low)),
                Expr::Lit(Lit::Int(high)),
            ],
        )
    };
    let is_null = |e: Expr| Expr::IsNull(Box::new(e));
    assert_eq!(on(&is_null(narrowed(0, 255)), &stored), Truth::True);
    assert_eq!(on(&is_null(narrowed(-32768, 32767)), &stored), Truth::False);
    assert_eq!(on(&is_null(narrowed(400, 500)), &stored), Truth::True);
    // a duration or bytes compare with nothing but text that spells one
    let pm10 =
        json!({"id": "urn:ngsi-ld:P:2", "type": "P", "pm10": {"type": "Property", "value": 12}});
    assert_eq!(on(&cmp(Cmp::Eq, "pm10", d("P1D")), &pm10), Truth::Unknown);
    assert_eq!(
        on(&cmp(Cmp::Eq, "pm10", Lit::Binary(vec![12])), &pm10),
        Truth::Unknown
    );
    // and read back as their text
    for (lit, text) in [
        (d("P1DT2H30M"), "P1DT2H30M"),
        (Lit::Binary(b"test".to_vec()), "dGVzdA"),
    ] {
        let value = compile(&Expr::Lit(lit), now()).and_then(|p| p.value(&pm10));
        assert_eq!(value, Ok(Some(json!(text))));
    }
    for text in [
        "PT1.5S",
        "P1DT2H30M",
        "PT2H",
        "PT30M",
        "PT45S",
        "P2DT0.000001S",
    ] {
        assert_eq!(
            Duration::parse(text).map(|d| d.to_string()),
            Some(text.into())
        );
    }
    assert_eq!(
        Duration::parse("-P1D").map(|d| d.to_string()),
        Some("-P1D".into())
    );
    assert_eq!(
        Duration::parse("P0D").map(|d| d.to_string()),
        Some("PT0S".into())
    );
    for bad in [
        "P",
        "PT",
        "P1Y",
        "P1M",
        "PT1S2H",
        "1D",
        "P1DT",
        "PT.5S",
        "PT99999999999999999999S",
    ] {
        assert_eq!(Duration::parse(bad), None, "{bad}");
    }
}
