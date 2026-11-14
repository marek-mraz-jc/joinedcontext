//! `jc_render` (T-6860, design mcp-pictures draft 2): a picture of one Endpoint's data for an
//! agent. The input is a closed subset of Vega-Lite 5 (marks line, bar, area, point, arc, rule;
//! channels x, y, color, size, theta; tooltip accepted and ignored) whose `data` is one
//! `jc_data_query` read, so the rows are read through the gateway as the caller, as that tool
//! reads them: a chart never shows what the caller could not read as rows, and an Endpoint of
//! another Organization answers the same 404. Every member outside the subset is refused 422
//! naming it: no `url`, no expression, no `transform`, no `image` mark, no `href`. The picture
//! is drawn in this process ([`super::chart`]) to a PNG of at most 2 MiB and 4000 x 4000 px,
//! under a 10 s cap with at most [`RENDER_SLOTS`] renders at once; a render past the cap answers
//! `limited`. The answer marks itself `untrusted`: category values come from the data and are
//! named in `categories`, never drawn.

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Map, Value};

use super::chart::{self, Cell, Channel, Chart, Mark, Point, Scale};
use super::data_query::{self, DataQueryInput};
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::error::ApiError;
use crate::state::AppState;

pub const NAME: &str = "jc_render";
const MAX_SIDE: u32 = 4000;
const MIN_SIDE: u32 = 50;
const MAX_PNG: usize = 2 * 1024 * 1024;
const MAX_POINTS: usize = 20_000;
const MAX_TITLE: usize = 120;
pub(crate) const TIME_CAP: Duration = Duration::from_secs(10);
/// Renders at once in this process: each holds at most a 4000 x 4000 RGB canvas (48 MiB).
const RENDER_SLOTS: usize = 2;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(RENDER_SLOTS);

const TOP: &[&str] = &[
    "$schema",
    "title",
    "description",
    "width",
    "height",
    "mark",
    "encoding",
    "data",
];
const CHANNELS: &[&str] = &["x", "y", "color", "size", "theta", "tooltip"];
const FIELD: &[&str] = &["field", "type", "title"];

fn invalid(path: impl Into<String>, message: impl Into<String>) -> OpError {
    OpError::InvalidInput {
        path: path.into(),
        message: message.into(),
    }
}

/// Why a member is refused, in words a caller can act on.
fn refused_member(at: &str, key: &str, allowed: &[&str]) -> OpError {
    let why = match key {
        "url" | "href" | "values" | "name" | "sequence" | "graticule" | "sphere" => {
            "jc_render reads no URL and no inline rows: name the Endpoint read in data.query"
                .to_owned()
        }
        "transform" | "params" | "selection" | "calculate" | "filter" | "expr" | "signal"
        | "condition" => {
            "jc_render evaluates no expression; filter in data.query (q, attrs, temporal)"
                .to_owned()
        }
        _ => format!(
            "not in the jc_render subset of Vega-Lite; allowed here: {}",
            allowed.join(", ")
        ),
    };
    invalid(format!("{at}/{key}"), why)
}

/// `value` as an object holding only `allowed` members.
fn members<'a>(
    value: &'a Value,
    at: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, OpError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(if at.is_empty() { "/" } else { at }, "an object"))?;
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(refused_member(at, key, allowed)),
        None => Ok(object),
    }
}

fn text(value: &Value, at: &str) -> Result<String, OpError> {
    let text = value.as_str().ok_or_else(|| invalid(at, "a string"))?;
    if text.chars().count() > MAX_TITLE || text.chars().any(char::is_control) {
        return Err(invalid(
            at,
            format!("at most {MAX_TITLE} characters on one line"),
        ));
    }
    Ok(text.to_owned())
}

/// One field channel: the row member it reads, its scale and its axis title.
#[derive(Debug, Clone)]
struct Field {
    field: String,
    scale: Scale,
    title: String,
}

