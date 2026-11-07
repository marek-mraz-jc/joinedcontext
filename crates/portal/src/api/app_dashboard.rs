//! A dashboard App edited without code (T-6402): `GET` and `POST
//! /api/v1/projects/{project}/apps/{name}/dashboard`.
//!
//! A dashboard App keeps what a person changes (grid columns and computed columns, filters, one
//! colour rule, one chart, the KPI tiles) in one file of its repository, `src/dashboard.json`,
//! which the SDK's `Dashboard` draws. The editor sends the whole file back; the Portal checks it
//! with the same rules as the SDK's `parseDashboardConfig` and against the attributes of the App's
//! type the person may read, then commits it on a branch of the App's repository and proposes the
//! App at that commit as a Change. Approving the Change merges the branch, which builds the App
//! (AP-77, AP-100). `dryRun` answers the file before and after without writing anything.
//!
//! The file never widens what the App reads: its grid's `source` and `type` stay as they are, and
//! the App keeps reading through its own Endpoint grant. A formula is parsed, never run.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::app_build::{app_of, NOT_ON_FORGE};
use crate::api::mutate::{propose_with_identity, ProposeOutcome};
use crate::auth::CurrentUser;
use crate::change::{Change, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, GitError, GiteaClient, MergeStyle};
use crate::pipeline_validation::Attribute;
use crate::state::AppState;

/// The App's editable file.
pub const DASHBOARD_FILE: &str = "src/dashboard.json";
/// The branches an edit commits to; an approved App Change merges the one it names.
const BRANCH_PREFIX: &str = "dashboard/";
/// What the file and every list in it may hold at most, as the SDK's parse allows.
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_COLUMNS: usize = 40;
const MAX_FILTERS: usize = 8;
const MAX_BANDS: usize = 6;
const MAX_KPIS: usize = 6;
const MAX_LABEL: usize = 60;
const MAX_PAGE_SIZE: u64 = 5000;
const TONES: &[&str] = &["neutral", "info", "success", "warning", "danger"];
const AGGS: &[&str] = &["count", "sum", "avg", "min", "max"];
const FILTER_OPS: &[&str] = &["==", "!=", ">", ">=", "<", "<="];
/// The grid keys the editor changes; any other key the App's file holds is carried unchanged.
const EDITED_GRID_KEYS: &[&str] = &[
    "source",
    "type",
    "columns",
    "map",
    "pageSize",
    "density",
    "entityTimestamps",
];
const COLUMN_KEYS: &[&str] = &[
    "attr", "label", "width", "pinned", "show", "editable", "format", "formula",
];

/// The parse of a computed column's formula, as `sdk/src/grid/formula.ts` reads it.
mod formula {
    pub const MAX_CHARS: usize = 500;
    const MAX_DEPTH: usize = 32;
    const OPERATORS: &[&str] = &[
        "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "!", "(", ")", "?",
        ":",
    ];
    const BINARY: &[&[&str]] = &[
        &["||"],
        &["&&"],
        &["==", "!="],
        &["<", "<=", ">", ">="],
        &["+", "-"],
        &["*", "/", "%"],
    ];

    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Num,
        Name(String),
        Op(&'static str),
    }

    /// Why the text is refused and at which character (0-based), as the SDK says it.
    pub type Refused = (String, usize);

