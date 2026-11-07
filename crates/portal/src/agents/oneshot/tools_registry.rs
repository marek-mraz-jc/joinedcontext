//! Every registered operation as a tool of the conversation (AG-64, ADR-N-021).
//!
//! The assistant calls the platform the way every other client does: one operation, one Verdict,
//! one Change. The list it sees is the registry filtered twice — by the profile's access block and
//! by the person's own grants — so a tool a viewer may not run is not offered and is refused if
//! asked for anyway. The hand-written tools beside these stay for the shapes the registry has no
//! operation for; the model reaches for a `jc_` tool first because it is the one that changes
//! anything.

use super::*;

use crate::ops::OperationSummary;

/// One `jc_` call of an answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RegistryCall {
    pub name: String,
    pub arguments: Value,
}

/// An operation that opens a Change by itself, and what the conversation does instead (AG-77: the
/// assistant opens the kind's form, or the deletion's confirmation, prefilled, and never proposes
/// on its own). The profile names these for the runs that do propose — a workspace run brings its
/// copy back — so they are taken away here, where the person is watching and one click away, and
/// refused if the model asks for one anyway.
pub(super) fn opens_a_change(name: &str) -> Option<&'static str> {
    match name {
        "jc_resource_delete" | "jc_project_delete" | "jc_syncsource_detach" => Some(
            "open the removal with change_resource and `delete: true`; the person types the name \
             there and proposes it",
        ),
        // T-6533: a sync proposes what it finds as Changes of its own.
        "jc_syncsource_sync" => {
            Some("the person starts it with Sync now on the SyncSource's page; open that page")
        }
        _ if name.ends_with("_propose") || name == crate::connectors::APPLY => Some(
            "draft it with change_resource — `create: true` for a resource that does not exist \
             yet, a `patch` for one that does — and the kind's form opens filled for the person to \
             propose",
        ),
        _ => None,
    }
}

/// The operations this run may call: the profile's half and the person's half, both checked the
/// way [`Access::check`] checks them at call time, so the list never offers what the call refuses,
/// and without the ones that would propose in the person's place.
pub(super) fn offered(
    access: &Access,
    identity: &Identity,
    state: &AppState,
    project: &str,
) -> Vec<OperationSummary> {
    let caller = crate::ops::Caller {
        bearer: None,
        identity: identity.clone(),
        via: crate::ops::Via::Agent,
        access: None,
    };
    crate::ops::listing(&caller, state, project)
        .into_iter()
        .filter(|op| crate::ops::find(&op.name).is_some_and(|operation| access.names(operation)))
        .filter(|op| opens_a_change(&op.name).is_none())
        // T-5948: kind schemas reach MCP clients (tools/list, jc_schema_get), not this prompt,
        // which keeps its 4k-token turn (AG-72) until T-5958 replaces it.
        .filter(|op| op.name != crate::ops::schemas::GET)
        // T-5956, T-5974: a plan or revert plan is applied only through a connector
        // (jc_change_apply); here the person proposes from the form change_resource opens.
        .filter(|op| {
            ![crate::connectors::PLAN, crate::connectors::REVERT].contains(&op.name.as_str())
        })
        // A run reads data through the agent proxy's query_endpoint; jc_data_query exchanges the
        // caller's own token, which a run does not hold (T-5986).
        .filter(|op| op.name != crate::ops::data_query::NAME)
        // T-6039: the project's members reach access connectors; this prompt keeps its 4k-token
        // turn (AG-72), and a person picks a subject from the RoleBinding form.
        .filter(|op| op.name != crate::ops::people::MEMBERS)
        // T-6290: a capture reaches pipeline connectors; a person reads it on the broker's page.
        .filter(|op| op.name != crate::ops::mqtt::CAPTURED)
        // JC2-28..34, T-6271, T-6389: the MCP parity reads and the entity write reach MCP clients
        // and connectors; this prompt keeps its 4k-token turn (AG-72), a person reads them on
        // their pages and writes entities on the Endpoint's own.
        .filter(|op| !MCP_ONLY.contains(&op.name.as_str()))
        .collect()
}

/// The operations offered to MCP clients and connectors and not on this prompt.
const MCP_ONLY: &[&str] = &[
    crate::ops::model_catalog::SEARCH,
    crate::ops::model_catalog::IMPORT,
    crate::ops::model_catalog::GENERATE,
    crate::ops::changes::HISTORY,
    crate::ops::access_tools::PERMISSIONS,
    crate::ops::access_tools::SIMULATE,
    crate::ops::access_tools::EXPLORE,
    crate::ops::access_tools::EXPLAIN,
    crate::ops::access_tools::PREVIEW,
    crate::ops::access_tools::CATALOGUE_DRAFT,
    crate::ops::space_reads::GOVERNANCE,
    crate::ops::mqtt::TEST_PUBLISH,
    crate::ops::data_write::NAME,
];

#[cfg(test)]
mod mcp_only_tests {
    #[test]
    fn every_mcp_only_name_is_an_operation() {
        for name in super::MCP_ONLY {
            assert!(crate::ops::find(name).is_some(), "{name}");
        }
    }
}

/// The prompt section: what the platform can do, and how to ask for the detail of one.
pub(super) fn section(ops: &[OperationSummary]) -> String {
    if ops.is_empty() {
        return String::new();
    }
    let listed: String = ops
        .iter()
        .map(|op| {
            format!(
                "- `{}` — {}{}\n",
                op.name,
                // The first sentence: describe_tool answers the rest (AG-72).
                op.description.trim().split(". ").next().unwrap_or_default(),
                if acts(&op.name) {
                    " (changes something: a draft, a Verdict or a Change)"
                } else {
                    ""
                }
            )
        })
        .collect();
    format!(
        r#"## THE PLATFORM'S OPERATIONS

{listed}
Ask for one operation's input schema before you call it the first time:

```json
{{ "tool": "describe_tool", "name": "<operation name>" }}
```

Call it with its arguments:

```json
{{ "tool": "<operation name>", "arguments": {{ }} }}
```

Open the page you are talking about, so the person
sees what you mean — the page is one of `spaces`, `space`, `models`, `model`, `endpoints`,
`endpoint`, `policies`, `shared`, `draft`, with the name of the one to open. A kind you do not draft
yourself (a ServiceAccount, a Subscription, a policy…) is created
by the person in its own form: open it empty with `new` and the kind's plural, say what the form
asks for, and propose nothing:

```json
{{ "tool": "jc_ui_navigate", "arguments": {{ "page": "space", "name": "helsinki" }} }}
```

```json
{{ "tool": "jc_ui_navigate", "arguments": {{ "page": "new", "plural": "serviceaccounts" }} }}
```

An AI client's connector is the person's own, no ServiceAccount: open page `connectors`, where
they create, edit and revoke it; you do neither. Call `jc_pipeline_run` unasked: the platform asks
the person and their answer runs it.

To show the data, open `entities` with the endpoint, the entity type and a `q` for a subset; to map
it, `map` with the same and `color`, a number attribute. A row lacking an attribute (`!attr`)
is not reporting, never 0:

```json
{{ "tool": "jc_ui_navigate", "arguments": {{ "page": "entities", "endpoint": "bikes-public", "type": "BikeHireDockingStation", "q": "availableBikeNumber==0" }} }}
```

And ask, rather than guess, whenever a choice is the person's — which space, which base model,
which unit. When more than one resource fits the ask, ask; never take the first. To choose among
the project's resources, name the kind in `pick` (`endpoints`, `spaces`, `datamodels`,
`pipelines`, `datasources`, `policies`, `projects`): the platform lists what the person may read,
and `options` then only narrows that list by name. `multiple` takes several answers, with an
optional `min` and `max`. Otherwise give the options you would take; the panel draws them as
buttons and one click answers. To have the person hand data over, add `input`: `["file"]` for a
CSV, TSV or JSON file of theirs, `["url"]` for a feed's address, or both; you then read the
file's name, size and first lines, or the address. Ask one question at a time and wait for the
answer:

```json
{{ "tool": "jc_ask", "arguments": {{ "question": "Which endpoints should the app read?", "pick": "endpoints", "multiple": true, "min": 1 }} }}
```

```json
{{ "tool": "jc_ask", "arguments": {{ "question": "Which unit?", "options": ["µg/m³", "ppm"], "default": "µg/m³" }} }}
```

A call that is refused answers with the reason; correct it and call again. Nothing here writes
context data or approves anything on the person's behalf: an operation that changes the
configuration leaves a draft or a Change for the person to approve.
"#
    )
}

