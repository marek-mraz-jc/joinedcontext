//! The Portal assistant's own tools (T-5959, GOAL-connectors §6, UI-59): actions the person's
//! browser runs on the open page, beside the registry tools of the conversation's segment.
//!
//! None of them writes: `jc_ui_open_form` and `jc_ui_show_diff` open the kind's editor with the
//! values filled in and the person proposes from there, `jc_ui_highlight` points at a field and
//! `jc_ui_filter_table` types into the page's table filter. There is no submit, propose or approve
//! action, so a model asking for one gets the refusal every unknown tool gets.
//!
//! Everything is checked here before the browser sees it: the page against the Portal's own table,
//! the kind and every field against the kind's schema, the object and the plan against what the
//! person may read. The browser renders the texts as text, never as markup.

use super::model::{ToolCall, ToolSpec};
use super::*;

use std::borrow::Cow;

use chrono::Utc;

use crate::error::ApiError;

/// The names of the browser actions, in the order the model reads them.
pub(crate) const UI_TOOLS: [&str; 5] = [
    "jc_ui_navigate",
    "jc_ui_open_form",
    "jc_ui_highlight",
    "jc_ui_show_diff",
    "jc_ui_filter_table",
];

/// Kinds whose create form takes a whole manifest as its values; every other kind's editor opens
/// on an object that exists.
const CREATE_FORMS: [&str; 1] = ["Pipeline"];
/// Characters of a highlight's message.
const MESSAGE_CHARS: usize = 280;
/// Characters of a table filter.
const QUERY_CHARS: usize = 200;
/// Characters of a field path.
const FIELD_CHARS: usize = 200;
/// How deep a field path's `$ref`s and `allOf`s are followed.
const SCHEMA_DEPTH: usize = 16;

/// The browser actions as native tool definitions.
pub(super) fn ui_specs() -> Vec<ToolSpec> {
    let text = |description: &str, max: usize| json!({ "type": "string", "minLength": 1, "maxLength": max, "description": description });
    let spec = |name: &'static str, description: &'static str, input: Value| ToolSpec {
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        input_schema: input,
    };
    vec![
        spec(
            "jc_ui_navigate",
            "Open a Portal page for the person in their browser. Nothing is changed.",
            json!({
                "type": "object",
                "properties": {
                    "page": { "type": "string", "enum": tools_registry::page_names() },
                    "name": text("The name of what the page opens, e.g. a space or an app", 200),
                    "plural": text("The plural of the kind, for the pages `new`, `draft` and `resource`", 64),
                    "endpoint": text("The Endpoint the `entities` grid or `map` shows", 200),
                    "type": text("Its entity type", 200),
                    "q": text("An NGSI-LD filter, e.g. availableBikeNumber==0", 500),
                    "color": text("The number the `map` colours by", 200)
                },
                "required": ["page"],
                "additionalProperties": false
            }),
        ),
        spec(
            "jc_ui_open_form",
            "Open the editor of an object with fields filled in, as a JSON merge patch of its \
             manifest. The person reads the form and proposes it; nothing is saved. `new: true` \
             opens the create form (Pipeline only).",
            json!({
                "type": "object",
                "properties": {
                    "kind": text("The kind, e.g. Pipeline", 64),
                    "name": text("The object's name", 200),
                    "new": { "type": "boolean", "description": "Open the create form of a new object named `name`" },
                    "prefill": {
                        "type": "object",
                        "description": "Fields to fill in, e.g. {\"spec\": {\"compute\": {\"kind\": \"bloblang\", \"bloblang\": \"root = this\"}}}"
                    }
                },
                "required": ["kind", "name"],
                "additionalProperties": false
            }),
        ),
        spec(
            "jc_ui_highlight",
            "Point the person at one field of the open form with a short message.",
            json!({
                "type": "object",
                "properties": {
                    "kind": text("The kind whose form is open, e.g. Pipeline", 64),
                    "field": text("The field as a dotted manifest path, e.g. spec.compute.bloblang", FIELD_CHARS),
                    "message": text("What the person should look at, one or two sentences", MESSAGE_CHARS)
                },
                "required": ["kind", "field", "message"],
                "additionalProperties": false
            }),
        ),
        spec(
            "jc_ui_show_diff",
            "Open the editor on a plan jc_change_plan made, with its change filled in, so the \
             person reads what changes and proposes it. Nothing is applied.",
            json!({
                "type": "object",
                "properties": { "planId": text("The planId jc_change_plan answered", 64) },
                "required": ["planId"],
                "additionalProperties": false
            }),
        ),
        spec(
            "jc_ui_filter_table",
            "Type a search into the table filter of the open page.",
            json!({
                "type": "object",
                "properties": { "query": text("The search text, e.g. bikes", QUERY_CHARS) },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
    ]
}

/// A refusal before the operation runs, as the document every door answers (API/09).
pub(super) fn refused(error: ApiError) -> String {
    error.into_document().1.to_string()
}

/// A browser action the Portal will not run, as the 422 every door answers (API/09).
fn unprocessable(reason: &str) -> ApiError {
    ApiError::Unprocessable {
        detail: reason.to_string(),
        errors: vec![reason.to_string()],
    }
}

/// One line of plain text from the model: control characters refused, length bounded.
fn plain(arguments: &Value, member: &str, max: usize) -> Result<String, String> {
    let text = arguments
        .get(member)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("`{member}` is missing; give it as text"))?;
    if text.chars().count() > max {
        return Err(format!("`{member}` is longer than {max} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!(
            "`{member}` holds a control character or a line break"
        ));
    }
    Ok(text.to_owned())
}

/// The schema nodes `node` stands for: itself, what its `$ref` names and the members of its
/// `allOf`, `anyOf` and `oneOf`, followed [`SCHEMA_DEPTH`] deep.
fn expanded<'a>(root: &'a Value, node: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
    if depth == 0 {
        return;
    }
    out.push(node);
    if let Some(target) = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
    {
        expanded(root, target, depth - 1, out);
    }
    for combinator in ["allOf", "anyOf", "oneOf"] {
        for member in node
            .get(combinator)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            expanded(root, member, depth - 1, out);
        }
    }
}

