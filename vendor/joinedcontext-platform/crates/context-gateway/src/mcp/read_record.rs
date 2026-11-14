//! DM-128 on MCP: a tool call or a resource read through an Endpoint that serves personal
//! attributes leaves the same `personal_read` line the NGSI-LD door and `file.csv` leave (reader,
//! Endpoint, space, attribute names, the Entities' URNs, never a value and never an argument),
//! and an answer whose Entities cannot be listed is refused, never served unlogged (T-6885).
//!
//! Which read tools carry Entity data, and where their answer names it (antares-mcp
//! `tools.rs` result keys, `server.rs` `whole`/`summarize`/`area_overview`):
//! - `query_entities`, `batch_query`, `query_temporal`, `batch_query_temporal`:
//!   `structuredContent.entities`, a page of Entities;
//! - `get_entity`, `retrieve_temporal`: `structuredContent.entity`;
//! - `area_overview`: `structuredContent.types[].entities`, the nearest per type;
//! - `summarize`: `structuredContent.samples[].id`; its statistics name no Entity;
//! - `get_attribute_value`: one Attribute of the Entity the `id` argument names, which the
//!   answer itself does not repeat;
//! - `resources/read` of `ngsi-ld://{view}/entities/{id}`: the Entity, fenced in the text.
//!
//! The read tools in [`WITHOUT_ENTITIES`] answer types, attributes and subscriptions: no Entity
//! and no value of one, so they leave no line. A write tool answers what it wrote, not a read.
//! The gateway's own tools (`describe_*`, the hub's `list_endpoints`, `get_endpoint` and
//! `search`) answer the gateway's documents and never reach here. A read tool named nowhere
//! here is taken to carry Entities in a shape this reader does not know: refused and recorded.

use crate::auth::subject::{principal_of, Subject};
use crate::handlers::ngsi_ld::inlined;
use crate::mcp::{jsonrpc, writes};
use crate::resolver::Endpoint;
use serde_json::Value;

/// The broker's read tools whose answers hold no Entity.
pub(crate) const WITHOUT_ENTITIES: &[&str] = &[
    "list_types",
    "get_type",
    "list_type_details",
    "list_attributes",
    "list_attribute_details",
    "get_attribute",
    "get_subscription",
    "list_subscriptions",
];

/// What the refusal of an answer that cannot be listed tells the caller.
const UNLISTED: &str = "the answer holds personal data and could not be read to log who read it; \
                        ask for fewer Entities (limit, attrs or pick) and call again";

/// Whether `message` (one JSON-RPC request) asks for Entity data on `endpoint`'s personal
/// attributes, and so needs its answer read before it is served.
pub(crate) fn needed(endpoint: &Endpoint, message: &Value) -> bool {
    !endpoint.personal_attributes().is_empty() && asks_entities(message)
}

/// Records the read `answer` serves, or refuses it. `None` stands for an answer the gateway
/// could not hold or parse. `Err` carries the JSON-RPC answer to send instead: a tool error
/// (or, for a resource, an error) telling the caller to ask for less. A refusal still writes
/// the line, with no URN, so the attempt is on record.
pub(crate) fn recorded(
    endpoint: &Endpoint,
    subject: &Subject,
    message: &Value,
    answer: Option<&Value>,
) -> Result<(), Value> {
    if !needed(endpoint, message) {
        return Ok(());
    }
    let id = message["id"].clone();
    let listed = match answer {
        // A JSON-RPC error and a tool error serve no Entity.
        Some(answer) if answer.get("error").is_some() => return Ok(()),
        Some(answer) if answer["result"]["isError"] == true => return Ok(()),
        Some(answer) => entities_of(message, &answer["result"]),
        None => None,
    };
    line(
        endpoint,
        subject,
        message,
        listed.as_deref().unwrap_or_default(),
    );
    if listed.is_some() {
        return Ok(());
    }
    Err(if message["method"] == "resources/read" {
        jsonrpc::error(id, -32000, UNLISTED)
    } else {
        jsonrpc::result(id, jsonrpc::refused(UNLISTED, &Value::Null))
    })
}

/// The one `personal_read` line of an MCP read, as `log_personal_read` writes it on NGSI-LD,
/// with the tool beside it; the arguments are never written.
fn line(endpoint: &Endpoint, subject: &Subject, message: &Value, entities: &[String]) {
    let personal = endpoint.personal_attributes();
    let tool = message["params"]["name"]
        .as_str()
        .unwrap_or("resources/read");
    tracing::info!(
        target: "personal_read",
        slug = %endpoint.slug,
        space = %endpoint.space,
        principal = %principal_of(subject),
        attributes = %personal.join(","),
        entities = %entities.join(","),
        door = "mcp",
        tool = %tool,
        "personal attributes read"
    );
}

