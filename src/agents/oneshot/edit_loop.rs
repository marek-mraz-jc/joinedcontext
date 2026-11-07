//! The editing agent: every instruction after the first run is a tool loop over the run's
//! files, the check, the functions runtime and the frame's errors (SDK-20, SDK-11, SDK-12,
//! SDK-17, AP-60, AG-25). The first run stays a whole-project pass; from then on the model
//! reads what it edits, checks what it changed and finishes with a sentence for the person.

use std::time::Instant;

use super::model::{assistant_turn_message, tool_result_message, ToolAnswer, ToolCall, ToolSpec};
use super::*;
use crate::agents::{code, patch};
use crate::api::agent_runs::{invoke_function, Caller, InvokeError};

/// Lines one `read_file` returns at most: a whole file of the application, as a rule.
const READ_WINDOW: usize = 2000;
/// Bytes of the application's own files the opening turn carries whole, smallest first: an edit
/// then starts from the code instead of a crawl of `read_file` windows (T-0896).
const OPENING_BYTES: usize = 200_000;
/// What a tool result carries back to the model at most.
const RESULT_CAP: usize = 8 * 1024;
/// What a tool event keeps of a result at most.
const EVENT_CAP: usize = 2 * 1024;
/// How many `preview_error` events one call returns at most.
const ERROR_WINDOW: usize = 20;
/// Output budget of one editing call. Most answers are a tool call or a sentence, and only what
/// is written is paid for; but one answer carries every edit a request needs, a reasoning model
/// thinks inside the same budget, and 8000 cut a two-file follow-up on dev with nothing applied
/// (T-3044).
const EDIT_OUTPUT_BUDGET: u32 = 32_000;
/// Rounds in a row that end with the same problems before the turn stops (T-3077).
const NO_PROGRESS_ROUNDS: u32 = 3;
/// The failing tool calls of one turn, by tool and input: how many times in a row each answered
/// the same failure (T-3135). A check alternating with the same failing `call_function` never
/// ended two rounds alike, so T-3077's count never saw it, and twelve identical 500s spent the
/// whole instruction.
#[derive(Default)]
struct Repeats(std::collections::HashMap<String, (String, u32)>);

impl Repeats {
    /// Notes one call's answer and returns how many times in a row this very call has failed
    /// with this very answer; a success or another answer starts it over.
    fn note(&mut self, tool: &str, input: &Value, result: &str, ok: bool) -> u32 {
        let key = format!("{tool} {input}");
        if ok {
            self.0.remove(&key);
            return 0;
        }
        let entry = self.0.entry(key).or_insert_with(|| (result.to_owned(), 0));
        if entry.0 == result {
            entry.1 += 1;
        } else {
            *entry = (result.to_owned(), 1);
        }
        entry.1
    }
}

/// What `preview_errors` answers when the frame reported nothing.
const NO_PREVIEW_ERRORS: &str = "no errors since the last reload";

/// What `call_function` answers when the function's data call was refused for want of an identity:
/// the run's call reads as the person who started it (ADR-N-038 decision 6), and this run holds no
/// grant of theirs, so calling it again would only repeat the refusal.
const FUNCTION_READS_DATA: &str = "status 401\nThe function asked its endpoint for data and was \
     refused: this run reads as the person who started it, and it holds no identity of theirs (the \
     run was not started from a signed-in session, or that session has ended). This is not a fault \
     in the function: its data path is proven by its own test over `fakeContext`. Do not call it \
     again; call a function that computes without reading data, or change its test.";
/// Model calls one instruction may make (SDK-20): a request that needs more is asked in steps.
const EDIT_CALLS: u32 = 12;
/// Input tokens one instruction may send, over all of its calls (SDK-20).
const EDIT_INPUT_TOKENS: u64 = 500_000;

/// The rules of the loop, as the model reads them (SDK-11, SDK-12, SDK-20).
const EDIT_SYSTEM: &str =
    "You edit a small web application that already runs. You work with tools only: \
list_files, read_file, edit_file, write_file, delete_file, check, call_function, preview_errors \
and finish.\n\
Read a file before you edit it, and edit with an exact search text. You may write only \
src/**/*.tsx, src/**/*.ts, functions/*.ts and tests/**/*.test.ts; imports come from \
@joinedcontext/sdk, react and the application's own files, nothing else. Run check after you \
change files; the preview reloads when a check passes. preview_errors tells you what the frame \
reported since the last reload; call_function runs one of the application's functions. When \
the request is done, call finish with one plain sentence for the person: what changed, in \
their words, no file names unless they asked.\n\
Every model call costs the person time and money, so make each one count: the files shown in \
the request are as the turn began, do not read them again unless you changed them; put several \
tool calls in one answer when they do not depend on each other, every edit the request needs \
in one answer, then check once. Your last calls come back whole, the newest files you read stay \
in the request while they are unchanged, and every other earlier call is one line: call a tool \
again only if you need its answer.";

