//! An App's origin from the name in a request (`{name}.apps.{apex}`): a name that is no DNS label
//! gets none, and an origin is always exactly one label under the apps apex, so no name reaches
//! another host.
#![no_main]

use joinedcontext_portal::apps::static_host::app_origin;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (apex, name) = text
        .split_once('\n')
        .unwrap_or(("https://example.org", text));
    let Ok(apex) = url::Url::parse(apex) else {
        return;
    };
    if let Some(origin) = app_origin(&apex, name) {
        let host = origin.host_str().expect("an origin has a host");
        let under = format!(".apps.{}", apex.host_str().expect("an apex with a host"));
        assert_eq!(host.strip_suffix(&under), Some(name));
        assert!(!name.is_empty() && !name.contains('.'));
        assert_eq!(origin.path(), "/");
        assert_eq!(origin.query(), None);
        assert_eq!(origin.scheme(), apex.scheme());
    }
});
