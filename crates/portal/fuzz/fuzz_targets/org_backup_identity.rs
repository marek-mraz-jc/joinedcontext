//! The identity an administrator pastes to open a backup (T-7784): any text is an identity or a
//! 422 that says what to paste, and a public key is told apart from garbage.
#![no_main]

use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::org_backup::restore::identity_of;
use libfuzzer_sys::fuzz_target;
use zeroize::Zeroizing;

fuzz_target!(|data: &[u8]| {
    let text = Zeroizing::new(String::from_utf8_lossy(data).into_owned());
    match identity_of(&text) {
        Ok(identity) => {
            use age::secrecy::ExposeSecret;
            assert!(identity
                .to_string()
                .expose_secret()
                .eq_ignore_ascii_case(text.trim()));
        }
        Err(ApiError::Unprocessable { detail, errors }) => {
            assert!(detail.starts_with("identity: "), "{detail}");
            assert_eq!(errors, vec![detail.clone()]);
            assert_eq!(
                detail.contains("public key"),
                text.trim().starts_with("age1")
            );
        }
        Err(other) => panic!("not a 422: {other}"),
    }
});