/// The tools of the loop, in the shape both providers read.
fn tools() -> Vec<ToolSpec> {
    let path = json!({ "type": "string", "description": "the file, relative to the application" });
    vec![
        ToolSpec {
            name: "list_files".into(),
            description: "Every file of the application with its size.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "read_file".into(),
            description: "A file whole, or the lines from..to of it (1-based); read a file whole unless it is very long.".into(),
            input_schema: json!({ "type": "object", "properties": { "path": path, "from": { "type": "integer" }, "to": { "type": "integer" } }, "required": ["path"] }),
        },
        ToolSpec {
            name: "edit_file".into(),
            description: "Replaces one exact occurrence of search with replace; an empty search creates or overwrites the file.".into(),
            input_schema: json!({ "type": "object", "properties": { "path": path, "search": { "type": "string" }, "replace": { "type": "string" } }, "required": ["path", "search", "replace"] }),
        },
        ToolSpec {
            name: "write_file".into(),
            description: "Writes a whole file.".into(),
            input_schema: json!({ "type": "object", "properties": { "path": path, "content": { "type": "string" } }, "required": ["path", "content"] }),
        },
        ToolSpec {
            name: "delete_file".into(),
            description: "Removes a file.".into(),
            input_schema: json!({ "type": "object", "properties": { "path": path }, "required": ["path"] }),
        },
        ToolSpec {
            name: "check".into(),
            description: "Builds the application: ok, or every problem, one per line.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "call_function".into(),
            description: "Runs functions/{name}.ts of the application with a JSON body and returns its status and answer.".into(),
            input_schema: json!({ "type": "object", "properties": { "name": { "type": "string" }, "body": {}, "query": { "type": "object" } }, "required": ["name"] }),
        },
        ToolSpec {
            name: "preview_errors".into(),
            description: "The errors the preview frame reported since the last reload, oldest first.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "finish".into(),
            description: "Ends the turn with one sentence for the person.".into(),
            input_schema: json!({ "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }),
        },
    ]
}

/// A path the model named, made safe: relative, no `..`, no empty segment (SDK-11).
fn clean_path(raw: &str) -> Option<String> {
    let path = raw.trim();
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    Some(parts.join("/"))
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(limit);
    format!("{}\n… ({} more bytes)", &text[..end], text.len() - end)
}

fn arg<'a>(input: &'a Value, name: &str) -> &'a str {
    input.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn list_files(files: &BTreeMap<String, String>) -> String {
    files
        .iter()
        .map(|(path, content)| format!("{path} ({} bytes)", content.len()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every file with its size, and the application's own files whole while they fit in
/// `OPENING_BYTES`, smallest first; a file left out says so, so the model reads it. Answers the
/// paths it carried whole beside the text.
fn opening(files: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let mut out = String::from("Files:\n");
    out.push_str(&list_files(files));
    let template = crate::agents::preview::template_files();
    let mut own: Vec<(&String, &String)> = files
        .iter()
        .filter(|(path, _)| code::writable(path))
        .collect();
    own.sort_by_key(|(path, content)| (content.len(), (*path).clone()));
    let mut carried = 0usize;
    let mut whole = Vec::new();
    let mut left_out = Vec::new();
    for (path, content) in own {
        // An unchanged template component by its exports, an unchanged test by name (T-3076).
        let text = match code::shown(path, content, &template) {
            code::Shown::Whole => None,
            code::Shown::Outline(exports) => Some(exports),
            code::Shown::Named => {
                left_out.push(path.as_str());
                continue;
            }
        };
        let size = text.as_ref().map_or(content.len(), String::len);
        if carried + size > OPENING_BYTES {
            left_out.push(path.as_str());
            continue;
        }
        carried += size;
        match text {
            Some(exports) => out.push_str(&format!(
                "\n\n=== {path} (unchanged template component, its exports; read_file for the whole) ===\n{exports}"
            )),
            None => {
                whole.push(path.clone());
                out.push_str(&format!("\n\n=== {path} ===\n{content}"));
            }
        }
    }
    if !left_out.is_empty() {
        out.push_str(&format!(
            "\n\nNot shown, read before editing: {}",
            left_out.join(", ")
        ));
    }
    (out, whole)
}

/// A read of a file the opening message already carries whole, unchanged since, answers that
/// instead of the file a second time (SDK-20).
fn read_again(
    files: &BTreeMap<String, String>,
    shown: &BTreeMap<String, String>,
    input: &Value,
) -> (String, bool) {
    let path = clean_path(arg(input, "path"));
    let windowed = input.get("from").is_some() || input.get("to").is_some();
    match path {
        Some(path)
            if !windowed
                && shown
                    .get(&path)
                    .is_some_and(|c| files.get(&path) == Some(c)) =>
        {
            (
                format!("{path}: shown whole in the request and unchanged since; read it there"),
                true,
            )
        }
        _ => read_file(files, input),
    }
}

/// One model call of a turn and what its tool calls answered, by call id.
struct Round {
    answer: ToolAnswer,
    results: Vec<(String, String)>,
}

/// How long the running summary of a turn's earlier rounds may grow, about 500 tokens (T-3074).
const SUMMARY_CHARS: usize = 2_000;
/// How much of a result's first line one summary line repeats.
const SUMMARY_LINE: usize = 120;
/// What the reads of earlier rounds may keep in the request, about 4000 tokens (T-6385).
const KEPT_READS: usize = 2 * RESULT_CAP;

/// The earlier rounds of a turn as one short text (T-3074): the files changed so far, always,
/// then one line per tool call (its name, its file, the first line of its result), the newest
/// that fit. Empty when there is nothing earlier.
fn summary(rounds: &[Round]) -> String {
    let mut changed = std::collections::BTreeSet::new();
    let mut lines = Vec::new();
    for round in rounds {
        for call in &round.answer.calls {
            let path = call.input.get("path").and_then(Value::as_str);
            if matches!(
                call.name.as_str(),
                "edit_file" | "write_file" | "delete_file"
            ) {
                if let Some(path) = path {
                    changed.insert(path.to_owned());
                }
            }
            let first = round
                .results
                .iter()
                .find(|(id, _)| *id == call.id)
                .and_then(|(_, result)| result.lines().next())
                .unwrap_or_default();
            lines.push(format!(
                "- {}{}: {}\n",
                call.name,
                path.map(|path| format!(" {path}")).unwrap_or_default(),
                cap(first, SUMMARY_LINE)
            ));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("Earlier in this turn, one line per tool call:\n");
    if !changed.is_empty() {
        let names = changed.into_iter().collect::<Vec<_>>().join(", ");
        out.push_str(&format!(
            "Files you changed: {}; read_file shows their current text.\n",
            cap(&names, SUMMARY_CHARS / 4)
        ));
    }
    let mut kept = 0;
    let mut size = out.len();
    for line in lines.iter().rev() {
        if size + line.len() > SUMMARY_CHARS {
            break;
        }
        size += line.len();
        kept += 1;
    }
    let left_out = lines.len() - kept;
    if left_out > 0 {
        out.push_str(&format!("({left_out} earlier steps left out)\n"));
    }
    for line in &lines[left_out..] {
        out.push_str(line);
    }
    out
}

/// The newest reads of the rounds before the last, whole, while the file stands as it was read
/// (T-6385). A model that reads one file per call lost each file on the next call, read it again
/// and spent all of `EDIT_CALLS` on reading without one edit: a two-line follow-up on dev.
fn kept_reads(rounds: &[Round]) -> String {
    let mut changed = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut kept = String::new();
    for (age, round) in rounds.iter().rev().enumerate() {
        for call in round.answer.calls.iter().rev() {
            let Some(path) = call.input.get("path").and_then(Value::as_str) else {
                continue;
            };
            if call.name != "read_file" {
                if matches!(
                    call.name.as_str(),
                    "edit_file" | "write_file" | "delete_file"
                ) {
                    changed.insert(path.to_owned());
                }
                continue;
            }
            let window = call.input.to_string();
            let result = round.results.iter().find(|(id, _)| *id == call.id);
            match result {
                Some((_, text))
                    if age > 0
                        && !changed.contains(path)
                        && !seen.contains(&window)
                        && text.starts_with(&format!("{path}: lines "))
                        && kept.len() + text.len() <= KEPT_READS =>
                {
                    kept.push_str(text);
                    kept.push('\n');
                }
                _ => {}
            }
            seen.insert(window);
        }
    }
    if kept.is_empty() {
        return kept;
    }
    format!("Files you read earlier in this turn, unchanged since:\n{kept}")
}

/// What the next call sends (T-3074, SDK-20): the opening, the same bytes on every call of the
/// turn and cached by the provider (T-3073); the running summary of the rounds before the last
/// with the reads that still hold (T-6385); and the last round whole, the model's calls and
/// their results. Any other older result is never sent again, so what follows the opening stays
/// bounded however long the turn runs.
fn transcript(provider: &str, opening: &str, rounds: &[Round]) -> Vec<Value> {
    let mut messages = vec![json!({ "role": "user", "content": opening })];
    let Some((last, earlier)) = rounds.split_last() else {
        return messages;
    };
    let earlier = summary(earlier);
    if !earlier.is_empty() {
        let content = format!("{earlier}{}", kept_reads(rounds));
        messages.push(json!({ "role": "user", "content": content }));
    }
    messages.push(assistant_turn_message(provider, &last.answer));
    for (id, result) in &last.results {
        messages.push(tool_result_message(provider, id, result));
    }
    messages
}

/// What one instruction has spent on the model so far.
#[derive(Debug, Default, Clone, Copy)]
struct Spent {
    calls: u32,
    input: u64,
    output: u64,
}

impl Spent {
    /// The sentence that ends the turn before the next call, when a ceiling is reached: the
    /// profile's step limit (AG-25) or the instruction's calls and input tokens (SDK-20).
    fn over(&self, steps: u32, step_limit: u32) -> Option<String> {
        let reached = if steps >= step_limit {
            format!("the step limit of this run ({step_limit} tool calls a message)")
        } else if self.calls >= EDIT_CALLS {
            format!("the limit of {EDIT_CALLS} model calls a message")
        } else if self.input >= EDIT_INPUT_TOKENS {
            format!("the limit of {EDIT_INPUT_TOKENS} input tokens a message")
        } else {
            return None;
        };
        Some(format!(
            "I stopped before finishing: this message reached {reached} after {} model calls, \
             {} input and {} output tokens. What passes the check is on screen; ask for the \
             rest in a smaller step.",
            self.calls, self.input, self.output
        ))
    }
}

fn read_file(files: &BTreeMap<String, String>, input: &Value) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")) else {
        return ("not a path of the application".to_owned(), false);
    };
    let Some(content) = files.get(&path) else {
        return (format!("{path}: no such file"), false);
    };
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let from = input
        .get("from")
        .and_then(Value::as_u64)
        .map_or(1, |n| n.max(1) as usize)
        .min(total.max(1));
    let to = input
        .get("to")
        .and_then(Value::as_u64)
        .map_or(total, |n| n as usize)
        .clamp(from, total.max(from))
        .min(from + READ_WINDOW - 1);
    let mut out = format!("{path}: lines {from}-{to} of {total}\n");
    for (offset, line) in lines.iter().skip(from - 1).take(to + 1 - from).enumerate() {
        out.push_str(&format!("{:>4} | {line}\n", from + offset));
    }
    (out, true)
}

/// Whether one more file of `bytes` keeps the project inside SDK-11's limits, counted over the
/// writable files as [`code::problems`] counts them: the template's own files are not the
/// model's to fit (F-POR1-021).
fn fits(files: &BTreeMap<String, String>, path: &str, bytes: usize) -> Result<(), String> {
    let writable = || files.iter().filter(|(p, _)| code::writable(p));
    let others: usize = writable()
        .filter(|(p, _)| p.as_str() != path)
        .map(|(_, c)| c.len())
        .sum();
    let count = writable().count() + usize::from(!files.contains_key(path));
    if count > code::MAX_FILES || others + bytes > code::MAX_BYTES {
        return Err(format!(
            "the application may hold at most {} files and {} bytes together",
            code::MAX_FILES,
            code::MAX_BYTES
        ));
    }
    Ok(())
}

/// Why `path` is refused in a turn scoped to one file (SDK-46), or None when it is that file or
/// the turn has no scope.
fn outside(scope: Option<&str>, path: &str) -> Option<String> {
    scope.filter(|file| *file != path).map(|file| {
        format!(
            "this instruction may change {file} only, the file the person pointed at in the \
             preview; finish and say what else would have to change"
        )
    })
}

fn edit_file(
    files: &mut BTreeMap<String, String>,
    input: &Value,
    scope: Option<&str>,
) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")) else {
        return (code::REFUSAL.to_owned(), false);
    };
    if let Some(refused) = outside(scope, &path) {
        return (refused, false);
    }
    let replace = arg(input, "replace");
    if let Err(reason) = fits(files, &path, replace.len()) {
        return (reason, false);
    }
    let block = patch::Block {
        path,
        search: arg(input, "search").to_owned(),
        replace: replace.to_owned(),
    };
    let (applied, refused) = patch::apply_where(files, &[block], code::writable, code::REFUSAL);
    match (applied.first(), refused.first()) {
        (Some(done), _) => (format!("{}: {}", done.path, done.how), true),
        (None, Some(no)) => (format!("{}: {}", no.path, no.reason), false),
        (None, None) => ("nothing to apply".to_owned(), false),
    }
}

fn write_file(
    files: &mut BTreeMap<String, String>,
    input: &Value,
    scope: Option<&str>,
) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")).filter(|path| code::writable(path)) else {
        return (code::REFUSAL.to_owned(), false);
    };
    if let Some(refused) = outside(scope, &path) {
        return (refused, false);
    }
    let content = arg(input, "content");
    if let Err(reason) = fits(files, &path, content.len()) {
        return (reason, false);
    }
    let how = if files.contains_key(&path) {
        "replaced"
    } else {
        "created"
    };
    files.insert(path.clone(), content.to_owned());
    (format!("{path}: {how}, {} bytes", content.len()), true)
}

