//! `get_schema` over the platform's kinds (AG-146, T-5948): the JSON Schema jc-core exports for a
//! kind, the same document `GET /api/v1/kinds/{kind}/schema`, the Portal forms and `jcctl validate`
//! read, so an MCP client learns a kind's fields in one call instead of guessing them.
//!
//! T-6057: the default answer is compact, because the whole schema was 70-89 % of the bytes a
//! connector client read (docs Testing/09): the kind's required fields, one line per field
//! (`spec.class: one of auto|resident|scheduled, required`) and one minimal manifest that passes
//! jc-core's validation. `path`, a JSON pointer into the manifest, answers that one field's whole
//! JSON Schema with the definitions it names.
//!
//! Decided (worker-t5948, owner may override): the schema is served in jc-core's own draft-07
//! dialect, because jc-core pins draft-07 (schema_export_tests) and every other reader takes that
//! document; a 2020-12 rewrite here would be a second copy that can drift. A 2020-12 validator
//! reads it by its `$schema`.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{annotations, parse_input, OpError, Operation};
use crate::change::Lane;
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaInput {
    kind: String,
    #[serde(default)]
    path: Option<String>,
}

/// Every kind jc-core registers, in its order.
pub fn kinds() -> Vec<&'static str> {
    jc_core::registry::KINDS
        .iter()
        .map(|info| info.kind)
        .collect()
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "description": "Kind", "examples": ["Pipeline"], "enum": kinds() },
            "path": { "type": "string", "description": "JSON pointer", "maxLength": MAX_PATH }
        },
        "required": ["kind"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "required": { "type": "array", "items": { "type": "string" } },
            "fields": { "type": "string" },
            "example": { "type": "object" },
            "more": { "type": "string" },
            "note": { "type": "string" },
            "path": { "type": "string" },
            "schema": { "type": "object" }
        },
        "required": ["kind"]
    })
}

/// The answer for one kind: compact, or one field's schema with `path`; an unknown kind is 404,
/// as the REST door answers it, and a path naming no field is 422 on `/path`.
pub fn schema_of(kind: &str, path: Option<&str>) -> Result<Value, OpError> {
    let schema = jc_core::registry::schema_of(kind).ok_or_else(|| {
        OpError::Api(ApiError::NotFound(format!(
            "the platform has no kind named `{kind}`"
        )))
    })?;
    match path {
        None => Ok(compact(kind, &schema)),
        Some(path) => {
            Ok(json!({ "kind": kind, "path": path, "schema": field_schema(&schema, path)? }))
        }
    }
}

fn bad_path(message: String) -> OpError {
    OpError::InvalidInput {
        path: "/path".into(),
        message,
    }
}

/// The schema of the field `pointer` names, with every definition it reaches.
fn field_schema(schema: &Value, pointer: &str) -> Result<Value, OpError> {
    if pointer.len() > MAX_PATH {
        return Err(bad_path(format!("a path is at most {MAX_PATH} bytes")));
    }
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(bad_path(format!(
            "`{pointer}` is no JSON pointer: start it with /, e.g. /spec/source"
        )));
    }
    if pointer.is_empty() {
        return Ok(schema.clone());
    }
    let defs = defs_of(schema);
    let mut at = schema;
    for raw in pointer.split('/').skip(1) {
        let step = raw.replace("~1", "/").replace("~0", "~");
        let here = resolve(&defs, at);
        let next = here
            .get("properties")
            .and_then(|p| p.get(&step))
            .or_else(|| {
                (types(here).contains(&"array") && step.parse::<usize>().is_ok())
                    .then(|| here.get("items"))
                    .flatten()
            })
            .or_else(|| here.get("additionalProperties").filter(|a| a.is_object()));
        at = next.ok_or_else(|| {
            bad_path(format!(
                "`{pointer}` names no field of the manifest: `{step}` is not a member there; the field lines name every member"
            ))
        })?;
    }
    let mut reached = Map::new();
    let mut todo = vec![at];
    while let Some(next) = todo.pop() {
        collect_refs(next, &mut |name| {
            if !reached.contains_key(name) {
                if let Some(def) = defs.get(name) {
                    reached.insert(name.to_owned(), def.clone());
                    todo.push(def);
                }
            }
        });
    }
    let mut out = at.clone();
    if !reached.is_empty() {
        out = json!({ "allOf": [out], "definitions": reached });
    }
    Ok(out)
}

