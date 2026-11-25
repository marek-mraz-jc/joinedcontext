//! A presented bearer token (`auth::bearer::BearerVerifier::verify` and its logout-token door):
//! never panics and never verifies, since nothing here holds the private half of the one key the
//! verifier trusts. Its key-set URL is port 9 (discard), so an unknown `kid` costs one refused
//! connection a minute and no realm.
#![no_main]
use std::sync::OnceLock;

use joinedcontext_portal::auth::bearer::BearerVerifier;
use libfuzzer_sys::fuzz_target;

fn verifier() -> &'static BearerVerifier {
    static VERIFIER: OnceLock<BearerVerifier> = OnceLock::new();
    VERIFIER.get_or_init(|| {
        let issuer = url::Url::parse("https://realm.test/realms/fuzz").expect("issuer");
        let verifier = BearerVerifier::with_jwks_url(
            &issuer,
            "portal",
            url::Url::parse("http://127.0.0.1:9/certs").expect("jwks"),
        );
        // A public P-256 key whose private half was thrown away.
        let set = serde_json::from_value(serde_json::json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "alg": "ES256", "kid": "k1",
            "x": "zewAG8XTcGGrQ_POTM9Y9xeREt_hjBaoop53ciE7m6A",
            "y": "lnn6ypjxmsUtf3fK3BnllcMPwO-V2xVuhSRKsnCxmTo"
        }]}))
        .expect("key set");
        assert_eq!(verifier.install(&set), 1);
        verifier
    })
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    })
}

fuzz_target!(|data: &[u8]| {
    // An `Authorization` header is capped far below this by the edge and hyper.
    if data.len() > 16 * 1024 {
        return;
    }
    let Ok(token) = std::str::from_utf8(data) else {
        return;
    };
    runtime().block_on(async {
        assert!(verifier().verify(token).await.is_err());
        assert!(verifier().verify_logout_token(token, "edge").await.is_err());
    });
});
