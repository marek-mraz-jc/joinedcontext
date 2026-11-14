//! The closed subset of Vega-Lite 5 both chart doors take (T-6860, T-6881): marks line, bar,
//! area, point, arc, rule; channels x, y, color, size, theta; tooltip accepted and ignored. Every
//! member outside the subset is refused naming it: no `url`, no expression, no `transform`, no
//! `image` mark, no `href`. A [`Door`] says how its own tool names itself in a refusal and how
//! much one call may draw; the rows come flat ([`rows_of`]) and only channel fields reach a
//! chart or its view points.

use crate::raster::{Cell, Channel, Chart, Mark, Point, Scale};
use serde_json::{json, Map, Value};

/// A title, an axis title or a description: at most this many characters on one line.
pub const MAX_TITLE: usize = 120;
/// The smallest picture side, in pixels.
pub const MIN_SIDE: u32 = 50;

/// One door's words and caps: the tool's name and where its read is named (refusals say both),
/// the largest side, the rows a chart draws and the points its view gets.
#[derive(Debug, Clone, Copy)]
pub struct Door {
    pub tool: &'static str,
    pub read: &'static str,
    /// The widest `width`, in pixels.
    pub max_side: u32,
    /// The tallest `height`, in pixels.
    pub max_height: u32,
    pub max_points: usize,
    pub max_view_points: usize,
}

/// Why a spec is refused: the JSON pointer of the member and what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Invalid {
        path: String,
        message: String,
    },
    /// A member outside the subset at `path`, its siblings `allowed`.
    Member {
        path: String,
        key: String,
        allowed: Vec<&'static str>,
    },
}

impl Refusal {
    /// The member it names and the words a caller of `door` can act on.
    pub fn describe(&self, door: &Door) -> (String, String) {
        match self {
            Self::Invalid { path, message } => (path.clone(), message.clone()),
            Self::Member { path, key, allowed } => {
                let (tool, read) = (door.tool, door.read);
                let why = match key.as_str() {
                    "url" | "href" | "values" | "name" | "sequence" | "graticule" | "sphere" => {
                        format!("{tool} reads no URL and no inline rows: name the Endpoint read in {read}")
                    }
                    "transform" | "params" | "selection" | "calculate" | "filter" | "expr"
                    | "signal" | "condition" => {
                        format!(
                            "{tool} evaluates no expression; filter in {read} (q, attrs, temporal)"
                        )
                    }
                    _ => format!(
                        "not in the {tool} subset of Vega-Lite; allowed here: {}",
                        allowed.join(", ")
                    ),
                };
                (path.clone(), why)
            }
        }
    }
}

pub const CHANNELS: &[&str] = &["x", "y", "color", "size", "theta", "tooltip"];
pub const FIELD: &[&str] = &["field", "type", "title"];

pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Refusal {
    Refusal::Invalid {
        path: path.into(),
        message: message.into(),
    }
}

/// A member outside the subset at `at`, its siblings `allowed`.
pub fn refused_member(at: &str, key: &str, allowed: &[&'static str]) -> Refusal {
    Refusal::Member {
        path: format!("{at}/{key}"),
        key: key.to_owned(),
        allowed: allowed.to_vec(),
    }
}

/// `value` as an object holding only `allowed` members.
pub fn members<'a>(
    value: &'a Value,
    at: &str,
    allowed: &[&'static str],
) -> Result<&'a Map<String, Value>, Refusal> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(if at.is_empty() { "/" } else { at }, "an object"))?;
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(refused_member(at, key, allowed)),
        None => Ok(object),
    }
}

pub fn text(value: &Value, at: &str) -> Result<String, Refusal> {
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
pub struct Field {
    pub field: String,
    pub scale: Scale,
    pub title: String,
}

pub fn field_name(value: &Value, at: &str) -> Result<String, Refusal> {
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

pub fn field(value: &Value, at: &str) -> Result<Field, Refusal> {
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

/// A checked spec: what to draw.
#[derive(Debug, Clone)]
pub struct Spec {
    pub mark: Mark,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub x: Option<Field>,
    pub y: Option<Field>,
    pub color: Option<Field>,
    pub size: Option<Field>,
    pub theta: Option<Field>,
}

impl Spec {
    /// The row members its channels read.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        [&self.x, &self.y, &self.color, &self.size, &self.theta]
            .into_iter()
            .flatten()
            .map(|f| f.field.as_str())
    }
}

pub fn side(value: Option<&Value>, at: &str, default: u32, max: u32) -> Result<u32, Refusal> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| (MIN_SIDE..=max).contains(v))
        .ok_or_else(|| invalid(at, format!("pixels, {MIN_SIDE} to {max}")))
}

