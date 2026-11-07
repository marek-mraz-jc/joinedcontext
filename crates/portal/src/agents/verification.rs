//! What a generated version shows, checked against the run with no model call (SDK-28,
//! Architecture/20 §4.1): the runtime errors and failed function calls since the version went
//! on screen, the requests the bridge answered with an error, the words a broken binding leaves
//! on a page, whether each page has exactly one level-1 heading, and whether the entities the run
//! sampled appear anywhere. Every value of the observation is text the frame wrote; it is
//! matched, never interpreted.

use serde_json::Value;

use crate::agents::run::AgentRunEvent;

/// Words a page shows when a value never arrived or was drawn from the wrong shape.
const BROKEN_WORDS: [&str; 4] = ["NaN", "undefined", "[object Object]", "Invalid Date"];
/// What a map of the template says when none of its rows has a location (the SDK's
/// `NO_LOCATIONS`).
const NO_LOCATIONS: &str = "No locations in this data.";
/// How much of one page's text a verification pass sends back to the model.
const RENDERED_CHARS: usize = 4000;
/// A local id shorter than this matches too much page text to say anything.
const MIN_LOCAL_ID: usize = 3;

/// What the check found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub problems: Vec<String>,
    /// Pages the observation holds.
    pub pages: usize,
    /// Sampled entities some page shows, of all sampled.
    pub shown: usize,
    pub sampled: usize,
}

impl Found {
    /// The one line a clean check says on the conversation.
    pub fn summary(&self) -> String {
        format!(
            "Checked {} page{}: {} of {} sampled entities shown, no errors.",
            self.pages,
            if self.pages == 1 { "" } else { "s" },
            self.shown,
            self.sampled
        )
    }
}

/// Checks one version: `since` are the run's events after its `preview` event, `samples` the
/// entities the run read per type (`options=keyValues`), `observation` the frame's
/// `preview_observation` payload when one came.
pub fn check(samples: &Value, since: &[AgentRunEvent], observation: Option<&Value>) -> Found {
    let mut found = Found::default();
    let mut push = |problem: String| {
        if !found.problems.contains(&problem) {
            found.problems.push(problem);
        }
    };
    for event in since {
        match event.kind.as_str() {
            "preview_error" => push(format!("the preview threw: {}", error_line(&event.payload))),
            "tool" => {
                let tool = event.payload.get("tool").and_then(Value::as_str);
                let failed = event.payload.get("status").and_then(Value::as_str) == Some("failed");
                if let (Some(tool), true) = (tool, failed) {
                    if let Some(function) = tool.strip_prefix("function:") {
                        let reason = event
                            .payload
                            .get("error")
                            .map(error_value)
                            .unwrap_or_else(|| "no reason given".to_owned());
                        push(format!("the function {function} failed: {reason}"));
                    }
                }
            }
            _ => {}
        }
    }
    let Some(observation) = observation else {
        return found;
    };
    // The SDK grid reads `access/attributes` for its column decisions and goes on without it, so
    // its refusal in the sandboxed frame is no fault of the App (T-6385).
    let failed = array(observation, "failedRequests")
        .iter()
        .filter(|failed| !text(failed, "path").contains("/access/attributes"));
    for failed in failed {
        push(format!(
            "the request {} answered {}",
            text(failed, "path"),
            failed.get("status").and_then(Value::as_u64).unwrap_or(0)
        ));
    }
    let pages = array(observation, "pages");
    for page in pages {
        let page_text = text(page, "text");
        for word in BROKEN_WORDS {
            if shows_word(page_text, word) {
                push(format!(
                    "the page \"{}\" shows {word} where a value belongs",
                    text(page, "label")
                ));
            }
        }
        match page.get("h1").and_then(Value::as_u64) {
            Some(0) => push(format!(
                "the page \"{}\" has no level-1 heading: keep the AppShell title, or give the page \
                 one <h1> naming it",
                text(page, "label")
            )),
            Some(many) if many > 1 => push(format!(
                "the page \"{}\" has {many} level-1 headings: keep one <h1> naming the page and \
                 make the others <h2>",
                text(page, "label")
            )),
            _ => {}
        }
        if page_text.contains("urn:ngsi-ld:") {
            push(format!(
                "the page \"{}\" shows entity ids (urn:ngsi-ld:…) where a name belongs: name rows \
                 with displayName(row)",
                text(page, "label")
            ));
        }
        if page_text.contains(NO_LOCATIONS) && located(samples) {
            push(format!(
                "the map on the page \"{}\" has no locations although the sampled entities carry \
                 one: read the location from the endpoint that serves it",
                text(page, "label")
            ));
        }
    }
    let (mut problems, mut shown, mut sampled) = (Vec::new(), 0, 0);
    for (entity_type, entities) in samples.as_object().into_iter().flatten() {
        let entities = entities.as_array().map(Vec::as_slice).unwrap_or_default();
        if entities.is_empty() {
            continue;
        }
        let mut names = Vec::new();
        let mut seen = 0;
        for entity in entities {
            let marks = marks_of(entity);
            if let Some(first) = marks.first() {
                names.push(first.clone());
            }
            if marks.iter().any(|mark| {
                pages
                    .iter()
                    .any(|page| text(page, "text").contains(mark.as_str()))
            }) {
                seen += 1;
            }
        }
        sampled += entities.len();
        shown += seen;
        if seen == 0 {
            names.truncate(3);
            problems.push(format!(
                "no page shows any of the sampled {entity_type} entities ({})",
                names.join(", ")
            ));
        }
    }
    for problem in problems {
        push(problem);
    }
    found.pages = pages.len();
    found.shown = shown;
    found.sampled = sampled;
    found
}

