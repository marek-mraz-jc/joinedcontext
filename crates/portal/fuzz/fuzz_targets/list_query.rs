//! A list query string and the cursor it carries: any text is a value or a 400, never a panic,
//! and a cursor the Portal sealed opens to the same key under its own listing only.
#![no_main]

use joinedcontext_portal::api::list_query::{open, seal, ListQuery};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(query) = ListQuery::parse(Some(text)) else {
        return;
    };
    let doc = json!({ "metadata": { "name": text, "labels": { "a": text } }, "spec": [text, 1] });
    if query.q.is_none() {
        assert!(query.finds(&doc));
    }
    let listing = query.listing_id(&[Some("helsinki"), None]);
    if let Some(cursor) = &query.cursor {
        if let Ok((_, name)) = open(cursor, &listing) {
            panic!("an unsealed cursor opened to {name}");
        }
    }
    let key = Value::String(text.to_owned());
    let sealed = seal(&listing, &key, text);
    let (opened, name) = open(&sealed, &listing).expect("a cursor this listing sealed");
    assert_eq!((opened, name.as_str()), (key, text));
    assert!(open(&sealed, "another listing").is_err());
});
