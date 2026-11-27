//! The scope a caller opens a workspace with (`jc_workspace_open`, `POST .../workspaces`): any
//! JSON is a scope or a refusal, a valid one covers each resource it lists and nothing of another
//! kind, and a space scope covers its own space.
#![no_main]

use joinedcontext_portal::ops::workspaces::Scope;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(scope) = serde_json::from_slice::<Scope>(data) else {
        return;
    };
    if scope.validate().is_err() {
        return;
    }
    match &scope {
        Scope::Project {} => assert!(scope.covers("Endpoint", "any", None)),
        Scope::Space { name } => {
            assert!(scope.covers("ContextSpace", name, None));
            assert!(scope.covers("Endpoint", "x", Some(name)));
        }
        Scope::Resources { items } => {
            assert!(!items.is_empty());
            for item in items {
                assert!(scope.covers(&item.kind, &item.name, None));
            }
            assert!(!scope.covers("NoSuchKind", "x", None));
        }
    }
});