    fn tokens(text: &str) -> Result<Vec<(Token, usize)>, Refused> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            if c == ' ' || c == '\t' {
                i += 1;
                continue;
            }
            let digit = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
            if digit(i) || (c == '.' && digit(i + 1)) {
                let start = i;
                while digit(i) {
                    i += 1;
                }
                if chars.get(i) == Some(&'.') && digit(i + 1) {
                    i += 1;
                    while digit(i) {
                        i += 1;
                    }
                }
                out.push((Token::Num, start));
                continue;
            }
            if c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    i += 1;
                }
                let name: String = chars.get(start..i).unwrap_or_default().iter().collect();
                out.push((Token::Name(name), start));
                continue;
            }
            let rest: String = chars.get(i..).unwrap_or_default().iter().take(2).collect();
            let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(**op)) else {
                let what = if matches!(c, '"' | '\'' | '`') {
                    "text in quotes".to_owned()
                } else {
                    format!("\"{c}\"")
                };
                return Err((format!("{what} is not part of a formula; use attribute names, numbers and + - * / % < > == != && || ! ?:"), i));
            };
            out.push((Token::Op(op), i));
            i += op.len();
        }
        Ok(out)
    }

    struct Parser<'a> {
        list: Vec<(Token, usize)>,
        pos: usize,
        len: usize,
        known: Option<&'a [String]>,
        attrs: Vec<String>,
    }

    impl Parser<'_> {
        fn end(&self) -> usize {
            self.list.get(self.pos).map_or(self.len, |(_, at)| *at)
        }

        fn is_op(&self, op: &str) -> bool {
            matches!(self.list.get(self.pos), Some((Token::Op(o), _)) if *o == op)
        }

        fn deep(&self, depth: usize) -> Result<(), Refused> {
            if depth > MAX_DEPTH {
                return Err((
                    format!("the formula nests deeper than {MAX_DEPTH} levels"),
                    self.end(),
                ));
            }
            Ok(())
        }

        fn conditional(&mut self, depth: usize) -> Result<(), Refused> {
            self.deep(depth)?;
            self.binary(0, depth)?;
            if !self.is_op("?") {
                return Ok(());
            }
            self.pos += 1;
            self.conditional(depth + 1)?;
            if !self.is_op(":") {
                return Err((
                    "a \"?\" needs its \":\" and the value for the other case".into(),
                    self.end(),
                ));
            }
            self.pos += 1;
            self.conditional(depth + 1)
        }

        fn binary(&mut self, level: usize, depth: usize) -> Result<(), Refused> {
            let Some(ops) = BINARY.get(level) else {
                return self.unary(depth);
            };
            self.binary(level + 1, depth)?;
            while matches!(self.list.get(self.pos), Some((Token::Op(o), _)) if ops.contains(o)) {
                self.pos += 1;
                self.binary(level + 1, depth)?;
            }
            Ok(())
        }

        fn unary(&mut self, depth: usize) -> Result<(), Refused> {
            self.deep(depth)?;
            if self.is_op("-") || self.is_op("!") {
                self.pos += 1;
                return self.unary(depth + 1);
            }
            self.primary(depth)
        }

        fn primary(&mut self, depth: usize) -> Result<(), Refused> {
            let Some((token, at)) = self.list.get(self.pos).cloned() else {
                return Err(("the formula ends where a value belongs".into(), self.len));
            };
            self.pos += 1;
            match token {
                Token::Num => Ok(()),
                Token::Name(name) if name == "true" || name == "false" => Ok(()),
                Token::Name(name) => {
                    if self.is_op("(") {
                        return Err((
                            format!("\"{name}(\" is a function, which a formula cannot use: write attribute names, numbers and + - * / only"),
                            at,
                        ));
                    }
                    if self.known.is_some_and(|known| !known.contains(&name)) {
                        return Err((format!("\"{name}\" is no attribute of this type"), at));
                    }
                    if !self.attrs.contains(&name) {
                        self.attrs.push(name);
                    }
                    Ok(())
                }
                Token::Op("(") => {
                    self.conditional(depth + 1)?;
                    if !self.is_op(")") {
                        return Err(("a \"(\" is not closed".into(), self.end()));
                    }
                    self.pos += 1;
                    Ok(())
                }
                Token::Op(op) => Err((format!("\"{op}\" stands where a value belongs"), at)),
            }
        }
    }

    /// The attributes `text` reads, or why it is refused and where; with `known`, a name outside
    /// it is refused too.
    pub fn parse(text: &str, known: Option<&[String]>) -> Result<Vec<String>, Refused> {
        let len = text.chars().count();
        if len > MAX_CHARS {
            return Err((
                format!("a formula holds at most {MAX_CHARS} characters"),
                MAX_CHARS,
            ));
        }
        let list = tokens(text)?;
        if list.is_empty() {
            return Err(("the formula is empty".into(), 0));
        }
        let mut parser = Parser {
            list,
            pos: 0,
            len,
            known,
            attrs: Vec::new(),
        };
        parser.conditional(0)?;
        if let Some((_, at)) = parser.list.get(parser.pos) {
            let follows: String = text.chars().skip(*at).take(12).collect();
            return Err((format!("\"{follows}\" follows a complete formula"), *at));
        }
        Ok(parser.attrs)
    }
}

/// What is wrong with a dashboard file, each as `config/<JSON path>: <why>`.
struct Findings(Vec<String>);

impl Findings {
    fn add(&mut self, path: &str, message: impl std::fmt::Display) {
        self.0.push(format!("config{path}: {message}"));
    }

    fn keys(&mut self, raw: &serde_json::Map<String, Value>, allowed: &[&str], path: &str) {
        for key in raw.keys().filter(|key| !allowed.contains(&key.as_str())) {
            self.add(&format!("{path}/{key}"), "unknown property");
        }
    }

