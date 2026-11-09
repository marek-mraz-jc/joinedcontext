#![forbid(unsafe_code)]
//! Error and variant paths of `kaisatsu-crypto` (W7, W10, W12): KEK files, AES-GCM tampering,
//! JWKS publication and rotation, JWS refusal reasons, Keycloak credential formats, the password
//! policy clauses and TOTP algorithms.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::{Engine, general_purpose::STANDARD as B64, general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use hmac::Hmac;
use kaisatsu_core::{CredentialRecord, KaisatsuError, SigningKeyRecord};
use kaisatsu_crypto::*;
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::Sha512;

fn claims() -> Value {
    json!({ "iss": "https://idm.example.test/realms/dev", "sub": "u1", "exp": Utc::now().timestamp() + 300 })
}

#[test]
fn kek_files_accept_raw_hex_and_base64_and_refuse_everything_else() {
    let dir = std::env::temp_dir().join(format!("kaisatsu-kek-{}", random_handle_256()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = [7u8; 32];
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("raw", raw.to_vec()),
        ("raw-lf", [raw.as_slice(), b"\n"].concat()),
        ("raw-crlf", [raw.as_slice(), b"\r\n"].concat()),
        ("hex", "07".repeat(32).into_bytes()),
        ("hex-lf", format!("{}\n", "07".repeat(32)).into_bytes()),
        ("b64", B64.encode(raw).into_bytes()),
        ("b64url", URL_SAFE_NO_PAD.encode([0xfbu8; 32]).into_bytes()),
    ];
    let reference = Kek::from_raw_bytes(&raw).unwrap();
    let sealed = reference.encrypt(b"aad", b"secret").unwrap();
    for (name, bytes) in &cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let kek = Kek::from_file(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        if *name != "b64url" {
            assert_eq!(kek.decrypt(b"aad", &sealed).unwrap(), b"secret", "{name}");
        }
        assert_eq!(format!("{kek:?}"), "Kek([redacted])");
    }
    for bad in [
        b"".to_vec(),
        vec![1u8; 31],
        "zz".repeat(32).into_bytes(),
        B64.encode([1u8; 16]).into_bytes(),
        vec![0xff, 0xfe, 0x00],
    ] {
        assert!(matches!(
            Kek::from_raw_bytes(&bad),
            Err(KaisatsuError::Crypto(_))
        ));
    }
    let missing = Kek::from_file(&dir.join("missing"))
        .unwrap_err()
        .to_string();
    assert!(missing.contains("cannot read KEK file"), "{missing}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn aes_gcm_refuses_short_tampered_and_wrong_context_ciphertext() {
    let kek = Kek::generate_for_tests();
    let sealed = kek.encrypt(b"dev:k1", b"pkcs8").unwrap();
    assert_eq!(kek.decrypt(b"dev:k1", &sealed).unwrap(), b"pkcs8");
    assert!(kek.decrypt(b"dev:k1", &sealed[..20]).is_err());
    assert!(kek.decrypt(b"dev:k2", &sealed).is_err());
    let mut flipped = sealed.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 1;
    assert!(kek.decrypt(b"dev:k1", &flipped).is_err());
    assert!(
        Kek::generate_for_tests()
            .decrypt(b"dev:k1", &sealed)
            .is_err()
    );
}

#[test]
fn jwks_publishes_live_keys_only_and_rotation_stages_keys() {
    let kek = Kek::generate_for_tests();
    let active = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
    let mut retired = KeyManager::generate_eddsa("dev", &kek, 80, "RETIRED").unwrap();
    retired.kid = "retired".into();
    let mut expired = KeyManager::generate_es256("dev", &kek, 90, "PASSIVE").unwrap();
    expired.expires_at = Some(Utc::now() - Duration::seconds(1));
    let mut later = KeyManager::generate_es256("dev", &kek, 90, "PASSIVE").unwrap();
    later.expires_at = Some(Utc::now() + Duration::hours(1));
    let mut broken = active.clone();
    broken.kid = "broken".into();
    broken.public_jwk_json = "{".into();
    let jwks = KeyManager::build_jwks(&[active.clone(), retired, expired, later.clone(), broken]);
    let kids: Vec<_> = jwks.keys.iter().map(|k| k.kid.as_str()).collect();
    assert_eq!(kids, vec![active.kid.as_str(), later.kid.as_str()]);

    for alg in ["RS256", "EdDSA", "ES256"] {
        let mut existing = vec![
            KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap(),
            KeyManager::generate_eddsa("dev", &kek, 80, "ACTIVE").unwrap(),
        ];
        let staged = KeyManager::rotate("dev", &kek, &mut existing, alg, 60, false).unwrap();
        assert_eq!(staged.algorithm, alg);
        assert_eq!(staged.status, "PASSIVE");
        assert!(staged.activate_at.unwrap() >= Utc::now() + Duration::seconds(290));
        assert!(existing.iter().all(|k| k.status == "ACTIVE"));
        let now = KeyManager::rotate("dev", &kek, &mut existing, alg, 60, true).unwrap();
        assert_eq!(now.status, "ACTIVE");
        for k in &existing {
            if k.algorithm == alg {
                assert_eq!(k.status, "PASSIVE", "{alg}");
                assert!(k.expires_at.is_some());
            } else {
                assert_eq!(k.status, "ACTIVE", "{alg}");
            }
        }
        let jwt = sign_jwt(&now, &kek, None, &claims()).unwrap();
        let (h, _): (_, Value) = verify_jwt(&jwt, std::slice::from_ref(&now), None, None).unwrap();
        assert_eq!(h.typ.as_deref(), Some("JWT"));
    }
    for alg in ["HS256", "none", "es256"] {
        let err = KeyManager::rotate("dev", &kek, &mut [], alg, 60, true).unwrap_err();
        assert!(matches!(err, KaisatsuError::BadRequest(_)), "{alg}");
    }
}

fn header_b64(v: Value) -> String {
    URL_SAFE_NO_PAD.encode(v.to_string())
}

#[test]
fn signing_refuses_unknown_algorithms_foreign_keks_and_broken_key_material() {
    let kek = Kek::generate_for_tests();
    let es = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
    let mut hs = es.clone();
    hs.algorithm = "HS256".into();
    assert!(
        sign_jwt(&hs, &kek, None, &claims())
            .unwrap_err()
            .to_string()
            .contains("HS* is forbidden")
    );
    assert!(sign_jwt(&es, &Kek::generate_for_tests(), None, &claims()).is_err());
    // An RS256 record whose private key is an EC key is refused, as is garbage key material.
    let mut rs_with_ec = es.clone();
    rs_with_ec.algorithm = "RS256".into();
    assert!(
        sign_jwt(&rs_with_ec, &kek, None, &claims())
            .unwrap_err()
            .to_string()
            .contains("not PKCS#8")
    );
    let mut garbage = es.clone();
    garbage.encrypted_private_key = kek
        .encrypt(format!("dev:{}", es.kid).as_bytes(), b"not a key")
        .unwrap();
    assert!(
        sign_jwt(&garbage, &kek, None, &claims())
            .unwrap_err()
            .to_string()
            .contains("JWT sign failed")
    );
}

#[test]
fn verification_names_every_refusal() {
    let kek = Kek::generate_for_tests();
    let es = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
    let rs = KeyManager::generate_rs256("dev", &kek, 50, "ACTIVE").unwrap();
    let ed = KeyManager::generate_eddsa("dev", &kek, 80, "ACTIVE").unwrap();
    let good = sign_jwt(&es, &kek, Some("at+jwt"), &claims()).unwrap();
    let keys = vec![es.clone(), rs.clone(), ed.clone()];
    let refuse = |token: &str, keys: &[SigningKeyRecord]| -> String {
        verify_jwt::<Value>(
            token,
            keys,
            Some("https://idm.example.test/realms/dev"),
            None,
        )
        .unwrap_err()
        .to_string()
    };
    assert!(refuse("not.a.jwt", &keys).contains("malformed JWT header"));
    let body = good.split('.').skip(1).collect::<Vec<_>>().join(".");
    let no_kid = format!("{}.{body}", header_b64(json!({"alg":"ES256","typ":"JWT"})));
    assert!(refuse(&no_kid, &keys).contains("missing kid"));
    let other_kid = format!(
        "{}.{body}",
        header_b64(json!({"alg":"ES256","kid":"nobody"}))
    );
    assert!(refuse(&other_kid, &keys).contains("unknown or retired kid"));
    let mut retired = es.clone();
    retired.status = "RETIRED".into();
    assert!(refuse(&good, &[retired]).contains("unknown or retired kid"));
    let mut stored_hs = es.clone();
    stored_hs.algorithm = "HS256".into();
    assert!(refuse(&good, &[stored_hs]).contains("unsupported signing algorithm"));
    let confused = format!(
        "{}.{body}",
        header_b64(json!({"alg":"RS256","kid": es.kid}))
    );
    assert!(refuse(&confused, &keys).contains("algorithm mismatch"));
    let mut bad_jwk = es.clone();
    bad_jwk.public_jwk_json = "[]".into();
    assert!(refuse(&good, &[bad_jwk]).contains("invalid stored public JWK"));
    // Another key's public part under the same kid: the signature fails.
    let mut swapped = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
    swapped.kid = es.kid.clone();
    let mut jwk: Value = serde_json::from_str(&swapped.public_jwk_json).unwrap();
    jwk["kid"] = json!(es.kid);
    swapped.public_jwk_json = jwk.to_string();
    assert!(refuse(&good, &[swapped]).contains("validation failed"));
    // Wrong issuer, expiry beyond the leeway, wrong audience.
    let foreign = sign_jwt(
        &ed,
        &kek,
        None,
        &json!({"iss":"https://evil.example","exp": Utc::now().timestamp() + 60}),
    )
    .unwrap();
    assert!(refuse(&foreign, &keys).contains("validation failed"));
    let expired = sign_jwt(
        &rs,
        &kek,
        None,
        &json!({"iss":"https://idm.example.test/realms/dev","exp": Utc::now().timestamp() - 120}),
    )
    .unwrap();
    assert!(refuse(&expired, &keys).contains("validation failed"));
    // An expected audience or issuer that the token leaves out is a refusal, not a pass.
    assert!(verify_jwt::<Value>(&good, &keys, None, Some("portal-api")).is_err());
    let no_iss = sign_jwt(
        &es,
        &kek,
        None,
        &json!({"sub":"u1","aud":"portal-api","exp": Utc::now().timestamp() + 60}),
    )
    .unwrap();
    assert!(refuse(&no_iss, &keys).contains("validation failed"));
    let with_aud = sign_jwt(&es, &kek, None, &json!({"iss":"https://idm.example.test/realms/dev","aud":["portal-api","x"],"exp": Utc::now().timestamp() + 60})).unwrap();
    verify_jwt::<Value>(
        &with_aud,
        &keys,
        Some("https://idm.example.test/realms/dev"),
        Some("portal-api"),
    )
    .unwrap();
    let jwk: Value = serde_json::from_str(&es.public_jwk_json).unwrap();
    assert!(
        verify_jwt_with_jwk::<Value>(&no_iss, &jwk, Some("client-1"), Some("portal-api")).is_err()
    );
    assert!(verify_jwt_with_jwk::<Value>(&good, &jwk, None, Some("portal-api")).is_err());
    let (h, c): (_, Value) = verify_jwt(&good, &keys, None, None).unwrap();
    assert_eq!(h.typ.as_deref(), Some("at+jwt"));
    assert_eq!(c["sub"], "u1");
}

#[test]
fn verification_against_a_presented_jwk() {
    let kek = Kek::generate_for_tests();
    let es = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&es.public_jwk_json).unwrap();
    let token = sign_jwt(&es, &kek, Some("dpop+jwt"), &claims()).unwrap();
    let (_, c): (_, Value) = verify_jwt_with_jwk(&token, &jwk, None, None).unwrap();
    assert_eq!(c["sub"], "u1");
    let err = |t: &str, j: &Value| -> String {
        verify_jwt_with_jwk::<Value>(t, j, None, None)
            .unwrap_err()
            .to_string()
    };
    assert!(err("x", &jwk).contains("malformed JWT header"));
    let body = token.split('.').skip(1).collect::<Vec<_>>().join(".");
    let hs = format!("{}.{body}", header_b64(json!({"alg":"HS256"})));
    assert!(err(&hs, &jwk).contains("unsupported JWT signature algorithm"));
    let e = err(&token, &json!(["not", "a", "jwk"]));
    assert!(e.contains("invalid JWK"), "{e}");
    let mut bad_x = jwk.clone();
    bad_x["x"] = json!("!!!not-base64!!!");
    assert!(err(&token, &bad_x).contains("cannot build decoding key"));
}

fn argon_secret(
    password: &str,
    salt: &[u8],
    alg: Algorithm,
    ver: Version,
    m: u32,
    t: u32,
) -> String {
    let argon = Argon2::new(alg, ver, Params::new(m, t, 1, Some(32)).unwrap());
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .unwrap();
    json!({ "value": B64.encode(out), "salt": B64.encode(salt) }).to_string()
}

#[test]
fn keycloak_argon2_variants_and_parameter_shapes() {
    let pw = "CorrectHorse99!Battery";
    let salt = b"0123456789abcdef";
    // argon2i, version 1.0 given as strings; iterations from additionalParameters as a number.
    let s = argon_secret(pw, salt, Algorithm::Argon2i, Version::V0x10, 4096, 3);
    let c = json!({"algorithm":"argon2","hashIterations":0,"additionalParameters":{"memory":4096,"iterations":3,"type":"i","version":"1.0"}}).to_string();
    assert!(verify_keycloak_credential(pw, &s, &c).unwrap());
    assert!(!verify_keycloak_credential("nope", &s, &c).unwrap());
    // argon2d, version 16 and parameters as arrays of numbers / strings.
    let s = argon_secret(pw, salt, Algorithm::Argon2d, Version::V0x10, 2048, 2);
    let c = json!({"algorithm":"ARGON2","additionalParameters":{"memory":[2048],"iterations":["2"],"parallelism":["x"],"type":["d"],"version":["16"]}}).to_string();
    assert!(verify_keycloak_credential(pw, &s, &c).unwrap());
    // Empty credentialData means Keycloak's argon2id defaults.
    let s = argon_secret(pw, salt, Algorithm::Argon2id, Version::V0x13, 7168, 5);
    assert!(verify_keycloak_credential(pw, &s, " ").unwrap());
    let c = json!({"additionalParameters":{"memory":{"n":1}}}).to_string();
    assert!(
        verify_keycloak_credential(pw, &s, &c).unwrap(),
        "an odd shape falls back to the default"
    );
    // Impossible parameters and a too-short salt are errors, not a silent pass.
    let c = json!({"algorithm":"argon2","additionalParameters":{"memory":["1"]}}).to_string();
    assert!(verify_keycloak_credential(pw, &s, &c).is_err());
    let short = json!({"value": B64.encode([0u8; 32]), "salt": B64.encode(b"abc")}).to_string();
    assert!(verify_keycloak_credential(pw, &short, "").is_err());
}

#[test]
fn keycloak_pbkdf2_variants_and_malformed_credentials() {
    let pw = "CorrectHorse99!Battery";
    let salt = b"0123456789abcdef";
    let mut out512 = [0u8; 64];
    let _ = pbkdf2::pbkdf2::<Hmac<Sha512>>(pw.as_bytes(), salt, 1000, &mut out512);
    let s512 = json!({"value": B64.encode(out512), "salt": B64.encode(salt)}).to_string();
    let c512 = json!({"algorithm":"pbkdf2-sha512","hashIterations":1000}).to_string();
    assert!(verify_keycloak_credential(pw, &s512, &c512).unwrap());
    assert!(!verify_keycloak_credential("x", &s512, &c512).unwrap());
    let mut out1 = [0u8; 20];
    let _ = pbkdf2::pbkdf2::<Hmac<Sha1>>(pw.as_bytes(), salt, 1000, &mut out1);
    let s1 = json!({"value": B64.encode(out1), "salt": B64.encode(salt)}).to_string();
    for alg in ["pbkdf2", "pbkdf2-sha1"] {
        let c = json!({"algorithm": alg, "hashIterations": 1000}).to_string();
        assert!(verify_keycloak_credential(pw, &s1, &c).unwrap(), "{alg}");
    }
    // A legacy hash that verifies is rehashed to argon2id; a wrong one is not.
    let rec = CredentialRecord {
        secret_data: s512.clone(),
        credential_data: c512.clone(),
        ..Default::default()
    };
    assert!(
        verify_and_maybe_rehash(pw, &rec)
            .unwrap()
            .rehashed_phc
            .is_some()
    );
    let wrong = verify_and_maybe_rehash("x", &rec).unwrap();
    assert!(!wrong.valid && wrong.rehashed_phc.is_none());

    let msg = |s: &str, c: &str| {
        verify_keycloak_credential(pw, s, c)
            .unwrap_err()
            .to_string()
    };
    assert!(msg("{", &c512).contains("secretData JSON"));
    assert!(msg(&s512, "{").contains("credentialData JSON"));
    assert!(msg(r#"{"value":"***"}"#, &c512).contains("base64 hash"));
    assert!(msg(r#"{"value":"AAAA","salt":"***"}"#, &c512).contains("base64 salt"));
    assert!(msg(&s512, r#"{"algorithm":"md5"}"#).contains("unsupported"));

    // PHC strings: a broken one is an error, a wrong password is invalid without a rehash.
    let broken = CredentialRecord {
        secret_data: "$argon2id$v=19$broken".into(),
        ..Default::default()
    };
    assert!(verify_and_maybe_rehash(pw, &broken).is_err());
    let phc = CredentialRecord {
        secret_data: hash_password(pw).unwrap(),
        ..Default::default()
    };
    assert!(verify_and_maybe_rehash(pw, &phc).unwrap().valid);
    let out = verify_and_maybe_rehash("x", &phc).unwrap();
    assert!(!out.valid && out.rehashed_phc.is_none());
    password::dummy_verify_password("anything");
}

#[test]
fn every_policy_clause_refuses_its_violation() {
    let ok = "Abcdefgh12!x";
    let cases = [
        ("length(13)", ok, "at least 13 characters"),
        ("length(x)", "Abc1!", "at least 12 characters"),
        ("upperCase(2)", ok, "uppercase"),
        ("upperCase(x)", "abc", "uppercase"),
        ("lowerCase(11)", ok, "lowercase"),
        ("lowerCase(x)", "ABC", "lowercase"),
        ("digits(3)", ok, "digit"),
        ("digits(x)", "abc", "digit"),
        ("specialChars(2)", ok, "special"),
        ("specialChars(x)", "abc", "special"),
        ("notUsername(undefined)", "xxAlicexx1!", "username"),
        ("notEmail(undefined)", "a@B.cz-1!X", "e-mail"),
    ];
    for (policy, pw, want) in cases {
        let err = check_password_policy(policy, pw, "alice", Some("a@b.cz"), &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains(want), "{policy}: {err}");
    }
    let policy = "length(12) and upperCase(1) and lowerCase(1) and digits(2) and specialChars(1) and notUsername and notEmail and passwordHistory(x) and unknownClause(3) and ";
    check_password_policy(policy, ok, "alice", Some("a@b.cz"), &[]).unwrap();
    check_password_policy("notUsername and notEmail", ok, "", Some(""), &[]).unwrap();
    check_password_policy("notEmail", ok, "u", None, &[]).unwrap();
    // History: a record that cannot be parsed never blocks, a matching one does.
    let history = vec![
        CredentialRecord {
            secret_data: "{".into(),
            ..Default::default()
        },
        CredentialRecord {
            secret_data: hash_password(ok).unwrap(),
            ..Default::default()
        },
    ];
    assert!(check_password_policy("passwordHistory(1)", ok, "u", None, &history).is_ok());
    assert!(check_password_policy("passwordHistory(2)", ok, "u", None, &history).is_err());
}

#[test]
fn totp_algorithms_and_malformed_credentials() {
    let now = 1_700_000_000u64;
    for alg in ["HmacSHA1", "SHA1", "HmacSHA512", "SHA512", "HmacSHA256"] {
        let cred = CredentialRecord {
            secret_data: "raw-seed-1234567890".into(),
            credential_data: json!({"digits":8,"period":60,"algorithm":alg}).to_string(),
            ..Default::default()
        };
        let code = generate_totp_code("raw-seed-1234567890", 8, 60, alg, now).unwrap();
        assert_eq!(code.len(), 8);
        assert_eq!(generate_totp_code_for_credential(&cred, now).unwrap(), code);
        assert_eq!(
            verify_totp_code(&cred, &code, 0, now).unwrap(),
            Some(now / 60)
        );
    }
    assert_ne!(
        generate_totp_code("seed", 6, 30, "HmacSHA1", now).unwrap(),
        generate_totp_code("seed", 6, 30, "HmacSHA512", now).unwrap()
    );
    let plain = CredentialRecord {
        secret_data: "seed".into(),
        ..Default::default()
    };
    let code = generate_totp_code("seed", 6, 30, "HmacSHA256", now).unwrap();
    assert_eq!(
        verify_totp_code(&plain, &code, 0, now).unwrap(),
        Some(now / 30)
    );
    for not_a_code in ["", "   ", "12a456", "１２３４５６"] {
        assert_eq!(verify_totp_code(&plain, not_a_code, 1, now).unwrap(), None);
    }
    let bad_secret = CredentialRecord {
        secret_data: "{\"nope\":1}".into(),
        ..Default::default()
    };
    assert!(verify_totp_code(&bad_secret, "123456", 1, now).is_err());
    let bad_meta = CredentialRecord {
        secret_data: "seed".into(),
        credential_data: "{".into(),
        ..Default::default()
    };
    assert!(generate_totp_code_for_credential(&bad_meta, now).is_err());
    assert!(verify_recovery_code("  abcde-fghjk ", &[]).is_none());
}

#[test]
fn handles_and_comparisons() {
    let a = random_handle_256();
    assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 32);
    assert_ne!(a, random_handle_256());
    assert!(constant_time_str_eq("abc", "abc"));
    assert!(!constant_time_str_eq("abc", "abd"));
    assert!(!constant_time_str_eq("abc", "abcd"));
    assert_eq!(sha256_hex("").len(), 64);
}

/// D1: device user codes are `XXXX-XXXX` over the 20-letter alphabet, and the page compares them
/// case- and hyphen-blind.
#[test]
fn user_codes_are_eight_letters_of_the_alphabet() {
    use kaisatsu_crypto::{USER_CODE_ALPHABET, normalize_user_code, random_user_code};
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..200 {
        let code = random_user_code();
        assert_eq!(code.len(), 9, "{code}");
        assert_eq!(&code[4..5], "-", "{code}");
        let plain = normalize_user_code(&code);
        assert_eq!(plain.len(), 8, "{code}");
        assert!(
            plain.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)),
            "{code}"
        );
        seen.insert(plain);
    }
    assert!(seen.len() > 190, "codes repeat: {}", seen.len());
    assert_eq!(normalize_user_code(" bcdf-ghjk "), "BCDFGHJK");
    assert_eq!(
        normalize_user_code("A1E0-OIU<>"),
        "",
        "vowels, digits and markup are no letters of a code"
    );
}
