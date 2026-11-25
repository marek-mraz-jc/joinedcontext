//! A user as the realm's admin API answers one: unknown members are ignored and any shape that
//! parses can be asked whether it is a service account and which language it chose.
#![no_main]

use joinedcontext_portal::people::KcUser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(user) = serde_json::from_slice::<KcUser>(data) {
        let service = user.is_service_account();
        assert_eq!(
            service,
            user.service_account_client_link.is_some()
                || user.username.starts_with("service-account-")
        );
        if let Some(locale) = user.locale() {
            assert_eq!(
                user.attributes
                    .get("locale")
                    .and_then(|values| values.first())
                    .map(String::as_str),
                Some(locale)
            );
        }
    }
    if let Ok(users) = serde_json::from_slice::<Vec<KcUser>>(data) {
        for user in &users {
            let _ = (user.is_service_account(), user.locale());
        }
    }
});