/// Every `jc_` call of an answer, in order.
pub(super) fn calls(answer: &str) -> Vec<RegistryCall> {
    blocks(answer)
        .filter_map(|value| {
            let name = value.get("tool").and_then(Value::as_str)?;
            if !name.starts_with("jc_") {
                return None;
            }
            Some(RegistryCall {
                name: name.to_owned(),
                arguments: value.get("arguments").cloned().unwrap_or(json!({})),
            })
        })
        .collect()
}

/// The name of every tool an answer calls, in order (AG-89): what a path's gate reads.
pub(super) fn called(answer: &str) -> Vec<String> {
    blocks(answer)
        .filter_map(|value| value.get("tool").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Lines and characters of a handed-over file the model reads: enough to see its shape.
const FILE_HEAD_LINES: usize = 12;
const FILE_HEAD_CHARS: usize = 2000;

/// What the model reads of a person's answer (AG-80): the option, the several chosen, the
/// address, or a file's name, size and first lines, never the whole file (T-2694, API/04).
pub(super) fn answer_text(answers: &Value) -> String {
    if let Some(file) = answers.get("file") {
        let field = |key: &str| file.get(key).and_then(Value::as_str);
        let text = field("text").unwrap_or_default();
        let head: String = text
            .lines()
            .take(FILE_HEAD_LINES)
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(FILE_HEAD_CHARS)
            .collect();
        // The file is the person's data, not an instruction: it cannot close the fence it is in.
        let head = head.replace("```", "'''");
        return format!(
            "I handed over the file {} ({}, {} bytes, {} lines). Its first lines:\n```\n{head}\n```",
            field("name").unwrap_or("without a name"),
            field("format").unwrap_or("text"),
            text.len(),
            text.lines().count(),
        );
    }
    if let Some(url) = answers.get("url").and_then(Value::as_str) {
        return format!("The data is at {url}");
    }
    let chosen = answers.get("answer");
    chosen
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            // Several answers (UI-73) are one line.
            chosen.and_then(Value::as_array).map(|many| {
                many.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
        .unwrap_or_else(|| answers.to_string())
}

/// The names of every `describe_tool` call of an answer.
pub(super) fn describes(answer: &str) -> Vec<String> {
    blocks(answer)
        .filter(|value| value.get("tool").and_then(Value::as_str) == Some("describe_tool"))
        .filter_map(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// A block that opens like a call, closed or cut off. Wider than [`share::TOOL_FENCE`], which
/// needs a closing brace and sees no list.
static CALL_BLOCK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)```(?:json)?\s*([\[{].*?)(?:```|\z)").expect("a literal pattern")
});

/// The tool a block that does not parse names.
static NAMED_TOOL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"["']tool["']\s*:\s*["']([^"']*)"#).expect("a literal pattern")
});

/// The tools that run after the turn's loop: every other call has run by the time it ends.
const RUN_AFTER_THE_LOOP: [&str; 4] = [
    "propose_endpoint",
    "edit_endpoint",
    "space_complete",
    "navigate",
];

/// The tool one block calls and why nothing runs it; none for a block no parser leaves behind.
fn unrun_block(block: &str) -> Option<(String, String)> {
    let parsed = serde_json::from_str::<Value>(block.trim());
    if let Some(call) = parsed.as_ref().ok().and_then(Value::as_object) {
        let tool = call.get("tool").and_then(Value::as_str)?;
        let reason = if tool == "describe_tool" {
            "describe_tool takes `name`, the operation to describe, and this call has none, so \
             it did not run; send it again, or answer in plain prose"
                .to_owned()
        } else {
            format!(
                "{tool} lacks a field it needs or is no tool of this turn, so it did not run; \
                 call a tool you were given, or answer in plain prose"
            )
        };
        return (!RUN_AFTER_THE_LOOP.contains(&tool)).then(|| (tool.to_owned(), reason));
    }
    let tool = NAMED_TOOL.captures(block)?[1].to_owned();
    let why = parsed
        .err()
        .map_or("a list".to_owned(), |err| err.to_string());
    let reason = format!(
        "the {tool} call is not one JSON object ({why}), so it did not run; send it again as \
         one object in one fence, or answer in plain prose"
    );
    Some((tool, reason))
}

/// The first call of a finished answer that nothing will run (T-6479): a block naming `tool` that
/// does not parse, or that no parser takes. It goes back to the model; it is never the answer.
pub(super) fn unrun_call(answer: &str) -> Option<(String, String)> {
    CALL_BLOCK
        .captures_iter(answer)
        .find_map(|block| unrun_block(&block[1]))
}

/// The answer without the blocks [`unrun_call`] finds.
pub(super) fn without_unrun_calls(answer: &str) -> String {
    CALL_BLOCK
        .replace_all(answer, |block: &regex::Captures<'_>| {
            match unrun_block(&block[1]) {
                Some(_) => String::new(),
                None => block[0].to_owned(),
            }
        })
        .trim()
        .to_owned()
}

pub(super) fn blocks(answer: &str) -> impl Iterator<Item = Value> + '_ {
    share::TOOL_FENCE
        .captures_iter(answer)
        .filter_map(|fence| serde_json::from_str::<Value>(&fence[1]).ok())
}

/// Whether a `jc_` tool changes something, which is what makes a call the data wrote refusable
/// (AG-20). The lane says it: green reads, yellow leaves a draft or a Change, red is the
/// dangerous one. An operation nobody registered is treated as acting: unknown is not safe.
pub(super) fn acts(name: &str) -> bool {
    crate::ops::find(name).is_none_or(|op| op.lane != crate::change::Lane::Green)
}