/// What the preview rendered, page by page, as the last item of a verification pass.
pub fn rendered(observation: &Value) -> String {
    let mut out = String::from("What the preview rendered, page by page:");
    for page in array(observation, "pages") {
        let page_text: String = text(page, "text").chars().take(RENDERED_CHARS).collect();
        out.push_str(&format!("\n### {}\n{page_text}", text(page, "label")));
    }
    out
}

/// What identifies an entity on a page: its name, then its local id (the URN's last segment).
fn marks_of(entity: &Value) -> Vec<String> {
    let mut marks = Vec::new();
    let name = entity.get("name");
    // keyValues gives a LanguageProperty as `{languageMap: {fi: "…"}}` (T-6385).
    let languages = name
        .and_then(|name| name.get("languageMap"))
        .and_then(Value::as_object);
    let names = name
        .into_iter()
        .chain(languages.into_iter().flat_map(|map| map.values()));
    for name in names.filter_map(Value::as_str) {
        if !name.trim().is_empty() {
            marks.push(name.trim().to_owned());
        }
    }
    if let Some(local) = entity
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.rsplit(':').next())
    {
        if local.chars().count() >= MIN_LOCAL_ID {
            marks.push(local.to_owned());
        }
    }
    marks
}

/// Whether some sampled entity carries a GeoJSON value, so a map of the samples has points.
fn located(samples: &Value) -> bool {
    samples
        .as_object()
        .into_iter()
        .flat_map(|types| types.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|entity| entity.values())
        .any(|value| {
            value.get("type").is_some_and(Value::is_string) && value.get("coordinates").is_some()
        })
}

/// Whether `word` stands on its own in `text`, not inside a longer word.
fn shows_word(text: &str, word: &str) -> bool {
    let joined = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    text.match_indices(word).any(|(at, _)| {
        !joined(text[..at].chars().next_back()) && !joined(text[at + word.len()..].chars().next())
    })
}

fn error_line(payload: &Value) -> String {
    let message = text(payload, "message");
    let message = if message.is_empty() {
        "an error without a message"
    } else {
        message
    };
    match (
        payload.get("file").and_then(Value::as_str),
        payload.get("line").and_then(Value::as_u64),
    ) {
        (Some(file), Some(line)) => format!("{message} ({file}:{line})"),
        (Some(file), None) => format!("{message} ({file})"),
        _ => message.to_owned(),
    }
}

