//! A data write as an MCP client sends it (T-6451, DM-113): the body is outside input. Any bytes
//! are refused or pass the check, never a panic; a delete that passes names its ids and the
//! person's reason, and nothing else.
#![no_main]

use joinedcontext_portal::ops::data_write::{checked, Action, DataWriteInput};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<DataWriteInput>(data) else {
        return;
    };
    if checked(&input).is_ok() && input.action == Action::Delete {
        assert!(!input.ids.is_empty(), "a delete names its ids");
        assert!(input.entities.is_empty(), "a delete carries no entities");
        assert!(input.reason.is_some(), "a delete names why");
    }
});
