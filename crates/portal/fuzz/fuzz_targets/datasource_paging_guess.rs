//! How a source pages, guessed from its first answer (T-7094): a public service's JSON body and
//! its `Link` header, both outside input. The first line of the input is the header, the rest the
//! body. Any bytes yield a guess, never a panic, and the pages a walk reads follow from the
//! records the answer announces.
#![no_main]

use joinedcontext_portal::ops::datasource_paging::guess;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (link, body) = text.split_once('\n').unwrap_or(("", &text));
    let body = serde_json::from_str::<Value>(body).unwrap_or(Value::String(body.to_owned()));
    let url = url::Url::parse("https://data.example/api/items?limit=100").expect("a url");
    for link in [None, Some(link)] {
        let guessed = guess(&url, &body, link);
        if guessed.pages.is_some() {
            assert!(guessed.total.is_some(), "pages come from an announced total");
        }
        let _ = serde_json::to_string(&guessed);
    }
});
