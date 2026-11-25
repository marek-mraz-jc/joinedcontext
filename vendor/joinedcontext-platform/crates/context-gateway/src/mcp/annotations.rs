//! The title and the three hints of each listed tool (T-4732).

use serde_json::{json, Map, Value};

/// Every listed tool with a human title, as `title` and `annotations.title` (MCP 2025-06-18),
/// and with `readOnlyHint`, `destructiveHint` and `openWorldHint` as booleans where they can be
/// known: a directory of connectors refuses a tool without them (T-4732). A tool keeps a title
/// or a hint it already has. Otherwise the title is its name in words (`query_entities` as
/// "Query entities"). A read-only tool is not destructive. A write that does not say otherwise
/// is, as the MCP default has it. And a tool reads this platform's own broker and catalogue, a
/// closed world (the broker says `openWorldHint: false` of its own). A missing `readOnlyHint` is
/// never guessed. The name itself never changes, since clients call tools by it.
pub(crate) fn annotate(tools: &mut [Value]) {
    for tool in tools.iter_mut().filter_map(Value::as_object_mut) {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let title = match tool.get("title").and_then(Value::as_str) {
            Some(own) if !own.trim().is_empty() => own.to_owned(),
            _ => in_words(name),
        };
        let mut annotations = match tool.remove("annotations") {
            Some(Value::Object(own)) => own,
            _ => Map::new(),
        };
        annotations.insert("title".to_owned(), json!(title));
        hinted(&mut annotations);
        tool.insert("annotations".to_owned(), Value::Object(annotations));
        tool.insert("title".to_owned(), json!(title));
    }
}

/// The hints that follow from what the tool already says.
fn hinted(annotations: &mut Map<String, Value>) {
    let missing = |annotations: &Map<String, Value>, hint: &str| {
        !annotations.get(hint).is_some_and(Value::is_boolean)
    };
    if let Some(read_only) = annotations.get("readOnlyHint").and_then(Value::as_bool) {
        if missing(annotations, "destructiveHint") {
            annotations.insert("destructiveHint".to_owned(), json!(!read_only));
        }
    }
    if missing(annotations, "openWorldHint") {
        annotations.insert("openWorldHint".to_owned(), json!(false));
    }
}

/// `query_entities` as "Query entities".
fn in_words(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool without a name is left as it is; one whose annotations are no object gets an
    /// object; an own title and own hints stay.
    #[test]
    fn a_nameless_tool_stays_and_odd_annotations_are_replaced() {
        let mut tools = vec![
            json!({"description": "no name"}),
            json!("not a tool"),
            json!({"name": "query_entities", "annotations": "odd"}),
            json!({"name": "x", "title": "Own", "annotations": {"readOnlyHint": false, "openWorldHint": true}}),
            json!({"name": "", "title": "  "}),
        ];
        annotate(&mut tools);
        assert_eq!(tools[0], json!({"description": "no name"}));
        assert_eq!(tools[1], json!("not a tool"));
        assert_eq!(tools[2]["title"], json!("Query entities"));
        assert_eq!(
            tools[2]["annotations"],
            json!({"title": "Query entities", "openWorldHint": false})
        );
        assert_eq!(
            tools[3]["annotations"],
            json!({"title": "Own", "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true})
        );
        assert_eq!(tools[4]["title"], json!(""));
    }
}