    fn label(&mut self, raw: Option<&Value>, path: &str) {
        let ok = raw
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty() && text.chars().count() <= MAX_LABEL);
        if !ok {
            self.add(
                path,
                format!("must be a text of 1 to {MAX_LABEL} characters"),
            );
        }
    }

    fn named(&mut self, raw: Option<&Value>, path: &str, names: &[String]) {
        match raw.and_then(Value::as_str) {
            Some(name) if !name.is_empty() => {
                if !names.iter().any(|known| known == name) {
                    self.add(
                        path,
                        format!("\"{name}\" is no attribute of this type you may read"),
                    );
                }
            }
            _ => self.add(path, "must name an attribute"),
        }
    }

    fn formula(&mut self, raw: Option<&Value>, path: &str, stored: &[String]) -> Vec<String> {
        let Some(text) = raw.and_then(Value::as_str) else {
            self.add(path, "must be a string");
            return Vec::new();
        };
        match formula::parse(text, Some(stored)) {
            Ok(reads) => reads,
            Err((why, at)) => {
                self.add(path, format!("{why} (at character {})", at + 1));
                Vec::new()
            }
        }
    }

    fn one_of(&mut self, raw: Option<&Value>, path: &str, allowed: &[&str]) {
        if !raw
            .and_then(Value::as_str)
            .is_some_and(|value| allowed.contains(&value))
        {
            self.add(path, format!("must be one of {}", allowed.join(", ")));
        }
    }
}

fn is_finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}

/// The grid columns; the names of the computed ones.
fn check_columns(raw: Option<&Value>, stored: &[String], out: &mut Findings) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Some(columns) = raw.as_array().filter(|list| list.len() <= MAX_COLUMNS) else {
        out.add(
            "/grid/columns",
            format!("must be a list of at most {MAX_COLUMNS} columns"),
        );
        return Vec::new();
    };
    let mut computed = Vec::new();
    for (i, column) in columns.iter().enumerate() {
        let path = format!("/grid/columns/{i}");
        let Some(column) = column.as_object() else {
            out.add(&path, "must be an object");
            continue;
        };
        out.keys(column, COLUMN_KEYS, &path);
        let attr = column.get("attr").and_then(Value::as_str).unwrap_or("");
        if let Some(label) = column.get("label") {
            out.label(Some(label), &format!("{path}/label"));
        }
        if let Some(width) = column.get("width") {
            if !width.as_u64().is_some_and(|w| w > 0) {
                out.add(&format!("{path}/width"), "must be a positive integer");
            }
        }
        for flag in ["pinned", "editable"] {
            if column.get(flag).is_some_and(|v| !v.is_boolean()) {
                out.add(&format!("{path}/{flag}"), "must be a boolean");
            }
        }
        if let Some(format) = column.get("format") {
            out.one_of(
                Some(format),
                &format!("{path}/format"),
                &["text", "number", "date", "link"],
            );
        }
        if column.get("show").is_some_and(|v| !v.is_object()) {
            out.add(&format!("{path}/show"), "must be an object");
        }
        match column.get("formula") {
            None => out.named(column.get("attr"), &format!("{path}/attr"), stored),
            Some(text) => {
                let name_ok = !attr.is_empty()
                    && attr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !stored.iter().any(|s| s == attr);
                if !name_ok {
                    out.add(
                        &format!("{path}/attr"),
                        "a computed column needs a new name of letters, digits and _, such as occupancy",
                    );
                }
                let reads = out.formula(Some(text), &format!("{path}/formula"), stored);
                if reads.iter().any(|read| read == attr) {
                    out.add(
                        &format!("{path}/formula"),
                        format!(
                            "reads \"{attr}\", the column it computes; name the column differently"
                        ),
                    );
                }
                if column.get("editable") == Some(&Value::Bool(true)) {
                    out.add(
                        &format!("{path}/editable"),
                        "a computed column is worked out from the row and cannot be edited",
                    );
                }
                computed.push(attr.to_owned());
            }
        }
    }
    computed
}

/// The grid part: `source` and `type` as before, the edited keys checked, the rest carried.
fn check_grid(
    raw: Option<&Value>,
    before: &Value,
    stored: &[String],
    out: &mut Findings,
) -> Vec<String> {
    let Some(grid) = raw.and_then(Value::as_object) else {
        out.add("/grid", "must be an object");
        return Vec::new();
    };
    for key in ["source", "type"] {
        if grid.get(key) != before.pointer(&format!("/grid/{key}")) {
            out.add(
                &format!("/grid/{key}"),
                "stays as the App reads it; the App keeps reading through its own Endpoint",
            );
        }
    }
    for (key, value) in grid {
        if !EDITED_GRID_KEYS.contains(&key.as_str())
            && before.pointer(&format!("/grid/{key}")) != Some(value)
        {
            out.add(
                &format!("/grid/{key}"),
                "is carried unchanged; ask the assistant to change it",
            );
        }
    }
    if grid.get("pageSize").is_some_and(|size| {
        !size
            .as_u64()
            .is_some_and(|size| (1..=MAX_PAGE_SIZE).contains(&size))
    }) {
        out.add(
            "/grid/pageSize",
            format!("must be an integer between 1 and {MAX_PAGE_SIZE}"),
        );
    }
    if let Some(density) = grid.get("density") {
        out.one_of(Some(density), "/grid/density", &["compact", "comfortable"]);
    }
    if grid
        .get("entityTimestamps")
        .is_some_and(|v| !v.is_boolean())
    {
        out.add("/grid/entityTimestamps", "must be a boolean");
    }
    if let Some(map) = grid.get("map") {
        match map.as_object() {
            None => out.add("/grid/map", "must be an object"),
            Some(map) => {
                out.keys(map, &["enabled", "attr", "position"], "/grid/map");
                if map.get("enabled").is_some_and(|v| !v.is_boolean()) {
                    out.add("/grid/map/enabled", "must be a boolean");
                }
                if let Some(attr) = map.get("attr") {
                    out.named(Some(attr), "/grid/map/attr", stored);
                }
                if let Some(position) = map.get("position") {
                    out.one_of(Some(position), "/grid/map/position", &["right", "bottom"]);
                }
            }
        }
    }
    check_columns(grid.get("columns"), stored, out)
}