pub fn mark_of(value: Option<&Value>, at: &str) -> Result<Mark, Refusal> {
    let (name, at) = match value {
        Some(Value::String(name)) => (name.as_str(), format!("{at}/mark")),
        Some(object @ Value::Object(_)) => {
            let object = members(object, &format!("{at}/mark"), &["type"])?;
            (
                object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                format!("{at}/mark/type"),
            )
        }
        _ => ("", format!("{at}/mark")),
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

/// A `$schema` is only named, never fetched: Vega-Lite 5 or none.
pub fn schema_of(top: &Map<String, Value>, at: &str) -> Result<(), Refusal> {
    let ok = top.get("$schema").is_none_or(|schema| {
        schema
            .as_str()
            .is_some_and(|s| s.starts_with("https://vega.github.io/schema/vega-lite/v5"))
    });
    if ok {
        Ok(())
    } else {
        Err(invalid(
            format!("{at}/$schema"),
            "https://vega.github.io/schema/vega-lite/v5.json or none",
        ))
    }
}

/// The drawing members of `top` (everything but `data`), each refusal under `at`; `width` and
/// `height` default to `size`.
pub fn look_of(
    top: &Map<String, Value>,
    at: &str,
    size: (u32, u32),
    door: &Door,
) -> Result<Spec, Refusal> {
    schema_of(top, at)?;
    let title = match top.get("title") {
        Some(title) => text(title, &format!("{at}/title"))?,
        None => String::new(),
    };
    if let Some(description) = top.get("description") {
        text(description, &format!("{at}/description"))?;
    }
    let mark = mark_of(top.get("mark"), at)?;
    let empty = json!({});
    let encoding = members(
        top.get("encoding").unwrap_or(&empty),
        &format!("{at}/encoding"),
        CHANNELS,
    )?;
    let channel = |name: &str| -> Result<Option<Field>, Refusal> {
        encoding
            .get(name)
            .map(|v| field(v, &format!("{at}/encoding/{name}")))
            .transpose()
    };
    let (x, y, color, size_channel, theta) = (
        channel("x")?,
        channel("y")?,
        channel("color")?,
        channel("size")?,
        channel("theta")?,
    );
    let needs = |what: &str| {
        invalid(
            format!("{at}/encoding"),
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
    for (name, channel) in [("size", &size_channel), ("theta", &theta)] {
        if channel
            .as_ref()
            .is_some_and(|c| c.scale != Scale::Quantitative)
        {
            return Err(invalid(
                format!("{at}/encoding/{name}/type"),
                "quantitative",
            ));
        }
    }
    Ok(Spec {
        mark,
        title,
        width: side(
            top.get("width"),
            &format!("{at}/width"),
            size.0,
            door.max_side,
        )?,
        height: side(
            top.get("height"),
            &format!("{at}/height"),
            size.1,
            door.max_height,
        )?,
        x,
        y,
        color,
        size: size_channel,
        theta,
    })
}

pub fn mark_name(mark: Mark) -> &'static str {
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
pub fn rows_of(entities: &[Value]) -> Vec<Map<String, Value>> {
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

/// The category values of one nominal channel, in order of first appearance, with their index
/// by value so a read of 20,000 distinct values stays linear.
#[derive(Default)]
pub struct Categories(Vec<String>, std::collections::HashMap<String, usize>);

impl Categories {
    fn index(&mut self, value: &Value) -> Option<usize> {
        let key = match value {
            Value::String(s) => s.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => return None,
        };
        let next = self.0.len();
        let at = *self.1.entry(key.clone()).or_insert(next);
        if at == next {
            self.0.push(key);
        }
        Some(at)
    }
}

pub fn cell(value: Option<&Value>, scale: Scale, cats: &mut Categories) -> Option<Cell> {
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

pub fn scale_name(scale: Scale) -> &'static str {
    match scale {
        Scale::Quantitative => "quantitative",
        Scale::Temporal => "temporal",
        Scale::Nominal => "nominal",
    }
}

/// A chart and what the answer says of it.
pub struct Charted {
    pub chart: Chart,
    /// The category values it numbered, by channel.
    pub categories: Value,
    /// More rows than `door.max_points` were read.
    pub truncated: bool,
    /// The drawn points as the chart view reads them: only the channels' numbers and the time.
    pub view_points: Vec<Value>,
}

/// One drawn point for the chart view: its channel values as numbers (a category as its index,
/// a time as epoch milliseconds) and the row's `observedAt` as RFC 3339, nothing else of the row.
pub fn view_point(point: &Point, row: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    let cells = [("x", point.x), ("y", point.y)];
    for (name, cell) in cells {
        if let Some(cell) = cell {
            let v = match cell {
                Cell::Number(v) => json!(v),
                Cell::Category(i) => json!(i),
            };
            out.insert(name.into(), v);
        }
    }
    if let Some(color) = point.color {
        out.insert("color".into(), json!(color));
    }
    for (name, v) in [("size", point.size), ("theta", point.theta)] {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            out.insert(name.into(), json!(v));
        }
    }
    let observed = row
        .get("observedAt")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    if let Some(t) = observed {
        let utc = t.with_timezone(&chrono::Utc);
        out.insert(
            "observedAt".into(),
            json!(utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        );
    }
    Value::Object(out)
}

/// What the chart view draws, beside the PNG: the checked spec, at most `door.max_view_points`
/// points and whether some were left out. The answer carries it in `_meta`, never in
/// `structuredContent` or the text (T-6866).
pub fn view_of(spec: &Spec, charted: &Charted, door: &Door) -> Value {
    let channel = |f: &Option<Field>| {
        f.as_ref()
            .map(|f| json!({ "scale": scale_name(f.scale), "title": f.title }))
    };
    let points: Vec<&Value> = charted
        .view_points
        .iter()
        .take(door.max_view_points)
        .collect();
    json!({
        "joinedcontext.com/chartSpec": {
            "mark": mark_name(spec.mark),
            "title": spec.title,
            "width": spec.width,
            "height": spec.height,
            "x": channel(&spec.x),
            "y": channel(&spec.y),
            "color": channel(&spec.color),
            "size": channel(&spec.size),
            "theta": channel(&spec.theta),
        },
        "joinedcontext.com/chartPoints": points,
        "joinedcontext.com/pointsTruncated":
            charted.truncated || charted.view_points.len() > door.max_view_points,
    })
}

/// The chart of `spec` over `rows`, the category values it numbered and its view points.
pub fn chart_of(spec: &Spec, rows: &[Map<String, Value>], door: &Door) -> Charted {
    let (mut xc, mut yc, mut cc) = (
        Categories::default(),
        Categories::default(),
        Categories::default(),
    );
    let mut points = Vec::new();
    let mut view_points = Vec::new();
    for row in rows.iter().take(door.max_points) {
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
            let point = Point {
                x,
                y,
                color,
                size,
                theta,
            };
            // One more than the view takes, so the view knows some were left out.
            if view_points.len() <= door.max_view_points {
                view_points.push(view_point(&point, row));
            }
            points.push(point);
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
    Charted {
        chart,
        categories: Value::Object(categories),
        truncated: rows.len() > door.max_points,
        view_points,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOOR: Door = Door {
        tool: "render_chart",
        read: "data.arguments",
        max_side: 1200,
        max_height: 800,
        max_points: 3,
        max_view_points: 2,
    };

    fn look(spec: Value) -> Result<Spec, (String, String)> {
        let top = spec.as_object().expect("an object");
        look_of(top, "/spec", (800, 450), &DOOR).map_err(|r| r.describe(&DOOR))
    }

    #[test]
    fn each_member_outside_the_subset_is_refused_in_the_doors_words() {
        let line = json!({ "mark": "line", "encoding": {
            "x": { "field": "observedAt", "type": "temporal" },
            "y": { "field": "pm10", "type": "quantitative" } } });
        assert!(look(line.clone()).is_ok());
        let cases = [
            (
                "/encoding/x/url",
                json!({ "field": "a", "type": "nominal", "url": "x" }),
                "reads no URL",
            ),
            (
                "/encoding/x/transform",
                json!({ "field": "a", "type": "nominal", "transform": [] }),
                "evaluates no expression",
            ),
            (
                "/encoding/x/params",
                json!({ "field": "a", "type": "nominal", "params": [] }),
                "evaluates no expression",
            ),
            (
                "/encoding/x/href",
                json!({ "field": "a", "type": "nominal", "href": "x" }),
                "reads no URL",
            ),
            (
                "/encoding/x/aggregate",
                json!({ "field": "a", "type": "nominal", "aggregate": "sum" }),
                "subset",
            ),
        ];
        for (at, x, words) in cases {
            let mut spec = line.clone();
            spec["encoding"]["x"] = x;
            let (path, message) = look(spec).expect_err(at);
            assert_eq!(path, format!("/spec{at}"));
            assert!(message.contains("render_chart"), "{message}");
            assert!(message.contains(words), "{message}");
        }
        let mut image = line.clone();
        image["mark"] = json!("image");
        assert_eq!(look(image).expect_err("image").0, "/spec/mark");
        let mut wide = line;
        wide["width"] = json!(1201);
        assert!(look(wide).expect_err("wide").1.contains("50 to 1200"));
    }

    #[test]
    fn a_chart_keeps_the_doors_caps_and_only_channel_fields() {
        let spec = look(json!({ "mark": "bar", "encoding": {
            "x": { "field": "name", "type": "nominal" },
            "y": { "field": "pm10", "type": "quantitative" } } }))
        .expect("a bar");
        assert_eq!(spec.fields().collect::<Vec<_>>(), ["name", "pm10"]);
        let rows = rows_of(
            &(0..5)
                .map(|i| {
                    json!({ "id": format!("urn:x:{i}"), "type": "T",
            "name": format!("n{i}"), "pm10": i, "secret": "s" })
                })
                .collect::<Vec<_>>(),
        );
        let charted = chart_of(&spec, &rows, &DOOR);
        assert!(charted.truncated);
        assert_eq!(charted.chart.points.len(), 3);
        let view = view_of(&spec, &charted, &DOOR);
        assert_eq!(
            view["joinedcontext.com/chartPoints"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(view["joinedcontext.com/pointsTruncated"], json!(true));
        assert!(!view.to_string().contains("secret") && !view.to_string().contains("urn:x"));
    }
}
