//! A webhook address the forge answers (`jc_repository_settings`): what an administrator reads
//! of it is scheme, host, port and path, never its query or fragment, where a secret rides along.
#![no_main]

use joinedcontext_portal::ops::repositories::shown_address;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let raw = String::from_utf8_lossy(data);
    let shown = shown_address(&raw);
    match url::Url::parse(&raw) {
        Ok(url) => {
            assert!(
                shown.starts_with(&format!("{}://", url.scheme())),
                "{raw:?} shown as {shown:?}"
            );
            assert!(
                !shown.contains('?') && !shown.contains('#'),
                "{raw:?} shown with its query or fragment: {shown:?}"
            );
            if let Some(password) = url.password().filter(|p| p.len() > 8) {
                assert!(
                    !shown.contains(&format!(":{password}@")),
                    "{raw:?} shown with its password"
                );
            }
        }
        Err(_) => assert_eq!(shown, "(an address that does not parse)"),
    }
});