impl Driver {
    /// One registry call, as the person who started the run (AG-70). The answer is what goes back
    /// to the model; the `tool` event is what the person sees (AG-56).
    pub(super) async fn registry_call(&self, call: &RegistryCall) -> Result<String, String> {
        let started = std::time::Instant::now();
        // A proposal the person never read must not reach the approval queue (AG-77): the
        // conversation drafts, the form opens, the person proposes.
        if let Some(instead) = opens_a_change(&call.name) {
            let reason = format!(
                "'{}' opens a Change of its own, which the assistant never does: {instead}",
                call.name
            );
            self.event(
                "tool",
                failed_step(&call.name, started, &call.arguments, &reason),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        if let Err(reason) =
            self.access
                .check(&call.name, &self.identity, &self.state, &self.project)
        {
            self.event(
                "tool",
                failed_step(&call.name, started, &call.arguments, &reason),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        let op = match crate::ops::find(&call.name) {
            Some(op) => op,
            None => {
                let reason = format!("operation '{}' is not registered", call.name);
                self.event(
                    "tool",
                    failed_step(&call.name, started, &call.arguments, &reason),
                )
                .await?;
                return Ok(format!("error: {reason}"));
            }
        };
        let caller = crate::ops::Caller {
            bearer: None,
            identity: self.identity.clone(),
            via: crate::ops::Via::Agent,
            access: None,
        };
        let called = crate::ops::call(
            op,
            &caller,
            &self.state,
            &self.project,
            call.arguments.clone(),
        )
        .await;
        // What a conversation reads reaches the model provider: the attributes the project's
        // models mark personal leave it, as they leave a connector's answer (AG-158, JC1-03).
        let called = match called {
            Ok(output) if crate::personal_data::carries_entities(&output) => {
                let marks = crate::personal_data::of_project(&self.state, &self.project).await;
                match super::connector_turn::without_personal(output, marks) {
                    Ok(output) => Ok(output),
                    Err(why) => {
                        self.event(
                            "tool",
                            failed_step(&call.name, started, &call.arguments, &why),
                        )
                        .await?;
                        return Ok(format!("error: {why}"));
                    }
                }
            }
            other => other,
        };
        match called {
            Ok(output) => {
                self.event(
                    "tool",
                    json!({
                        "tool": call.name,
                        "status": "ok",
                        "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "input": call.arguments,
                        "output": output,
                    }),
                )
                .await?;
                Ok(serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_owned()))
            }
            Err(err) => {
                let reason = err.to_string();
                self.event(
                    "tool",
                    failed_step(&call.name, started, &call.arguments, &reason),
                )
                .await?;
                Ok(format!("error: {reason}"))
            }
        }
    }

    /// The input schema of one operation, or why there is none to show.
    pub(super) fn describe(&self, name: &str, offered: &[OperationSummary]) -> String {
        match offered.iter().find(|op| op.name == name) {
            Some(op) => serde_json::to_string_pretty(&json!({
                "name": op.name,
                "title": op.title,
                "description": op.description,
                "input": op.input_schema,
                "output": op.output_schema,
            }))
            .unwrap_or_else(|_| "{}".to_owned()),
            None => format!(
                "error: '{name}' is not an operation you may call; the ones you may call are \
                 listed above"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The two tools of the conversation itself (UI-57, UI-59, AG-80)
// ---------------------------------------------------------------------------

/// The pages the assistant may open, and what each one needs (UI-59). The route is built here,
/// so a page the enum does not name cannot be reached however the model spells it.
const PAGES: [(&str, &str); 22] = [
    ("spaces", "/projects/{project}/spaces"),
    ("space", "/projects/{project}/spaces/{name}"),
    ("models", "/projects/{project}/models"),
    ("model", "/projects/{project}/models?name={name}"),
    ("endpoints", "/projects/{project}/endpoints"),
    ("endpoint", "/projects/{project}/endpoints?name={name}"),
    ("policies", "/projects/{project}/policies"),
    ("shared", "/projects/{project}/shared"),
    // The draft's own kind picks the page (T-2768): `open_page` reads it from the drafts.
    ("draft", "/projects/{project}/{section}?draft={name}"),
    // The rest of what `ui/src/router.tsx` serves (T-1011, T-1012): the assistant opens the
    // page a person would, so every route a person reaches by clicking is one it can name.
    ("activity", "/projects/{project}/activity"),
    ("approvals", "/projects/{project}/approvals"),
    ("approval", "/projects/{project}/approvals/{name}"),
    ("explore", "/projects/{project}/explore"),
    // One entity of the explorer, opened on its detail (T-1017): "show me this one" opens the
    // row already selected instead of the list the person then searches by hand.
    ("entity", "/projects/{project}/explore?entityId={name}"),
    // One endpoint's grid, already narrowed (UI-64, UI-67): a person who asked a question wants
    // the rows that answer it, not the list to filter by hand. `q` is the only optional part —
    // without it the grid opens on the whole type.
    (
        "entities",
        "/projects/{project}/explore?endpoint={endpoint}&type={type}&q={q}",
    ),
    // The same view on its map (T-6442): "map the stations" ends in a map on screen, coloured by
    // a number attribute when `color` names one. `q` and `color` are optional.
    (
        "map",
        "/projects/{project}/explore?endpoint={endpoint}&type={type}&q={q}&colorBy={color}&view=map",
    ),
    ("ckan", "/projects/{project}/ckan"),
    ("assistant", "/projects/{project}/assistant"),
    // A person's own connectors (T-6482, AG-151, AG-157): no project's page, and the assistant
    // only opens it; the person creates and revokes there.
    ("connectors", "/ai-connectors"),
    ("app", "/projects/{project}/apps/{name}"),
    // A resource of any kind, the way `model` and `endpoint` open one of theirs.
    // Its page is the kind's, never its plural spelled as a path (`datamodels` is `models`).
    ("resource", "/projects/{project}/{section}?name={name}"),
    // A kind's empty create form, in the section a person creates one in (T-2577, AG-73): what
    // "create a ServiceAccount" opens for a kind the chat does not draft itself.
    ("new", "/projects/{project}/{section}/new"),
];

/// One `jc_ui_navigate` call.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NavigateCall {
    pub page: String,
    pub name: Option<String>,
    pub plural: Option<String>,
    /// The endpoint whose data the `entities` grid shows.
    pub endpoint: Option<String>,
    /// The entity type of that grid, and the NGSI-LD filter it opens narrowed by.
    pub entity_type: Option<String>,
    pub q: Option<String>,
    /// The number attribute the `map` page colours its points by.
    pub color: Option<String>,
}

/// One option of a question: the value the answer carries and what the person reads.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AskOption {
    pub value: String,
    pub title: String,
    pub description: Option<String>,
    /// Why the person cannot take this option now (UI-44); the Portal's to set, never the model's.
    pub disabled: Option<String>,
}

/// The formats a question takes a file in: text a person can read and a sample the runner splits.
/// An Excel workbook is read in the browser and answered as the CSV of one sheet (JC1-07).
pub(crate) const SAMPLE_FORMATS: [&str; 4] = ["csv", "tsv", "xlsx", "json"];

/// The largest file a question takes, in bytes: a sample, not a dataset (T-2694, T-2697).
pub(crate) const SAMPLE_MAX_BYTES: usize = 256 * 1024;

/// What a question asks the person to hand over besides choosing (T-2694, API/04 section 5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AskInput {
    pub file: bool,
    pub url: bool,
}

impl AskInput {
    /// The event's `input`: `None` when the question asks for nothing but a choice or words.
    pub(super) fn payload(self) -> Option<Value> {
        let mut input = serde_json::Map::new();
        if self.file {
            input.insert(
                "file".into(),
                json!({ "accept": SAMPLE_FORMATS, "maxBytes": SAMPLE_MAX_BYTES }),
            );
        }
        if self.url {
            input.insert("url".into(), json!(true));
        }
        (!input.is_empty()).then_some(Value::Object(input))
    }
}

/// One `jc_ask` call: the question, its options and the answer taken when nobody chooses.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AskCall {
    pub question: String,
    pub options: Vec<AskOption>,
    /// A string, or for `multiple` an array of strings; settled against the options.
    pub default: Option<Value>,
    /// The kind whose resources the platform offers (AG-83), as `pick` names it.
    pub pick: Option<&'static str>,
    pub multiple: bool,
    pub min: Option<usize>,
    pub max: Option<usize>,
    pub input: AskInput,
    /// Which step of a path the Portal takes itself this question is (T-2695); the model's own
    /// questions are none.
    pub step: Option<&'static str>,
}

/// The kinds a question may offer by name (AG-83): `pick` as the model spells it, and the kind.
const PICKS: [(&str, &str); 7] = [
    ("endpoints", "Endpoint"),
    ("spaces", "ContextSpace"),
    ("datamodels", "DataModel"),
    ("pipelines", "Pipeline"),
    ("datasources", "DataSource"),
    ("policies", "Policy"),
    ("projects", "Project"),
];

pub(super) fn navigate_call(answer: &str) -> Option<Result<NavigateCall, String>> {
    let value = blocks(answer)
        .find(|value| value.get("tool").and_then(Value::as_str) == Some("jc_ui_navigate"))?;
    let arguments = value.get("arguments").cloned().unwrap_or(value.clone());
    Some(navigate_args(&arguments))
}

/// The page names the assistant may open, as a tool's `enum` lists them.
pub(super) fn page_names() -> Vec<&'static str> {
    PAGES.iter().map(|(name, _)| *name).collect()
}

/// A `jc_ui_navigate` call's arguments, the page checked against [`PAGES`].
pub(super) fn navigate_args(arguments: &Value) -> Result<NavigateCall, String> {
    let Some(page) = arguments.get("page").and_then(Value::as_str) else {
        return Err(format!(
            "jc_ui_navigate names no page; the pages are {}",
            PAGES.map(|(name, _)| name).join(", ")
        ));
    };
    if !PAGES.iter().any(|(name, _)| *name == page) {
        return Err(format!(
            "'{page}' is not a page of the Portal; the pages are {}",
            PAGES.map(|(name, _)| name).join(", ")
        ));
    }
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(NavigateCall {
        page: page.to_owned(),
        name: text("name"),
        plural: text("plural"),
        endpoint: text("endpoint"),
        entity_type: text("type"),
        q: text("q"),
        color: text("color"),
    })
}

/// The route a call opens, or the reason it opens none (UI-59). Every placeholder the template
/// names has to be filled, so a route cannot reach a page with a hole in it; `q` and `color` alone
/// are optional, because a question without a filter is the whole grid.
pub(super) fn route_of(project: &str, call: &NavigateCall) -> Result<String, String> {
    let template = PAGES
        .iter()
        .find(|(name, _)| *name == call.page)
        .map(|(_, template)| *template)
        .ok_or_else(|| format!("'{}' is not a page of the Portal", call.page))?;
    let section = match (call.page.as_str(), call.plural.as_deref()) {
        ("new", Some(plural)) => Some(crate::agents::change::section(plural.trim())?),
        ("draft" | "resource", Some(kind)) => Some(crate::agents::change::page_of(kind.trim())?),
        _ => None,
    };
    // An organization kind is created and edited on its Organization tab, where its drafts are
    // kept (T-2845): the same page under `/organization`, never a project's list of it.
    let template = match (call.page.as_str(), call.plural.as_deref()) {
        ("new" | "draft" | "resource", Some(kind))
            if crate::agents::change::organization_tab(kind.trim()).is_some() =>
        {
            template.replacen("/projects/{project}/", "/organization/", 1)
        }
        _ => template.to_owned(),
    };
    let filled = [
        ("project", Some(project)),
        ("section", section),
        ("name", call.name.as_deref()),
        ("endpoint", call.endpoint.as_deref()),
        ("type", call.entity_type.as_deref()),
        ("q", call.q.as_deref()),
        ("color", call.color.as_deref()),
    ];
    let mut route = template;
    for (placeholder, value) in filled {
        let hole = format!("{{{placeholder}}}");
        if !route.contains(&hole) {
            continue;
        }
        let value = value.unwrap_or_default().trim();
        if value.is_empty() && !matches!(placeholder, "q" | "color") {
            let what = needed(placeholder, &call.page);
            return Err(format!("the page '{}' needs {what}", call.page));
        }
        // A section may be a page inside another (`settings/service-accounts`, T-2606): it comes
        // from the Portal's own table, and each of its segments is still encoded on its own.
        let filled = if placeholder == "section" {
            value
                .split('/')
                .map(urlencoding)
                .collect::<Vec<_>>()
                .join("/")
        } else {
            urlencoding(value)
        };
        route = route.replace(&hole, &filled);
    }
    Ok(without_empty_pairs(&route))
}

/// What an empty `placeholder` of `page` asks the model for, in words it can act on.
fn needed<'a>(placeholder: &'a str, page: &str) -> &'a str {
    match placeholder {
        "name" => "the name of what to open",
        "endpoint" => "the endpoint whose data to open",
        "type" => "the entity type to show",
        "section" if page == "draft" => "the kind of the draft",
        "section" if page == "resource" => "the plural of the resource's kind",
        "section" => "the plural of the kind to create",
        other => other,
    }
}