/// Whether `field` (a dotted manifest path; a number steps into a list) is a field of `schema`.
fn has_field(schema: &Value, field: &str) -> bool {
    let mut at = vec![schema];
    for segment in field.split('.') {
        let mut nodes = Vec::new();
        for node in &at {
            expanded(schema, node, SCHEMA_DEPTH, &mut nodes);
        }
        at = nodes
            .into_iter()
            .filter_map(|node| {
                if segment.parse::<usize>().is_ok() {
                    node.get("items")
                } else {
                    node.get("properties").and_then(|p| p.get(segment))
                }
            })
            .collect();
        if at.is_empty() {
            return false;
        }
    }
    true
}

/// The kind's schema, or the reason the kind is none the person can open here.
fn kind_schema(kind: &str) -> Result<(&'static crate::resource::KindInfo, Value), String> {
    let info = crate::resource::by_kind(kind.trim())
        .filter(|info| info.scope.allows_project() || info.scope.allows_organization())
        .ok_or_else(|| format!("'{kind}' is not a kind with a form in the Portal"))?;
    let schema =
        jc_core::registry::schema_of(info.kind).ok_or_else(|| format!("'{kind}' has no schema"))?;
    Ok((info, schema))
}

/// What of `manifest` the kind's schema refuses, a missing field excepted: a form is opened to
/// be finished. A member the schema does not name is refused by name.
fn refused_fields(schema: &Value, manifest: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema).map_err(|e| e.to_string())?;
    let found: Vec<String> = validator
        .iter_errors(manifest)
        .filter(|error| {
            !matches!(
                error.kind(),
                jsonschema::error::ValidationErrorKind::Required { .. }
            )
        })
        .map(|error| {
            let at = error.instance_path().to_string();
            format!("{}: {error}", if at.is_empty() { "/" } else { &at })
        })
        .take(5)
        .collect();
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the form cannot hold these values: {}",
            found.join("; ")
        ))
    }
}

/// The values a kind's editor reads: the Endpoint form its own, every other the manifest.
fn form_prefill(kind: &str, manifest: Value) -> Value {
    if kind == "Endpoint" {
        crate::agents::share::form_values(&manifest)
    } else {
        manifest
    }
}

