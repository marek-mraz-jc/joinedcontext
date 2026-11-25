//! An activity page cursor comes back from the caller as text: decoding takes any string
//! without a panic, and a cursor that decodes encodes to one that decodes again.
#![no_main]

use joinedcontext_portal::activity::Cursor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(cursor) = Cursor::decode(text) {
        let again = Cursor::decode(&cursor.encode()).expect("an encoded cursor decodes");
        assert_eq!(again.id, cursor.id);
    }
});
