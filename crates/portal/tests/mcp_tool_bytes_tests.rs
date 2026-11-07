//! T-6058: the tool definitions a model re-reads on every turn stay under a byte ratchet.
//!
//! A client hands its model each listed tool's name, description and input schema on every
//! turn (Claude, Claude Code and the connector lane's `tests/connector_scenarios/model.rs` alike),
//! so those bytes are paid once per turn per tool. `project`'s meaning is said once, in the
//! `initialize` instructions, and each tool's schema keeps its bounds.

use joinedcontext_portal::mcp::tool_definition;
use joinedcontext_portal::ops::{registry, summary};
use serde_json::{json, Value};

/// What a model reads of one tool.
fn read_by_the_model(tool: &Value) -> usize {
    json!({
        "name": tool["name"],
        "description": tool["description"],
        "inputSchema": tool["inputSchema"],
    })
    .to_string()
    .len()
}

/// The whole registry, as the Portal door lists it to a person who may run every operation: 78
/// tools measured 62,597 bytes on main 0023cec1 (125,945 with output schemas and the rest a client
/// keeps for itself). The ratchet only comes down.
#[test]
fn the_tool_definitions_a_model_reads_stay_under_their_byte_ratchet() {
    let mut sizes: Vec<(usize, &str)> = registry()
        .iter()
        .map(|op| (read_by_the_model(&tool_definition(&summary(op))), op.name))
        .collect();
    sizes.sort_unstable();
    let total: usize = sizes.iter().map(|(bytes, _)| bytes).sum();
    eprintln!("{} tools, {total} bytes: {sizes:?}", sizes.len());
    assert!(total < RATCHET, "{total} bytes, the ratchet is {RATCHET}");
}

/// 53,459 once `project` is said once in `initialize` and the descriptions lost what their names
/// and bounds already say (T-6058); 50,592 once the four per-kind propose tools, whose input was
/// `jc_resource_propose`'s, left the registry (T-6067); 50,747 once `jc_change_list` took
/// `limit` and `cursor` (T-6291): 155 bytes a turn against answers of up to 49 KB that had no page;
/// 50,875 once `jc_pipeline_sample_source` took `planId` (T-6269): 125 bytes a turn against an
/// open-data import that stopped before its first sample; 51,809 with `jc_knowledge_status` (409)
/// and `jc_knowledge_search` (562), the knowledge segment's two reads, which no other tool
/// answers (T-6284); 52,379 once `jc_mqtt_captured` joined
/// (T-6290, 570 bytes): the only door to a capture broker's messages, in the route's own words
/// (route_coverage_tests); 63,964 with the fourteen tools an agent needed to do a steward's
/// page work over MCP (T-6271, T-6380, T-6381, T-6389; T-6372 rows JC2-28..34, 58..60):
/// jc_endpoint_preview 1,840, jc_access_simulate 1,135, jc_data_write 1,061, jc_pipeline_inspect
/// 969, jc_change_history 925, jc_space_governance 902, jc_mqtt_test_publish 885,
/// jc_access_explore 793, jc_attribute_explain 689, jc_model_sdm_search 593,
/// jc_model_sdm_import 514, jc_model_generate 513, jc_catalogue_draft 466, jc_permissions_me 315.
/// Four pipeline panels and six governance readings went in one tool each (`view`, `aspect`), and
/// a connector lists only its segments' share; the assistant's own turn lists none of them
/// (tools_registry.rs MCP_ONLY); 64,034 once `jc_kpi_compute` took `endpoint` (T-6484, 69 bytes): a
/// KPI read from an Endpoint needed no rows carried through the client's model.
const RATCHET: usize = 64_035;

/// The cut never opens a schema: every MCP input stays closed, and `project` stays bounded.
#[test]
fn every_listed_input_stays_closed_and_project_bounded() {
    for op in registry() {
        let tool = tool_definition(&summary(op));
        let schema = &tool["inputSchema"];
        assert_eq!(schema["additionalProperties"], json!(false), "{}", op.name);
        assert_eq!(
            schema["properties"]["project"]["maxLength"],
            json!(63),
            "{}",
            op.name
        );
        assert_eq!(schema.get("description"), None, "{}", op.name);
    }
}