impl Driver {
    /// One browser action: checked, shown as a `tool` event, and handed to the page as a
    /// `navigate` (a route and its form values) or a `ui` event. A refusal goes back to the model
    /// as a problem document naming what to correct.
    pub(super) async fn ui_action(&self, call: &ToolCall) -> Result<String, String> {
        let started = std::time::Instant::now();
        let arguments = if call.input.is_object() {
            call.input.clone()
        } else {
            json!({})
        };
        let done = match call.name.as_str() {
            "jc_ui_navigate" => {
                let opened = match tools_registry::navigate_args(&arguments) {
                    Ok(page) => self.open_page(&page).await?,
                    Err(reason) => format!("error: {reason}"),
                };
                // `open_page` reports its own tool event, a refusal included.
                return Ok(match opened.strip_prefix("error: ") {
                    Some(reason) => refused(unprocessable(reason)),
                    None => json!({ "opened": opened.trim_start_matches("opened ") }).to_string(),
                });
            }
            other => for_tab(&self.asker(), other, &arguments),
        };
        match done {
            Ok((kind, payload)) => {
                self.event(
                    "tool",
                    json!({
                        "tool": call.name,
                        "status": "ok",
                        "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "input": arguments,
                        "output": { "route": payload.get("route"), "action": payload.get("action") },
                    }),
                )
                .await?;
                let answer =
                    json!({ "done": payload.get("route").or_else(|| payload.get("action")) });
                self.event(kind, payload).await?;
                Ok(answer.to_string())
            }
            Err(reason) => {
                self.event(
                    "tool",
                    failed_step(&call.name, started, &arguments, &reason),
                )
                .await?;
                Ok(refused(unprocessable(&reason)))
            }
        }
    }
}

/// `jc_ui_highlight`: one field of the kind's form and a one-line message.
fn highlight(arguments: &Value) -> Result<(&'static str, Value), String> {
    let (info, schema) = kind_schema(arguments.get("kind").and_then(Value::as_str).unwrap_or(""))?;
    let field = plain(arguments, "field", FIELD_CHARS)?;
    if !has_field(&schema, &field) {
        return Err(format!(
            "'{field}' is no field of {}; name it as its manifest path, e.g. spec.title",
            info.kind
        ));
    }
    let message = plain(arguments, "message", MESSAGE_CHARS)?;
    Ok((
        "ui",
        json!({ "action": "highlight", "kind": info.kind, "field": field, "message": message }),
    ))
}

/// One browser action but `jc_ui_navigate` of the assistant, which resolves a draft's kind
/// first: the frame the page runs, `navigate` (a route and its form values) or `ui`.
pub(crate) fn for_tab(
    asker: &Asker<'_>,
    name: &str,
    arguments: &Value,
) -> Result<(&'static str, Value), String> {
    match name {
        "jc_ui_navigate" => {
            let call = tools_registry::navigate_args(arguments)?;
            let route = tools_registry::route_of(asker.project, &call)?;
            Ok(("navigate", json!({ "route": route })))
        }
        "jc_ui_open_form" => asker.open_form(arguments),
        "jc_ui_show_diff" => asker.show_diff(arguments),
        "jc_ui_highlight" => highlight(arguments),
        "jc_ui_filter_table" => plain(arguments, "query", QUERY_CHARS)
            .map(|query| ("ui", json!({ "action": "filter", "query": query }))),
        other => Err(format!("'{other}' is not a browser action")),
    }
}

/// The browser actions as the MCP tools of a connector whose owner paired a tab (T-5967), each
/// naming one of the connector's `projects`. They change nothing: the person reads and proposes.
pub(crate) fn mcp_tools(projects: &[&str]) -> Vec<Value> {
    ui_specs()
        .into_iter()
        .map(|spec| {
            let mut input = spec.input_schema;
            input["properties"]["project"] = json!({
                "type": "string",
                "enum": projects,
                "description": "The project whose page the person's tab opens",
            });
            if let Some(required) = input["required"].as_array_mut() {
                required.push(json!("project"));
            }
            json!({
                "name": spec.name,
                "title": spec.name,
                "description": format!("{} Runs in the Portal tab the person connected to this connector; it never saves, proposes or approves.", spec.description),
                "inputSchema": input,
                "annotations": { "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false },
            })
        })
        .collect()
}

impl Driver {
    pub(super) fn asker(&self) -> Asker<'_> {
        Asker {
            state: &self.state,
            identity: &self.identity,
            project: &self.project,
            connector: None,
        }
    }
}

/// Who asks for a browser action, in which project, and through which connector: the checks
/// every action passes before a browser sees it, for the Portal assistant and for a connector
/// steering a paired tab (T-5967) alike.
pub(crate) struct Asker<'a> {
    pub state: &'a AppState,
    pub identity: &'a Identity,
    pub project: &'a str,
    /// The connector a paired tab is steered through: an editor opens only on what its scope
    /// reaches, besides what the person reads.
    pub connector: Option<&'a crate::connectors::Connector>,
}

