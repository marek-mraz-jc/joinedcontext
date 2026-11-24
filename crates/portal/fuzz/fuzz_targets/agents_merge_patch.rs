//! A model's JSON merge patch of a manifest (T-7711, MF-04): an accepted patch never changes the
//! kind, the name or the namespace, never leaves a status and always changes something; a new
//! resource always carries a spec.
#![no_main]

use joinedcontext_portal::agents::change;
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(patch) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let current = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "bikes", "namespace": "helsinki" },
        "spec": { "period": "15m", "source": { "endpointRef": "bikes" } },
        "status": { "phase": "Ready" },
    });
    if let Ok(after) = change::patched(&current, &patch) {
        for pointer in ["/apiVersion", "/kind", "/metadata/name", "/metadata/namespace"] {
            assert_eq!(after.pointer(pointer), current.pointer(pointer), "{pointer}");
        }
        assert!(after.get("status").is_none(), "{after}");
        let mut before = current.clone();
        before.as_object_mut().map(|m| m.remove("status"));
        assert_ne!(after, before);
        let _ = change::reaches_the_test(&current, &after);
    }
    for kind in change::CREATABLE {
        if let Ok(manifest) = change::new_manifest(kind, "helsinki", "x", Some(&patch)) {
            assert_eq!(manifest["kind"], kind);
            assert!(manifest["spec"].as_object().is_some_and(|s| !s.is_empty()));
        }
    }
    if let Some(spec) = change::spec_of(&patch) {
        assert!(spec.is_object());
    }
    let _ = change::named_attributes(&patch);
});
