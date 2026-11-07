//! 4.17 Entity Type Selection Language (`type=`): never panics, whatever the
//! text, and selecting from no types matches nothing (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

static CTX: std::sync::OnceLock<antares_jsonld::Context> = std::sync::OnceLock::new();

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    let Ok(sel) = std::str::from_utf8(data) else { return };
    let ctx = CTX.get_or_init(antares_jsonld::core_context);
    let _ = antares_ql::type_selection_matches(sel, &["https://example.org/T", "https://example.org/U"], ctx);
    if sel.trim() != "*" {
        assert!(!antares_ql::type_selection_matches(sel, &[], ctx), "{sel:?} matched an entity of no type");
    }
});