fn delete_file(
    files: &mut BTreeMap<String, String>,
    input: &Value,
    scope: Option<&str>,
) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")).filter(|path| code::writable(path)) else {
        return (code::REFUSAL.to_owned(), false);
    };
    if let Some(refused) = outside(scope, &path) {
        return (refused, false);
    }
    match files.remove(&path) {
        Some(_) => (format!("{path}: removed"), true),
        None => (format!("{path}: no such file"), false),
    }
}

impl Driver {
    /// One instruction after the first run: tool calls until `finish`, an answer without a
    /// call, the step limit or a failed model call (SDK-20). Returns the version it published,
    /// when a check passed after a change.
    pub(super) async fn edit_turn(
        &self,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        conversation: &mut Vec<(String, String)>,
        instruction: &str,
        on_screen: Option<&Shown>,
    ) -> Result<Option<Shown>, String> {
        self.scoped_edit_turn(files, committed, conversation, instruction, on_screen, None)
            .await
    }

    /// `edit_turn` that may write `scope` only, the file a person pointed at (SDK-46).
    pub(super) async fn scoped_edit_turn(
        &self,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        conversation: &mut Vec<(String, String)>,
        instruction: &str,
        on_screen: Option<&Shown>,
        scope: Option<&str>,
    ) -> Result<Option<Shown>, String> {
        let tools = tools();
        let (opening, whole) = self.edit_user_turn(files, conversation, instruction);
        // The files the opening carries whole, as it carries them: a read of one unchanged is
        // answered from there.
        let shown_whole: BTreeMap<String, String> = whole
            .into_iter()
            .filter_map(|path| files.get(&path).map(|c| (path, c.clone())))
            .collect();
        let mut rounds: Vec<Round> = Vec::new();
        let mut spent = Spent::default();
        let mut shown: Option<Shown> = None;
        // Files changed since the last publish, and whether the current files passed a check.
        let mut dirty = false;
        let mut since_seq = on_screen.map_or(0, |shown| shown.seq);
        let limit = self.steps_per_run.max(1);
        let mut steps = 0u32;
        // The problems the last round ended with, and how many rounds running ended so.
        let mut failing = String::new();
        let mut same_rounds = 0u32;
        let mut repeats = Repeats::default();
        loop {
            // The ceilings are checked before the model is asked, never after a call it
            // already made (AG-25, SDK-20); what builds is published with the reason.
            if let Some(stopped) = spent.over(steps, limit) {
                return self
                    .finish_turn(
                        files,
                        committed,
                        conversation,
                        instruction,
                        &stopped,
                        dirty,
                        shown,
                    )
                    .await;
            }
            let messages = transcript(&self.provider, &opening, &rounds);
            if let Some(stopped) = self.over_budget(&messages, &tools, spent).await {
                return self
                    .finish_turn(
                        files,
                        committed,
                        conversation,
                        instruction,
                        &stopped,
                        dirty,
                        shown,
                    )
                    .await;
            }
            let answer = self
                .complete_tools(EDIT_SYSTEM, &messages, &tools, EDIT_OUTPUT_BUDGET)
                .await
                .map_err(|err| err.said(EDIT_OUTPUT_BUDGET))?;
            // A provider that reports no input tokens is counted by what was sent, four bytes
            // a token, so the ceiling holds for it too.
            let input = if answer.input_tokens > 0 {
                answer.input_tokens
            } else {
                u64::try_from(Value::Array(messages).to_string().len() / 4).unwrap_or(u64::MAX)
            };
            spent.calls += 1;
            spent.input += input;
            spent.output += answer.output_tokens;
            self.record_call(&answer, input, steps).await?;
            let mut round = Round {
                answer: answer.clone(),
                results: Vec::new(),
            };
            if answer.calls.is_empty() {
                let message = answer.text.unwrap_or_default();
                return self
                    .finish_turn(
                        files,
                        committed,
                        conversation,
                        instruction,
                        &message,
                        dirty,
                        shown,
                    )
                    .await;
            }
            let mut ended_with = String::new();
            for call in &answer.calls {
                steps += 1;
                let started = Instant::now();
                let (result, ok) = match call.name.as_str() {
                    "list_files" => (list_files(files), true),
                    "read_file" => read_again(files, &shown_whole, &call.input),
                    "edit_file" => {
                        let done = edit_file(files, &call.input, scope);
                        dirty |= done.1;
                        done
                    }
                    "write_file" => {
                        let done = write_file(files, &call.input, scope);
                        dirty |= done.1;
                        done
                    }
                    "delete_file" => {
                        let done = delete_file(files, &call.input, scope);
                        dirty |= done.1;
                        done
                    }
                    "check" => {
                        let problems = code::problems(files);
                        if !problems.is_empty() {
                            (with_named_lines(&problems.join("\n"), files), false)
                        } else {
                            if dirty {
                                let next = self
                                    .publish_code(
                                        files,
                                        "The change passes the check; the preview shows it.",
                                        Some(committed),
                                        false,
                                    )
                                    .await?;
                                since_seq = next.seq;
                                shown = Some(next);
                                dirty = false;
                            }
                            ("ok".to_owned(), true)
                        }
                    }
                    "call_function" => self.call_function_tool(&call.input).await,
                    "preview_errors" => {
                        let (said, ok) = self.preview_errors_tool(since_seq).await;
                        (with_named_lines(&said, files), ok)
                    }
                    "finish" => {
                        self.tool_event(call, "finished", true, started, steps)
                            .await?;
                        return self
                            .finish_turn(
                                files,
                                committed,
                                conversation,
                                instruction,
                                arg(&call.input, "message"),
                                dirty,
                                shown,
                            )
                            .await;
                    }
                    other => (format!("no tool named '{other}'"), false),
                };
                self.tool_event(call, &result, ok, started, steps).await?;
                // `check` and `preview_errors` are counted by the rounds below, with their own words.
                let counted = !matches!(call.name.as_str(), "check" | "preview_errors");
                if counted
                    && repeats.note(&call.name, &call.input, &result, ok) >= NO_PROGRESS_ROUNDS
                {
                    let stopped = format!(
                        "I stopped: `{}` failed {NO_PROGRESS_ROUNDS} times in a row with the same \
                         answer, so another call would only repeat it. What passes the check is \
                         on screen; tell me what to change. The answer:\n{}",
                        call.name,
                        cap(result.trim_end(), 1_500)
                    );
                    return self
                        .finish_turn(
                            files,
                            committed,
                            conversation,
                            instruction,
                            &stopped,
                            dirty,
                            shown,
                        )
                        .await;
                }
                if (call.name == "check" && !ok)
                    || (call.name == "preview_errors" && result != NO_PREVIEW_ERRORS)
                {
                    ended_with.push_str(&result);
                    ended_with.push('\n');
                }
                round
                    .results
                    .push((call.id.clone(), cap(&result, RESULT_CAP)));
            }
            rounds.push(round);
            same_rounds = match (ended_with.is_empty(), ended_with == failing) {
                (true, _) => 0,
                (false, true) => same_rounds + 1,
                (false, false) => 1,
            };
            failing = ended_with;
            if same_rounds >= NO_PROGRESS_ROUNDS {
                let stopped = format!(
                    "I stopped: {NO_PROGRESS_ROUNDS} tries in a row ended with the same problems, \
                     so another try would spend your budget on the same fix. What passes the \
                     check is on screen; tell me what to change. Still failing:\n{}",
                    cap(failing.trim_end(), 1_500)
                );
                return self
                    .finish_turn(
                        files,
                        committed,
                        conversation,
                        instruction,
                        &stopped,
                        dirty,
                        shown,
                    )
                    .await;
            }
        }
    }