/// Calls `found` with the definition name of every `#/definitions/...` reference in `schema`.
fn collect_refs<'a>(schema: &'a Value, found: &mut impl FnMut(&'a str)) {
    match schema {
        Value::Object(members) => {
            for (key, value) in members {
                match (key.as_str(), value.as_str()) {
                    ("$ref", Some(target)) => {
                        if let Some(name) = target.strip_prefix("#/definitions/") {
                            found(name);
                        }
                    }
                    _ => collect_refs(value, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_refs(item, found)),
        _ => {}
    }
}

/// The byte budget of a kind's field lines: the whole default answer stays under 4,000 bytes.
const FIELD_BYTES: usize = 2_400;
/// How deep the field lines go at most; a deeper field is read with `path`.
const MAX_DEPTH: usize = 4;
/// Enum values a line lists before it ends in `…`.
const MAX_ENUM: usize = 8;
/// A pattern longer than this is left to the field's own schema.
const MAX_PATTERN: usize = 48;
/// The longest `path` the tool takes (a JSON pointer).
const MAX_PATH: usize = 256;

type Defs = Map<String, Value>;

fn defs_of(schema: &Value) -> Defs {
    schema
        .get("definitions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Follows `$ref`, a lone `allOf` and an `anyOf` of one schema and null to the schema they wrap.
fn resolve<'a>(defs: &'a Defs, mut schema: &'a Value) -> &'a Value {
    for _ in 0..16 {
        if let Some(target) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/definitions/"))
            .and_then(|name| defs.get(name))
        {
            schema = target;
        } else if let Some([one]) = schema
            .get("allOf")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
        {
            schema = one;
        } else if let Some(one) = schema
            .get("anyOf")
            .and_then(Value::as_array)
            .and_then(|alts| lone_non_null(alts))
        {
            schema = one;
        } else {
            break;
        }
    }
    schema
}

fn lone_non_null(alts: &[Value]) -> Option<&Value> {
    let mut rest = alts
        .iter()
        .filter(|alt| alt.get("type").and_then(Value::as_str) != Some("null"));
    match (rest.next(), rest.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

fn types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => Vec::new(),
    }
}

fn one_of(values: &[Value]) -> String {
    let mut shown: Vec<String> = values
        .iter()
        .take(MAX_ENUM)
        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
        .collect();
    if values.len() > MAX_ENUM {
        shown.push("…".into());
    }
    format!("one of {}", shown.join("|"))
}

/// The values of a unit-variant enum written as `oneOf` alternatives, if it is one.
fn variant_values(defs: &Defs, alts: &[Value]) -> Option<Vec<Value>> {
    let mut values = Vec::new();
    for alt in alts {
        let alt = resolve(defs, alt);
        if let Some(listed) = alt.get("enum").and_then(Value::as_array) {
            values.extend(listed.iter().cloned());
        } else if let Some(inner) = alt
            .get("oneOf")
            .or_else(|| alt.get("anyOf"))
            .and_then(Value::as_array)
        {
            values.extend(variant_values(defs, inner)?);
        } else {
            values.push(alt.get("const")?.clone());
        }
    }
    Some(values)
}

/// A field's type in one word or a few: `string`, `integer[]`, `map of string`, `one of a|b`.
fn type_text(defs: &Defs, schema: &Value, depth: usize) -> String {
    let schema = resolve(defs, schema);
    if depth > MAX_DEPTH {
        return "any".into();
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return one_of(values);
    }
    if let Some(value) = schema.get("const") {
        return one_of(std::slice::from_ref(value));
    }
    if let Some(alts) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        if let Some(values) = variant_values(defs, alts) {
            return one_of(&values);
        }
        let mut seen: Vec<String> = Vec::new();
        for alt in alts {
            if types(alt) == ["null"] || alt.get("type") == Some(&json!("null")) {
                continue;
            }
            let text = type_text(defs, alt, depth + 1);
            if !seen.contains(&text) {
                seen.push(text);
            }
        }
        return seen.join(" | ");
    }
    match types(schema).as_slice() {
        ["array"] => {
            let items = type_text(defs, schema.get("items").unwrap_or(&Value::Null), depth + 1);
            if items.contains(' ') {
                format!("({items})[]")
            } else {
                format!("{items}[]")
            }
        }
        ["object"] if schema.get("properties").is_none() => {
            match schema.get("additionalProperties") {
                Some(inner @ Value::Object(_)) => {
                    format!("map of {}", type_text(defs, inner, depth + 1))
                }
                _ => "object".into(),
            }
        }
        [] => "any".into(),
        many => many.join(" | "),
    }
}

/// A field's limits as the schema states them: `maxLength 63`, `pattern ^[a-z]+$`.
fn limits_text(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minItems",
        "maxItems",
    ] {
        if let Some(value) = schema.get(key).and_then(Value::as_f64) {
            out.push(format!("{key} {value}"));
        }
    }
    // A Rust integer width (`uint32`, `int64`) says nothing a client can act on.
    if let Some(format) = schema.get("format").and_then(Value::as_str).filter(|f| {
        !f.starts_with("int") && !f.starts_with("uint") && *f != "double" && *f != "float"
    }) {
        out.push(format!("format {format}"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        if pattern.len() <= MAX_PATTERN {
            out.push(format!("pattern {pattern}"));
        }
    }
    out
}

/// The object schema a field's lines go on with: the field itself, or its array's items.
fn children<'a>(
    defs: &'a Defs,
    schema: &'a Value,
) -> Option<(&'a Map<String, Value>, Vec<&'a str>, bool)> {
    let schema = resolve(defs, schema);
    let (schema, array) = if types(schema).contains(&"array") {
        (resolve(defs, schema.get("items")?), true)
    } else {
        (schema, false)
    };
    let props = schema.get("properties")?.as_object()?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Some((props, required, array))
}

struct Line {
    path: String,
    parent: String,
    depth: usize,
    text: String,
    required: bool,
}

/// Every field line of one manifest, `spec` down to [`MAX_DEPTH`] levels, in tree order.
fn field_lines(defs: &Defs, schema: &Value) -> Vec<Line> {
    fn walk(
        defs: &Defs,
        schema: &Value,
        path: &str,
        chain: bool,
        depth: usize,
        out: &mut Vec<Line>,
    ) {
        let Some((props, required, array)) = children(defs, schema) else {
            return;
        };
        let base = if array {
            format!("{path}[]")
        } else {
            path.to_owned()
        };
        for (name, field) in props {
            let path = if base.is_empty() {
                name.clone()
            } else {
                format!("{base}.{name}")
            };
            let is_required = required.contains(&name.as_str());
            let mut parts = vec![type_text(defs, field, 0)];
            if is_required {
                parts.push("required".into());
            }
            parts.extend(limits_text(resolve(defs, field)));
            out.push(Line {
                text: format!("{path}: {}", parts.join(", ")),
                parent: base.clone(),
                depth,
                path: path.clone(),
                required: chain && is_required,
            });
            if depth < MAX_DEPTH {
                walk(defs, field, &path, chain && is_required, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    let props = schema.get("properties").cloned().unwrap_or_default();
    for name in ["apiVersion", "kind", "metadata", "spec"] {
        let mut one = json!({ "type": "object", "properties": {}, "required": [name] });
        one["properties"][name] = props.get(name).cloned().unwrap_or_default();
        walk(defs, &one, "", true, 0, &mut out);
    }
    out
}

/// The lines that fit [`FIELD_BYTES`]: the top two levels, then level by level each parent's
/// fields whole, so a listed object shows all its fields or none; and whether any were left out.
fn fitting(lines: &[Line]) -> (Vec<&Line>, bool) {
    let size = |l: &Line| l.text.len() + 1;
    let mut kept: Vec<bool> = lines.iter().map(|l| l.depth <= 1).collect();
    let mut used: usize = lines.iter().filter(|l| l.depth <= 1).map(size).sum();
    let mut cut = false;
    for depth in 2..=MAX_DEPTH {
        let mut parents: Vec<&str> = Vec::new();
        for line in lines.iter().filter(|l| l.depth == depth) {
            if !parents.contains(&line.parent.as_str()) {
                parents.push(&line.parent);
            }
        }
        for parent in parents {
            let shown = lines
                .iter()
                .zip(&kept)
                .any(|(l, k)| *k && l.path == parent.trim_end_matches("[]"));
            if !shown {
                continue;
            }
            let group: usize = lines
                .iter()
                .filter(|l| l.depth == depth && l.parent == parent)
                .map(size)
                .sum();
            if used + group > FIELD_BYTES {
                cut = true;
                continue;
            }
            used += group;
            for (line, keep) in lines.iter().zip(kept.iter_mut()) {
                if line.depth == depth && line.parent == parent {
                    *keep = true;
                }
            }
        }
    }
    let shown = lines
        .iter()
        .zip(kept)
        .filter(|(_, k)| *k)
        .map(|(l, _)| l)
        .collect();
    (shown, cut)
}

/// A field path of the lines (`spec.sources[].name`) as the JSON pointer `path` takes.
fn pointer_of(line_path: &str) -> String {
    format!("/{}", line_path.replace("[]", "/0").replace('.', "/"))
}

/// The default answer: the kind's required fields, one line per field, one minimal manifest.
fn compact(kind: &str, schema: &Value) -> Value {
    let defs = defs_of(schema);
    let lines = field_lines(&defs, schema);
    let (shown, cut) = fitting(&lines);
    let text: String = shown
        .iter()
        .map(|l| match minted(kind, &l.path) {
            true => format!("{} (minted by the platform: leave it out)\n", l.text),
            false => format!("{}\n", l.text),
        })
        .collect();
    let required: Vec<&str> = lines
        .iter()
        .filter(|l| l.required && !minted(kind, &l.path))
        .map(|l| l.path.as_str())
        .collect();
    // An object whose fields are not listed: the pointer the hint offers.
    let deeper = shown
        .iter()
        .find(|l| {
            l.path.starts_with("spec.")
                && lines
                    .iter()
                    .any(|c| c.parent.trim_end_matches("[]") == l.path)
                && !shown
                    .iter()
                    .any(|c| c.parent.trim_end_matches("[]") == l.path)
        })
        .map_or_else(|| "/spec".into(), |l| pointer_of(&l.path));
    let mut example = example_of(&defs, schema, 0);
    example["apiVersion"] = json!(jc_core::envelope::API_VERSION);
    example["kind"] = json!(kind);
    let namespace = match jc_core::registry::by_kind(kind).map(|info| info.scope) {
        Some(jc_core::envelope::Scope::Project) => "example-project",
        _ => jc_core::envelope::ORG_NAMESPACE,
    };
    example["metadata"] = json!({ "name": "example", "namespace": namespace });
    if let Some(patch) = EXAMPLES
        .iter()
        .find(|(of, _)| *of == kind)
        .and_then(|(_, patch)| serde_json::from_str::<Value>(patch).ok())
    {
        crate::plan::merge_patch(&mut example, &patch);
    }
    json!({
        "kind": kind,
        "required": required,
        "fields": text,
        "example": example,
        "more": format!(
            "{}a field's whole JSON Schema, limits and description: call again with path, a JSON pointer such as {deeper} (an array's items are /0)",
            if cut { "more fields than fit here; " } else { "" }
        ),
    })
}

/// A field the platform writes and a caller leaves out: an Endpoint's slug (EP-75).
fn minted(kind: &str, path: &str) -> bool {
    kind == "Endpoint" && path == "spec.slug"
}

/// What the schema cannot say: rules jc-core's validation holds (a semantic version, a slug's
/// entropy, a reference that names something), as a JSON merge patch onto the generated manifest.
/// Every value names a placeholder, never a real project's object.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "Organization",
        r#"{"spec":{"domain":"example.org","defaultLocale":"en","locales":["en"]}}"#,
    ),
    (
        "DataModel",
        r#"{"spec":{"linkml":"example.linkml.yaml","version":"1.0.0"}}"#,
    ),
    (
        "Mapping",
        r#"{"spec":{"source":{"version":"1"},"target":{"name":"example-target","version":"1"},"transformation":{"class_derivations":{"Example":{"populated_from":"Example"}}},"tests":[{"input":"{}","expect":"{}"}]}}"#,
    ),
    (
        "Policy",
        r#"{"spec":{"assigner":"did:web:{orgDomain}","assignee":{"kind":"group","id":"example-group"},"operations":["retrieveOps"]}}"#,
    ),
    (
        "ScopeDefinition",
        r#"{"spec":{"scopeString":"/domain/example","isRoot":false,"isChildOf":"/domain"}}"#,
    ),
    ("Endpoint", r#"{"spec":{"slug":null,"audience":"public"}}"#),
    (
        "ModelProjection",
        r#"{"spec":{"dataModelRef":{"version":"1"},"classes":[{"name":"Example"}]}}"#,
    ),
    (
        "SharedSpaceReference",
        r#"{"spec":{"endpointRef":{"name":"example","project":"example-project"}}}"#,
    ),
    (
        "ContextSourceRegistration",
        r#"{"spec":{"endpointRef":"example","information":[{"entities":[{"type":"Example"}]}],"federation":{"identity":"caller"}}}"#,
    ),
    (
        "Subscription",
        r#"{"spec":{"entities":[{"type":"Example"}],"notification":{"endpoint":{"uri":"https://example.org/notify"}}}}"#,
    ),
    (
        "ServiceAccount",
        r#"{"spec":{"roles":[{"role":"reader","scope":{"project":"example-project"},"operations":["retrieveOps"]}],"credentials":[{"kind":"oauth-client","name":"main"}]}}"#,
    ),
    (
        "Pipeline",
        r#"{"apiVersion":"joinedcontext.com/v1alpha2","spec":{"class":"scheduled","schedule":"0 * * * *","source":null,"targetEndpoint":null,"output":null,"sources":[{"dataSourceRef":"example"}],"steps":[{"kind":"bloblang","bloblang":"root = this"}],"outputs":[{"targetEndpoint":"urn:ngsi-ld:Endpoint:example","type":"Example","mode":"upsert"}]}}"#,
    ),
    (
        "DataSource",
        r#"{"spec":{"type":"http","http":{"url":"https://example.org/data.json"}}}"#,
    ),
    (
        "App",
        r#"{"spec":{"source":{"path":"apps/example"},"dataNeeds":[{"contextSpaceRef":"example","types":["Example"],"operations":["retrieveOps"]}]}}"#,
    ),
    (
        "CkanInstance",
        r#"{"spec":{"url":"https://ckan.example.org"}}"#,
    ),
    (
        "KnowledgeSource",
        r#"{"spec":{"startUrls":["https://example.org/"]}}"#,
    ),
    (
        "AssistantDeployment",
        r#"{"spec":{"channel":"internal","sources":["example"]}}"#,
    ),
    (
        "McpServer",
        r#"{"spec":{"members":[{"kind":"Endpoint","name":"example","namespace":"example-project"}]}}"#,
    ),
    (
        "MqttBroker",
        r#"{"spec":{"roles":[{"name":"device","publish":["example/${clientid}/#"]}]}}"#,
    ),
    (
        "Blueprint",
        r#"{"spec":{"version":"1.0.0","allowedRoles":["steward"],"parameterSchema":{"type":"object"},"templates":[{"name":"endpoint","template":"kind: Endpoint"}]}}"#,
    ),
    (
        "AgentProfile",
        r#"{"spec":{"runtime":{"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","image":"registry.example.org/agent"},"limits":{"wallClock":"PT10M"},"workspace":{"cpu":"500m","memory":"512Mi","ephemeralStorage":"1Gi"},"model":{"name":"example-model"},"egress":{"allowedHosts":["api.example.org"]}}}"#,
    ),
    (
        "DataSpaceParticipant",
        r#"{"spec":{"did":"did:web:example.org","connectorUrl":"https://example.org/dsp","credentialIssuer":"https://example.org/issuer"}}"#,
    ),
    (
        "DataOffer",
        r#"{"spec":{"endpointRefs":["example"],"policy":{"permission":[{"action":"use"}]}}}"#,
    ),
    (
        "DataAgreement",
        r#"{"spec":{"remoteParticipant":"did:web:example.org","offerRef":"example"}}"#,
    ),
    (
        "SyncSource",
        r#"{"spec":{"source":{"git":{"url":"https://git.example.org/config.git","ref":"main","secretRef":{"name":"example-git"}}},"schedule":{"interval":"5m"}}}"#,
    ),
    (
        "Bundle",
        r#"{"spec":{"sourceRevision":"0123456789abcdef","items":[{"kind":"Endpoint","name":"example","path":"spaces/example/endpoints/example.yaml"}]}}"#,
    ),
    (
        "UiSchema",
        r#"{"metadata":{"name":"endpoint"},"spec":{"for":"Endpoint"}}"#,
    ),
    (
        "Role",
        r#"{"spec":{"rules":[{"kinds":["Endpoint"],"verbs":["read"]}]}}"#,
    ),
    (
        "RoleBinding",
        r#"{"spec":{"role":"reader","subjects":[{"group":"example-group"}],"scope":{"project":"example-project"}}}"#,
    ),
    (
        "DelegationPolicy",
        r#"{"spec":{"subjects":[{"kind":"Group","name":"example-group"}],"projects":["example-project"]}}"#,
    ),
    (
        "ContextBroker",
        r#"{"spec":{"image":"registry.example.org/broker@sha256:0000000000000000000000000000000000000000000000000000000000000000"}}"#,
    ),
    (
        "Dashboard",
        r#"{"spec":{"pages":[{"title":"Overview","widgets":[{"widgetType":"kpi"}]}]}}"#,
    ),
    ("Layer", r#"{"spec":{"entityType":"ExampleType"}}"#),
];

/// The smallest value a schema accepts that has every required member.
fn example_of(defs: &Defs, schema: &Value, depth: usize) -> Value {
    let schema = resolve(defs, schema);
    if depth > 8 {
        return Value::Null;
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
    {
        return first.clone();
    }
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(first) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .and_then(|alts| alts.iter().find(|a| a.get("type") != Some(&json!("null"))))
    {
        return example_of(defs, first, depth + 1);
    }
    match types(schema).first().copied() {
        Some("string") => json!(match schema.get("format").and_then(Value::as_str) {
            Some("date-time") => "2026-01-01T00:00:00Z",
            Some("date") => "2026-01-01",
            Some("uri") => "https://example.org/",
            Some("email") => "person@example.org",
            _ => "example",
        }),
        Some("integer") | Some("number") => json!(1),
        Some("boolean") => json!(false),
        Some("array") => {
            let count = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let item = example_of(defs, schema.get("items").unwrap_or(&Value::Null), depth + 1);
            Value::Array((0..count).map(|_| item.clone()).collect())
        }
        _ => {
            let mut out = Map::new();
            let props = schema.get("properties").and_then(Value::as_object);
            for name in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let field = props.and_then(|p| p.get(name)).unwrap_or(&Value::Null);
                out.insert(name.to_owned(), example_of(defs, field, depth + 1));
            }
            Value::Object(out)
        }
    }
}

/// The registry name of get_schema (AG-146).
pub const GET: &str = "jc_schema_get";

/// What a kind draws while this installation hides the Dashboards page (T-2874, T-6442): a client
/// that plans a Layer for a map would get nothing on screen.
fn hidden_note(kind: &str) -> Option<&'static str> {
    matches!(kind, "Dashboard" | "Layer").then_some(
        "This installation hides the Dashboards page, so nothing draws this kind on screen. For \
         a map, send the person the Portal's Explore address with view=map (the connector's \
         instructions spell it), or use jc_ui_navigate with page `map` in a paired tab.",
    )
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: GET,
        title: "Get Kind Schema",
        description: "Kind fields + minimal manifest; path: a field's JSON Schema",
        input: input_schema,
        output: output_schema,
        annotations: annotations(true, false, true),
        kind: "*",
        verb: None,
        lane: Lane::Green,
        validate: |val| parse_input::<SchemaInput>(val.clone()).map(|_| ()),
        run: |_caller, state, _project, val| {
            Box::pin(async move {
                let input = parse_input::<SchemaInput>(val)?;
                let mut answer = schema_of(&input.kind, input.path.as_deref())?;
                if !state.config.dashboards {
                    if let Some(note) = hidden_note(&input.kind) {
                        answer["note"] = json!(note);
                    }
                }
                Ok(answer)
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6442: a connector planned a Layer and got nothing on screen: while the Dashboards page
    /// is hidden, the schema of what only that page draws says where a map is drawn instead.
    #[test]
    fn what_only_the_hidden_dashboards_page_draws_says_so() {
        for kind in ["Dashboard", "Layer"] {
            assert!(hidden_note(kind).is_some_and(|note| note.contains("view=map")));
        }
        assert_eq!(hidden_note("Endpoint"), None);
        assert!(output_schema()["properties"]["note"].is_object());
    }

    fn invalid_at(result: Result<Value, OpError>) -> (String, String) {
        match result {
            Err(OpError::InvalidInput { path, message }) => (path, message),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_kind_answers_a_compact_schema_with_a_manifest_that_validates() {
        let kinds = kinds();
        assert!(kinds.len() >= 30, "only {} kinds", kinds.len());
        for kind in &kinds {
            let answer = schema_of(kind, None).expect("a registered kind has a schema");
            let bytes = answer.to_string().len();
            assert!(bytes < 4_000, "{kind}: {bytes} bytes");
            assert_eq!(answer["kind"], *kind);
            let fields = answer["fields"].as_str().expect("fields");
            assert!(
                fields.contains("\nmetadata.name: string, required\n"),
                "{kind}: {fields}"
            );
            assert!(
                fields.contains("\nspec: object, required\n"),
                "{kind}: {fields}"
            );
            assert_eq!(answer["required"][0], "apiVersion", "{kind}");
            // An Endpoint's example leaves its slug out: every write door mints it (EP-75).
            let mut example = answer["example"].clone();
            crate::endpoint_slug::settle(None, None, false, &mut example).expect("minted");
            let example = example.to_string();
            let checked = jc_core::registry::validate_yaml(kind, &example).expect("registered");
            assert!(checked.is_ok(), "{kind}: {checked:?} for {example}");
        }
        assert_eq!(input_schema()["properties"]["kind"]["enum"], json!(kinds));
    }

    /// T-6058 counts every tool definition a client reads per turn: the field list and path cost
    /// the definition ~300 bytes over main's (1,309 measured here, the 37-kind enum most of it).
    #[test]
    fn the_tool_definition_stays_small() {
        let op = &operations()[0];
        let bytes =
            (op.input)().to_string().len() + (op.output)().to_string().len() + op.description.len();
        assert!(bytes <= 1_350, "{bytes} bytes");
    }

    #[test]
    fn every_example_row_names_a_kind() {
        for (kind, patch) in EXAMPLES {
            assert!(jc_core::registry::by_kind(kind).is_some(), "{kind}");
            let patch: Value = serde_json::from_str(patch).expect("a row is JSON");
            assert!(patch.is_object(), "{kind}");
        }
    }

    /// T-6392: the Pipeline example is the v1alpha2 shape a client copies, it parses and validates,
    /// and its output names the Endpoint by its name alone, the id jc-manifest resolves.
    #[test]
    fn the_pipeline_example_is_a_valid_v1alpha2_pipeline_naming_its_endpoint() {
        let answer = schema_of("Pipeline", None).expect("Pipeline");
        let example = &answer["example"];
        assert_eq!(
            example["apiVersion"],
            jc_core::envelope::API_VERSION_V1ALPHA2
        );
        for v1 in ["source", "targetEndpoint", "output"] {
            assert!(example["spec"].get(v1).is_none(), "{v1}: {example}");
        }
        let pipeline = jc_core::kinds::Pipeline::from_yaml(&example.to_string())
            .unwrap_or_else(|err| panic!("{err}: {example}"));
        pipeline
            .validate()
            .unwrap_or_else(|err| panic!("{err}: {example}"));
        let outputs = pipeline.spec.outputs();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].target_endpoint.local_id(), "example");
    }

    #[test]
    fn a_line_says_type_requirement_and_limits() {
        let answer = schema_of("Pipeline", None).expect("Pipeline");
        let fields = answer["fields"].as_str().expect("fields");
        assert!(
            fields.contains("\nspec.class: one of auto|resident|scheduled, required\n"),
            "{fields}"
        );
        assert!(
            fields.contains("\nspec.quotas.maxMemoryMb: integer, minimum 0\n"),
            "{fields}"
        );
        let required = answer["required"].as_array().expect("required");
        assert!(required.contains(&json!("spec.class")), "{required:?}");
        assert!(
            !required.contains(&json!("spec.output.mode")),
            "only a chain of required fields"
        );
    }

    #[test]
    fn a_path_answers_that_fields_whole_schema() {
        let answer = schema_of("Pipeline", Some("/spec/sources/0")).expect("a field");
        assert_eq!(answer["path"], "/spec/sources/0");
        let schema = &answer["schema"];
        let reached = schema["definitions"]
            .as_object()
            .expect("the definitions it names");
        let mut refs = Vec::new();
        collect_refs(schema, &mut |name| refs.push(name.to_owned()));
        assert!(!refs.is_empty());
        for name in refs {
            assert!(reached.contains_key(&name), "{name} dangles");
        }
        let defs = defs_of(schema);
        let item = resolve(&defs, &schema["allOf"][0]);
        assert!(item["properties"].is_object(), "{item}");
        let name = schema_of("Pipeline", Some("/metadata/name")).expect("a leaf");
        assert_eq!(name["schema"]["type"], "string");
        assert!(name["schema"].get("definitions").is_none());
        let whole = schema_of("Pipeline", Some("")).expect("the whole manifest");
        assert!(whole["schema"]["properties"]["spec"].is_object());
        let label = schema_of("Pipeline", Some("/metadata/labels/team")).expect("a map value");
        assert_eq!(label["schema"]["type"], "string");
    }

    #[test]
    fn a_path_naming_no_field_is_refused_on_path() {
        let (at, message) = invalid_at(schema_of("Pipeline", Some("/spec/nosuch")));
        assert_eq!(at, "/path");
        assert!(message.contains("`nosuch`"), "{message}");
        let (at, message) = invalid_at(schema_of("Pipeline", Some("spec.class")));
        assert_eq!(at, "/path");
        assert!(message.contains("start it with /"), "{message}");
        let long = format!("/{}", "a".repeat(MAX_PATH));
        let (at, _) = invalid_at(schema_of("Pipeline", Some(&long)));
        assert_eq!(at, "/path");
        let (at, _) = invalid_at(schema_of("Pipeline", Some("/spec/class/0")));
        assert_eq!(at, "/path");
    }

    #[test]
    fn an_unknown_kind_is_not_found() {
        assert!(matches!(
            schema_of("NoSuchKind", None),
            Err(OpError::Api(ApiError::NotFound(_)))
        ));
    }

    #[test]
    fn an_endpoints_slug_is_shown_as_minted_and_left_out_of_the_example() {
        let answer = schema_of("Endpoint", None).expect("a kind");
        assert!(answer["example"]["spec"].get("slug").is_none(), "{answer}");
        assert!(!answer["required"]
            .as_array()
            .expect("a list")
            .contains(&json!("spec.slug")));
        assert!(answer["fields"]
            .as_str()
            .expect("text")
            .contains("minted by the platform"));
    }

    #[test]
    fn merge_follows_rfc_7386() {
        let mut target = json!({ "a": 1, "b": { "c": 2, "d": 3 } });
        crate::plan::merge_patch(
            &mut target,
            &json!({ "a": null, "b": { "c": 4 }, "e": [1] }),
        );
        assert_eq!(target, json!({ "b": { "c": 4, "d": 3 }, "e": [1] }));
    }
}
