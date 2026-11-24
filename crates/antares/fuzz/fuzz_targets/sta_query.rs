//! SensorThings query options (`antares_sta::query`, `antares_sta::residual::parse`): a request's
//! `$top`, `$skip`, `$orderby`, `$expand`, `$filter` and resource path never panic, and what
//! `options` accepts stays inside its caps (T-7803).
#![no_main]
use antares_sta::query;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // a query string is capped far below this before it reaches the parsers
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // `name=value&...`, as axum's Query hands the pairs over
    let pairs: Vec<(String, String)> = text
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => (p.to_owned(), String::new()),
        })
        .collect();
    if let Ok(o) = query::options(&pairs) {
        assert!(o.top <= query::MAX_TOP, "$top {} over the cap", o.top);
    }
    let names: query::Names<'_> = &|n: &str| Ok(n.to_owned());
    let _ = query::filter_to_q(text, names);
    let _ = antares_sta::residual::parse(text);
    if let Ok(by) = query::by_id(text, "Things") {
        let _ = by.admits(Some(text));
        let _ = by.admits(None);
    }
    let _ = query::series_filter(text);
    let _ = query::steps(text);
    let _ = query::property("Things", text);
    let _ = antares_sta::mapping::part(text);
});