    /// The sentence that ends the turn before a call that would take the run past the
    /// profile's `maxTokensPerRun` (AG-25, T-3077): what the call would send, read as tokens,
    /// plus its whole output budget, on top of what the run has used. The run's count is the
    /// proxy's (T-3072); without a run on record, what this turn spent stands for it.
    async fn over_budget(
        &self,
        messages: &[Value],
        tools: &[ToolSpec],
        spent: Spent,
    ) -> Option<String> {
        let sent = EDIT_SYSTEM.len()
            + Value::Array(messages.to_vec()).to_string().len()
            + tools
                .iter()
                .map(|tool| tool.description.len() + tool.input_schema.to_string().len())
                .sum::<usize>();
        let projected = u64::try_from(sent / 4).unwrap_or(u64::MAX) + u64::from(EDIT_OUTPUT_BUDGET);
        let used = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => u64::try_from(run.tokens_used).unwrap_or(0),
            _ => spent.input + spent.output,
        };
        (used.saturating_add(projected) > self.max_tokens_per_run).then(|| {
            format!(
                "I stopped before asking the model again: the next call needs about {projected} \
                 tokens and this run has used {used} of its token budget of {}. What passes \
                 the check is on screen; start a new conversation to go on, or ask an \
                 administrator to raise the agent profile's maxTokensPerRun.",
                self.max_tokens_per_run
            )
        })
    }

    /// One model call on the run's counters (AG-44) and as a `usage` event (SDK-20, API/04 §4),
    /// when the proxy did not count it already: see [`own_count`].
    pub(super) async fn record_call(
        &self,
        answer: &ToolAnswer,
        input: u64,
        step: u32,
    ) -> Result<(), String> {
        let Some(tokens) = own_count(answer, input) else {
            return Ok(());
        };
        if let Err(err) = self
            .state
            .agents
            .record_usage(&self.run_id, i64::try_from(tokens).unwrap_or(i64::MAX), 1)
            .await
        {
            tracing::warn!(run = %self.run_id, error = %err, "usage not recorded");
        }
        let cumulative = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => Some(run.tokens_used),
            _ => None,
        };
        self.event(
            "usage",
            json!({
                "step": step,
                "tokensThisStep": tokens,
                "inputTokens": input,
                "outputTokens": answer.output_tokens,
                "cumulativeTokens": cumulative,
            }),
        )
        .await
    }

    /// The turn's end: unpublished changes are checked and published with the message, or
    /// the message says why the preview keeps the last version.
    #[allow(clippy::too_many_arguments)]
    async fn finish_turn(
        &self,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        conversation: &mut Vec<(String, String)>,
        instruction: &str,
        message: &str,
        dirty: bool,
        mut shown: Option<Shown>,
    ) -> Result<Option<Shown>, String> {
        let message = if message.trim().is_empty() {
            "Done."
        } else {
            message.trim()
        };
        if dirty {
            let problems = code::problems(files);
            if problems.is_empty() {
                shown = Some(
                    self.publish_code(files, message, Some(committed), false)
                        .await?,
                );
            } else {
                self.thought(&format!(
                    "{message}\n\nThe files do not build, so the preview keeps the last \
                     version:\n{}",
                    problems.join("\n")
                ))
                .await?;
            }
        } else {
            self.thought(message).await?;
        }
        conversation.push((instruction.to_owned(), message.to_owned()));
        Ok(shown)
    }

    /// One `tool` event per call (AG-25): the tool, its arguments, what it answered, the step.
    async fn tool_event(
        &self,
        call: &ToolCall,
        result: &str,
        ok: bool,
        started: Instant,
        step: u32,
    ) -> Result<(), String> {
        self.event(
            "tool",
            json!({
                "tool": call.name,
                "status": if ok { "ok" } else { "failed" },
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": call.input,
                "result": cap(result, EVENT_CAP),
                "step": step,
            }),
        )
        .await
    }

    /// The person's instruction with what the model needs to start: the row types, the files,
    /// the turns before and the request (SDK-20). What changes least comes first (T-3073): the
    /// row types change with the data needs only and the files with the edits, while the
    /// conversation and the request are new on every turn, so a provider's prefix cache keeps
    /// what one turn shares with the next.
    fn edit_user_turn(
        &self,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
    ) -> (String, Vec<String>) {
        let mut turn = String::new();
        if let Some(types) = files.get(code::TYPES) {
            turn.push_str(&format!("Row types ({}):\n{types}\n\n", code::TYPES));
        }
        let (files_text, whole) = opening(files);
        turn.push_str(&files_text);
        if !conversation.is_empty() {
            turn.push_str("\n\nEarlier in this conversation:\n");
            for (asked, answered) in conversation.iter().rev().take(6).rev() {
                turn.push_str(&format!("- asked: {asked}\n  answered: {answered}\n"));
            }
        }
        turn.push_str(&format!("\n\nThe request:\n{instruction}"));
        (turn, whole)
    }

    /// `call_function`: one function of the run in jc-functions, as the person who started the
    /// run (AG-70, SDK-18).
    async fn call_function_tool(&self, input: &Value) -> (String, bool) {
        let name = arg(input, "name").to_owned();
        let body = input.get("body").cloned().unwrap_or(Value::Null);
        let query: BTreeMap<String, String> = input
            .get("query")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| {
                        let value = match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        (key.clone(), value)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let run = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return ("the run is gone".to_owned(), false),
            Err(err) => return (err.to_string(), false),
        };
        // The run's own call reads as the run, through the proxy, with a five-minute credential
        // of the run (ADR-N-038 decision 6). Without one the function reads nothing, and says so.
        let caller = match crate::agents::identity::data_credential(
            self.state.oidc.as_deref(),
            &self.proxy_base,
            &self.run_id,
        )
        .await
        {
            Ok(credential) => Caller::Run(credential),
            Err(reason) => {
                tracing::warn!(run = %self.run_id, %reason, "no data credential for the run's function call");
                Caller::Person(None)
            }
        };
        match invoke_function(
            &self.state,
            &run,
            &name,
            body,
            &query,
            &self.identity,
            caller,
        )
        .await
        {
            Ok(invocation) => {
                let status = invocation.outcome["status"].as_u64().unwrap_or(500);
                let error = invocation
                    .outcome
                    .get("error")
                    .filter(|error| !error.is_null());
                // The run's call carries no person's identity (T-3134): a function that reads
                // data is refused by the gateway, and calling it again changes nothing.
                if error.is_some_and(crate::api::agent_runs::unauthenticated) {
                    return (FUNCTION_READS_DATA.to_owned(), false);
                }
                let payload = match error {
                    Some(error) => format!("error: {error}"),
                    None => invocation
                        .outcome
                        .get("body")
                        .map(Value::to_string)
                        .unwrap_or_default(),
                };
                let logs = invocation
                    .outcome
                    .get("logs")
                    .and_then(Value::as_array)
                    .filter(|logs| !logs.is_empty())
                    .map(|logs| format!("\nlogs: {}", Value::Array(logs.clone())))
                    .unwrap_or_default();
                (
                    format!("status {status}\n{payload}{logs}"),
                    error.is_none() && status < 400,
                )
            }
            Err(InvokeError::NoFunction) => (format!("no function '{name}' in functions/"), false),
            Err(InvokeError::DoesNotBuild(problems)) => (
                format!("the functions do not build:\n{}", problems.join("\n")),
                false,
            ),
            Err(InvokeError::Unavailable(reason)) => (reason, false),
            Err(InvokeError::Refused { reason, .. }) => (reason, false),
        }
    }

    /// `preview_errors`: what the frame reported since the version on screen was published
    /// (SDK-14), oldest first.
    async fn preview_errors_tool(&self, since_seq: i64) -> (String, bool) {
        let events = match self
            .state
            .agents
            .events_since(&self.run_id, since_seq)
            .await
        {
            Ok(events) => events,
            Err(err) => return (err.to_string(), false),
        };
        let errors: Vec<String> = events
            .iter()
            .filter(|event| event.kind == "preview_error")
            .map(|event| event.payload.to_string())
            .collect();
        if errors.is_empty() {
            return (NO_PREVIEW_ERRORS.to_owned(), true);
        }
        let skipped = errors.len().saturating_sub(ERROR_WINDOW);
        let mut out = errors[skipped..].join("\n");
        if skipped > 0 {
            out = format!("({skipped} older errors not shown)\n{out}");
        }
        (out, true)
    }
}

