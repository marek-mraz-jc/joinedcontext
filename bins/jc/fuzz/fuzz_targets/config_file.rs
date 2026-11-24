//! Any jc.toml text: the parser and the merge with the command line never panic; a refusal is
//! an error naming the file (T-7634).
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let file = match jc::Config::parse_file(Path::new("jc.toml"), text) {
        Ok(file) => file,
        Err(e) => {
            assert!(!e.to_string().is_empty());
            return;
        }
    };
    let cli = jc::Cli {
        role: Some("all".into()),
        ..jc::Cli::default()
    };
    if let Err(e) = jc::Config::merge(cli, file) {
        assert!(!e.to_string().is_empty());
    }
});