fn check_filters(raw: Option<&Value>, stored: &[String], out: &mut Findings) {
    let Some(raw) = raw else { return };
    let Some(filters) = raw.as_array().filter(|list| list.len() <= MAX_FILTERS) else {
        return out.add(
            "/filters",
            format!("must be a list of at most {MAX_FILTERS} filters"),
        );
    };
    for (i, filter) in filters.iter().enumerate() {
        let path = format!("/filters/{i}");
        let Some(filter) = filter.as_object() else {
            out.add(&path, "must be an object");
            continue;
        };
        out.keys(filter, &["attr", "op", "value"], &path);
        out.named(filter.get("attr"), &format!("{path}/attr"), stored);
        if !filter
            .get("op")
            .and_then(Value::as_str)
            .is_some_and(|op| FILTER_OPS.contains(&op))
        {
            out.add(
                &format!("{path}/op"),
                format!("must be one of {}", FILTER_OPS.join(" ")),
            );
        }
        let ok = match filter.get("value") {
            Some(Value::Bool(_)) => true,
            Some(number @ Value::Number(_)) => is_finite(number),
            Some(Value::String(text)) => {
                text.chars().count() <= 200 && !text.contains(['"', '\\', ';', '|', '(', ')', '\n'])
            }
            _ => false,
        };
        if !ok {
            out.add(
                &format!("{path}/value"),
                "must be a number, true/false or a text of at most 200 characters without \" \\ ; | ( )",
            );
        }
    }
}

fn check_colour(raw: Option<&Value>, readable: &[String], out: &mut Findings) {
    let Some(raw) = raw else { return };
    let Some(colour) = raw.as_object() else {
        return out.add("/colour", "must be an object");
    };
    out.keys(colour, &["attr", "bands"], "/colour");
    out.named(colour.get("attr"), "/colour/attr", readable);
    let Some(bands) = colour
        .get("bands")
        .and_then(Value::as_array)
        .filter(|list| (1..=MAX_BANDS).contains(&list.len()))
    else {
        return out.add(
            "/colour/bands",
            format!("must be a list of 1 to {MAX_BANDS} bands"),
        );
    };
    let mut last = f64::NEG_INFINITY;
    for (i, band) in bands.iter().enumerate() {
        let path = format!("/colour/bands/{i}");
        let Some(band) = band.as_object() else {
            out.add(&path, "must be an object");
            continue;
        };
        out.keys(band, &["below", "tone", "label"], &path);
        match band.get("below") {
            None if i + 1 != bands.len() => out.add(
                &format!("{path}/below"),
                "only the last band may leave out `below`",
            ),
            None => {}
            Some(below) => match below.as_f64().filter(|b| b.is_finite() && *b > last) {
                Some(below) => last = below,
                None => out.add(
                    &format!("{path}/below"),
                    "must be a number greater than the band before",
                ),
            },
        }
        out.one_of(band.get("tone"), &format!("{path}/tone"), TONES);
        out.label(band.get("label"), &format!("{path}/label"));
    }
}

fn check_chart(raw: Option<&Value>, readable: &[String], out: &mut Findings) {
    let Some(raw) = raw else { return };
    let Some(chart) = raw.as_object() else {
        return out.add("/chart", "must be an object");
    };
    out.keys(chart, &["x", "y", "agg"], "/chart");
    out.named(chart.get("x"), "/chart/x", readable);
    out.one_of(chart.get("agg"), "/chart/agg", AGGS);
    if chart.get("agg").and_then(Value::as_str) != Some("count") || chart.contains_key("y") {
        out.named(chart.get("y"), "/chart/y", readable);
    }
}

fn check_kpis(raw: Option<&Value>, stored: &[String], readable: &[String], out: &mut Findings) {
    let Some(raw) = raw else { return };
    let Some(kpis) = raw.as_array().filter(|list| list.len() <= MAX_KPIS) else {
        return out.add(
            "/kpis",
            format!("must be a list of at most {MAX_KPIS} tiles"),
        );
    };
    for (i, kpi) in kpis.iter().enumerate() {
        let path = format!("/kpis/{i}");
        let Some(kpi) = kpi.as_object() else {
            out.add(&path, "must be an object");
            continue;
        };
        out.keys(kpi, &["label", "agg", "attr", "where"], &path);
        out.label(kpi.get("label"), &format!("{path}/label"));
        out.one_of(kpi.get("agg"), &format!("{path}/agg"), AGGS);
        if kpi.get("agg").and_then(Value::as_str) != Some("count") || kpi.contains_key("attr") {
            out.named(kpi.get("attr"), &format!("{path}/attr"), readable);
        }
        if let Some(text) = kpi.get("where") {
            out.formula(Some(text), &format!("{path}/where"), stored);
        }
    }
}

