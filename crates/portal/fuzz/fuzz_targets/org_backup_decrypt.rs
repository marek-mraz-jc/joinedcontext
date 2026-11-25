//! A configuration bundle decrypted under a cap (T-7784): arbitrary bytes are refused as
//! unreadable, never a panic; bytes sealed to the identity come back whole up to the cap and are
//! refused as too large past it, never read past `cap + 1`.
#![no_main]

use std::io::Write;
use std::sync::OnceLock;

use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::org_backup::restore::decrypt_capped;
use libfuzzer_sys::fuzz_target;

fn identity() -> &'static age::x25519::Identity {
    static IDENTITY: OnceLock<age::x25519::Identity> = OnceLock::new();
    IDENTITY.get_or_init(age::x25519::Identity::generate)
}

fn sealed(plain: &[u8]) -> Vec<u8> {
    let recipient = identity().to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .expect("one recipient");
    let mut out = Vec::new();
    let mut writer = encryptor.wrap_output(&mut out).expect("wrap");
    writer.write_all(plain).expect("write");
    writer.finish().expect("finish");
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&head, rest)) = data.split_first() else {
        return;
    };
    let cap = u64::from(head >> 1);
    if head & 1 == 0 {
        match decrypt_capped(&sealed(rest), identity(), cap) {
            Ok(plain) => assert_eq!(plain, rest),
            Err(ApiError::Unprocessable { detail, .. }) => {
                assert!(rest.len() as u64 > cap, "{detail}");
                assert!(detail.contains("too large") || detail.contains("more than"));
            }
            Err(other) => panic!("not a 422: {other}"),
        }
    } else {
        match decrypt_capped(rest, identity(), cap) {
            Ok(plain) => assert!(plain.len() as u64 <= cap),
            Err(ApiError::Unprocessable { detail, .. }) => {
                assert!(detail.starts_with("identity: ") || detail.starts_with("version: "))
            }
            Err(other) => panic!("not a 422: {other}"),
        }
    }
});