fn field_name(value: &Value, at: &str) -> Result<String, OpError> {
    let name = value
        .as_str()
        .ok_or_else(|| invalid(at, "a field name, e.g. pm10"))?;
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:@-".contains(c));
    if ok {
        Ok(name.to_owned())
    } else {
        Err(invalid(
            at,
            "a field name of letters, digits and _ . : @ -, e.g. pm10",
        ))
    }
}

fn field(value: &Value, at: &str) -> Result<Field, OpError> {
    let object = members(value, at, FIELD)?;
    let name = field_name(
        object.get("field").unwrap_or(&Value::Null),
        &format!("{at}/field"),
    )?;
    let scale = match object.get("type").and_then(Value::as_str) {
        Some("quantitative") => Scale::Quantitative,
        Some("temporal") => Scale::Temporal,
        Some("nominal") => Scale::Nominal,
        _ => {
            return Err(invalid(
                format!("{at}/type"),
                "quantitative, temporal or nominal",
            ))
        }
    };
    let title = match object.get("title") {
        Some(title) => text(title, &format!("{at}/title"))?,
        None => name.clone(),
    };
    Ok(Field {
        field: name,
        scale,
        title,
    })
}

/// A checked spec: what to draw and the read that fills it.
#[derive(Debug)]
struct Spec {
    mark: Mark,
    title: String,
    width: u32,
    height: u32,
    x: Option<Field>,
    y: Option<Field>,
    color: Option<Field>,
    size: Option<Field>,
    theta: Option<Field>,
    query: DataQueryInput,
}

fn side(value: Option<&Value>, at: &str, default: u32) -> Result<u32, OpError> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| (MIN_SIDE..=MAX_SIDE).contains(v))
        .ok_or_else(|| invalid(at, format!("pixels, {MIN_SIDE} to {MAX_SIDE}")))
}

fn mark_of(value: Option<&Value>) -> Result<Mark, OpError> {
    let (name, at) = match value {
        Some(Value::String(name)) => (name.as_str(), "/mark"),
        Some(object @ Value::Object(_)) => {
            let object = members(object, "/mark", &["type"])?;
            (
                object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "/mark/type",
            )
        }
        _ => ("", "/mark"),
    };
    Ok(match name {
        "line" => Mark::Line,
        "bar" => Mark::Bar,
        "area" => Mark::Area,
        "point" => Mark::Point,
        "arc" => Mark::Arc,
        "rule" => Mark::Rule,
        "image" => return Err(invalid(at, "the image mark loads a URL and is refused")),
        _ => return Err(invalid(at, "one of line, bar, area, point, arc, rule")),
    })
}