/// Which draft named `name` a `draft` page opens, as its kind (T-2768). A plural or kind the
/// model gave narrows drafts of one name that differ in kind; one it got wrong does not win over
/// the only draft that has the name. Of two left, the person's own is the one they mean.
fn draft_of(
    drafts: &[crate::ops::drafts::Draft],
    name: &str,
    plural: Option<&str>,
    person: &str,
) -> Result<String, String> {
    let named: Vec<_> = drafts
        .iter()
        .filter(|d| d.workspace.is_none() && d.name == name)
        .collect();
    if named.is_empty() {
        let mut held: Vec<String> = drafts
            .iter()
            .filter(|d| d.workspace.is_none())
            .map(|d| format!("{} '{}'", d.kind, d.name))
            .collect();
        held.sort();
        return Err(if held.is_empty() {
            format!("there is no draft named '{name}': this project holds no drafts")
        } else {
            format!(
                "there is no draft named '{name}'; the drafts of this project are {}",
                held.join(", ")
            )
        });
    }
    let kind = plural.map(str::trim).and_then(|p| {
        crate::resource::by_plural(p)
            .or_else(|| crate::resource::by_kind(p))
            .map(|info| info.kind)
    });
    let of_kind: Vec<_> = named
        .iter()
        .copied()
        .filter(|d| Some(d.kind.as_str()) == kind)
        .collect();
    let left = if of_kind.is_empty() { named } else { of_kind };
    let own: Vec<_> = left
        .iter()
        .copied()
        .filter(|d| d.touched_by == person)
        .collect();
    match (left.as_slice(), own.as_slice()) {
        ([one], _) | (_, [one]) => Ok(one.kind.clone()),
        _ => {
            let mut kinds: Vec<&str> = left.iter().map(|d| d.kind.as_str()).collect();
            kinds.sort_unstable();
            Err(format!(
                "drafts of more than one kind are named '{name}' ({}); name the plural of the \
                 one to open",
                kinds.join(", ")
            ))
        }
    }
}

/// Drops the query pairs nothing filled: an optional placeholder left `q=` behind, and an empty
/// filter is not the request the person asking meant.
fn without_empty_pairs(route: &str) -> String {
    let Some((path, query)) = route.split_once('?') else {
        return route.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.ends_with('='))
        .collect();
    if kept.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

pub(super) fn ask_call(answer: &str) -> Option<Result<AskCall, String>> {
    let value =
        blocks(answer).find(|value| value.get("tool").and_then(Value::as_str) == Some("jc_ask"))?;
    let arguments = value.get("arguments").cloned().unwrap_or(value.clone());
    Some(parse_ask(&arguments))
}

fn parse_ask(arguments: &Value) -> Result<AskCall, String> {
    let question = arguments
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if question.is_empty() {
        return Err("jc_ask asks nothing: give it a question".to_owned());
    }
    let pick = match arguments.get("pick").and_then(Value::as_str) {
        None => None,
        Some(name) => Some(
            PICKS
                .iter()
                .find(|(pick, _)| *pick == name)
                .map(|(pick, _)| *pick)
                .ok_or_else(|| {
                    format!(
                        "'{name}' is not a kind to pick from; pick one of {}",
                        PICKS.map(|(pick, _)| pick).join(", ")
                    )
                })?,
        ),
    };
    let count = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    let (min, max) = (count("min"), count("max"));
    if let (Some(min), Some(max)) = (min, max) {
        if max < min {
            return Err(format!("jc_ask asks for at least {min} and at most {max}"));
        }
    }
    let mut input = AskInput::default();
    let asked: Vec<&Value> = match arguments.get("input") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(many)) => many.iter().collect(),
        Some(one) => vec![one],
    };
    for one in asked {
        match one.as_str() {
            Some("file") => input.file = true,
            Some("url") => input.url = true,
            _ => return Err(format!("jc_ask asks for a file or a url, not {one}")),
        }
    }
    if input != AskInput::default() && multiple_of(arguments) {
        return Err("a question that asks for data takes one answer".to_owned());
    }
    let mut options: Vec<AskOption> = Vec::new();
    for option in arguments
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| {
            option
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let value = option
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .or_else(|| text("value"))
            .or_else(|| text("const"))
            .or_else(|| text("name"))
            .or_else(|| text("title"));
        let Some(value) = value else { continue };
        if options.iter().any(|known| known.value == value) {
            continue;
        }
        options.push(AskOption {
            value: value.to_owned(),
            title: text("title").unwrap_or(value).to_owned(),
            description: text("description").map(str::to_owned),
            disabled: None,
        });
    }
    Ok(AskCall {
        question,
        options,
        default: arguments.get("default").cloned(),
        pick,
        multiple: multiple_of(arguments),
        min,
        max,
        input,
        step: None,
    })
}

fn multiple_of(arguments: &Value) -> bool {
    arguments.get("multiple").and_then(Value::as_bool) == Some(true)
}

/// The kind a `pick` names.
fn picked_kind(pick: &str) -> &'static str {
    PICKS
        .iter()
        .find(|(name, _)| *name == pick)
        .map_or("", |(_, kind)| *kind)
}

/// The options final, the default is settled against them: one of them for one answer, a set of
/// them (at least `min`) for several, the model's own text only for a question with no options.
fn settle(mut call: AskCall) -> Result<AskCall, String> {
    // A disabled option is shown, never suggested.
    let values: Vec<&str> = call
        .options
        .iter()
        .filter(|o| o.disabled.is_none())
        .map(|o| o.value.as_str())
        .collect();
    if call.multiple {
        if values.is_empty() {
            return Err("a question with several answers needs options or a pick".to_owned());
        }
        let min = call.min.unwrap_or(0);
        if values.len() < min {
            return Err(format!(
                "the question asks for at least {min} answers and offers {}",
                values.len()
            ));
        }
        let given: Vec<String> = match &call.default {
            Some(Value::String(one)) => vec![one.clone()],
            Some(Value::Array(many)) => many
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        let mut chosen: Vec<String> = Vec::new();
        for value in given.iter().map(String::as_str) {
            if values.contains(&value) && !chosen.iter().any(|c| c == value) {
                chosen.push(value.to_owned());
            }
        }
        for value in &values {
            if min <= chosen.len() {
                break;
            }
            if !chosen.iter().any(|c| c == value) {
                chosen.push((*value).to_owned());
            }
        }
        chosen.truncate(call.max.unwrap_or(usize::MAX).max(min));
        call.default = Some(json!(chosen));
    } else {
        let given = call
            .default
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_owned);
        call.default = match given {
            Some(one) if values.is_empty() || values.contains(&one.as_str()) => Some(json!(one)),
            _ => values.first().map(|first| json!(first)),
        };
    }
    Ok(call)
}

