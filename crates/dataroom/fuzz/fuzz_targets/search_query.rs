//! Input: one byte that picks a reader, then the text a caller types into a search: the `q` of
//! `package_search` or the portal's search box, or a CKAN `fq` filter. Each answers terms, a
//! filter or a refusal, never a panic, and keeps the bounds its docs promise.
#![no_main]

use dataroom_core::DatasetFilter;
use dataroom_search::{
    apply_fq_to_filter, fold_unaccent, jc_search_words, jc_stem, parse_search_terms,
    term_candidates, word_stem_candidates,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match pick % 4 {
        0 => {
            if let Ok(terms) = parse_search_terms(text) {
                assert!(terms.len() <= 32, "{} terms", terms.len());
                assert!(terms.iter().all(|t| !t.is_empty()), "an empty term");
                assert!(!text.contains("{!"), "local params accepted: {text:?}");
            }
            let _ = term_candidates(text);
        }
        1 => {
            let mut filter = DatasetFilter::default();
            if apply_fq_to_filter(text, &mut filter).is_ok() {
                assert!(!text.contains("{!"), "local params accepted: {text:?}");
            }
        }
        2 => {
            let folded = fold_unaccent(text);
            if text.is_ascii() {
                assert_eq!(folded, text.to_ascii_lowercase(), "ASCII changed: {text:?}");
            }
            let _ = jc_search_words(text);
        }
        _ => {
            let _ = jc_stem(text);
            let _ = word_stem_candidates(text);
        }
    }
});