/// Every refusal of `raw` as `config/<path>: <why>`, empty when it may be proposed: the rules of
/// the SDK's `parseDashboardConfig`, names checked against `stored` (the attributes of the App's
/// type the person may read), and the grid's `source` and `type` as `before` holds them.
pub fn check(raw: &Value, before: &Value, stored: &[String]) -> Vec<String> {
    let mut out = Findings(Vec::new());
    let Some(file) = raw.as_object() else {
        out.add("", "must be an object");
        return out.0;
    };
    out.keys(
        file,
        &["title", "grid", "filters", "colour", "chart", "kpis"],
        "",
    );
    if let Some(title) = file.get("title") {
        out.label(Some(title), "/title");
    }
    if raw.pointer("/grid/filters/preset").is_some() {
        out.add(
            "/grid/filters/preset",
            "a dashboard keeps its filters in `filters`",
        );
    }
    let computed = check_grid(file.get("grid"), before, stored, &mut out);
    let readable: Vec<String> = stored.iter().cloned().chain(computed).collect();
    check_filters(file.get("filters"), stored, &mut out);
    check_colour(file.get("colour"), &readable, &mut out);
    check_chart(file.get("chart"), &readable, &mut out);
    check_kpis(file.get("kpis"), stored, &readable, &mut out);
    out.0
}

/// The attributes of the dashboard's type the person may read: the type's attributes in the
/// model of the space the App's data need names for it, narrowed to the need's `attrs`, and none
/// when the person may not read that space (T-6402, AP-05).
fn readable_attributes(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    app: &Value,
    entity_type: &str,
) -> Vec<Attribute> {
    let needs = app
        .pointer("/spec/dataNeeds")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Some(need) = needs.iter().find(|need| {
        need["types"]
            .as_array()
            .is_some_and(|types| types.iter().any(|t| t.as_str() == Some(entity_type)))
    }) else {
        return Vec::new();
    };
    let Some(space) = need
        .pointer("/contextSpaceRef/name")
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    if crate::api::data_views::readable_space(state, &user.0.identity, project, space).is_err() {
        return Vec::new();
    }
    let Some(class) = state
        .model_schemas
        .get(project, space)
        .and_then(|model| model.attributes(entity_type))
    else {
        return Vec::new();
    };
    let narrowed: Vec<&str> = need["attrs"]
        .as_array()
        .map(|attrs| attrs.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    class
        .attributes
        .into_iter()
        .filter(|attr| narrowed.is_empty() || narrowed.contains(&attr.name.as_str()))
        .collect()
}

/// Whether the caller may propose the App, or why not.
fn edit_refusal(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    app: &Value,
) -> Option<String> {
    crate::permissions::for_request(state, &user.0.identity, project)
        .check("App", Verb::Propose, Some(app))
        .is_err()
        .then(|| format!("Editing the dashboard needs propose on App in project {project}"))
}

/// The file on the App's default branch, parsed; `None` when it holds none.
async fn current_file(repo: &GiteaClient) -> Result<Option<(String, Value)>, ApiError> {
    let refused = |err: GitError| ApiError::Unavailable(format!("the forge did not answer: {err}"));
    let default = repo.default_branch().await.map_err(refused)?;
    let Some(file) = repo
        .get_file(DASHBOARD_FILE, &default)
        .await
        .map_err(refused)?
    else {
        return Ok(None);
    };
    let parsed = serde_json::from_str(&file.content).unwrap_or(Value::Null);
    Ok(Some((default, parsed)))
}

/// Whether Edit is offered, and why not when it is not.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditOffer {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The App's dashboard file and what the editor offers for it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppDashboard {
    /// The file's path in the App's repository: `src/dashboard.json`.
    pub path: String,
    /// The file as the default branch holds it; `null` when the App keeps none.
    #[schema(value_type = Object)]
    pub config: Value,
    /// The entity type the dashboard lists, from its grid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// The attributes of that type the person may read: every attribute dropdown offers these.
    pub attributes: Vec<Attribute>,
    pub edit: EditOffer,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/dashboard",
    summary = "The Editable Dashboard Of An Application",
    description = "The App's src/dashboard.json, the attributes of its type the caller may read, and whether the caller may propose an edit (T-6402).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The dashboard file and what may go in it", body = AppDashboard),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge did not answer", body = ProblemDetails)
    )
)]
pub async fn dashboard(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<AppDashboard>, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    let file = match &repo {
        Some(repo) => current_file(repo).await?,
        None => None,
    };
    let config = file.map(|(_, config)| config).unwrap_or(Value::Null);
    let entity_type = config
        .pointer("/grid/type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let attributes = entity_type
        .as_deref()
        .map(|t| readable_attributes(&state, &user, &project, &app, t))
        .unwrap_or_default();
    let reason = if repo.is_none() {
        Some(NOT_ON_FORGE.to_owned())
    } else if config.is_null() {
        Some(format!(
            "The App keeps no {DASHBOARD_FILE}: ask the assistant to keep its dashboard there"
        ))
    } else if attributes.is_empty() {
        Some("You may read none of the attributes this dashboard shows".to_owned())
    } else {
        edit_refusal(&state, &user, &project, &app)
    };
    Ok(Json(AppDashboard {
        path: DASHBOARD_FILE.to_owned(),
        config,
        entity_type,
        attributes,
        edit: EditOffer {
            allowed: reason.is_none(),
            reason,
        },
    }))
}

/// The edited file, and whether to only preview it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardEdit {
    /// The whole new `src/dashboard.json`.
    #[schema(value_type = Object)]
    pub config: Value,
    /// `true` answers the file before and after and writes nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// What an edit proposes: the file before and after, and the Change once it is proposed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProposal {
    pub before: String,
    pub after: String,
    /// The Change that proposes the App at the edited commit; absent on a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
    /// The App repository's merge request the approval merges; absent on a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
}