/// The schema the panel renders: options are a `oneOf` of `{const, title, description}`, several
/// answers an array of them, no options a text box (UI-57, UI-73).
fn question_schema(call: &AskCall) -> Value {
    let choices: Vec<Value> = call
        .options
        .iter()
        .map(|option| {
            let mut one = json!({ "const": option.value, "title": option.title });
            if let Some(description) = &option.description {
                one["description"] = json!(description);
            }
            one
        })
        .collect();
    let mut answer = if call.multiple {
        let mut many = json!({
            "type": "array",
            "title": call.question,
            "items": { "type": "string", "oneOf": choices },
            "uniqueItems": true,
        });
        if let Some(min) = call.min {
            many["minItems"] = json!(min);
        }
        if let Some(max) = call.max {
            many["maxItems"] = json!(max);
        }
        many
    } else {
        let mut one = json!({ "type": "string", "title": call.question });
        if !choices.is_empty() {
            one["oneOf"] = json!(choices);
        }
        one
    };
    if let Some(default) = call.default.as_ref() {
        answer["default"] = default.clone();
    }
    json!({
        "type": "object",
        "title": call.question,
        "properties": { "answer": answer },
        "required": ["answer"],
    })
}

/// One line about a resource a question offers: where it lives and, for an endpoint, what it
/// serves and to whom (AG-83).
fn describe_option(state: &AppState, project: &str, kind: &str, item: &Value) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(space) = item.get("space").and_then(Value::as_str) {
        parts.push(format!("space {space}"));
    }
    if kind == "Endpoint" {
        let name = item.get("name").and_then(Value::as_str)?;
        if let Some(endpoint) = state.mirror.get(project, kind, name) {
            let representations = crate::api::catalogue::served_representations(&endpoint.spec);
            if !representations.is_empty() {
                parts.push(representations.join(", "));
            }
            if let Some(audience) = endpoint.spec["audience"].as_str() {
                parts.push(audience.to_owned());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl Driver {
    /// Opens a page for the person (UI-59). The route is this table's, never the model's text.
    pub(super) async fn open_page(&self, call: &NavigateCall) -> Result<String, String> {
        let started = std::time::Instant::now();
        let resolved = if call.page == "draft" {
            self.draft_kind(call).await.map(|kind| NavigateCall {
                plural: kind,
                ..call.clone()
            })
        } else {
            Ok(call.clone())
        };
        let route = match resolved.and_then(|call| route_of(&self.project, &call)) {
            Ok(route) => route,
            Err(reason) => {
                self.event(
                    "tool",
                    failed_step(
                        "jc_ui_navigate",
                        started,
                        &json!({ "page": call.page }),
                        &reason,
                    ),
                )
                .await?;
                return Ok(format!("error: {reason}"));
            }
        };
        self.event(
            "tool",
            json!({
                "tool": "jc_ui_navigate",
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": {
                    "page": call.page, "name": call.name, "plural": call.plural,
                    "endpoint": call.endpoint, "type": call.entity_type, "q": call.q,
                },
                "output": { "route": route },
            }),
        )
        .await?;
        self.event("navigate", json!({ "route": route })).await?;
        Ok(format!("opened {route}"))
    }

    /// The kind of the draft a `draft` page opens (T-2768), read from the drafts the project
    /// holds under that name, never guessed: a draft of an endpoint opened the model screen when
    /// a missing plural defaulted to `models`. `None` for a call without a name, which the route
    /// refuses in its own words.
    async fn draft_kind(&self, call: &NavigateCall) -> Result<Option<String>, String> {
        let Some(name) = call
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            return Ok(None);
        };
        // Only the drafts the person may read, as `jc_draft_list` lists them (PF-59): a name
        // they could not read is no draft of theirs, and its kind is not told. An organization
        // kind's draft is kept under `org` (T-2845), and read there by the organization's rules.
        let org = crate::permissions::ORG_NAMESPACE;
        let mut homes = vec![self.project.as_str()];
        if self.project != org {
            homes.push(org);
        }
        let mut drafts = Vec::new();
        for home in homes {
            let effective = crate::permissions::for_request(&self.state, &self.identity, home);
            drafts.extend(
                self.state
                    .drafts
                    .list(home)
                    .await
                    .map_err(|e| format!("the drafts cannot be read now: {e}"))?
                    .into_iter()
                    .filter(|draft| {
                        home == self.project
                            || crate::agents::change::organization_tab(&draft.kind).is_some()
                    })
                    .filter(|draft| effective.may_read_manifest(&draft.kind, &draft.manifest)),
            );
        }
        draft_of(&drafts, name, call.plural.as_deref(), &self.created_by).map(Some)
    }

    /// The options of a `pick` question, from what the person may read (AG-83): the same
    /// listing the registry answers them with, narrowed by the names the model gave, never
    /// widened by them. `Err` is the refusal the model reads.
    pub(super) async fn fill_options(&self, mut call: AskCall) -> Result<AskCall, String> {
        if let Some(pick) = call.pick {
            let kind = picked_kind(pick);
            let listed = self.readable(pick).await?;
            let named: Vec<String> = call.options.iter().map(|o| o.value.clone()).collect();
            call.options = listed
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?;
                    if !named.is_empty() && !named.iter().any(|n| n == name) {
                        return None;
                    }
                    Some(AskOption {
                        value: name.to_owned(),
                        title: item
                            .get("title")
                            .and_then(crate::resource::title_text)
                            .unwrap_or_else(|| name.to_owned()),
                        description: describe_option(&self.state, &self.project, kind, item),
                        disabled: None,
                    })
                })
                .collect();
            if call.options.is_empty() {
                return Err(if named.is_empty() {
                    format!(
                        "there are no {pick} in project '{}' to pick from",
                        self.project
                    )
                } else {
                    format!("none of the {pick} named is one the person may pick")
                });
            }
        }
        settle(call)
    }

    /// What of a `pick`'s kind the person may read, as the registry lists it for them (AG-83).
    pub(super) async fn readable(&self, pick: &str) -> Result<Vec<Value>, String> {
        let op = crate::ops::find("jc_resource_list")
            .ok_or_else(|| "the resource listing is not registered".to_owned())?;
        let caller = crate::ops::Caller {
            bearer: None,
            identity: self.identity.clone(),
            via: crate::ops::Via::Agent,
            access: None,
        };
        let listed = crate::ops::call(
            op,
            &caller,
            &self.state,
            &self.project,
            json!({ "kind": picked_kind(pick) }),
        )
        .await
        .map_err(|err| err.to_string())?;
        Ok(listed["items"].as_array().cloned().unwrap_or_default())
    }

    /// Asks the person one question and leaves the turn (AG-80): the answer arrives as an
    /// `answer` event, which starts the next turn with what they chose.
    pub(super) async fn ask_person(
        &self,
        call: &AskCall,
        elapsed_ms: Option<u64>,
    ) -> Result<String, String> {
        let id = format!("q-{}", crate::agents::store::now_rfc3339().replace(':', ""));
        let options: Vec<Value> = call
            .options
            .iter()
            .map(|o| {
                let mut one =
                    json!({ "value": o.value, "title": o.title, "description": o.description });
                if let Some(reason) = &o.disabled {
                    one["disabledReason"] = json!(reason);
                }
                one
            })
            .collect();
        self.event(
            "question",
            json!({
                "questionId": id,
                "schema": question_schema(call),
                "default": call.default,
                "options": options,
                "pick": call.pick,
                "multiple": call.multiple,
                "min": call.min,
                "max": call.max,
                "input": call.input.payload(),
                "step": call.step,
                "elapsedMs": elapsed_ms,
            }),
        )
        .await?;
        self.thought(&call.question).await?;
        Ok(call.question.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_outside_the_enum_is_refused_with_the_pages_there_are() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"/etc/passwd\" } }\n```";
        let err = navigate_call(answer)
            .expect("a call")
            .expect_err("a page nobody named");
        assert!(err.contains("spaces"), "{err}");
    }

    #[test]
    fn a_page_of_the_enum_carries_its_name() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"space\", \"name\": \"helsinki\" } }\n```";
        assert_eq!(
            navigate_call(answer).expect("a call").expect("a page"),
            NavigateCall {
                page: "space".to_owned(),
                name: Some("helsinki".to_owned()),
                plural: None,
                endpoint: None,
                entity_type: None,
                q: None,
                color: None,
            }
        );
    }

    #[test]
    fn seven_options_survive_and_the_default_is_the_first() {
        let answer = r#"```json
{ "tool": "jc_ask", "arguments": { "question": "Which space?", "options": ["a","b","c","d","e","f","g"] } }
```"#;
        let call = settle(ask_call(answer).expect("a call").expect("a question")).expect("settled");
        assert_eq!(call.options.len(), 7);
        assert_eq!(call.default, Some(json!("a")));
        let schema = question_schema(&call);
        assert_eq!(
            schema["properties"]["answer"]["oneOf"][6]["const"],
            json!("g")
        );
        assert_eq!(schema["required"], json!(["answer"]));
    }

    #[test]
    fn an_option_keeps_its_title_and_description_and_unicode() {
        let call = parse_ask(&json!({ "question": "Unit?", "options": [
            { "value": "ugm3", "title": "µg/m³", "description": "Mikrogramm je Kubikmeter" }] }))
        .expect("a question");
        let schema = question_schema(&settle(call).expect("settled"));
        let one = &schema["properties"]["answer"]["oneOf"][0];
        assert_eq!(one["const"], "ugm3");
        assert_eq!(one["title"], "µg/m³");
        assert_eq!(one["description"], "Mikrogramm je Kubikmeter");
    }

    #[test]
    fn several_answers_are_an_array_whose_default_is_a_subset() {
        let call = parse_ask(&json!({ "question": "Which?", "options": ["a", "b", "c"],
            "multiple": true, "min": 1, "max": 2, "default": ["c", "zzz"] }))
        .expect("a question");
        let call = settle(call).expect("settled");
        assert_eq!(call.default, Some(json!(["c"])));
        let answer = &question_schema(&call)["properties"]["answer"];
        assert_eq!(answer["type"], "array");
        assert_eq!(answer["minItems"], 1);
        assert_eq!(answer["maxItems"], 2);
        assert_eq!(answer["uniqueItems"], true);
        assert_eq!(answer["items"]["oneOf"][1]["const"], "b");
    }

    #[test]
    fn a_minimum_the_options_cannot_meet_and_an_unknown_pick_are_refused() {
        let call = parse_ask(
            &json!({ "question": "Which?", "options": ["a"], "multiple": true, "min": 2 }),
        )
        .expect("parsed");
        assert!(settle(call).is_err());
        let err =
            parse_ask(&json!({ "question": "Which?", "pick": "secrets" })).expect_err("refused");
        assert!(err.contains("endpoints"), "{err}");
        assert!(parse_ask(&json!({ "question": "Which?", "min": 3, "max": 1 })).is_err());
    }

    #[test]
    fn a_question_asks_for_a_file_or_an_address_and_nothing_else() {
        let call = parse_ask(&json!({ "question": "Where is it?", "input": ["file", "url"] }))
            .expect("a question");
        assert_eq!(
            call.input,
            AskInput {
                file: true,
                url: true
            }
        );
        let input = call.input.payload().expect("an input");
        assert_eq!(input["file"]["accept"], json!(SAMPLE_FORMATS));
        assert_eq!(
            input["file"]["accept"],
            json!(["csv", "tsv", "xlsx", "json"])
        );
        assert_eq!(input["file"]["maxBytes"], json!(SAMPLE_MAX_BYTES));
        assert_eq!(input["url"], json!(true));
        let one = parse_ask(&json!({ "question": "Which file?", "input": "file" })).expect("one");
        assert_eq!(
            one.input,
            AskInput {
                file: true,
                url: false
            }
        );
        assert!(AskInput::default().payload().is_none());
        assert!(parse_ask(&json!({ "question": "Run?", "input": ["shell"] })).is_err());
        assert!(
            parse_ask(&json!({ "question": "Which?", "options": ["a", "b"],
            "multiple": true, "input": ["file"] }))
            .is_err()
        );
    }

    #[test]
    fn a_disabled_option_is_shown_and_never_suggested() {
        let mut call = parse_ask(
            &json!({ "question": "From?", "options": ["datasource", "space"],
            "default": "datasource" }),
        )
        .expect("a question");
        call.options[0].disabled = Some("This project has no data source yet.".to_owned());
        let call = settle(call).expect("settled");
        assert_eq!(call.default, Some(json!("space")));
        assert_eq!(
            call.options.len(),
            2,
            "the disabled option stays, with its reason"
        );
    }

    #[test]
    fn the_model_reads_an_answer_as_words_and_a_file_as_its_shape() {
        assert_eq!(answer_text(&json!({ "answer": "space" })), "space");
        assert_eq!(answer_text(&json!({ "answer": ["a", "b"] })), "a, b");
        assert_eq!(
            answer_text(&json!({ "url": "https://example.org/feed.json" })),
            "The data is at https://example.org/feed.json"
        );
        let text: String = (0..40).map(|row| format!("r{row},```x\n")).collect();
        let read =
            answer_text(&json!({ "file": { "name": "a.csv", "format": "csv", "text": text } }));
        assert!(
            read.starts_with("I handed over the file a.csv (csv, "),
            "{read}"
        );
        assert!(
            read.contains("40 lines") && read.contains("r11,") && !read.contains("r12,"),
            "{read}"
        );
        assert_eq!(
            read.matches("```").count(),
            2,
            "the file cannot close the fence: {read}"
        );
    }

    #[test]
    fn a_question_without_options_is_a_text_box() {
        let answer = "```json\n{ \"tool\": \"jc_ask\", \"arguments\": { \"question\": \"What should it be called?\" } }\n```";
        let call = ask_call(answer).expect("a call").expect("a question");
        assert!(call.options.is_empty());
        assert!(question_schema(&call)["properties"]["answer"]["oneOf"].is_null());
    }

    #[test]
    fn a_question_with_no_text_is_refused() {
        let answer =
            "```json\n{ \"tool\": \"jc_ask\", \"arguments\": { \"question\": \"  \" } }\n```";
        assert!(ask_call(answer).expect("a call").is_err());
    }

    #[test]
    fn a_jc_block_is_a_call_and_anything_else_is_not() {
        let answer = r#"Sure.

```json
{ "tool": "jc_catalog_search", "arguments": { "q": "bikes" } }
```

```json
{ "tool": "query_endpoint", "endpoint": "e", "name": "n", "arguments": {} }
```
"#;
        assert_eq!(
            calls(answer),
            vec![RegistryCall {
                name: "jc_catalog_search".to_owned(),
                arguments: json!({ "q": "bikes" }),
            }]
        );
    }

    #[test]
    fn a_call_without_arguments_is_an_empty_object() {
        let answer = "```json\n{ \"tool\": \"jc_draft_list\" }\n```";
        assert_eq!(calls(answer)[0].arguments, json!({}));
    }

    /// T-6479: the two answers the live walk T-5962 showed as prose (evidence/T-5962,
    /// first-pass/assistant/S-27.json and assistant/S-43.json).
    const STRAY_BRACE: &str =
        "```json\n{ \"tool\": \"describe_tool\", \"name\": \"grant_role\" }\n}\n```";
    const NO_NAME: &str = "```json\n{\"additionalPropertiesSchema\":{\"name\":\"change-resource\"},\"tool\":\"describe_tool\"}\n```";

    #[test]
    fn a_recorded_call_nothing_runs_goes_back_with_the_reason() {
        assert!(describes(STRAY_BRACE).is_empty() && calls(STRAY_BRACE).is_empty());
        let (tool, reason) = unrun_call(STRAY_BRACE).expect("the stray brace");
        assert_eq!(tool, "describe_tool");
        assert!(reason.contains("not one JSON object"), "{reason}");
        assert!(describes(NO_NAME).is_empty() && calls(NO_NAME).is_empty());
        let (tool, reason) = unrun_call(NO_NAME).expect("the missing name");
        assert_eq!(tool, "describe_tool");
        assert!(reason.contains("takes `name`"), "{reason}");
    }

    #[test]
    fn a_broken_call_is_found_however_it_is_broken() {
        for answer in [
            "Looking.\n```json\n{ \"tool\": \"jc_draft_list\", \"arguments\": {\n```",
            "```json\n{ \"tool\": \"jc_draft_list\"",
            "```json\n[{ \"tool\": \"jc_draft_list\" }]\n```",
            "```\n{ 'tool': 'jc_draft_list' }\n```",
        ] {
            assert_eq!(
                unrun_call(answer).map(|(tool, _)| tool).as_deref(),
                Some("jc_draft_list"),
                "{answer}"
            );
        }
        let unknown = "```json\n{ \"tool\": \"made_up\" }\n```";
        assert_eq!(unrun_call(unknown).expect("unknown").0, "made_up");
    }

    #[test]
    fn prose_and_calls_that_run_are_no_unrun_call() {
        for answer in [
            "Twelve stations.",
            "The body:\n```json\n{ \"title\": \"a\" }\n```",
            "```bash\ncurl -d '{\"tool\": \"x\"}'\n```",
            "```json\n{ \"tool\": \"edit_endpoint\", \"name\": \"a\" }\n```",
            "```json\n{ \"tool\": \"navigate\", \"route\": \"/\" }\n```",
            "```json\n{ \"tool\": \"propose_endpoint\" }\n```",
            "```json\n{ \"tool\": \"space_complete\" }\n```",
        ] {
            assert_eq!(unrun_call(answer), None, "{answer}");
        }
    }

    #[test]
    fn only_the_broken_block_leaves_the_answer() {
        let kept = "```json\n{ \"title\": \"a\" }\n```";
        assert_eq!(
            without_unrun_calls(&format!("The body:\n{kept}\n{STRAY_BRACE}")),
            format!("The body:\n{kept}")
        );
        assert_eq!(without_unrun_calls(NO_NAME), "");
    }

    #[test]
    fn describe_names_the_operation_it_asks_about() {
        let answer = "```json\n{ \"tool\": \"describe_tool\", \"name\": \"jc_kpi_compute\" }\n```";
        assert_eq!(describes(answer), vec!["jc_kpi_compute".to_owned()]);
    }

    /// AG-77: the assistant opens the kind's form prefilled and never proposes on its own. The
    /// whole registry is walked, so an operation added or renamed later is classified here too
    /// rather than quietly reaching the approval queue from the dock.
    #[test]
    fn every_operation_that_opens_a_change_is_kept_from_the_conversation() {
        let opening: Vec<&str> = crate::ops::registry()
            .iter()
            .map(|op| op.name)
            .filter(|name| opens_a_change(name).is_some())
            .collect();
        assert_eq!(
            opening,
            vec![
                "jc_endpoint_propose",
                "jc_resource_propose",
                "jc_resource_delete",
                "jc_project_delete",
                "jc_syncsource_sync",
                "jc_syncsource_detach",
                "jc_workspace_propose",
                "jc_change_apply",
            ],
            "the operations the conversation may not call, in registry order"
        );
        // What the conversation does need: it drafts, checks and tests, and it reads.
        for kept in [
            "jc_draft_put",
            "jc_manifest_dry_run",
            "jc_datasource_check",
            "jc_pipeline_test",
            "jc_kpi_compute",
            "jc_space_complete",
            "jc_catalog_search",
            "jc_resource_get",
            "jc_change_list",
            "jc_workspace_open",
            "jc_syncsource_pause",
        ] {
            assert!(
                opens_a_change(kept).is_none(),
                "{kept} is how the conversation works and stays offered"
            );
        }
        // And the way out is named, so the refusal tells the model what to do instead.
        assert!(opens_a_change("jc_resource_propose")
            .expect("named")
            .contains("change_resource"));
    }

    #[test]
    fn a_read_only_operation_does_not_act_and_an_unknown_one_does() {
        assert!(!acts("jc_catalog_search"));
        assert!(acts("jc_endpoint_propose"));
        assert!(acts("jc_not_an_operation"));
    }

    #[test]
    fn the_section_says_which_operations_change_something() {
        let ops = crate::ops::registry();
        let summaries: Vec<OperationSummary> = ops
            .iter()
            .filter(|op| op.name == "jc_catalog_search" || op.name == "jc_endpoint_propose")
            .map(crate::ops::summary)
            .collect();
        let text = section(&summaries);
        assert!(text.contains("`jc_catalog_search`"), "{text}");
        assert!(
            text.contains("`jc_endpoint_propose` —")
                && text.contains("(changes something: a draft, a Verdict or a Change)"),
            "{text}"
        );
    }

    #[test]
    fn no_operation_is_no_section() {
        assert_eq!(section(&[]), "");
    }

    /// T-1011, T-1012: the assistant opens the page a person would, so every route
    /// `ui/src/router.tsx` serves is one it can name. A page the router has and the table does
    /// not is a page the person has to find by hand while the assistant says it cannot.
    #[test]
    fn every_page_a_person_reaches_is_one_the_assistant_can_name() {
        for page in [
            "activity",
            "approvals",
            "approval",
            "explore",
            "entity",
            "ckan",
            "assistant",
            "app",
            "resource",
            "entities",
        ] {
            assert!(
                PAGES.iter().any(|(name, _)| *name == page),
                "{page} is a route of the Portal and not a page of the table"
            );
        }
    }

    /// Every template names only the placeholders the call can fill, so a route cannot be built
    /// with a hole in it.
    #[test]
    fn a_page_template_names_only_the_placeholders_a_call_fills() {
        for (page, template) in PAGES {
            let mut rest = template;
            while let Some(start) = rest.find('{') {
                let end = rest[start..]
                    .find('}')
                    .map(|offset| start + offset)
                    .unwrap_or_else(|| panic!("{page}: unclosed placeholder in {template}"));
                let placeholder = &rest[start + 1..end];
                assert!(
                    matches!(
                        placeholder,
                        "project"
                            | "name"
                            | "plural"
                            | "section"
                            | "endpoint"
                            | "type"
                            | "q"
                            | "color"
                    ),
                    "{page}: {template} names {placeholder}, which no call fills"
                );
                rest = &rest[end + 1..];
            }
        }
    }

    /// A page that takes a name is refused without one rather than opening the list instead.
    #[test]
    fn the_new_pages_carry_what_they_name() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"approval\", \"name\": \"chg-0000beef\" } }\n```";
        let call = navigate_call(answer).expect("a call").expect("a page");
        assert_eq!(call.page, "approval");
        assert_eq!(call.name.as_deref(), Some("chg-0000beef"));
    }

    /// The grid opens on the filter the question carried, and the filter reaches the route encoded
    /// (UI-64, UI-67): `==` and the space of a two-word value are not query syntax of their own.
    #[test]
    fn the_entities_page_opens_the_grid_narrowed_by_the_question() {
        let answer = r#"```json
{ "tool": "jc_ui_navigate", "arguments": { "page": "entities", "endpoint": "bikes-public",
  "type": "BikeHireDockingStation", "q": "availableBikeNumber==0;name~=\"Rautatientori\"" } }
```"#;
        let call = navigate_call(answer).expect("a call").expect("a page");
        assert_eq!(call.endpoint.as_deref(), Some("bikes-public"));
        assert_eq!(call.entity_type.as_deref(), Some("BikeHireDockingStation"));
        assert_eq!(
            route_of("helsinki", &call).expect("a route"),
            "/projects/helsinki/explore?endpoint=bikes-public&type=BikeHireDockingStation\
             &q=availableBikeNumber%3D%3D0%3Bname~%3D%22Rautatientori%22"
        );
    }

    /// Without a filter the grid opens on the whole type, and the route carries no empty pair:
    /// `q=` is a filter that matches nothing, not the absence of one. A model that answers `""`
    /// or a blank means the same as one that leaves it out.
    #[test]
    fn a_grid_without_a_filter_carries_no_empty_query_pair() {
        for q in [None, Some(""), Some("   ")] {
            let call = NavigateCall {
                page: "entities".to_owned(),
                name: None,
                plural: None,
                endpoint: Some("air-public".to_owned()),
                entity_type: Some("AirQualityObserved".to_owned()),
                q: q.map(str::to_owned),
                color: None,
            };
            assert_eq!(
                route_of("helsinki", &call).expect("a route"),
                "/projects/helsinki/explore?endpoint=air-public&type=AirQualityObserved",
                "q = {q:?}"
            );
        }
    }

    /// T-6442: "map the stations" ends in a map: the same view as the grid, on its map tab, with
    /// the colour attribute and the filter optional and the endpoint and type required.
    #[test]
    fn the_map_page_opens_the_view_on_its_map() {
        let call = |q: Option<&str>, color: Option<&str>, endpoint: Option<&str>| NavigateCall {
            page: "map".to_owned(),
            name: None,
            plural: None,
            endpoint: endpoint.map(str::to_owned),
            entity_type: Some("BikeHireDockingStation".to_owned()),
            q: q.map(str::to_owned),
            color: color.map(str::to_owned),
        };
        assert_eq!(
            route_of(
                "helsinki",
                &call(None, Some("availableBikeNumber"), Some("bikes"))
            )
            .expect("a route"),
            "/projects/helsinki/explore?endpoint=bikes&type=BikeHireDockingStation\
             &colorBy=availableBikeNumber&view=map"
        );
        assert_eq!(
            route_of(
                "helsinki",
                &call(Some("status==\"working\""), None, Some("bikes"))
            )
            .expect("a route"),
            "/projects/helsinki/explore?endpoint=bikes&type=BikeHireDockingStation\
             &q=status%3D%3D%22working%22&view=map"
        );
        assert!(route_of("helsinki", &call(None, None, None))
            .expect_err("no endpoint")
            .contains("the endpoint"));
    }

    /// T-2577, AG-73: "create a ServiceAccount" opens the kind's empty form in the section a
    /// person creates one in; the kind may be named by plural or by kind, and what is no kind is
    /// refused rather than opening a page with nothing on it.
    #[test]
    fn the_new_page_opens_a_kinds_create_form_in_its_section() {
        let new = |plural: Option<&str>| NavigateCall {
            page: "new".to_owned(),
            name: None,
            plural: plural.map(str::to_owned),
            endpoint: None,
            entity_type: None,
            q: None,
            color: None,
        };
        for (plural, route) in [
            (
                "serviceaccounts",
                "/projects/helsinki/settings/service-accounts/new",
            ),
            (
                "ServiceAccount",
                "/projects/helsinki/settings/service-accounts/new",
            ),
            ("subscriptions", "/projects/helsinki/subscriptions/new"),
            ("csrs", "/projects/helsinki/csrs/new"),
            ("syncsources", "/projects/helsinki/syncsources/new"),
        ] {
            assert_eq!(
                route_of("helsinki", &new(Some(plural))).expect("a route"),
                route,
                "{plural}"
            );
        }
        let unknown = route_of("helsinki", &new(Some("widgets"))).expect_err("no kind");
        assert!(unknown.contains("widgets"), "{unknown}");
        // T-2582: a kind whose page has no routed create form is not sent to a `/new` that
        // answers "this form cannot be opened".
        let formless = route_of("helsinki", &new(Some("mappings"))).expect_err("no form");
        assert!(formless.contains("Mapping"), "{formless}");
        let none = route_of("helsinki", &new(None)).expect_err("no plural");
        assert!(none.contains("plural"), "{none}");
    }

    fn draft(kind: &str, name: &str, by: &str) -> crate::ops::drafts::Draft {
        crate::ops::drafts::Draft {
            project: "helsinki".to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            workspace: None,
            manifest: json!({}),
            verdict: None,
            touched_by: by.to_owned(),
            touched_kind: "assistant".to_owned(),
            version: 1,
            updated_at: chrono::Utc::now(),
        }
    }

    fn open(page: &str, name: &str, plural: Option<&str>) -> NavigateCall {
        NavigateCall {
            page: page.to_owned(),
            name: Some(name.to_owned()),
            plural: plural.map(str::to_owned),
            endpoint: None,
            entity_type: None,
            q: None,
            color: None,
        }
    }

    /// T-2768: a draft opens on its own kind's page, and a draft or resource page without a
    /// kind is refused instead of opening the model screen, as `sample-endpoint` did.
    #[test]
    fn a_draft_opens_on_its_kinds_page_and_never_on_a_guessed_one() {
        for (kind, route) in [
            (
                "Endpoint",
                "/projects/helsinki/endpoints?draft=sample-endpoint",
            ),
            (
                "DataModel",
                "/projects/helsinki/models?draft=sample-endpoint",
            ),
            (
                "ContextSpace",
                "/projects/helsinki/spaces?draft=sample-endpoint",
            ),
        ] {
            let call = open("draft", "sample-endpoint", Some(kind));
            assert_eq!(route_of("helsinki", &call).expect("a route"), route);
        }
        let none =
            route_of("helsinki", &open("draft", "sample-endpoint", None)).expect_err("no kind");
        assert!(none.contains("the kind of the draft"), "{none}");
        assert_eq!(
            route_of("helsinki", &open("resource", "air", Some("datamodels"))).expect("a route"),
            "/projects/helsinki/models?name=air"
        );
        let none = route_of("helsinki", &open("resource", "air", None)).expect_err("no kind");
        assert!(none.contains("the plural of the resource's kind"), "{none}");
    }

    /// T-2845: an organization kind's empty form, draft and resource open on its Organization
    /// tab, where the form reads the drafts kept under `org`, never under the project.
    #[test]
    fn an_organization_kind_opens_on_the_organization_page() {
        let new = |plural: &str| NavigateCall {
            page: "new".to_owned(),
            name: None,
            plural: Some(plural.to_owned()),
            endpoint: None,
            entity_type: None,
            q: None,
            color: None,
        };
        for (kind, plural) in [
            ("Blueprint", "blueprints"),
            ("AgentProfile", "agentprofiles"),
            ("DataSpaceParticipant", "dataspaceparticipants"),
            ("Environment", "environments"),
        ] {
            for asked in [kind, plural] {
                assert_eq!(
                    route_of("helsinki", &new(asked)).expect("a route"),
                    format!("/organization/{plural}/new"),
                    "{asked}"
                );
                assert_eq!(
                    route_of("helsinki", &open("draft", "weather", Some(asked))).expect("a route"),
                    format!("/organization/{plural}?draft=weather"),
                    "{asked}"
                );
                assert_eq!(
                    route_of("helsinki", &open("resource", "weather", Some(asked)))
                        .expect("a route"),
                    format!("/organization/{plural}?name=weather"),
                    "{asked}"
                );
            }
        }
        // A project kind stays in the project.
        assert_eq!(
            route_of("helsinki", &new("pipelines")).expect("a route"),
            "/projects/helsinki/pipelines/new"
        );
    }

    #[test]
    fn the_draft_the_name_means_is_found_by_its_name() {
        let me = "piper@hel.fi";
        let drafts = vec![
            draft("Endpoint", "sample-endpoint", me),
            draft("DataModel", "bikes", me),
            draft("ContextSpace", "bikes", "someone@hel.fi"),
        ];
        // The only draft of the name wins over a plural the model got wrong.
        for plural in [None, Some("endpoints"), Some("models")] {
            assert_eq!(
                draft_of(&drafts, "sample-endpoint", plural, me).as_deref(),
                Ok("Endpoint"),
                "{plural:?}"
            );
        }
        assert_eq!(
            draft_of(&drafts, "bikes", Some("spaces"), me).as_deref(),
            Ok("ContextSpace")
        );
        assert_eq!(
            draft_of(&drafts, "bikes", None, me).as_deref(),
            Ok("DataModel")
        );
        let two = draft_of(&drafts, "bikes", None, "viewer@hel.fi").expect_err("two kinds");
        assert!(two.contains("ContextSpace, DataModel"), "{two}");
        let unknown = draft_of(&drafts, "trams", None, me).expect_err("no draft");
        assert!(
            unknown.contains("DataModel 'bikes'") && unknown.contains("Endpoint 'sample-endpoint'"),
            "{unknown}"
        );
        let mut elsewhere = draft("Endpoint", "trams", me);
        elsewhere.workspace = Some("try-trams".to_owned());
        let empty = draft_of(&[elsewhere], "trams", None, me).expect_err("in a workspace");
        assert!(empty.contains("holds no drafts"), "{empty}");
    }

    /// A grid without an endpoint or without a type is refused: the explorer would open on
    /// nothing and the person would read the assistant's sentence as if it had.
    #[test]
    fn a_grid_missing_its_endpoint_or_its_type_is_refused() {
        for (endpoint, entity_type, needed) in [
            (None, Some("AirQualityObserved"), "the endpoint"),
            (Some("air-public"), None, "the entity type"),
            (Some("air-public"), Some("   "), "the entity type"),
        ] {
            let call = NavigateCall {
                page: "entities".to_owned(),
                name: None,
                plural: None,
                endpoint: endpoint.map(str::to_owned),
                entity_type: entity_type.map(str::to_owned),
                q: Some("availableBikeNumber==0".to_owned()),
                color: None,
            };
            let err = route_of("helsinki", &call).expect_err("no route");
            assert!(err.contains(needed), "{err}");
        }
    }
}