fn spec_of(input: &Value) -> Result<Spec, OpError> {
    let top = members(input, "", TOP)?;
    if let Some(schema) = top.get("$schema") {
        // Only named, never fetched.
        let ok = schema
            .as_str()
            .is_some_and(|s| s.starts_with("https://vega.github.io/schema/vega-lite/v5"));
        if !ok {
            return Err(invalid(
                "/$schema",
                "https://vega.github.io/schema/vega-lite/v5.json or none",
            ));
        }
    }
    let title = match top.get("title") {
        Some(title) => text(title, "/title")?,
        None => String::new(),
    };
    if let Some(description) = top.get("description") {
        text(description, "/description")?;
    }
    let mark = mark_of(top.get("mark"))?;
    let empty = json!({});
    let encoding = members(top.get("encoding").unwrap_or(&empty), "/encoding", CHANNELS)?;
    let channel = |name: &str| -> Result<Option<Field>, OpError> {
        encoding
            .get(name)
            .map(|v| field(v, &format!("/encoding/{name}")))
            .transpose()
    };
    let (x, y, color, size, theta) = (
        channel("x")?,
        channel("y")?,
        channel("color")?,
        channel("size")?,
        channel("theta")?,
    );
    let needs = |what: &str| {
        invalid(
            "/encoding",
            format!("a {} mark needs {what}", mark_name(mark)),
        )
    };
    match mark {
        Mark::Arc
            if theta
                .as_ref()
                .is_none_or(|t| t.scale != Scale::Quantitative) =>
        {
            return Err(needs("a quantitative theta"))
        }
        Mark::Rule if x.is_none() && y.is_none() => return Err(needs("x or y")),
        Mark::Line | Mark::Bar | Mark::Area | Mark::Point if x.is_none() || y.is_none() => {
            return Err(needs("x and y"))
        }
        _ => {}
    }
    for (name, channel) in [("size", &size), ("theta", &theta)] {
        if channel
            .as_ref()
            .is_some_and(|c| c.scale != Scale::Quantitative)
        {
            return Err(invalid(format!("/encoding/{name}/type"), "quantitative"));
        }
    }
    let data = members(top.get("data").unwrap_or(&Value::Null), "/data", &["query"])?;
    let query = data
        .get("query")
        .ok_or_else(|| invalid("/data/query", "the jc_data_query read that fills the chart, e.g. {\"endpoint\": \"air\", \"type\": \"AirQualityObserved\"}"))?;
    let mut query: DataQueryInput = parse_input(query.clone()).map_err(|err| match err {
        OpError::InvalidInput { path, message } => invalid(
            format!("/data/query{}", path.trim_end_matches('/')),
            message,
        ),
        other => other,
    })?;
    if query.count {
        return Err(invalid(
            "/data/query/count",
            "a chart draws rows; leave count out",
        ));
    }
    // The rows come in the shape the flattening below reads.
    if query
        .temporal
        .as_ref()
        .is_some_and(|t| t.aggr_methods.is_empty())
    {
        query.format = Some("temporalValues".into());
    } else if query.temporal.is_none() {
        query.format = Some("keyValues".into());
    }
    data_query::request_of(&query).map_err(|err| match err {
        OpError::InvalidInput { path, message } => invalid(format!("/data/query{path}"), message),
        other => other,
    })?;
    Ok(Spec {
        mark,
        title,
        width: side(top.get("width"), "/width", 800)?,
        height: side(top.get("height"), "/height", 450)?,
        x,
        y,
        color,
        size,
        theta,
        query,
    })
}

fn mark_name(mark: Mark) -> &'static str {
    match mark {
        Mark::Line => "line",
        Mark::Bar => "bar",
        Mark::Area => "area",
        Mark::Point => "point",
        Mark::Arc => "arc",
        Mark::Rule => "rule",
    }
}

/// The entities of one read as flat rows: a current read gives one row per entity, a history
/// one row per value with `observedAt` (an aggregate's period start) and the value under the
/// attribute's name and, aggregated, `<attribute>.<method>`.
fn rows_of(entities: &[Value]) -> Vec<Map<String, Value>> {
    let mut rows = Vec::new();
    for entity in entities {
        let Some(object) = entity.as_object() else {
            continue;
        };
        let mut base = Map::new();
        for key in ["id", "type"] {
            if let Some(v) = object.get(key) {
                base.insert(key.to_owned(), v.clone());
            }
        }
        let mut series = false;
        for (attr, value) in object {
            if attr == "id" || attr == "type" || attr == "@context" {
                continue;
            }
            let instances: Vec<&Value> = match value {
                Value::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            for instance in instances {
                let Some(held) = instance.as_object() else {
                    continue;
                };
                if let Some(values) = held.get("values").and_then(Value::as_array) {
                    series = true;
                    for pair in values {
                        let mut row = base.clone();
                        row.insert(attr.clone(), pair[0].clone());
                        row.insert("observedAt".into(), pair[1].clone());
                        rows.push(row);
                    }
                }
                for (method, values) in held {
                    let Some(values) = values
                        .as_array()
                        .filter(|_| !["type", "values"].contains(&method.as_str()))
                    else {
                        continue;
                    };
                    series = true;
                    for triple in values {
                        let mut row = base.clone();
                        row.insert(format!("{attr}.{method}"), triple[0].clone());
                        row.insert(attr.clone(), triple[0].clone());
                        row.insert("observedAt".into(), triple[1].clone());
                        row.insert("method".into(), json!(method));
                        rows.push(row);
                    }
                }
            }
        }
        if !series {
            let mut row = base;
            for (attr, value) in object {
                // A normalized attribute holds its value under `value`.
                let value = value.get("value").unwrap_or(value);
                row.entry(attr.clone()).or_insert_with(|| value.clone());
            }
            rows.push(row);
        }
    }
    rows
}

/// The category values of one nominal channel, in order of first appearance.
#[derive(Default)]
struct Categories(Vec<String>);

impl Categories {
    fn index(&mut self, value: &Value) -> Option<usize> {
        let key = match value {
            Value::String(s) => s.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => return None,
        };
        Some(match self.0.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                self.0.push(key);
                self.0.len() - 1
            }
        })
    }
}

