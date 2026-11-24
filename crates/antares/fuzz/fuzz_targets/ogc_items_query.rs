//! OGC API Features `items` query and `next` token (`antares_ogc::fuzzing`): any query string
//! never panics, and a token read plain is the same token read with no store cursor (T-7803).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // a query string is capped far below this before it reaches the readers
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let pairs: Vec<(String, String)> = text
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => (p.to_owned(), String::new()),
        })
        .collect();
    let _ = antares_ogc::fuzzing::items_query(&pairs);
    let digest = data
        .iter()
        .fold(0u64, |h, b| h.rotate_left(5) ^ u64::from(*b));
    for (t, d) in [(text, digest), (text, 0)] {
        let (plain, keyset) = antares_ogc::fuzzing::next_token(t, d, 4);
        if let Some((part, offset)) = plain {
            assert!(part < 4, "{t:?}");
            assert_eq!(keyset, Some((part, offset, None)), "{t:?}");
        }
    }
    // a token this crate writes reads back
    let (_, keyset) =
        antares_ogc::fuzzing::next_token(&format!("1.{}.{digest:016x}", data.len()), digest, 4);
    assert_eq!(keyset, Some((1, data.len(), None)));
});
