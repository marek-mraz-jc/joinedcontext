//! The answer a client sends back to an AG-63 question (`McpElicitations::answer`, T-5699):
//! never panics, whatever it sends a person's open question is never accepted on the client's
//! word, and the digest of any arguments is 16 hex digits and the same every time.
#![no_main]
use joinedcontext_portal::mcp::elicitation::{digest_of, Answer, McpElicitations};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(sent) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let digest = digest_of(&sent);
    assert_eq!(digest.len(), 16);
    assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(digest, digest_of(&sent.clone()));

    let questions = McpElicitations::new();
    let (owner, op, project) = ("f:1:ana", "jc_resource_delete", "doprava");
    let id = questions.ask(owner, op, project, &digest, "Delete", "/projects/doprava");
    // The raw answer, and the same answer naming the open question's id.
    let mut naming = sent.clone();
    if let Some(fields) = naming.as_object_mut() {
        fields.insert("elicitationId".to_owned(), json!(id));
    }
    for answer in [&sent, &naming] {
        let read = questions.answer(owner, op, project, &digest, answer, false);
        assert_ne!(read, Answer::Accepted, "{answer}");
    }
    // Another subject never reads it at all.
    let read = questions.answer("f:1:eve", op, project, &digest, &naming, true);
    assert!(matches!(read, Answer::Unknown), "{naming}");
});