fn cell(value: Option<&Value>, scale: Scale, cats: &mut Categories) -> Option<Cell> {
    let value = value?;
    match scale {
        Scale::Quantitative => value.as_f64().filter(|v| v.is_finite()).map(Cell::Number),
        Scale::Temporal => match value {
            Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| Cell::Number(t.timestamp_millis() as f64)),
            other => other.as_f64().map(Cell::Number),
        },
        Scale::Nominal => cats.index(value).map(Cell::Category),
    }
}

/// The chart of `spec` over `rows`, and the category values it numbered.
fn chart_of(spec: &Spec, rows: &[Map<String, Value>]) -> (Chart, Value, bool) {
    let (mut xc, mut yc, mut cc) = (
        Categories::default(),
        Categories::default(),
        Categories::default(),
    );
    let mut points = Vec::new();
    for row in rows.iter().take(MAX_POINTS) {
        let get = |f: &Option<Field>| f.as_ref().and_then(|f| row.get(&f.field));
        let x = spec
            .x
            .as_ref()
            .and_then(|f| cell(row.get(&f.field), f.scale, &mut xc));
        let y = spec
            .y
            .as_ref()
            .and_then(|f| cell(row.get(&f.field), f.scale, &mut yc));
        let color = get(&spec.color).and_then(|v| cc.index(v));
        let size = get(&spec.size).and_then(Value::as_f64);
        let theta = get(&spec.theta).and_then(Value::as_f64);
        let drawn = match spec.mark {
            Mark::Arc => theta.is_some(),
            Mark::Rule => x.is_some() || y.is_some(),
            _ => x.is_some() && y.is_some(),
        };
        if drawn {
            points.push(Point {
                x,
                y,
                color,
                size,
                theta,
            });
        }
    }
    let channel = |f: &Option<Field>, cats: &Categories| {
        f.as_ref().map(|f| Channel {
            scale: f.scale,
            title: f.title.clone(),
            categories: cats.0.len(),
        })
    };
    let mut categories = Map::new();
    for (name, cats) in [("x", &xc), ("y", &yc), ("color", &cc)] {
        if !cats.0.is_empty() {
            let numbered: Vec<Value> = cats
                .0
                .iter()
                .enumerate()
                .map(|(i, v)| json!({ "label": format!("#{}", i + 1), "value": v }))
                .collect();
            categories.insert(name.to_owned(), Value::Array(numbered));
        }
    }
    let chart = Chart {
        mark: spec.mark,
        width: spec.width,
        height: spec.height,
        title: spec.title.clone(),
        x: channel(&spec.x, &xc),
        y: channel(&spec.y, &yc),
        colors: cc.0.len(),
        points,
    };
    (chart, Value::Object(categories), rows.len() > MAX_POINTS)
}

fn limited() -> OpError {
    OpError::Api(ApiError::TooManyRequests(format!(
        "limited: the render ran past its {} s cap; read fewer rows (limit, a shorter temporal window, an aggregation) or ask for a smaller picture",
        TIME_CAP.as_secs()
    )))
}