impl Asker<'_> {
    pub(super) fn home(&self, kind: &str) -> String {
        match crate::resource::by_kind(kind) {
            Some(info) => crate::resource::home(info, self.project),
            None => self.project.to_owned(),
        }
    }

    pub(super) fn names_of(&self, kind: &str) -> Vec<String> {
        let homes = match crate::resource::by_kind(kind) {
            Some(info) => crate::resource::homes(info, self.project),
            None => vec![self.project.to_owned()],
        };
        let mut names: Vec<String> = homes
            .iter()
            .flat_map(|home| {
                self.state
                    .mirror
                    .list(home, kind, &crate::store::ListOptions::default())
                    .items
            })
            .map(|envelope| envelope.metadata.name)
            .collect();
        names.sort();
        names
    }

    /// Whether the connector, if any, reaches `kind` `name` of the project.
    fn reaches(&self, kind: &str, name: &str) -> Result<(), String> {
        self.connector.map_or(Ok(()), |c| {
            let space = crate::connectors::space_now(&self.state.mirror, self.project, kind, name);
            c.may_change(self.project, kind, name, space.as_deref(), false)
        })
    }

    /// Whether the person may read `kind` where it lives: an editor opens only on what they read.
    fn may_read_kind(&self, kind: &str) -> Result<(), String> {
        crate::permissions::for_request(self.state, self.identity, &self.home(kind))
            .check(kind, Verb::Read, None)
            .map_err(|err| err.to_string())
    }

    /// `jc_ui_open_form`: the object's manifest with the prefill merged in, checked against the
    /// kind's schema, as the values of the kind's editor.
    pub(crate) fn open_form(&self, arguments: &Value) -> Result<(&'static str, Value), String> {
        let (info, schema) =
            kind_schema(arguments.get("kind").and_then(Value::as_str).unwrap_or(""))?;
        self.may_read_kind(info.kind)?;
        let name = plain(arguments, "name", 200)?;
        self.reaches(info.kind, &name)?;
        let prefill = match arguments.get("prefill") {
            None | Some(Value::Null) => json!({}),
            Some(value) if value.is_object() => value.clone(),
            Some(_) => return Err("`prefill` is a JSON merge patch: an object".to_owned()),
        };
        let home = self.home(info.kind);
        let current = self.state.mirror.get(&home, info.kind, &name);
        let create = arguments.get("new").and_then(Value::as_bool) == Some(true);
        let (base, route) = match (current, create) {
            (Some(_), true) => {
                return Err(format!(
                    "{} '{name}' exists already; open it without `new`",
                    info.kind
                ))
            }
            (None, true) if !CREATE_FORMS.contains(&info.kind) => {
                return Err(format!(
                    "a new {} is not opened prefilled; the create forms that take values are {}",
                    info.kind,
                    CREATE_FORMS.join(", ")
                ))
            }
            (None, true) => (
                crate::agents::change::manifest(info.kind, &home, &name, json!({})),
                crate::agents::change::create_route(self.project, info.kind, info.plural),
            ),
            (None, false) => {
                let names = self.names_of(info.kind);
                return Err(if names.is_empty() {
                    format!(
                        "{} '{name}' does not exist; the project has none",
                        info.kind
                    )
                } else {
                    format!(
                        "{} '{name}' does not exist; the project's are {}",
                        info.kind,
                        names.join(", ")
                    )
                });
            }
            (Some(envelope), false) => (
                serde_json::to_value(&envelope).map_err(|e| e.to_string())?,
                crate::agents::change::route(self.project, info.kind, info.plural, &name, false),
            ),
        };
        // `patched` keeps the kind, the name and the namespace, and drops `status`; a form
        // opened without values opens on the object as it is.
        let manifest = if prefill.as_object().is_some_and(serde_json::Map::is_empty) {
            let mut as_is = base;
            if let Some(fields) = as_is.as_object_mut() {
                fields.remove("status");
            }
            as_is
        } else {
            crate::agents::change::patched(&base, &prefill)?
        };
        refused_fields(&schema, &manifest)?;
        Ok((
            "navigate",
            json!({ "route": route, "prefill": form_prefill(info.kind, manifest) }),
        ))
    }

    /// `jc_ui_show_diff`: the first change of the person's own plan, opened in its editor with
    /// the planned manifest as the values.
    pub(crate) fn show_diff(&self, arguments: &Value) -> Result<(&'static str, Value), String> {
        let id = plain(arguments, "planId", 64)?;
        let items = self
            .state
            .plans
            .peek(&id, &self.identity.subject, self.project, Utc::now())
            .ok_or_else(|| {
                format!(
                    "no plan '{id}' of yours in this project; a plan lives {} minutes, plan the \
                     change again",
                    crate::ops::plans::TTL_MINUTES
                )
            })?;
        let item = items
            .iter()
            .find(|item| item.manifest.is_some())
            .or_else(|| items.first())
            .ok_or_else(|| format!("plan '{id}' changes nothing"))?;
        let info = crate::resource::by_kind(&item.kind)
            .ok_or_else(|| format!("'{}' is not a kind of the Portal", item.kind))?;
        self.may_read_kind(info.kind)?;
        self.reaches(info.kind, &item.name)?;
        let exists = self
            .state
            .mirror
            .get(&item.home, info.kind, &item.name)
            .is_some();
        let route = match (&item.manifest, exists) {
            (Some(_), false) => {
                crate::agents::change::create_route(self.project, info.kind, info.plural)
            }
            (manifest, _) => crate::agents::change::route(
                self.project,
                info.kind,
                info.plural,
                &item.name,
                manifest.is_none(),
            ),
        };
        let mut payload = json!({ "route": route });
        // A model's editor opens on the planned LinkML, so the person reads its diff against the
        // published source there (JC2-11); every other kind's form takes the planned manifest.
        match (&item.source, &item.manifest) {
            (Some(source), _) => payload["prefill"] = json!({ "source": source }),
            (None, Some(manifest)) => {
                payload["prefill"] = form_prefill(info.kind, manifest.clone())
            }
            (None, None) => {}
        }
        Ok(("navigate", payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A person bound to space `a` reads Endpoints there, and helsinki has one Endpoint in
    /// space `a` and one in space `b`.
    fn space_a_reader() -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let put = |kind: &str, name: &str, namespace: &str, spec: Value| {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        };
        let org = crate::permissions::ORG_NAMESPACE;
        put(
            "Role",
            "endpoint-reader",
            org,
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
        );
        put(
            "RoleBinding",
            "dev-space-a",
            org,
            json!({ "role": "endpoint-reader", "subjects": [{ "user": "dev@hel.fi" }],
                    "scope": { "contextSpace": "a" } }),
        );
        for space in ["a", "b"] {
            put(
                "ContextSpace",
                space,
                "helsinki",
                json!({ "projectRef": "helsinki" }),
            );
            put(
                "Endpoint",
                &format!("ep-{space}"),
                "helsinki",
                json!({ "contextSpaceRef": space, "description": format!("secret of {space}") }),
            );
        }
        state
    }

    /// F-R2POR1-001/-003 (T-6225, W-R2POR1-001): an editor opens on a kind only for a grant that
    /// reads it in the whole project; a space-bound reader's grant passes no check without a
    /// target, so neither a manifest nor a name of any space reaches the model or the prefill.
    #[test]
    fn a_space_reader_opens_no_editor_and_reads_no_name() {
        let state = space_a_reader();
        let identity = crate::permissions::named_person(&state.mirror, "dev@hel.fi");
        let asker = Asker {
            state: &state,
            identity: &identity,
            project: "helsinki",
            connector: None,
        };
        for name in ["ep-a", "ep-b"] {
            let refused = asker
                .open_form(&json!({ "kind": "Endpoint", "name": name }))
                .expect_err("a space-bound reader opens no editor");
            assert!(
                !refused.contains("secret") && !refused.contains("ep-"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_field_is_named_by_its_manifest_path() {
        let (_, schema) = kind_schema("Pipeline").expect("Pipeline");
        assert!(has_field(&schema, "spec"));
        assert!(has_field(&schema, "metadata.name"));
        assert!(has_field(&schema, "spec.compute"));
        assert!(!has_field(&schema, "spec.nonsense"));
        assert!(!has_field(&schema, "constructor.prototype"));
        assert!(kind_schema("Nope").is_err());
    }

    #[test]
    fn a_text_is_one_bounded_line() {
        let args = json!({ "m": "look <b>here</b>", "n": "two\nlines", "o": "x".repeat(300) });
        assert_eq!(plain(&args, "m", 280).as_deref(), Ok("look <b>here</b>"));
        assert!(plain(&args, "n", 280).is_err());
        assert!(plain(&args, "o", 280).is_err());
        assert!(plain(&args, "missing", 280).is_err());
    }

    #[test]
    fn every_action_has_a_closed_schema_and_none_submits() {
        let specs = ui_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_ref()).collect();
        assert_eq!(names, UI_TOOLS);
        for spec in &specs {
            assert_eq!(spec.input_schema["additionalProperties"], json!(false));
            assert!(
                jsonschema::validator_for(&spec.input_schema).is_ok(),
                "{}",
                spec.name
            );
        }
        assert!(names
            .iter()
            .all(|n| !["submit", "propose", "approve", "apply"]
                .iter()
                .any(|w| n.contains(w))));
    }
}
