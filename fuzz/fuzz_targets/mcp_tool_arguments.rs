// SPDX-License-Identifier: EUPL-1.2
//! A tool call's arguments, whatever their bytes (MCP D-4, MC15): every
//! tool checks them against its schema and the view's names, and a call
//! that passes builds an NGSI-LD request; nothing panics.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(serde_json::Value::Object(given)) = serde_json::from_slice(data) {
        antares_mcp::fuzzing::tool_arguments(&given);
    }
});
