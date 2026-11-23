//! One count over several Endpoints in one tool call (T-7566 option b): "which city has the most
//! events next week" took one `query_entities` per city, four to a reply, so three model rounds
//! on jc-dev. `count_across` names the Endpoints, one type and an optional `q`; the loop then runs
//! one count-only `query_entities` per Endpoint through the same check, trace, personal-data
//! filter and citation as a direct call. The model writes no URL, code or expression: only
//! Endpoint names the deployment lists, a type and an NGSI-LD `q` each Endpoint's schema checks.

use serde_json::{json, Map, Value};

use super::agent::Connected;
use super::mcp::MAX_SERVER_ENDPOINTS;
use super::standard::{endpoint_of, ENDPOINT};

/// The tool's name.
pub const TOOL: &str = "count_across";
/// The longest `type` and `q` taken, as the routed path takes a `q` (routed.rs MAX_Q_CHARS).
const MAX_TYPE_CHARS: usize = 200;
const MAX_Q_CHARS: usize = 256;

/// The tool's definition, offered when at least two Endpoints answer `query_entities`.
pub fn spec(connectors: &[Connected]) -> Option<Value> {
    let counting = connectors
        .iter()
        .filter(|c| c.tools.iter().any(|t| t.name == "query_entities"))
        .count();
    (counting >= 2).then(|| {
        json!({"type": "function", "function": {
            "name": TOOL,
            "description": "Count the entities of one type on several Endpoints in one call, e.g. events per city: one count per Endpoint, each cited. Use it instead of one query_entities per Endpoint.",
            "parameters": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "endpoints": {"type": "array", "minItems": 2, "maxItems": MAX_SERVER_ENDPOINTS, "uniqueItems": true,
                        "items": {"type": "string", "minLength": 1, "maxLength": 200},
                        "description": "The Endpoints to count on, as the list of Endpoints names them"},
                    "type": {"type": "string", "minLength": 1, "maxLength": MAX_TYPE_CHARS, "description": "The entity type, e.g. Event"},
                    "q": {"type": "string", "maxLength": MAX_Q_CHARS, "description": "An NGSI-LD q every count applies, e.g. startDate>=2026-11-30T00:00:00Z;startDate<2026-12-07T00:00:00Z"}
                },
                "required": ["endpoints", "type"]
            }
        }})
    })
}