/// Lines either side of a line a failure names that go with it (T-3075).
const NAMED_LINES_AROUND: usize = 8;

/// A failure with the lines it names (`src/App.tsx:12`) appended, numbered, so the model fixes
/// them without a `read_file` round trip first (T-3075). Only the application's own files are
/// quoted, a window either side of each named line, and the whole stays within [`RESULT_CAP`]:
/// a window that does not fit is left out, and the file can still be read.
fn with_named_lines(failure: &str, files: &BTreeMap<String, String>) -> String {
    let mut out = failure.to_owned();
    for (path, content) in files.iter().filter(|(path, _)| code::writable(path)) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let needle = format!("{path}:");
        let mut named: Vec<usize> = failure
            .match_indices(&needle)
            .filter_map(|(at, _)| {
                let digits: String = failure[at + needle.len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse::<usize>().ok().filter(|line| *line > 0)
            })
            .map(|line| line.min(lines.len()))
            .collect();
        named.sort_unstable();
        named.dedup();
        // Overlapping windows are merged into one range.
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for line in named {
            let (from, to) = (
                line.saturating_sub(NAMED_LINES_AROUND).max(1),
                (line + NAMED_LINES_AROUND).min(lines.len()),
            );
            match ranges.last_mut() {
                Some(last) if from <= last.1 + 1 => last.1 = last.1.max(to),
                _ => ranges.push((from, to)),
            }
        }
        for (from, to) in ranges {
            let mut quoted = format!("\n\n=== {path}, lines {from}-{to} ===");
            for (number, line) in lines[from - 1..to].iter().enumerate() {
                quoted.push_str(&format!("\n{}: {line}", from + number));
            }
            if out.len() + quoted.len() > RESULT_CAP {
                return out;
            }
            out.push_str(&quoted);
        }
    }
    out
}