fn text_of(config: &Value) -> String {
    let mut text = serde_json::to_string_pretty(config).unwrap_or_default();
    text.push('\n');
    text
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/apps/{name}/dashboard",
    summary = "Propose An Edit Of An Application's Dashboard",
    description = "Checks the new src/dashboard.json (formulas parsed, never run; every attribute one of the App's type the caller may read; the grid's source and type unchanged), then commits it on a branch of the App's repository and proposes the App at that commit as a Change; approving it merges the branch and builds the App. dryRun answers the file before and after and writes nothing (T-6402).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    request_body(
        content = DashboardEdit,
        example = json!({ "dryRun": true, "config": { "grid": { "source": { "kind": "endpoint", "slug": "bikes" }, "type": "BikeHireDockingStation", "columns": [{ "attr": "name" }, { "attr": "occupancy", "label": "Occupancy %", "formula": "availableBikeNumber / totalSlotNumber * 100" }] }, "colour": { "attr": "occupancy", "bands": [{ "below": 10, "tone": "danger", "label": "Almost empty" }, { "tone": "success", "label": "Bikes available" }] } } })
    ),
    responses(
        (status = 200, description = "Dry run: the file before and after", body = DashboardProposal),
        (status = 202, description = "The Change that proposes the edit", body = DashboardProposal),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose App here, or reads none of its attributes", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 409, description = "Not built on the forge, no dashboard file, or nothing changed", body = ProblemDetails),
        (status = 422, description = "A field of the file is refused; each error names its path", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge refused", body = ProblemDetails)
    )
)]
pub async fn propose(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(edit): Json<DashboardEdit>,
) -> Result<Response, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    if let Some(reason) = edit_refusal(&state, &user, &project, &app) {
        return Err(ApiError::Denied(reason));
    }
    let repo = repo.ok_or_else(|| ApiError::Conflict(NOT_ON_FORGE.into()))?;
    let (default, before) = current_file(&repo).await?.ok_or_else(|| {
        ApiError::Conflict(format!(
            "the App keeps no {DASHBOARD_FILE}, so there is nothing to edit"
        ))
    })?;
    let entity_type = before
        .pointer("/grid/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stored: Vec<String> = readable_attributes(&state, &user, &project, &app, entity_type)
        .into_iter()
        .map(|attr| attr.name)
        .collect();
    let (before_text, after) = checked(&edit.config, &before, &stored)?;
    if edit.dry_run {
        let proposal = DashboardProposal {
            before: before_text,
            after,
            change: None,
            pull_request_url: None,
        };
        return Ok((StatusCode::OK, Json(proposal)).into_response());
    }
    let (sha, pull_request_url) = commit(&repo, &user, &project, &name, &default, &after).await?;
    let change = propose_at(&state, &user, &project, &name, sha).await?;
    let proposal = DashboardProposal {
        before: before_text,
        after,
        change: Some(change),
        pull_request_url: Some(pull_request_url),
    };
    Ok((StatusCode::ACCEPTED, Json(proposal)).into_response())
}