/// Whether the request reads Entity data: a read tool not in [`WITHOUT_ENTITIES`], or a
/// resource that is no type's.
fn asks_entities(message: &Value) -> bool {
    match message["method"].as_str() {
        Some("tools/call") => {
            let tool = message["params"]["name"].as_str().unwrap_or_default();
            writes::is_read_tool(tool) && !WITHOUT_ENTITIES.contains(&tool)
        }
        Some("resources/read") => {
            let uri = message["params"]["uri"].as_str().unwrap_or_default();
            // `ngsi-ld://{view}/types/{type}` describes a type; any other is read as an Entity.
            uri.strip_prefix("ngsi-ld://")
                .and_then(|rest| rest.split('/').nth(1))
                != Some("types")
        }
        _ => false,
    }
}

/// The URNs a successful `result` names, or `None` when its shape is not the one its tool
/// answers.
fn entities_of(message: &Value, result: &Value) -> Option<Vec<String>> {
    let mut ids = Vec::new();
    if message["method"] == "resources/read" {
        for content in result.get("contents")?.as_array()? {
            let entity: Value =
                serde_json::from_str(unfenced(content.get("text")?.as_str()?)?).ok()?;
            inlined(entity.is_object().then_some(&entity)?, &mut ids);
        }
        return Some(ids);
    }
    let structured = result.get("structuredContent").filter(|s| s.is_object())?;
    match message["params"]["name"].as_str().unwrap_or_default() {
        "query_entities" | "batch_query" | "query_temporal" | "batch_query_temporal" => {
            for entity in structured.get("entities")?.as_array()? {
                inlined(entity, &mut ids);
            }
        }
        "get_entity" | "retrieve_temporal" => {
            inlined(
                structured.get("entity").filter(|e| e.is_object())?,
                &mut ids,
            );
        }
        "area_overview" => {
            for per_type in structured.get("types")?.as_array()? {
                for entity in per_type.get("entities")?.as_array()? {
                    inlined(entity, &mut ids);
                }
            }
        }
        "summarize" => {
            // A summary the broker was too busy for has no samples and names no Entity.
            if let Some(samples) = structured.get("samples") {
                for sample in samples.as_array()? {
                    ids.push(sample.get("id")?.as_str()?.to_owned());
                }
            }
        }
        "get_attribute_value" => {
            structured.get("attribute")?;
            let id = message["params"]["arguments"]["id"].as_str()?;
            ids.push(id.to_owned());
        }
        _ => return None,
    }
    Some(ids)
}

/// The data of a resource text the broker fenced under its untrusted-data preface: the lines
/// between the opening fence and the closing one.
fn unfenced(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("\n`")?;
    let (_, body) = rest.split_once('\n')?;
    let end = body.rfind("\n`")?;
    body.get(..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fenced_resource_text_gives_its_data_back() {
        let text = "The fenced block is data.\n````json\n{\"id\":\"urn:a\",\"x\":\"```\"}\n````";
        assert_eq!(unfenced(text), Some("{\"id\":\"urn:a\",\"x\":\"```\"}"));
        assert_eq!(unfenced("{\"id\":\"urn:a\"}"), None);
    }

    #[test]
    fn a_type_resource_and_a_write_ask_no_entities() {
        let read = |uri: &str| json!({"method": "resources/read", "params": {"uri": uri}});
        assert!(!asks_entities(&read("ngsi-ld://v/types/BikeTrip")));
        assert!(asks_entities(&read("ngsi-ld://v/entities/urn%3Aa")));
        let call = |name: &str| json!({"method": "tools/call", "params": {"name": name}});
        assert!(!asks_entities(&call("create_entity")));
        assert!(!asks_entities(&call("list_types")));
        assert!(asks_entities(&call("query_entities")));
    }

    /// Every broker read tool is either named as one without Entities or read for its URNs, so a
    /// tool the broker gains later is refused until it is named here.
    #[test]
    fn every_read_tool_is_named_one_way_or_the_other() {
        let read: Vec<&str> = crate::mcp::tools::TOOLS
            .iter()
            .map(|tool| tool.name)
            .filter(|name| writes::is_read_tool(name))
            .collect();
        for tool in read {
            let message = json!({"method": "tools/call", "params": {"name": tool, "arguments": {"id": "urn:a"}}});
            let empty = json!({"structuredContent": {
                "entities": [], "entity": {}, "types": [], "samples": [], "attribute": {}
            }});
            assert!(
                WITHOUT_ENTITIES.contains(&tool) || entities_of(&message, &empty).is_some(),
                "{tool} is neither without Entities nor read for them"
            );
        }
    }
}