/// What the loop counts of one call itself (T-3072, API/04 §4). `jc-agent-proxy` writes the
/// `usage` frame and counts the tokens of every call it relays whose provider reported usage, so
/// such a call is the proxy's to count and counting it here too doubled the run's `tokensUsed`.
/// A provider that reported nothing gets nothing counted by the proxy; the loop counts that call
/// once, by `input`, the bytes it sent read as tokens.
fn own_count(answer: &ToolAnswer, input: u64) -> Option<u64> {
    let reported = answer.usage_tokens + answer.input_tokens + answer.output_tokens > 0;
    (!reported).then_some(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-POR1-021: the template's own files (public/, package.json) are not counted against the
    /// model's limit, as code::problems does not count them.
    #[test]
    fn a_template_full_of_files_still_fits_one_writable_file() {
        let mut files: BTreeMap<String, String> = (0..code::MAX_FILES)
            .map(|at| (format!("public/asset-{at}.svg"), "<svg/>".to_owned()))
            .collect();
        files.insert("package.json".to_owned(), "{}".to_owned());
        assert_eq!(fits(&files, "src/App.tsx", 10), Ok(()));
        for at in 0..code::MAX_FILES {
            files.insert(format!("src/part-{at}.tsx"), String::new());
        }
        assert!(fits(&files, "src/App.tsx", 10).is_err());
    }

    /// T-3044: a follow-up on dev ("Add a chart of stations by free slots") was cut at 8000
    /// output tokens and applied nothing: the system prompt asks for every edit in one answer,
    /// and a reasoning model spends part of the budget thinking before it writes. One answer
    /// holds a whole file the model may read, at a dozen tokens a line, and the reasoning.
    #[test]
    fn an_edit_answer_holds_a_whole_file_and_the_reasoning_before_it() {
        const TOKENS_PER_LINE: usize = 12;
        const REASONING: usize = 8000;
        assert!(EDIT_OUTPUT_BUDGET as usize >= READ_WINDOW * TOKENS_PER_LINE + REASONING);
    }

    #[test]
    fn a_path_outside_the_application_is_not_a_path() {
        assert_eq!(clean_path("src/App.tsx").as_deref(), Some("src/App.tsx"));
        assert!(clean_path("../../etc/passwd").is_none());
        assert!(clean_path("/etc/passwd").is_none());
        assert!(clean_path("src//App.tsx").is_none());
        assert!(clean_path("src/./App.tsx").is_none());
        assert!(clean_path("").is_none());
    }

    #[test]
    fn a_read_is_clamped_to_the_file_and_the_window() {
        let mut files = BTreeMap::new();
        files.insert("src/A.ts".to_owned(), "a\nb\nc\n".to_owned());
        let (text, ok) = read_file(&files, &json!({ "path": "src/A.ts", "from": 2, "to": 99 }));
        assert!(ok);
        assert!(text.starts_with("src/A.ts: lines 2-3 of 3\n"));
        assert!(text.contains("   2 | b\n   3 | c\n"));
        let (text, ok) = read_file(&files, &json!({ "path": "src/B.ts" }));
        assert!(!ok && text.ends_with("no such file"));
    }

    #[test]
    fn the_opening_turn_carries_the_small_files_whole_and_names_the_rest() {
        let mut files = BTreeMap::new();
        files.insert("package.json".to_owned(), "{}".to_owned());
        files.insert("src/App.tsx".to_owned(), "app".to_owned());
        files.insert("src/big.ts".to_owned(), "x".repeat(OPENING_BYTES));
        files.insert("functions/f.ts".to_owned(), "fn".to_owned());
        let (text, whole) = opening(&files);
        assert_eq!(whole, ["functions/f.ts", "src/App.tsx"]);
        assert!(text.contains("package.json (2 bytes)"));
        assert!(
            !text.contains("=== package.json ==="),
            "not the application's own"
        );
        assert!(text.contains("=== src/App.tsx ===\napp"));
        assert!(text.contains("=== functions/f.ts ===\nfn"));
        assert!(text.ends_with("Not shown, read before editing: src/big.ts"));
        assert!(opening(&BTreeMap::new()).0.starts_with("Files:\n"));
    }

    #[test]
    fn a_write_outside_the_sdk_paths_is_refused_and_writes_nothing() {
        let mut files = BTreeMap::new();
        for path in ["package.json", "../x.ts", "src/../../x.ts"] {
            let (text, ok) = write_file(&mut files, &json!({ "path": path, "content": "x" }), None);
            assert!(!ok, "{path}");
            assert_eq!(text, code::REFUSAL);
        }
        assert!(files.is_empty());
        let (_, ok) = write_file(
            &mut files,
            &json!({ "path": "src/pages/A.tsx", "content": "x" }),
            None,
        );
        assert!(ok);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn an_edit_needs_an_exact_search() {
        let mut files = BTreeMap::new();
        // Four lines: a search that matches none is refused (a one-line file would be taken
        // as a whole-file rewrite by the patch rules).
        files.insert(
            "src/A.ts".to_owned(),
            "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n".to_owned(),
        );
        let (text, ok) = edit_file(
            &mut files,
            &json!({ "path": "src/A.ts", "search": "let q = 7;", "replace": "let a = 3;" }),
            None,
        );
        assert!(!ok, "{text}");
        let (_, ok) = edit_file(
            &mut files,
            &json!({ "path": "src/A.ts", "search": "let a = 1;", "replace": "let a = 3;" }),
            None,
        );
        assert!(ok);
        assert!(files["src/A.ts"].starts_with("let a = 3;\nlet b = 2;"));
    }

    #[test]
    fn a_long_result_is_cut_at_the_cap() {
        let text = "é".repeat(5000);
        let cut = cap(&text, 100);
        assert!(cut.len() < 200);
        assert!(cut.contains("more bytes"));
    }

    fn round(n: usize) -> Round {
        Round {
            answer: ToolAnswer {
                text: None,
                calls: vec![ToolCall {
                    id: format!("c{n}"),
                    name: "write_file".into(),
                    input: json!({ "path": "src/A.tsx", "content": "x".repeat(30_000) }),
                }],
                ..ToolAnswer::default()
            },
            results: vec![(
                format!("c{n}"),
                format!("src/A.tsx: lines 1-900\n{}", "y".repeat(8_000)),
            )],
        }
    }

    fn sent_bytes(rounds: &[Round]) -> usize {
        Value::Array(transcript("anthropic", "opening", rounds))
            .to_string()
            .len()
    }

    /// T-3076: the editing opening over the template carries the components by their exports
    /// and leaves its tests out: within 15k tokens, where it carried ~50k.
    #[test]
    fn the_opening_over_the_template_stays_small() {
        let template = crate::agents::preview::template_files();
        let (text, whole) = opening(&template);
        assert!(text.len() / 4 <= 15_000, "{} tokens", text.len() / 4);
        assert!(whole.contains(&"src/App.tsx".to_owned()));
        assert!(!whole.contains(&"src/components/EntityForm.tsx".to_owned()));
        assert!(text.contains("=== src/components/EntityForm.tsx (unchanged template component"));
        assert!(
            text.contains("Not shown, read before editing:")
                && text.contains("EntityForm.test.tsx")
        );
    }

    /// T-3074: after the opening, a call carries the summary of the earlier rounds and the last
    /// round whole, so a turn of thirty rounds sends what a turn of three sends, and the part
    /// after the cached opening stays under 10k tokens even when the last round read four big
    /// files.
    #[test]
    fn a_later_call_sends_the_summary_and_the_last_round_only() {
        let three: Vec<Round> = (0..3).map(round).collect();
        let thirty: Vec<Round> = (0..30).map(round).collect();
        assert!(
            sent_bytes(&thirty) <= sent_bytes(&three) + SUMMARY_CHARS,
            "{} vs {}",
            sent_bytes(&thirty),
            sent_bytes(&three)
        );
        let messages = transcript("anthropic", "opening", &thirty);
        // opening, summary, the last answer, its result
        assert_eq!(messages.len(), 4);
        assert!(
            messages[3].to_string().contains(&"y".repeat(8_000)),
            "the newest result is whole"
        );
        assert!(messages[2].to_string().contains("\"c29\""));
        let summary = messages[1].to_string();
        assert!(
            summary.contains("write_file src/A.tsx: src/A.tsx: lines 1-900"),
            "{summary}"
        );
        assert!(
            summary.contains("Files you changed: src/A.tsx"),
            "{summary}"
        );
        assert!(!summary.contains(&"y".repeat(100)) && !summary.contains(&"x".repeat(100)));
        assert!(summary.len() <= SUMMARY_CHARS + 200, "{}", summary.len());

        let mut wide = round(30);
        wide.answer.calls = (0..4)
            .map(|n| ToolCall {
                id: format!("r{n}"),
                name: "read_file".into(),
                input: json!({ "path": format!("src/F{n}.tsx") }),
            })
            .collect();
        wide.results = (0..4)
            .map(|n| (format!("r{n}"), "z".repeat(RESULT_CAP)))
            .collect();
        let mut rounds: Vec<Round> = (0..29).map(round).collect();
        rounds.push(wide);
        let after_opening: usize = transcript("anthropic", "opening", &rounds)[1..]
            .iter()
            .map(|message| message.to_string().len())
            .sum();
        assert!(
            after_opening / 4 <= 10_000,
            "{} tokens after the opening",
            after_opening / 4
        );
    }

    fn reading(n: usize, path: &str, text: &str) -> Round {
        Round {
            answer: ToolAnswer {
                calls: vec![ToolCall {
                    id: format!("c{n}"),
                    name: "read_file".into(),
                    input: json!({ "path": path }),
                }],
                ..ToolAnswer::default()
            },
            results: vec![(format!("c{n}"), format!("{path}: lines 1-1 of 1\n{text}"))],
        }
    }

    /// T-6385: a model reading one file per call still holds the first file on its third call.
    /// Without it gemini-3.8-flash read the same two windows until the call limit, no edit made.
    #[test]
    fn an_earlier_read_stays_in_the_request_until_the_file_changes() {
        let rounds = [
            reading(0, "src/A.tsx", "first-file"),
            reading(1, "src/B.tsx", "second-file"),
            reading(2, "src/C.tsx", "third-file"),
        ];
        let sent = Value::Array(transcript("anthropic", "opening", &rounds)).to_string();
        assert!(
            sent.contains("first-file") && sent.contains("second-file"),
            "{sent}"
        );
        assert_eq!(
            sent.matches("third-file").count(),
            1,
            "the last round is sent once"
        );

        let mut edit = round(3);
        edit.answer.calls[0].input = json!({ "path": "src/A.tsx", "content": "new" });
        let rounds = [
            reading(0, "src/A.tsx", "first-file"),
            reading(1, "src/B.tsx", "second-file"),
            edit,
        ];
        let sent = Value::Array(transcript("anthropic", "opening", &rounds)).to_string();
        assert!(
            !sent.contains("first-file"),
            "a changed file is stale: {sent}"
        );
        assert!(sent.contains("second-file"), "{sent}");
    }

    /// T-6385: thirty big reads keep the newest within the bound, the same window once.
    #[test]
    fn kept_reads_stay_within_their_bound_and_hold_a_window_once() {
        let big = "z".repeat(RESULT_CAP - 100);
        let rounds: Vec<Round> = (0..30)
            .map(|n| reading(n, &format!("src/F{n}.tsx"), &big))
            .collect();
        let kept = kept_reads(&rounds);
        assert!(kept.len() <= KEPT_READS + 100, "{}", kept.len());
        assert!(kept.contains("src/F28.tsx") && !kept.contains("src/F20.tsx"));

        let again = [
            reading(0, "src/A.tsx", "same"),
            reading(1, "src/A.tsx", "same"),
            reading(2, "src/B.tsx", "other"),
        ];
        assert_eq!(kept_reads(&again).matches("same").count(), 1);
        let refused = [reading(0, "src/A.tsx", "x"), reading(1, "src/B.tsx", "y")];
        let mut no_file = refused;
        no_file[0].results[0].1 = "src/A.tsx: no such file".to_owned();
        assert_eq!(kept_reads(&no_file), "", "a refusal is no read");
    }

    /// T-3074: the summary keeps what the next step needs: every file changed, and the newest
    /// steps when the oldest no longer fit.
    #[test]
    fn the_summary_names_every_changed_file_and_keeps_the_newest_steps() {
        let mut rounds: Vec<Round> = (0..80).map(round).collect();
        rounds[0].answer.calls[0].input =
            json!({ "path": "src/First.tsx", "search": "a", "replace": "b" });
        rounds[0].answer.calls[0].name = "edit_file".into();
        rounds[1].answer.calls[0].name = "check".into();
        rounds[1].results[0].1 = "src/A.tsx:3:1: Unexpected token".into();
        let said = summary(&rounds);
        assert!(
            said.contains("src/First.tsx") && said.contains("src/A.tsx"),
            "{said}"
        );
        assert!(said.contains("earlier steps left out"), "{said}");
        assert!(
            said.ends_with("- write_file src/A.tsx: src/A.tsx: lines 1-900\n"),
            "{said}"
        );
        assert!(said.len() <= SUMMARY_CHARS + 200);
        assert_eq!(summary(&[]), "");
    }

    /// T-2467: the model reading a file the request already carries costs one line, until the
    /// file changes.
    #[test]
    fn a_file_shown_whole_is_not_read_again_until_it_changes() {
        let mut files = BTreeMap::new();
        files.insert("src/A.ts".to_owned(), "a\nb\n".to_owned());
        let shown = files.clone();
        let (text, ok) = read_again(&files, &shown, &json!({ "path": "src/A.ts" }));
        assert!(ok && text.contains("shown whole in the request"), "{text}");
        let (text, _) = read_again(&files, &shown, &json!({ "path": "src/A.ts", "from": 2 }));
        assert!(text.contains("   2 | b"), "a window is read: {text}");
        files.insert("src/A.ts".to_owned(), "c\n".to_owned());
        let (text, _) = read_again(&files, &shown, &json!({ "path": "src/A.ts" }));
        assert!(text.contains("   1 | c"), "a changed file is read: {text}");
    }

    #[test]
    fn a_ceiling_names_what_was_reached_and_what_was_spent() {
        let spent = Spent {
            calls: 3,
            input: 10,
            output: 2,
        };
        assert!(spent.over(0, 120).is_none());
        let said = spent.over(120, 120).expect("the step limit");
        assert!(said.contains("step limit of this run (120"), "{said}");
        let said = Spent {
            calls: EDIT_CALLS,
            ..spent
        }
        .over(0, 120)
        .expect("calls");
        assert!(
            said.contains("12 model calls") && said.contains("smaller step"),
            "{said}"
        );
        let said = Spent {
            input: EDIT_INPUT_TOKENS,
            ..spent
        }
        .over(0, 120)
        .expect("tokens");
        assert!(said.contains("500000 input tokens"), "{said}");
    }

    /// A model stub that answers every call with one `list_files` call and the given usage.
    async fn model_listing_forever(server: &wiremock::MockServer, input_tokens: u64) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "tool_use", "id": "t", "name": "list_files", "input": {} }],
                "usage": { "input_tokens": input_tokens, "output_tokens": 30 },
            })))
            .mount(server)
            .await;
    }

    /// The same stub for a provider that reports no usage at all.
    async fn model_listing_forever_unreported(server: &wiremock::MockServer) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "tool_use", "id": "t", "name": "list_files", "input": {} }],
            })))
            .mount(server)
            .await;
    }

    async fn usage_frames(driver: &Driver) -> Vec<Value> {
        driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "usage")
            .map(|event| event.payload)
            .collect()
    }

    /// The same stub on the chat-completions shape OpenRouter speaks.
    async fn chat_listing_forever(server: &wiremock::MockServer) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": null, "tool_calls": [
                    { "id": "t", "type": "function", "function": { "name": "list_files", "arguments": "{}" } }
                ] } }],
                "usage": { "prompt_tokens": 1_000, "completion_tokens": 30, "total_tokens": 1_030 },
            })))
            .mount(server)
            .await;
    }

    /// T-3073: every call of a turn starts with the same bytes (the tools, the rules and the
    /// opening), marked for the provider's cache; what changes per step only follows it.
    #[tokio::test]
    async fn every_call_of_a_turn_shares_one_cache_marked_prefix() {
        let server = wiremock::MockServer::start().await;
        chat_listing_forever(&server).await;
        let mut driver = edit_driver(&server).await;
        driver.provider = "openrouter".into();
        let mut files = BTreeMap::from([
            (
                "src/App.tsx".to_owned(),
                "export default function App() {}".to_owned(),
            ),
            (
                code::TYPES.to_owned(),
                "export interface Station { name: string }".to_owned(),
            ),
        ]);
        let mut conversation = vec![("show bikes".to_owned(), "Done.".to_owned())];
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut conversation,
                "add a form",
                None,
            )
            .await
            .expect("the turn ends");
        let bodies: Vec<Value> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
            .collect();
        assert!(bodies.len() > 2, "{}", bodies.len());
        let prefix = |body: &Value| {
            json!([body["tools"], body["messages"][0], body["messages"][1]]).to_string()
        };
        let first = prefix(&bodies[0]);
        for body in &bodies {
            assert_eq!(prefix(body), first, "a call changed the cached prefix");
        }
        // The per-step part grows after the prefix, never inside it.
        assert!(
            bodies[2]["messages"].as_array().map(Vec::len)
                > bodies[0]["messages"].as_array().map(Vec::len)
        );
        assert_eq!(bodies[0]["parallel_tool_calls"], json!(true), "T-3075");
        let opening = &bodies[0]["messages"][1]["content"][0];
        assert_eq!(opening["cache_control"], json!({ "type": "ephemeral" }));
        // Stable first: the row types and the files, then the turns before and the request.
        let text = opening["text"].as_str().expect("the opening");
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle} in {text}"))
        };
        assert!(at("Row types") < at("Files:"), "{text}");
        assert!(at("Files:") < at("Earlier in this conversation"), "{text}");
        assert!(
            at("Earlier in this conversation") < at("The request:"),
            "{text}"
        );
    }

    /// T-3075: the reads one answer asks for are all answered, and the next call carries every
    /// result at once: four files cost one round trip, not four.
    #[tokio::test]
    async fn several_reads_in_one_answer_are_answered_in_one_next_call() {
        let server = wiremock::MockServer::start().await;
        let reads: Vec<Value> = ["a", "b", "c", "d"]
            .iter()
            .map(|name| {
                json!({ "type": "tool_use", "id": format!("r-{name}"), "name": "read_file",
                        "input": { "path": format!("src/{name}.tsx") } })
            })
            .collect();
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": reads,
                "usage": { "input_tokens": 1_000, "output_tokens": 30 },
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "tool_use", "id": "f", "name": "finish",
                              "input": { "message": "Read them." } }],
                "usage": { "input_tokens": 1_000, "output_tokens": 10 },
            })))
            .mount(&server)
            .await;
        let driver = edit_driver(&server).await;
        // Too big for the opening, so each read really answers the file.
        let mut files: BTreeMap<String, String> = ["a", "b", "c", "d"]
            .iter()
            .map(|name| {
                (
                    format!("src/{name}.tsx"),
                    format!("// {name}\n{}", "x".repeat(OPENING_BYTES / 3)),
                )
            })
            .collect();
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut Vec::new(),
                "read",
                None,
            )
            .await
            .expect("the turn ends");
        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(requests.len(), 2, "four reads, one round trip");
        let second: Value = serde_json::from_slice(&requests[1].body).expect("JSON");
        let results = second.to_string().matches("\"tool_result\"").count();
        assert_eq!(results, 4, "{second}");
    }

    /// T-3075: a failure that names a file and a line carries those lines, so the model fixes
    /// it without a read first; within the result's cap, and nothing of a file it does not name.
    #[test]
    fn a_failure_carries_the_lines_it_names() {
        let app: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let files = BTreeMap::from([
            ("src/App.tsx".to_owned(), app),
            ("src/Other.tsx".to_owned(), "other".to_owned()),
        ]);
        let said = with_named_lines("src/App.tsx:12:3: x is not defined", &files);
        assert!(
            said.starts_with("src/App.tsx:12:3: x is not defined"),
            "{said}"
        );
        assert!(said.contains("=== src/App.tsx, lines 4-20 ==="), "{said}");
        assert!(
            said.contains("12: line 12")
                && said.contains("4: line 4")
                && said.contains("20: line 20"),
            "{said}"
        );
        assert!(
            !said.contains("21: line 21") && !said.contains("other"),
            "{said}"
        );
        // Nothing named, nothing added; a line past the end is clamped.
        assert_eq!(with_named_lines("ok", &files), "ok");
        assert!(with_named_lines("src/App.tsx:99: gone", &files).contains("lines 32-40"));
        // A huge file stays inside the cap.
        let big = BTreeMap::from([(
            "src/App.tsx".to_owned(),
            "y".repeat(400).repeat(40).replace("yyyy", "y\n"),
        )]);
        assert!(with_named_lines("src/App.tsx:50: e", &big).len() <= RESULT_CAP);
    }

    /// A model stub that answers every call with one `check`.
    async fn model_checking_forever(server: &wiremock::MockServer) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "tool_use", "id": "c", "name": "check", "input": {} }],
                "usage": { "input_tokens": 1_000, "output_tokens": 10 },
            })))
            .mount(server)
            .await;
    }

    /// T-3135: the same call failing with the same answer counts towards the stop; a success or
    /// a different answer starts it over, and another input is another call.
    #[test]
    fn a_call_that_keeps_failing_alike_is_counted_and_any_change_starts_over() {
        let mut repeats = Repeats::default();
        let body = json!({ "name": "summary", "body": {} });
        assert_eq!(
            repeats.note("call_function", &body, "status 500 boom", false),
            1
        );
        assert_eq!(repeats.note("check", &json!({}), "ok", true), 0);
        assert_eq!(
            repeats.note("call_function", &body, "status 500 boom", false),
            2
        );
        assert_eq!(
            repeats.note(
                "call_function",
                &json!({ "name": "other" }),
                "status 500 boom",
                false
            ),
            1
        );
        assert_eq!(
            repeats.note("call_function", &body, "status 500 bang", false),
            1,
            "another answer"
        );
        assert_eq!(
            repeats.note("call_function", &body, "status 500 bang", false),
            2
        );
        assert_eq!(
            repeats.note("call_function", &body, "status 200", true),
            0,
            "a success"
        );
        assert_eq!(
            repeats.note("call_function", &body, "status 500 bang", false),
            1
        );
    }

    /// T-3135: a model that calls the same failing function again and again is stopped at the
    /// third identical failure with the answer named, not at the instruction's call limit.
    #[tokio::test]
    async fn the_same_failing_call_three_times_stops_the_turn_with_its_answer() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "tool_use", "id": "f", "name": "call_function", "input": { "name": "summary", "body": {} } }],
                "usage": { "input_tokens": 1_000, "output_tokens": 10 },
            })))
            .mount(&server)
            .await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([(
            "src/App.tsx".to_owned(),
            "export default function App() { return <div />; }".to_owned(),
        )]);
        let mut conversation = Vec::new();
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut conversation,
                "test the summary",
                None,
            )
            .await
            .expect("the turn ends");
        let calls = server.received_requests().await.expect("recorded").len();
        assert_eq!(calls, NO_PROGRESS_ROUNDS as usize);
        let said = &conversation[0].1;
        assert!(
            said.contains("`call_function` failed 3 times in a row"),
            "{said}"
        );
        assert!(said.contains("tell me what to change"), "{said}");
    }

    /// T-3077: rounds that end with the same problems are no progress; the third one stops the
    /// turn with what still fails, instead of spending the instruction's calls on it.
    #[tokio::test]
    async fn the_same_problems_three_rounds_running_stop_the_turn_with_what_fails() {
        let server = wiremock::MockServer::start().await;
        model_checking_forever(&server).await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([(
            "src/App.tsx".to_owned(),
            "export default function App() { return <div>; }".to_owned(),
        )]);
        let mut conversation = Vec::new();
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut conversation,
                "fix it",
                None,
            )
            .await
            .expect("the turn ends");
        let calls = server.received_requests().await.expect("recorded").len();
        assert_eq!(calls, NO_PROGRESS_ROUNDS as usize);
        let said = &conversation[0].1;
        assert!(
            said.contains("the same problems") && said.contains("src/App.tsx"),
            "{said}"
        );
        assert!(said.contains("tell me what to change"), "{said}");
    }

    /// T-3077: a call that would take the run past its token budget is never made; the person
    /// reads why and what to do.
    #[tokio::test]
    async fn a_call_past_the_runs_token_budget_is_not_made() {
        let server = wiremock::MockServer::start().await;
        model_listing_forever(&server, 1_000).await;
        let mut driver = edit_driver(&server).await;
        driver.max_tokens_per_run = u64::from(EDIT_OUTPUT_BUDGET);
        let mut files = BTreeMap::from([("src/App.tsx".to_owned(), "app".to_owned())]);
        let mut conversation = Vec::new();
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut conversation,
                "add a form",
                None,
            )
            .await
            .expect("the turn ends");
        assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
        let said = &conversation[0].1;
        assert!(
            said.contains("token budget") && said.contains("new conversation"),
            "{said}"
        );
    }

    /// T-3072: the proxy writes the usage frame and counts the tokens of every call it relays,
    /// so a call whose provider reported usage is counted there alone; the loop counts only a
    /// call the provider said nothing about, which the proxy counted nothing for.
    #[test]
    fn a_call_is_counted_by_the_loop_only_when_the_provider_reported_no_usage() {
        let reported = ToolAnswer {
            usage_tokens: 1_030,
            input_tokens: 1_000,
            output_tokens: 30,
            ..ToolAnswer::default()
        };
        assert_eq!(own_count(&reported, 1_000), None);
        let only_total = ToolAnswer {
            usage_tokens: 900,
            ..ToolAnswer::default()
        };
        assert_eq!(own_count(&only_total, 225), None);
        assert_eq!(own_count(&ToolAnswer::default(), 2_500), Some(2_500));
    }

    #[tokio::test]
    async fn a_provider_that_reports_no_usage_gets_one_estimated_frame_per_call() {
        let server = wiremock::MockServer::start().await;
        model_listing_forever_unreported(&server).await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([("src/App.tsx".to_owned(), "app".to_owned())]);
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut Vec::new(),
                "add a form",
                None,
            )
            .await
            .expect("the turn ends");
        let calls = server.received_requests().await.expect("recorded").len();
        let frames = usage_frames(&driver).await;
        assert_eq!(frames.len(), calls);
        assert!(
            frames
                .iter()
                .all(|frame| frame["inputTokens"].as_u64() > Some(0)
                    && frame["tokensThisStep"] == frame["inputTokens"]),
            "{frames:?}"
        );
    }

    async fn edit_driver(server: &wiremock::MockServer) -> Driver {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.kind = "application".into();
        driver.steps_per_run = 120;
        driver
    }

    async fn thoughts(driver: &Driver) -> Vec<String> {
        driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "thought")
            .map(|event| {
                event.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    /// SDK-46: a turn scoped to the file the person pointed at refuses a write, an edit or a
    /// delete anywhere else as a tool error, and the model reads why.
    #[tokio::test]
    async fn a_scoped_turn_changes_the_pointed_file_only() {
        let server = wiremock::MockServer::start().await;
        let card = "export const Card = () => (\n  <h2 className=\"t\">Station</h2>\n);\n";
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    { "type": "tool_use", "id": "w", "name": "write_file", "input": { "path": "src/pages/Other.tsx", "content": "export const X = 1;" } },
                    { "type": "tool_use", "id": "d", "name": "delete_file", "input": { "path": "src/App.tsx" } },
                    { "type": "tool_use", "id": "e", "name": "edit_file", "input": { "path": "src/App.tsx", "search": "app", "replace": "changed" } },
                    { "type": "tool_use", "id": "c", "name": "edit_file", "input": { "path": "src/pages/Card.tsx", "search": "  <h2 className=\"t\">Station</h2>", "replace": "  <h2 className=\"t red\">Station</h2>" } }
                ],
                "usage": { "input_tokens": 1_000, "output_tokens": 30 },
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": "The title is red now." }],
                "usage": { "input_tokens": 1_000, "output_tokens": 10 },
            })))
            .mount(&server)
            .await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([
            ("src/App.tsx".to_owned(), "app".to_owned()),
            ("src/pages/Card.tsx".to_owned(), card.to_owned()),
        ]);
        driver
            .scoped_edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut Vec::new(),
                "make the title red",
                None,
                Some("src/pages/Card.tsx"),
            )
            .await
            .expect("the turn ends");

        assert_eq!(files["src/App.tsx"], "app");
        assert!(!files.contains_key("src/pages/Other.tsx"));
        assert!(files["src/pages/Card.tsx"].contains("className=\"t red\""));
        let second = server.received_requests().await.expect("recorded")[1].clone();
        let sent = String::from_utf8_lossy(&second.body).into_owned();
        assert_eq!(
            sent.matches("may change src/pages/Card.tsx only").count(),
            3,
            "{sent}"
        );
    }

    /// T-2467: a model that makes one trivial call after another is cut off at the ceiling
    /// with a sentence the person reads, instead of burning the profile's 120 steps.
    #[tokio::test]
    async fn the_call_ceiling_ends_a_turn_that_does_no_work() {
        let server = wiremock::MockServer::start().await;
        model_listing_forever(&server, 1_000).await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([("src/App.tsx".to_owned(), "app".to_owned())]);
        let mut conversation = Vec::new();
        let shown = driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut conversation,
                "add a form",
                None,
            )
            .await
            .expect("the turn ends");
        assert!(shown.is_none());
        let calls = server.received_requests().await.expect("recorded").len();
        assert_eq!(calls, EDIT_CALLS as usize);
        let said = thoughts(&driver).await;
        assert!(
            said.iter().any(|t| t.contains("12 model calls")),
            "{said:?}"
        );
        assert!(conversation[0].1.contains("12 model calls"));
        // Every call reported its usage, so each is the proxy's frame and count alone (T-3072).
        assert_eq!(usage_frames(&driver).await, Vec::<Value>::new());
    }

    /// T-2467: the input ceiling ends a turn whose calls each resend a large context.
    #[tokio::test]
    async fn the_token_ceiling_ends_a_turn_that_resends_too_much() {
        let server = wiremock::MockServer::start().await;
        model_listing_forever(&server, 200_000).await;
        let driver = edit_driver(&server).await;
        let mut files = BTreeMap::from([("src/App.tsx".to_owned(), "app".to_owned())]);
        driver
            .edit_turn(
                &mut files,
                &mut BTreeMap::new(),
                &mut Vec::new(),
                "add KPIs",
                None,
            )
            .await
            .expect("the turn ends");
        assert_eq!(server.received_requests().await.expect("recorded").len(), 3);
        let said = thoughts(&driver).await;
        assert!(
            said.iter().any(|t| t.contains("500000 input tokens")),
            "{said:?}"
        );
    }
}
