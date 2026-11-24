//! Any `Authorization` header against any credential: the comparison never panics, and the
//! forms a Portal client sends its own credential in always match (T-7634).
#![no_main]

use axum::http::HeaderValue;
use jc::Module;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let module = Module::ALL[usize::from(pick) % Module::ALL.len()];
    let (header, value) = rest.split_at(rest.len() / 2);
    let Ok(value) = std::str::from_utf8(value) else {
        return;
    };
    if let Ok(got) = HeaderValue::from_bytes(header) {
        let _ = jc::door::is_credential(module, &got, value);
    }
    // The credential itself, as a bearer token: admitted (daiya takes HTTP Basic only).
    let token = value.trim();
    if module != Module::Daiya
        && !token.is_empty()
        && !token.contains(' ')
        && let Ok(own) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        assert!(jc::door::is_credential(module, &own, token));
    }
});
