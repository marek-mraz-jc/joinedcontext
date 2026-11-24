//! Input: a list page's query string as a reader sends it. Every pager, facet and filter-pill
//! link the page builds from it carries the reader's own parameters back, decoded, minus the
//! page number and the `_<facet>_limit` switches, and nothing else.
#![no_main]

use dataroom_web::listing::{filter_pills, link_with};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    let params: BTreeMap<String, String> = url::form_urlencoded::parse(data)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let base = "/sk/dataset";
    let link = link_with(base, &params, &[("tags", Some("ovzdušie")), ("page", None)]);
    let query = match link.strip_prefix(base) {
        Some("") => "",
        Some(rest) => rest.strip_prefix('?').expect("a query follows the base"),
        None => panic!("the link leaves its base: {link:?}"),
    };
    let back: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut want: BTreeMap<String, String> = params
        .iter()
        .filter(|(k, v)| {
            !v.is_empty() && *k != "page" && !(k.starts_with('_') && k.ends_with("_limit"))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    want.insert("tags".into(), "ovzdušie".into());
    assert_eq!(back, want, "{link:?}");

    for pill in filter_pills(base, &params, &[("res_format", "Format".into())], |_, v| {
        v.to_owned()
    }) {
        let rest = pill.remove_href.strip_prefix(base).expect("under the base");
        let rest = rest.strip_prefix('?').unwrap_or(rest);
        assert!(
            url::form_urlencoded::parse(rest.as_bytes()).all(|(k, _)| k != "res_format"),
            "{pill:?}"
        );
    }
});
