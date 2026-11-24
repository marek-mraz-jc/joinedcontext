//! Input: ui_locales, the locale cookie, Accept-Language and the realm default, split at NUL.
//! The answer is always a shipped locale, and its bundle formats any arguments.
#![no_main]

use kaisatsu_core::i18n::{negotiate_locale, Bundle, SUPPORTED_LOCALES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.split('\0');
    let ui = parts.next();
    let cookie = parts.next();
    let accept = parts.next();
    let default = parts.next().unwrap_or("en");
    let got = negotiate_locale(ui, cookie, accept, default);
    assert!(SUPPORTED_LOCALES.contains(&got), "negotiated {got}");
    let bundle = Bundle::for_locale(default);
    let args: Vec<&str> = parts.collect();
    let key = accept.unwrap_or_default();
    let _ = bundle.format(key, &args);
    let _ = bundle.translate_client_name(key);
});