/// Draws `spec` over `rows` within `deadline` in a blocking thread; the PNG or `limited`.
async fn draw(
    spec: &Spec,
    rows: &[Map<String, Value>],
    deadline: Instant,
) -> Result<(Vec<u8>, Value, bool), OpError> {
    let (chart, categories, truncated) = chart_of(spec, rows);
    let png = tokio::task::spawn_blocking(move || chart::render(&chart, deadline))
        .await
        .map_err(|err| ApiError::Internal(format!("the render stopped: {err}")))?
        .map_err(|chart::Limited| limited())?;
    if png.len() > MAX_PNG {
        return Err(invalid(
            "/width",
            "the picture is larger than 2 MiB; ask for a smaller width and height",
        ));
    }
    Ok((png, categories, truncated))
}

async fn run(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: Value,
) -> Result<Value, OpError> {
    let spec = spec_of(&input)?;
    let deadline = Instant::now() + TIME_CAP;
    let work = async {
        let _slot = SLOTS
            .acquire()
            .await
            .map_err(|_| ApiError::Unavailable("the renderer is shutting down".into()))?;
        let endpoint = spec.query.endpoint.clone();
        let temporal = spec.query.temporal.is_some();
        let entity_type = spec.query.entity_type.clone();
        let read = data_query::run(caller, state, project, spec.query.clone()).await?;
        let entities = read["entities"].as_array().cloned().unwrap_or_default();
        let rows = rows_of(&entities);
        let (png, categories, truncated) = draw(&spec, &rows, deadline).await?;
        Ok::<_, OpError>(json!({
            "image": {
                "mimeType": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(&png),
            },
            "width": spec.width,
            "height": spec.height,
            "mark": mark_name(spec.mark),
            "rows": rows.len(),
            "truncated": truncated,
            "categories": categories,
            "sources": [{ "endpoint": endpoint, "type": entity_type, "temporal": temporal }],
            "untrusted": true,
        }))
    };
    tokio::time::timeout(TIME_CAP, work)
        .await
        .map_err(|_| limited())?
}