/// The `query_entities` arguments of each count, one per Endpoint in the order named, each with
/// [`ENDPOINT`] for `standard::resolved` to check against that Endpoint's own schema; or what
/// the model must correct. The type goes as a list where the Endpoint's schema wants one.
pub fn members(connectors: &[Connected], arguments: &Value) -> Result<Vec<Value>, String> {
    let refused = |why: &str| Err(format!("The call was not made: {why}."));
    let Some(object) = arguments.as_object() else {
        return refused(
            "count_across takes {\"endpoints\": [...], \"type\": \"...\", \"q\": \"...\"}",
        );
    };
    if let Some(other) = object
        .keys()
        .find(|k| !["endpoints", "type", "q"].contains(&k.as_str()))
    {
        return refused(&format!(
            "`{}` is not an argument of count_across",
            super::agent::quoted(other)
        ));
    }
    let Some(kind) = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TYPE_CHARS)
    else {
        return refused(&format!(
            "`type` is one entity type of at most {MAX_TYPE_CHARS} characters"
        ));
    };
    let q = match object.get("q") {
        None => None,
        Some(Value::String(q)) if q.chars().count() <= MAX_Q_CHARS => {
            Some(q.trim()).filter(|q| !q.is_empty())
        }
        Some(_) => {
            return refused(&format!(
                "`q` is a string of at most {MAX_Q_CHARS} characters"
            ))
        }
    };
    let names: Vec<&str> = match object.get("endpoints").and_then(Value::as_array) {
        Some(list) => list.iter().filter_map(Value::as_str).collect(),
        None => Vec::new(),
    };
    let listed = object
        .get("endpoints")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if names.len() != listed || !(2..=MAX_SERVER_ENDPOINTS).contains(&names.len()) {
        return refused(&format!(
            "`endpoints` lists 2 to {MAX_SERVER_ENDPOINTS} Endpoint names"
        ));
    }
    let mut seen = Vec::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let connector = endpoint_of(connectors, name)
            .map_err(|why| format!("The call was not made: {why}."))?;
        if seen.contains(&connector.endpoint) {
            return refused(&format!("Endpoint {} is named twice", connector.endpoint));
        }
        seen.push(connector.endpoint.clone());
        let as_list = connector
            .tools
            .iter()
            .find(|t| t.name == "query_entities")
            .and_then(|t| t.input_schema.pointer("/properties/type/type"))
            == Some(&json!("array"));
        let mut member = Map::new();
        member.insert(ENDPOINT.into(), json!(connector.endpoint));
        member.insert(
            "type".into(),
            if as_list { json!([kind]) } else { json!(kind) },
        );
        if let Some(q) = q {
            member.insert("q".into(), json!(q));
        }
        member.insert("count".into(), json!(true));
        out.push(Value::Object(member));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::mcp::{Surface, Tool};

    fn connector(endpoint: &str, tools: &[&str], type_schema: Value) -> Connected {
        Connected {
            titles: Default::default(),
            endpoint: endpoint.into(),
            title: None,
            surface: Surface {
                gateway: "http://gateway".into(),
                slug: endpoint.into(),
                timeout: std::time::Duration::from_secs(1),
                token: None,
            },
            tools: tools
                .iter()
                .map(|name| Tool {
                    name: (*name).into(),
                    description: String::new(),
                    input_schema: json!({"type": "object", "properties": {"type": type_schema.clone()}}),
                })
                .collect(),
            label: None,
            siblings: Vec::new(),
            dataset: None,
        }
    }

    fn cities() -> Vec<Connected> {
        vec![
            connector(
                "helsinki/events",
                &["query_entities"],
                json!({"type": "string"}),
            ),
            connector(
                "barcelona/events",
                &["query_entities"],
                json!({"type": "array"}),
            ),
            connector("praha/events", &["list_types"], json!({"type": "string"})),
        ]
    }

    #[test]
    fn it_is_offered_only_where_two_endpoints_can_count() {
        let all = cities();
        let spec = spec(&all).expect("two Endpoints count");
        assert_eq!(spec["function"]["name"], TOOL);
        assert_eq!(
            spec["function"]["parameters"]["properties"]["endpoints"]["maxItems"],
            MAX_SERVER_ENDPOINTS
        );
        assert!(super::spec(&all[..1]).is_none());
        assert!(super::spec(&all[1..]).is_none());
    }

    #[test]
    fn each_endpoint_gets_one_count_in_its_own_schema_shape() {
        let got = members(
            &cities(),
            &json!({"endpoints": ["helsinki/events", "barcelona/events"], "type": "Event", "q": "startDate>=2026-11-30T00:00:00Z"}),
        )
        .expect("admitted");
        assert_eq!(
            got,
            [
                json!({"endpoint": "helsinki/events", "type": "Event", "q": "startDate>=2026-11-30T00:00:00Z", "count": true}),
                json!({"endpoint": "barcelona/events", "type": ["Event"], "q": "startDate>=2026-11-30T00:00:00Z", "count": true}),
            ]
        );
        let bare = members(&cities(), &json!({"endpoints": ["helsinki/events", "barcelona/events"], "type": "Event", "q": " "}))
            .expect("admitted");
        assert!(bare[0].get("q").is_none());
    }

    #[test]
    fn a_call_outside_its_shape_is_refused_naming_what_to_correct() {
        let all = cities();
        let two = json!(["helsinki/events", "barcelona/events"]);
        let cases = [
            (json!(null), "count_across takes"),
            (
                json!({"endpoints": two, "type": "Event", "url": "http://x"}),
                "`url` is not an argument",
            ),
            (json!({"endpoints": two}), "`type` is one entity type"),
            (
                json!({"endpoints": two, "type": "x".repeat(MAX_TYPE_CHARS + 1)}),
                "`type` is one entity type",
            ),
            (
                json!({"endpoints": two, "type": "Event", "q": 5}),
                "`q` is a string",
            ),
            (
                json!({"endpoints": two, "type": "Event", "q": "a".repeat(MAX_Q_CHARS + 1)}),
                "`q` is a string",
            ),
            (
                json!({"endpoints": ["helsinki/events"], "type": "Event"}),
                "lists 2 to",
            ),
            (
                json!({"endpoints": ["helsinki/events", 3], "type": "Event"}),
                "lists 2 to",
            ),
            (
                json!({"endpoints": vec!["helsinki/events"; MAX_SERVER_ENDPOINTS + 1], "type": "Event"}),
                "lists 2 to",
            ),
            (
                json!({"endpoints": ["helsinki/events", "helsinki/events"], "type": "Event"}),
                "named twice",
            ),
            (
                json!({"endpoints": ["helsinki/events", "elsewhere/secret"], "type": "Event"}),
                "no Endpoint of this assistant",
            ),
        ];
        for (arguments, expected) in cases {
            let why = members(&all, &arguments).expect_err("refused");
            assert!(
                why.starts_with("The call was not made: ") && why.contains(expected),
                "{arguments}: {why}"
            );
        }
    }
}
