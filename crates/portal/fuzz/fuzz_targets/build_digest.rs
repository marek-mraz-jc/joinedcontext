//! What a build pod uploads as its image: a tar of an OCI layout whose index.json names one
//! manifest. Any bytes give a digest or a reason, never a panic.
#![no_main]

use joinedcontext_portal::api::build_listener::digest_of;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    assert!(digest_of(data, false).is_ok_and(|d| d.len() == "sha256:".len() + 64));
    let _ = digest_of(data, true);
});