fn input_schema() -> Value {
    let field = json!({
        "type": "object",
        "properties": {
            "field": { "type": "string", "description": "A row member, e.g. pm10, observedAt or id" },
            "type": { "type": "string", "enum": ["quantitative", "temporal", "nominal"] },
            "title": { "type": "string", "maxLength": MAX_TITLE, "description": "The axis title, e.g. PM10 (µg/m³)" }
        },
        "required": ["field", "type"],
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "description": "A Vega-Lite 5 subset. data.query is a jc_data_query read; no url, no transform, no expression",
        "properties": {
            "$schema": { "type": "string", "description": "https://vega.github.io/schema/vega-lite/v5.json, never fetched" },
            "title": { "type": "string", "maxLength": MAX_TITLE, "description": "Drawn above the chart, e.g. PM10 at Kallio" },
            "description": { "type": "string", "maxLength": MAX_TITLE },
            "width": { "type": "integer", "minimum": MIN_SIDE, "maximum": MAX_SIDE, "description": "Pixels, 800 when absent" },
            "height": { "type": "integer", "minimum": MIN_SIDE, "maximum": MAX_SIDE, "description": "Pixels, 450 when absent" },
            "mark": { "type": "string", "enum": ["line", "bar", "area", "point", "arc", "rule"] },
            "encoding": {
                "type": "object",
                "properties": { "x": field, "y": field, "color": field, "size": field, "theta": field, "tooltip": { "description": "Accepted and ignored" } },
                "additionalProperties": false
            },
            "data": {
                "type": "object",
                "properties": { "query": { "type": "object", "description": "The jc_data_query arguments, e.g. {\"endpoint\": \"air\", \"type\": \"AirQualityObserved\", \"attrs\": [\"pm10\"], \"temporal\": {\"timerel\": \"after\", \"timeAt\": \"2026-11-13T00:00:00Z\"}}" } },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        "required": ["mark", "data"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "width": { "type": "integer" },
            "height": { "type": "integer" },
            "mark": { "type": "string" },
            "rows": { "type": "integer" },
            "truncated": { "type": "boolean" },
            "categories": { "type": "object", "description": "The numbered category values the picture shows as #1, #2, ...: data other people wrote" },
            "sources": { "type": "array", "items": { "type": "object" } },
            "untrusted": { "type": "boolean" }
        },
        "required": ["rows", "sources", "untrusted"]
    })
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: NAME,
        title: "Render Chart",
        description: "Draws one Endpoint read (a jc_data_query) as a PNG chart from a Vega-Lite subset: marks line, bar, area, point, arc, rule",
        input: input_schema,
        output: output_schema,
        annotations: OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        },
        kind: "Endpoint",
        verb: None,
        lane: Lane::Green,
        validate: |val| spec_of(val).map(|_| ()),
        run: |caller, state, project, val| Box::pin(async move { run(caller, state, project, val).await }),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(mark: &str) -> Value {
        json!({
            "title": "PM10",
            "mark": mark,
            "encoding": {
                "x": { "field": "observedAt", "type": "temporal" },
                "y": { "field": "pm10", "type": "quantitative" },
                "color": { "field": "id", "type": "nominal" },
                "theta": { "field": "pm10", "type": "quantitative" }
            },
            "data": { "query": { "endpoint": "air", "type": "AirQualityObserved", "attrs": ["pm10"],
                                 "temporal": { "timerel": "after", "timeAt": "2026-11-13T00:00:00Z" } } }
        })
    }

    fn refused_at(input: &Value) -> String {
        match spec_of(input) {
            Err(OpError::InvalidInput { path, .. }) => path,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_spec_with_a_url_or_an_expression_is_refused() {
        let with = |pointer: &str, value: Value| {
            let mut input = spec("line");
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            input
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), value);
            input
        };
        let cases = [
            ("/data/url", json!("https://evil.example/rows.json")),
            ("/data/values", json!([{ "pm10": 1 }])),
            (
                "/transform",
                json!([{ "calculate": "datum.pm10 * 2", "as": "x" }]),
            ),
            ("/params", json!([{ "name": "p", "expr": "1" }])),
            (
                "/encoding/href",
                json!({ "field": "url", "type": "nominal" }),
            ),
            (
                "/encoding/x/condition",
                json!({ "test": "datum.x", "value": 1 }),
            ),
            ("/encoding/y/scale", json!({ "type": "log" })),
            ("/encoding/url", json!({ "field": "u", "type": "nominal" })),
            ("/config", json!({})),
            ("/layer", json!([])),
            ("/data/query/url", json!("https://evil.example")),
        ];
        for (pointer, value) in cases {
            assert_eq!(refused_at(&with(pointer, value)), pointer, "{pointer}");
        }
        let mut image = spec("line");
        image["mark"] = json!({ "type": "image", "url": "https://evil.example/x.png" });
        assert_eq!(refused_at(&image), "/mark/url");
        image["mark"] = json!("image");
        assert_eq!(refused_at(&image), "/mark");
        let mut schema = spec("line");
        schema["$schema"] = json!("https://evil.example/schema.json");
        assert_eq!(refused_at(&schema), "/$schema");
    }

    #[test]
    fn each_mark_of_the_subset_is_accepted_and_needs_its_channels() {
        for mark in ["line", "bar", "area", "point", "arc", "rule"] {
            assert!(spec_of(&spec(mark)).is_ok(), "{mark}");
        }
        let mut no_y = spec("bar");
        no_y["encoding"].as_object_mut().unwrap().remove("y");
        assert_eq!(refused_at(&no_y), "/encoding");
        let mut big = spec("line");
        big["width"] = json!(4001);
        assert_eq!(refused_at(&big), "/width");
        let mut counted = spec("line");
        counted["data"]["query"]["count"] = json!(true);
        assert_eq!(refused_at(&counted), "/data/query/count");
    }

    #[test]
    fn history_rows_flatten_to_one_row_per_value() {
        let entities = [
            json!({
                "id": "urn:ngsi-ld:AirQualityObserved:kallio", "type": "AirQualityObserved",
                "pm10": { "type": "Property", "values": [[12.5, "2026-11-13T01:00:00Z"], [14.0, "2026-11-13T02:00:00Z"]] }
            }),
            json!({
                "id": "urn:ngsi-ld:AirQualityObserved:mannerheimintie", "type": "AirQualityObserved",
                "pm10": { "type": "Property", "avg": [[20.0, "2026-11-13T00:00:00Z", "2026-11-13T01:00:00Z"]] }
            }),
        ];
        let rows = rows_of(&entities);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["pm10"], json!(12.5));
        assert_eq!(rows[1]["observedAt"], json!("2026-11-13T02:00:00Z"));
        assert_eq!(rows[2]["pm10.avg"], json!(20.0));
        let current = rows_of(&[json!({ "id": "urn:x", "type": "T", "pm10": 3 })]);
        assert_eq!(current[0]["pm10"], json!(3));
    }

    /// The pixels of a PNG this module wrote (no filter, one zlib stream).
    fn pixels(png: &[u8]) -> Vec<u8> {
        use std::io::Read as _;
        let mut at = 8;
        let mut idat = Vec::new();
        while at + 8 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            if &png[at + 4..at + 8] == b"IDAT" {
                idat.extend_from_slice(&png[at + 8..at + 8 + len]);
            }
            at += 12 + len;
        }
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&idat[..])
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    /// One chart per mark against its golden PNG in tests/fixtures/render/, within 1 % of the
    /// pixels. `JC_RENDER_GOLDEN=write` writes the goldens anew.
    #[tokio::test]
    async fn each_mark_matches_its_golden_picture() {
        let entities: Vec<Value> = ["kallio", "vallila"]
            .iter()
            .enumerate()
            .map(|(n, place)| json!({
                "id": format!("urn:ngsi-ld:AirQualityObserved:{place}"), "type": "AirQualityObserved",
                "pm10": { "type": "Property", "values": (0..12).map(|h| json!([10.0 + (n * 7) as f64 + f64::from(h % 5) * 2.5, format!("2026-11-13T{h:02}:00:00Z")])).collect::<Vec<_>>() }
            }))
            .collect();
        let rows = rows_of(&entities);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render");
        for mark in ["line", "bar", "area", "point", "arc", "rule"] {
            let mut input = spec(mark);
            input["width"] = json!(400);
            input["height"] = json!(240);
            if mark == "bar" || mark == "arc" {
                input["encoding"]["x"] = json!({ "field": "id", "type": "nominal" });
            }
            let parsed = spec_of(&input).expect("a spec");
            let (png, categories, _) = draw(&parsed, &rows, Instant::now() + TIME_CAP)
                .await
                .expect("drawn");
            assert_eq!(
                categories["color"][1]["value"],
                "urn:ngsi-ld:AirQualityObserved:vallila"
            );
            let golden = dir.join(format!("{mark}.png"));
            if std::env::var("JC_RENDER_GOLDEN").as_deref() == Ok("write") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&golden, &png).unwrap();
                continue;
            }
            let want =
                pixels(&std::fs::read(&golden).expect("golden; write with JC_RENDER_GOLDEN=write"));
            let got = pixels(&png);
            assert_eq!(want.len(), got.len(), "{mark}");
            let off = want
                .iter()
                .zip(&got)
                .filter(|(a, b)| a.abs_diff(**b) > 16)
                .count();
            assert!(
                off * 100 <= got.len(),
                "{mark}: {off} of {} bytes differ",
                got.len()
            );
        }
    }

    #[tokio::test]
    async fn a_render_over_the_time_cap_is_limited() {
        let parsed = spec_of(&spec("line")).expect("a spec");
        let rows = rows_of(&[
            json!({ "id": "urn:x", "type": "T", "pm10": { "values": [[1, "2026-11-13T00:00:00Z"]] } }),
        ]);
        let refused = draw(&parsed, &rows, Instant::now() - Duration::from_millis(1))
            .await
            .expect_err("past the cap");
        assert!(
            matches!(&refused, OpError::Api(ApiError::TooManyRequests(text)) if text.starts_with("limited")),
            "{refused}"
        );
    }
}
