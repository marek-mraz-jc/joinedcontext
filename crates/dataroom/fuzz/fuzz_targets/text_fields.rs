//! Input: one byte that picks a field, then the text a request or a manifest gives it: a file
//! name, file bytes, a slug, a language tag, an Organization id, a licence, a theme, a format, a
//! media type or a language IRI. Each answers a value or a refusal naming the field.
#![no_main]

use dataroom_core::files::{check_name, sniff};
use dataroom_core::model::{is_language_tag, validate_slug, OrgId};
use dataroom_core::vocab;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    if pick % 8 == 0 {
        if let Ok(mime) = sniff(rest) {
            assert!(!mime.is_empty());
        }
        return;
    }
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match pick % 8 {
        1 => {
            if let Ok(name) = check_name(text) {
                assert!(!name.is_empty() && !name.contains(['/', '\\']), "{name:?}");
                assert!(name != "." && name != "..");
            }
        }
        2 => {
            if validate_slug("name", text).is_ok() {
                assert!((2..=100).contains(&text.len()), "{text:?}");
            }
        }
        3 => {
            if is_language_tag(text) {
                assert!(!text.is_empty() && text.is_ascii(), "{text:?}");
            }
        }
        4 => {
            if let Ok(id) = OrgId::parse_verified(text) {
                assert!(!id.as_uuid().is_nil());
            }
        }
        5 => {
            let _ = vocab::find_licence(text);
            let _ = vocab::theme_to_iri(text);
            let _ = vocab::theme_code(text);
            let _ = vocab::theme_label(text, "sk");
        }
        6 => {
            let _ = vocab::format_to_eu_iri(text);
            let _ = vocab::media_type_to_iana_iri(text);
            let _ = vocab::iana_iri_to_media_type(text);
        }
        _ => {
            let _ = vocab::frequency_to_eu_iri(text);
            let _ = vocab::language_to_eu_iri(text);
            let _ = vocab::eu_iri_to_language(text);
        }
    }
});
