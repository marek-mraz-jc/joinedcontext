//! Input: a JSON array of manifests, judged as the proposals of one checkout in order onto an
//! empty Portal under the default bounds. Each refusal carries words a person can read.
#![no_main]

use jc_core::kinds::OrganizationBounds;
use joinedcontext_portal::judge::replay;
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::store::Mirror;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(envelopes) = serde_json::from_slice::<Vec<ResourceEnvelope>>(data) else {
        return;
    };
    if envelopes.len() > 64 {
        return;
    }
    let bounds = OrganizationBounds::default();
    let refused = replay(
        &Mirror::new(),
        (&bounds, Some("example.org"), None, None),
        envelopes,
    );
    for (_, reason) in &refused {
        assert!(!reason.trim().is_empty());
    }
});
