//! Input: registered redirect URI patterns (one per line), then NUL, then the candidate a client
//! sends. An accepted candidate is always an absolute URL without userinfo or fragment, for both
//! the login redirect and the post-logout redirect.
#![no_main]

use kaisatsu_core::ClientRepresentation;
use kaisatsu_oidc::{validate_post_logout_redirect_uri, validate_redirect_uri};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (patterns, candidate) = text.split_once('\0').unwrap_or((&text, ""));
    let registered: Vec<String> = patterns.lines().map(str::to_owned).collect();
    let accepted = validate_redirect_uri(&registered, candidate);
    let mut client = ClientRepresentation {
        redirect_uris: registered.clone(),
        ..ClientRepresentation::default()
    };
    client
        .attributes
        .insert("post.logout.redirect.uris".into(), patterns.replace('\n', "##"));
    let logout_ok = validate_post_logout_redirect_uri(&client, candidate);
    if accepted || logout_ok {
        let url = url::Url::parse(candidate.trim()).expect("accepted URI parses");
        assert!(url.username().is_empty() && url.password().is_none(), "userinfo accepted");
        assert!(url.fragment().is_none(), "fragment accepted");
    }
});
