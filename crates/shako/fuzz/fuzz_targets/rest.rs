//! Input: a method byte (GET, POST, PUT, PATCH, DELETE), then `<path under the repository>\n
//! <JSON body>`, sent as a reader to the Gitea-compatible REST surface of one repository.
#![no_main]

mod forge;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((which, path, body)) = forge::parts(data) else {
        return;
    };
    let method = ["GET", "POST", "PUT", "PATCH", "DELETE"][usize::from(which % 5)];
    let body = (method != "GET").then_some(body);
    let uri = format!("/api/v1/repos/joinedcontext/site/{path}");
    let (status, _) =
        forge::RT.block_on(forge::send(&forge::APP, method, &uri, forge::READER, body));
    assert!(status == 0 || (200..600).contains(&status));
});
