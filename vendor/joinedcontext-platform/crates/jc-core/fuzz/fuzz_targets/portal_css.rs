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
    // What a browser reads as CSS, every string's text blanked: a string may say anything.
    let bare = outside_strings(&lower);
    // A name right before is part of that name (`#0b53expression(` is a hash and a parenthesis).
    let after_name = |at: usize| {
        bare[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || "-_\\#@".contains(c) || !c.is_ascii())
    };
    for refused in [
        "@import",
        "@font-face",
        "@namespace",
        "expression(",
        "!important",
        ":has(",
    ] {
        for (at, _) in bare.match_indices(refused) {
            assert!(after_name(at), "{refused} in {out}");
        }
    }
    for line in bare.lines() {
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
    for (at, _) in bare.match_indices("url(") {
        assert!(
            after_name(at) || lower[at..].starts_with("url(\"/files/"),
            "{out}"
        );
    }
    // What the check writes checks again, and a second rewrite changes nothing more (the first
    // may still respell a token, `2e` + `x` as `2\65 x`). Taking the prefixes off is exact only
    // when the author's text names none itself.
    if !css.contains("jc-portal") {
        let strip = |text: &str| text.replace("body.jc-portal ", "");
        let again = scoped_stylesheet(&strip(&out), CHECK_CLASS)
            .expect("what the check writes checks again");
        let third = scoped_stylesheet(&strip(&again), CHECK_CLASS).expect("and again");
        assert_eq!(third, again);
    }
});

/// `css` with the text of every quoted string blanked, so a `url(` inside a string is not read.
fn outside_strings(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut quote = None;
    let mut escaped = false;
    for c in css.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                    out.push(c);
                    continue;
                }
                // Byte for byte, so an index into the result is one into `css`.
                out.extend(std::iter::repeat_n(' ', c.len_utf8()));
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}
