//! Input: a method byte (GET, POST, DELETE), then `<path and query under /odata/>\n<body>`, sent
//! as a reader: entity sets, keys, navigations, `$count`/`$export`/`$allowed`, functions and
//! actions with any parameters.
#![no_main]

mod forge;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((which, path, body)) = forge::parts(data) else {
        return;
    };
    let method = ["GET", "POST", "DELETE"][usize::from(which % 3)];
    let body = (method == "POST").then_some(body);
    let (status, _) = forge::RT.block_on(forge::send(
        &forge::APP,
        method,
        &format!("/odata/{path}"),
        forge::READER,
        body,
    ));
    assert!(status == 0 || (200..600).contains(&status));
});