/// A tool event's `error`: a sentence, or a runtime error with its file and line.
fn error_value(error: &Value) -> String {
    match error {
        Value::String(text) => text.clone(),
        Value::Object(_) => error_line(error),
        other => other.to_string(),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(kind: &str, payload: Value) -> AgentRunEvent {
        AgentRunEvent {
            run_id: "r".into(),
            seq: 1,
            kind: kind.into(),
            payload,
            created_at: String::new(),
        }
    }

    fn samples() -> Value {
        json!({ "BikeHireDockingStation": [
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "name": "Kaivopuisto" },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "name": "Laivasillankatu" }
        ] })
    }

    #[test]
    fn a_page_needs_exactly_one_level_one_heading_when_the_sdk_counts_them() {
        let observed = |h1: Value| {
            let observation = json!({ "version": 1, "pages": [
                { "label": "Stations", "text": "Kaivopuisto 7 bikes", "h1": h1 }
            ] });
            check(&samples(), &[], Some(&observation)).problems
        };
        assert_eq!(
            observed(json!(0)),
            ["the page \"Stations\" has no level-1 heading: keep the AppShell title, or give the \
              page one <h1> naming it"]
        );
        assert!(observed(json!(1)).is_empty());
        assert_eq!(
            observed(json!(2)),
            [
                "the page \"Stations\" has 2 level-1 headings: keep one <h1> naming the page and \
              make the others <h2>"
            ]
        );
        // An SDK older than T-3060 counts nothing, and nothing is claimed.
        assert!(observed(Value::Null).is_empty());
    }

    #[test]
    fn a_clean_version_names_its_pages_and_the_entities_it_shows() {
        let observation = json!({ "version": 1, "pages": [
            { "label": "Overview", "text": "Stations 2" },
            { "label": "Stations", "text": "Kaivopuisto 7 bikes" }
        ] });
        let found = check(&samples(), &[], Some(&observation));
        assert!(found.problems.is_empty(), "{found:?}");
        assert_eq!(
            found.summary(),
            "Checked 2 pages: 1 of 2 sampled entities shown, no errors."
        );
    }

    /// T-6385: dev's bike stations name themselves in a languageMap, and the grid's optional
    /// access read is refused in the frame: both were reported as faults of a working App, and
    /// each report spent a repair turn.
    #[test]
    fn a_language_name_counts_as_shown_and_the_grids_access_read_is_no_fault() {
        let sampled = json!({ "BikeHireDockingStation": [{
            "id": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001",
            "name": { "languageMap": { "fi": "Kaivopuisto" } }
        }] });
        let observation = json!({
            "version": 1,
            "pages": [{ "label": "Map", "h1": 1, "text": "Kaivopuisto 3 bikes" }],
            "failedRequests": [{ "path": "/api/endpoint/x/access/attributes?type=T", "status": 403 }]
        });
        let found = check(&sampled, &[], Some(&observation));
        assert_eq!(found.problems, Vec::<String>::new());
        assert_eq!((found.shown, found.sampled), (1, 1));

        let refused = json!({ "pages": [{ "label": "Map", "h1": 1, "text": "Kaivopuisto" }],
            "failedRequests": [{ "path": "/api/endpoint/x/ngsi-ld/v1/entities", "status": 403 }] });
        assert_eq!(
            check(&sampled, &[], Some(&refused)).problems,
            ["the request /api/endpoint/x/ngsi-ld/v1/entities answered 403"]
        );
    }

    #[test]
    fn errors_failed_functions_requests_broken_words_and_missing_entities_are_problems() {
        let since = [
            event(
                "preview_error",
                json!({ "message": "rows is undefined", "file": "src/pages/Stations.tsx", "line": 6 }),
            ),
            event(
                "tool",
                json!({ "tool": "function:summary", "status": "failed", "error": { "message": "URLSearchParams is not defined", "file": "@joinedcontext/sdk/server", "line": 97 } }),
            ),
            event(
                "tool",
                json!({ "tool": "function:summary", "status": "failed", "error": { "message": "URLSearchParams is not defined", "file": "@joinedcontext/sdk/server", "line": 97 } }),
            ),
            event("tool", json!({ "tool": "apply_patch", "exitCode": 1 })),
        ];
        let observation = json!({
            "version": 1,
            "pages": [{ "label": "Overview", "text": "Bikes NaN · undefinedness is fine · [object Object]" }],
            "failedRequests": [{ "path": "/functions/summary", "status": 500 }]
        });
        let found = check(&samples(), &since, Some(&observation));
        assert_eq!(
            found.problems,
            [
                "the preview threw: rows is undefined (src/pages/Stations.tsx:6)",
                "the function summary failed: URLSearchParams is not defined (@joinedcontext/sdk/server:97)",
                "the request /functions/summary answered 500",
                "the page \"Overview\" shows NaN where a value belongs",
                "the page \"Overview\" shows [object Object] where a value belongs",
                "no page shows any of the sampled BikeHireDockingStation entities (Kaivopuisto, Laivasillankatu)",
            ]
        );
    }

    #[test]
    fn raw_entity_ids_and_a_map_left_without_the_sampled_locations_are_problems() {
        let observation = json!({ "pages": [
            { "label": "Directory", "text": "urn:ngsi-ld:BikeHireDockingStation:001 — 7 bikes" },
            { "label": "Map", "text": "No locations in this data." }
        ] });
        let found = check(&samples(), &[], Some(&observation));
        assert_eq!(
            found.problems,
            ["the page \"Directory\" shows entity ids (urn:ngsi-ld:…) where a name belongs: name rows with displayName(row)"],
            "no sampled entity has a location, so an empty map is the data's own"
        );

        let located = json!({ "BikeHireDockingStation": [
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "name": "Kaivopuisto", "location": { "type": "Point", "coordinates": [24.95, 60.155] } }
        ] });
        let observation = json!({ "pages": [
            { "label": "Map", "text": "Kaivopuisto\nNo locations in this data." }
        ] });
        let found = check(&located, &[], Some(&observation));
        assert_eq!(
            found.problems,
            ["the map on the page \"Map\" has no locations although the sampled entities carry one: read the location from the endpoint that serves it"]
        );
    }

    #[test]
    fn the_empty_map_sentence_is_the_one_the_sdk_shows() {
        assert!(include_str!("../../sdk/src/sdk/map.ts").contains(&format!("\"{NO_LOCATIONS}\"")));
    }

    #[test]
    fn without_an_observation_only_the_errors_count() {
        let since = [event("preview_error", json!({ "message": "boom" }))];
        let found = check(&samples(), &since, None);
        assert_eq!(found.problems, ["the preview threw: boom"]);
    }

    #[test]
    fn a_local_id_on_a_page_counts_and_the_rendered_text_is_cut_per_page() {
        let observation = json!({ "pages": [
            { "label": "Stations", "text": format!("002 {}", "x".repeat(5000)) }
        ] });
        let found = check(&samples(), &[], Some(&observation));
        assert_eq!(found.shown, 1);
        let text = rendered(&observation);
        assert!(text.starts_with("What the preview rendered, page by page:\n### Stations\n002 "));
        assert!(text.chars().count() < 4100);
    }
}
