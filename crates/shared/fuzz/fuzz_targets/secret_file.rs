//! Any bytes as a secret file and any text as its variable: the read never panics, an error names
//! the variable and the path and never the value, and a binary file is held as its hex (T-7805).
#![no_main]

use jc_secret::{read, read_file};
use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let mut file = tempfile::NamedTempFile::new().expect("a temp file");
    file.write_all(data).expect("written");
    match read_file("JC_FUZZ", file.path()) {
        Ok(secret) => match std::str::from_utf8(data) {
            Ok(text) => assert_eq!(secret.expose(), text.trim()),
            Err(_) => assert_eq!(secret.expose().len(), data.len() * 2),
        },
        Err(e) => {
            let shown = e.to_string();
            assert!(shown.starts_with("JC_FUZZ_FILE "), "{shown}");
            assert!(std::str::from_utf8(data).is_ok_and(|t| t.trim().is_empty()));
        }
    }
    let value = String::from_utf8_lossy(data).into_owned();
    match read("JC_FUZZ", Some(value.clone()), None) {
        Ok(Some((secret, _))) => assert_eq!(secret.expose(), value.trim()),
        Ok(None) => unreachable!("a value was given"),
        Err(e) => {
            assert!(value.trim().is_empty());
            assert_eq!(e.to_string(), "JC_FUZZ is set and empty");
        }
    }
});
