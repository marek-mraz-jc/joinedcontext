//! Input: `<secret>\n<presented signature>\n<body>`. Only the signature computed over the body
//! with the secret verifies.
#![no_main]

use libfuzzer_sys::fuzz_target;
use shako_api::{compute_gitea_signature, verify_gitea_signature};

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(3, |&b| b == b'\n');
    let secret = String::from_utf8_lossy(parts.next().unwrap_or_default());
    let presented = String::from_utf8_lossy(parts.next().unwrap_or_default());
    let body = parts.next().unwrap_or_default();
    let Ok(right) = compute_gitea_signature(&secret, body) else {
        return;
    };
    assert!(verify_gitea_signature(&secret, body, &right));
    if presented.trim() != right {
        assert!(
            !verify_gitea_signature(&secret, body, &presented)
                || presented.trim().eq_ignore_ascii_case(&right)
        );
    }
});