/// The file before and after as text, or the refusal: no readable attribute (403), a field the
/// rules refuse (422 naming each), a file over the cap, or nothing changed (409).
fn checked(
    config: &Value,
    before: &Value,
    stored: &[String],
) -> Result<(String, String), ApiError> {
    if stored.is_empty() {
        return Err(ApiError::Denied(
            "you may read none of the attributes this dashboard shows".into(),
        ));
    }
    let after = text_of(config);
    let mut errors = check(config, before, stored);
    if after.len() > MAX_FILE_BYTES {
        errors.push(format!("config: holds at most {MAX_FILE_BYTES} bytes"));
    }
    if !errors.is_empty() {
        return Err(ApiError::Unprocessable {
            detail: errors.join("; "),
            errors,
        });
    }
    let before_text = text_of(before);
    if after == before_text {
        return Err(ApiError::Conflict(format!(
            "the edit leaves {DASHBOARD_FILE} as it is"
        )));
    }
    Ok((before_text, after))
}

/// Commits the file on a new `dashboard/` branch and opens its merge request: the commit and the
/// merge request's address.
async fn commit(
    repo: &GiteaClient,
    user: &CurrentUser,
    project: &str,
    name: &str,
    default: &str,
    after: &str,
) -> Result<(String, String), ApiError> {
    let refused = |err: GitError| {
        ApiError::Unavailable(format!(
            "the forge did not take the edit of '{name}': {err}"
        ))
    };
    let branch = format!("{BRANCH_PREFIX}{}", chrono::Utc::now().timestamp_millis());
    repo.create_branch(&branch, default)
        .await
        .map_err(refused)?;
    let (author_name, author_email) =
        crate::api::mutate::author_credentials(&user.0.identity, project);
    let message = format!("Edit the dashboard of {name} (T-6402)");
    let sha = repo
        .change_files(
            &branch,
            &message,
            Author {
                name: &author_name,
                email: &author_email,
            },
            &[(DASHBOARD_FILE.to_owned(), after.to_owned())],
            &[],
        )
        .await
        .map_err(refused)?;
    let pull = repo
        .create_pull_request(
            &branch,
            default,
            &message,
            &format!(
                "Changes {DASHBOARD_FILE} only. It merges when the Change that proposes App `{name}` at {sha} is approved in the Portal."
            ),
        )
        .await
        .map_err(refused)?;
    tracing::info!(app = %name, project = %project, branch = %branch, by = %user.0.identity.username, "dashboard edit committed");
    Ok((sha, pull.url))
}

/// Proposes the App as the mirror holds it, at `sha`, as the person's Change.
async fn propose_at(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
    sha: String,
) -> Result<Change, ApiError> {
    let mut manifest = state
        .mirror
        .get(project, "App", name)
        .ok_or_else(|| ApiError::NotFound(format!("app '{name}' not found")))?;
    manifest.strip_status();
    let mut manifest = serde_json::to_value(manifest)
        .map_err(|err| ApiError::Internal(format!("the App did not serialise: {err}")))?;
    let Some(git_ref) = manifest.pointer_mut("/spec/source/git/ref") else {
        return Err(ApiError::Conflict(NOT_ON_FORGE.into()));
    };
    *git_ref = Value::String(sha);
    let outcome = propose_with_identity(
        &user.0.identity,
        state,
        project,
        "apps",
        Some(name),
        Operation::Update,
        false,
        manifest,
    )
    .await?;
    match outcome {
        ProposeOutcome::Change(change) => Ok(change),
        _ => Err(ApiError::Internal(
            "the proposal answered with something other than a Change".into(),
        )),
    }
}

