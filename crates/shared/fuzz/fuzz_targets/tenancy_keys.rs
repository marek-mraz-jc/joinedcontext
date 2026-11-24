//! Any text as a label, an id (UUID or path segment) or an internal name: parsing never panics, an
//! accepted value prints back to what parses to the same key, and an internal name stays within its
//! documented length (T-7805).
#![no_main]

use jc_types::{INTERNAL_NAME_MAX, InternalName, Label, OrgId, ProjectId, internal_name};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(label) = Label::parse(text) {
        assert_eq!(label.as_str(), text);
        assert!(text.len() <= 63);
        let json = serde_json::to_string(&label).expect("a label serializes");
        assert_eq!(serde_json::from_str::<Label>(&json).ok(), Some(label));
    }
    let _ = serde_json::from_str::<Label>(text);
    if let Ok(id) = OrgId::parse_verified(text) {
        assert_eq!(OrgId::parse_verified(&id.to_string()), Ok(id));
        assert_eq!(OrgId::from_path_segment(&id.path_segment()), Ok(id));
    }
    if let Ok(id) = ProjectId::from_path_segment(text) {
        // A path segment has one spelling.
        assert_eq!(id.path_segment(), text);
    }
    if let Ok((key, label)) = InternalName::parse(text) {
        let name = internal_name(key, &label);
        assert_eq!(name.as_str(), text, "an internal name has one spelling");
        assert!(name.as_str().len() <= INTERNAL_NAME_MAX);
        assert!(key.dns_label().len() <= 63);
    }
});
