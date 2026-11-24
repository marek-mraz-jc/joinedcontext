//! A device's JSON Web Key Set as a person pastes it: a public key set or a 422, never a panic,
//! and nothing private passes.
#![no_main]

use joinedcontext_portal::api::mqtt_devices::public_jwks;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    if let Ok(jwks) = public_jwks(&value) {
        let back = serde_json::to_value(&jwks).expect("a key set serialises");
        for key in back["keys"].as_array().into_iter().flatten() {
            for private in ["d", "p", "q", "dp", "dq", "qi", "k"] {
                assert!(
                    key.get(private).is_none(),
                    "a private member {private} passed"
                );
            }
        }
    }
});
