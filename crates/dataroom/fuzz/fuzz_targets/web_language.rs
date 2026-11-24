//! Input: the request path, the Cookie header and the Accept-Language header a reader sends,
//! one per line. The page language is always one offered or shipped, a refused URL prefix is two
//! ASCII letters, and the language switch link of the answer stays under the mount prefix.
#![no_main]

use dataroom_web::i18n::{
    available_languages, resolve_request_language, url_in_lang, LanguageResolution,
    SHIPPED_LANGUAGES,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut lines = text.splitn(4, '\n');
    let path = lines.next().unwrap_or_default();
    let cookie = lines.next();
    let accept = lines.next();
    let default = lines.next().unwrap_or("sk");
    let offered: Vec<String> = vec!["sk".into(), "en".into()];
    match resolve_request_language(path, cookie, accept, &offered, default) {
        LanguageResolution::Resolved {
            lang,
            stripped_path,
            ..
        } => {
            let avail = available_languages(&offered);
            assert!(
                avail.contains(&lang) || SHIPPED_LANGUAGES.contains(&lang.as_str()),
                "{lang:?}"
            );
            let link = url_in_lang("/data/", &lang, &stripped_path, None);
            assert!(link.starts_with(&format!("/data/{lang}/")), "{link:?}");
        }
        LanguageResolution::UnknownUrlPrefix(prefix) => {
            assert!(
                prefix.len() == 2 && prefix.bytes().all(|b| b.is_ascii_alphabetic()),
                "{prefix:?}"
            );
            assert!(!SHIPPED_LANGUAGES.contains(&prefix.to_ascii_lowercase().as_str()));
        }
    }
});
