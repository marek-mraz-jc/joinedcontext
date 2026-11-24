//! `--org-vars <dir>/<org>.yaml` and `--org-tokens-file`: an error names a variable or a line,
//! never echoes a value it refused.
#![no_main]

use ekiben_server::org_vars::parse_org_vars;
use ekiben_server::tokens::parse_token_lines;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_org_vars(text);
    if let Err(e) = parse_token_lines(text) {
        assert!(e.starts_with("line ") || e.starts_with("more than "), "{e}");
    }
});