/// After an App Change is approved: the open dashboard merge request whose head is the commit
/// the approved manifest names merges, pinned to that commit, so what builds is what the
/// approver read (AP-77, PF-57). Any other App Change finds none and does nothing. A merge the
/// forge refuses is logged; the approval stands.
pub async fn merge_approved_edit(
    state: &AppState,
    project: &str,
    manifest: Option<&crate::resource::ResourceEnvelope>,
    approver: &str,
) {
    let Some(manifest) = manifest.filter(|manifest| manifest.kind == "App") else {
        return;
    };
    let Some(sha) = manifest
        .spec
        .pointer("/source/git/ref")
        .and_then(Value::as_str)
    else {
        return;
    };
    let Some(gitea) = state.gitea.as_deref() else {
        return;
    };
    let name = &manifest.metadata.name;
    let repo = gitea.for_application(crate::agents::repository::name(project, name));
    let pulls = match repo.list_pull_requests("open").await {
        Ok(pulls) => pulls,
        Err(err) => {
            tracing::warn!(app = %name, error = %err, "dashboard merge requests not listed");
            return;
        }
    };
    let Some(pull) = pulls
        .into_iter()
        .find(|pull| pull.head_branch.starts_with(BRANCH_PREFIX) && pull.head_sha == sha)
    else {
        return;
    };
    let message = format!("Edit the dashboard of {name}\n\nApproved in the Portal by {approver}");
    if let Err(err) = repo
        .merge(pull.number, MergeStyle::Merge, &message, Some(sha))
        .await
    {
        tracing::warn!(app = %name, pull = pull.number, error = %err, "approved dashboard edit not merged");
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/apps/{name}/dashboard",
        get(dashboard).post(propose),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    /// The cases the SDK's `parseDashboardConfig` answers in sdk/tests/dashboard.test.ts: both
    /// halves refuse the same files at the same paths, so an App never gets a file it refuses.
    #[test]
    fn the_server_and_the_sdk_refuse_the_same_files_at_the_same_paths() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../sdk/tests/fixtures/dashboard_cases.json"
        ))
        .expect("the shared cases");
        let known: Vec<String> = serde_json::from_value(cases["known"].clone()).expect("known");
        let before = &cases["before"];
        for case in cases["cases"].as_array().expect("cases") {
            let found = check(&case["config"], before, &known);
            let paths: Vec<String> = case["refused"]
                .as_array()
                .expect("refused")
                .iter()
                .map(|p| format!("config{}:", p.as_str().unwrap_or_default()))
                .collect();
            for path in &paths {
                assert!(
                    found.iter().any(|f| f.starts_with(path.as_str())),
                    "{}: {path} not among {found:?}",
                    case["name"]
                );
            }
            if paths.is_empty() {
                assert!(found.is_empty(), "{}: {found:?}", case["name"]);
            }
        }
    }

    /// Project ovzdusie with App `bikes`, which pia@hel.fi may read when `granted`; no forge.
    fn world(granted: bool) -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        let mut manifests = vec![(
            "App",
            "bikes",
            "ovzdusie",
            serde_json::json!({ "source": { "git": { "ref": "main" } } }),
        )];
        if granted {
            manifests.push((
                "Role",
                "reader",
                org,
                serde_json::json!({ "rules": [{ "kinds": ["App"], "verbs": ["read"] }] }),
            ));
            manifests.push((
                "RoleBinding",
                "reader",
                org,
                serde_json::json!({ "subjects": [{ "user": "pia@hel.fi" }], "role": "reader", "scope": { "project": "ovzdusie" } }),
            ));
        }
        for (kind, name, namespace, spec) in manifests {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        }
        state
    }

    fn pia() -> CurrentUser {
        use crate::auth::session::{Identity, Session};
        CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-pia".into(),
                username: "pia".into(),
                email: Some("pia@hel.fi".into()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    fn edit() -> DashboardEdit {
        DashboardEdit {
            config: serde_json::json!({ "grid": { "type": "BikeHireDockingStation" } }),
            dry_run: true,
        }
    }

    /// Both doors hide an App the person may not read (or that does not exist) behind 404, and
    /// answer 503 for a readable App while no forge is configured, before any file is read.
    #[tokio::test]
    async fn the_dashboard_doors_hide_unreadable_apps_and_need_a_forge() {
        let at = |app: &str| Path(("ovzdusie".to_owned(), app.to_owned()));
        for (granted, app) in [(false, "bikes"), (true, "trams")] {
            let read = dashboard(pia(), State(world(granted)), at(app)).await;
            assert!(
                matches!(read, Err(ApiError::NotFound(_))),
                "{app}: {read:?}"
            );
            let proposed = propose(pia(), State(world(granted)), at(app), Json(edit())).await;
            assert!(
                matches!(proposed, Err(ApiError::NotFound(_))),
                "{app}: {:?}",
                proposed.err()
            );
        }
        let read = dashboard(pia(), State(world(true)), at("bikes")).await;
        assert!(matches!(read, Err(ApiError::Unavailable(_))), "{read:?}");
        let proposed = propose(pia(), State(world(true)), at("bikes"), Json(edit())).await;
        assert!(
            matches!(proposed, Err(ApiError::Unavailable(_))),
            "{:?}",
            proposed.err()
        );
    }

    #[test]
    fn a_formula_reads_names_and_numbers_and_nothing_else() {
        let known = names(&["a", "b"]);
        assert_eq!(
            formula::parse("a / b * 100", Some(&known)),
            Ok(names(&["a", "b"]))
        );
        assert_eq!(
            formula::parse("a == 0 ? 1 : -.5", Some(&known)),
            Ok(names(&["a"]))
        );
        assert!(formula::parse("!(a >= b) && true || b % 2 != 1", Some(&known)).is_ok());
        for (text, at) in [
            ("alert(1)", 0),
            ("a.constructor", 1),
            ("\"x\"", 0),
            ("a = 1", 2),
            ("`${a}`", 0),
            ("c + 1", 0),
            ("a +", 3),
            ("(a", 2),
            ("a ? b", 5),
            ("a b", 2),
            ("", 0),
        ] {
            let refused = formula::parse(text, Some(&known));
            assert_eq!(
                refused.as_ref().map_err(|(_, at)| *at),
                Err(at),
                "{text}: {refused:?}"
            );
        }
        assert!(formula::parse(&"a+".repeat(300), Some(&known)).is_err());
        assert!(formula::parse(&format!("{}a{}", "(".repeat(40), ")".repeat(40)), None).is_err());
    }
}
