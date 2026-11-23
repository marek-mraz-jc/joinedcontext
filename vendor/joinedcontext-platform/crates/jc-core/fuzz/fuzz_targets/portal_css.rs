//! Any text: the check never panics, and what it accepts is scoped, fetches nothing from another
//! host, holds no `</` and checks again to itself (T-7745).
#![no_main]

use jc_core::kinds::stylesheet::{scoped_stylesheet, CHECK_CLASS};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(css) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(out) = scoped_stylesheet(css, CHECK_CLASS) else {
        return;
    };
    assert!(!out.contains("</"), "{out}");
    let lower = out.to_ascii_lowercase();
    for refused in ["@import", "@font-face", "@namespace", "expression(", "!important", ":has("] {
        assert!(!lower.contains(refused), "{refused} in {out}");
    }
    for line in out.lines() {
        let line = line.trim_start();
        if line.ends_with('{') && !line.starts_with("@media") {
            for selector in line.trim_end_matches('{').split(", body.") {
                let selector = selector.trim();
                assert!(
                    selector.starts_with("body.jc-portal ") || selector.starts_with("jc-portal "),
                    "unscoped {selector} in {out}"
                );
            }
        }
    }
    // A `url(` right after a name character is a dimension's unit and a parenthesis, which no
    // browser fetches; every function `url(` names a portal file.
    for (at, _) in lower.match_indices("url(") {
        let after_name = lower[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || "-_\\".contains(c));
        assert!(after_name || lower[at..].starts_with("url(\"/files/"), "{out}");
    }
    // Taking the prefixes off again is exact only when the author's text names none itself.
    if !css.contains("jc-portal") {
        let again = scoped_stylesheet(&out.replace("body.jc-portal ", ""), CHECK_CLASS)
            .expect("what the check writes checks again");
        assert_eq!(again, out);
    }
});
