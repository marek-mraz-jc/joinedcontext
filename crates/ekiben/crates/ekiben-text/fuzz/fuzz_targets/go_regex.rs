//! Input: a Go pattern, a NUL byte, then the haystack. A pattern compiles or is refused with Go's
//! message; a compiled one reports one name per group and matches any haystack.
#![no_main]

use ekiben_text::go_regex::compile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (pattern, hay) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &b""[..]),
    };
    let Ok(pattern) = std::str::from_utf8(pattern) else {
        return;
    };
    match compile(pattern) {
        Ok(re) => {
            assert_eq!(re.group_names().count(), re.captures_len());
            let hay = String::from_utf8_lossy(hay);
            for caps in re.captures_iter(&hay).take(1000) {
                assert!(caps.get(0).is_some());
            }
        }
        Err(e) => assert!(e.starts_with("error parsing regexp: "), "{e}"),
    }
});
