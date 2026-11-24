//! The login target of an interrupted navigation (`auth::refresh::login_redirect`): never
//! panics, always this Portal's `/login`, and its `redirect_to` reads back as the path given.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8 * 1024 {
        return;
    }
    let Ok(path) = std::str::from_utf8(data) else {
        return;
    };
    let target = joinedcontext_portal::auth::refresh::login_redirect(path);
    let query = target
        .strip_prefix("/login?")
        .expect("the login page of this Portal");
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(pairs, [("redirect_to".to_owned(), path.to_owned())]);
});
