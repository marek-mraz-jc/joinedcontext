#![forbid(unsafe_code)]
//! Unit and vector tests for `kaisatsu_crypto::webauthn` (W3C WebAuthn Level 3 §6.1, §6.5.1,
//! §7.1, §7.2, K11): CBOR bounds, COSE_Key parsing (`ES256`, `EdDSA`, `RS256`), registration
//! ceremony checks, assertion ceremony checks, and counter regression refusal.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair, RSA_PKCS1_SHA256,
};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use kaisatsu_crypto::webauthn::*;
use serde_json::json;
use sha2::{Digest, Sha256};

fn cbor_uint(major: u8, n: u64) -> Vec<u8> {
    let tag = major << 5;
    if n < 24 {
        vec![tag | (n as u8)]
    } else if n <= u64::from(u8::MAX) {
        vec![tag | 24, n as u8]
    } else if n <= u64::from(u16::MAX) {
        let b = (n as u16).to_be_bytes();
        vec![tag | 25, b[0], b[1]]
    } else {
        let b = (n as u32).to_be_bytes();
        vec![tag | 26, b[0], b[1], b[2], b[3]]
    }
}

fn cbor_nint(neg: i64) -> Vec<u8> {
    let u = (-1i64 - neg) as u64;
    cbor_uint(1, u)
}

fn cbor_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = cbor_uint(2, b.len() as u64);
    out.extend_from_slice(b);
    out
}

fn cbor_text(s: &str) -> Vec<u8> {
    let mut out = cbor_uint(3, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
    out
}

fn cose_ec2_cbor(x: &[u8], y: &[u8], alg: i64) -> Vec<u8> {
    let mut out = vec![0xa5]; // map(5)
    out.extend_from_slice(&[0x01, 0x02]); // 1: 2 (EC2)
    out.push(0x03); // 3: alg
    out.extend_from_slice(&cbor_nint(alg));
    out.extend_from_slice(&[0x20, 0x01]); // -1: 1 (P-256)
    out.push(0x21); // -2: x
    out.extend_from_slice(&cbor_bytes(x));
    out.push(0x22); // -3: y
    out.extend_from_slice(&cbor_bytes(y));
    out
}

fn cose_okp_cbor(x: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa4]; // map(4)
    out.extend_from_slice(&[0x01, 0x01]); // 1: 1 (OKP)
    out.extend_from_slice(&[0x03, 0x27]); // 3: -8 (EdDSA)
    out.extend_from_slice(&[0x20, 0x06]); // -1: 6 (Ed25519)
    out.push(0x21); // -2: x
    out.extend_from_slice(&cbor_bytes(x));
    out
}

fn cose_rsa_cbor(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa4]; // map(4)
    out.extend_from_slice(&[0x01, 0x03]); // 1: 3 (RSA)
    out.push(0x03); // 3: -257 (RS256)
    out.extend_from_slice(&cbor_nint(-257));
    out.push(0x20); // -1: n
    out.extend_from_slice(&cbor_bytes(n));
    out.push(0x21); // -2: e
    out.extend_from_slice(&cbor_bytes(e));
    out
}

fn build_attestation_object(
    rp_id: &str,
    flags: u8,
    sign_count: u32,
    cred_id: &[u8],
    cose_key: &[u8],
) -> String {
    let mut auth_data = Vec::new();
    auth_data.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
    auth_data.push(flags);
    auth_data.extend_from_slice(&sign_count.to_be_bytes());
    if (flags & AUTH_DATA_FLAG_AT) != 0 {
        auth_data.extend_from_slice(&[0u8; 16]); // aaguid
        auth_data.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
        auth_data.extend_from_slice(cred_id);
        auth_data.extend_from_slice(cose_key);
    }

    let mut att_obj = vec![0xa3]; // map(3)
    att_obj.extend_from_slice(&cbor_text("fmt"));
    att_obj.extend_from_slice(&cbor_text("none"));
    att_obj.extend_from_slice(&cbor_text("attStmt"));
    att_obj.push(0xa0); // empty map
    att_obj.extend_from_slice(&cbor_text("authData"));
    att_obj.extend_from_slice(&cbor_bytes(&auth_data));
    URL_SAFE_NO_PAD.encode(att_obj)
}

#[test]
fn cbor_decoder_enforces_size_depth_and_definite_length_bounds() {
    // 1. Oversized input (> 64 KiB) is refused without panic.
    let huge = vec![0u8; MAX_CBOR_BYTES + 1];
    assert!(decode_cbor(&huge).is_err());

    // 2. Indefinite-length items (additional info 31) are refused.
    assert!(decode_cbor(&[0x5f, 0xff]).is_err());
    assert!(decode_cbor(&[0xbf, 0xff]).is_err());

    // 3. Nesting depth > 8 is refused.
    let mut deep = vec![0x81; 10];
    deep.push(0x01);
    assert!(decode_cbor(&deep).is_err());

    // 4. Truncated byte string with huge declared length is refused without allocation panic.
    assert!(decode_cbor(&[0x5a, 0xff, 0xff, 0xff, 0xff, 0x00]).is_err());
}

#[test]
fn registration_and_authentication_ceremonies_verify_and_reject_every_violation() {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
    let pub_bytes = pair.public_key().as_ref();
    let x = &pub_bytes[1..33];
    let y = &pub_bytes[33..65];

    let rp_id = "idm.dev.example.test";
    let origin = "https://idm.dev.example.test".to_owned();
    let challenge = URL_SAFE_NO_PAD.encode([0x42u8; 32]);
    let cred_id = b"cred-id-vector-001";
    let cred_id_b64 = URL_SAFE_NO_PAD.encode(cred_id);
    let cose_bytes = cose_ec2_cbor(x, y, COSE_ALG_ES256);

    let cdata_create = URL_SAFE_NO_PAD.encode(
        json!({
            "type": "webauthn.create",
            "challenge": challenge,
            "origin": origin
        })
        .to_string(),
    );
    let att_b64 = build_attestation_object(
        rp_id,
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_BE | AUTH_DATA_FLAG_AT,
        1,
        cred_id,
        &cose_bytes,
    );

    let reg_payload = RegistrationClientResponse {
        id: cred_id_b64.clone(),
        raw_id: Some(cred_id_b64.clone()),
        cred_type: "public-key".into(),
        response: AuthenticatorAttestationResponsePayload {
            client_data_json: cdata_create.clone(),
            attestation_object: att_b64.clone(),
            transports: vec!["internal".into()],
        },
        authenticator_attachment: Some("platform".into()),
        client_extension_results: None,
    };

    // Happy-path registration
    let verified_reg = verify_registration_response(
        &reg_payload,
        &challenge,
        rp_id,
        std::slice::from_ref(&origin),
        true,
        &[COSE_ALG_ES256, COSE_ALG_EDDSA, COSE_ALG_RS256],
    )
    .expect("valid registration");
    assert_eq!(verified_reg.credential_id_b64, cred_id_b64);
    assert_eq!(verified_reg.sign_count, 1);
    assert!(
        verified_reg.uv_initialized && verified_reg.backup_eligible && !verified_reg.backup_state
    );

    // Registration refusals: wrong challenge, wrong origin, wrong rpId, missing UP, missing UV, BS=1 with BE=0, unoffered alg.
    assert!(
        verify_registration_response(
            &reg_payload,
            "wrong-challenge",
            rp_id,
            std::slice::from_ref(&origin),
            true,
            &[COSE_ALG_ES256]
        )
        .is_err()
    );
    assert!(
        verify_registration_response(
            &reg_payload,
            &challenge,
            rp_id,
            &["https://evil.example".into()],
            true,
            &[COSE_ALG_ES256]
        )
        .is_err()
    );
    assert!(
        verify_registration_response(
            &reg_payload,
            &challenge,
            "other.example.test",
            std::slice::from_ref(&origin),
            true,
            &[COSE_ALG_ES256]
        )
        .is_err()
    );
    assert!(
        verify_registration_response(
            &reg_payload,
            &challenge,
            rp_id,
            std::slice::from_ref(&origin),
            true,
            &[COSE_ALG_EDDSA]
        )
        .is_err()
    );

    for bad_flags in [
        AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_AT, // missing UP
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_AT, // missing UV when required
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_BS | AUTH_DATA_FLAG_AT, // BS=1 with BE=0
    ] {
        let mut bad = reg_payload.clone();
        bad.response.attestation_object =
            build_attestation_object(rp_id, bad_flags, 1, cred_id, &cose_bytes);
        assert!(
            verify_registration_response(
                &bad,
                &challenge,
                rp_id,
                std::slice::from_ref(&origin),
                true,
                &[COSE_ALG_ES256]
            )
            .is_err()
        );
    }

    // Build authentication assertion (signCount = 2)
    let auth_challenge = URL_SAFE_NO_PAD.encode([0x99u8; 32]);
    let cdata_get_bytes = json!({
        "type": "webauthn.get",
        "challenge": auth_challenge,
        "origin": origin
    })
    .to_string()
    .into_bytes();
    let cdata_get_hash = Sha256::digest(&cdata_get_bytes);

    let make_assertion = |flags: u8, counter: u32| -> AuthenticationClientResponse {
        let mut ad = Vec::new();
        ad.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
        ad.push(flags);
        ad.extend_from_slice(&counter.to_be_bytes());
        let mut signed = ad.clone();
        signed.extend_from_slice(&cdata_get_hash);
        let sig = pair.sign(&rng, &signed).unwrap();
        AuthenticationClientResponse {
            id: cred_id_b64.clone(),
            raw_id: Some(cred_id_b64.clone()),
            cred_type: "public-key".into(),
            response: AuthenticatorAssertionResponsePayload {
                client_data_json: URL_SAFE_NO_PAD.encode(&cdata_get_bytes),
                authenticator_data: URL_SAFE_NO_PAD.encode(&ad),
                signature: URL_SAFE_NO_PAD.encode(sig.as_ref()),
                user_handle: Some("uh-alice".into()),
            },
            authenticator_attachment: None,
            client_extension_results: None,
        }
    };

    let good_assertion =
        make_assertion(AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_BE, 2);
    let verified_assertion = verify_authentication_response(
        &good_assertion,
        &auth_challenge,
        rp_id,
        std::slice::from_ref(&origin),
        true,
        &cred_id_b64,
        "uh-alice",
        &verified_reg.public_key,
        1,
        true,
    )
    .expect("valid assertion");
    assert_eq!(verified_assertion.new_sign_count, 2);
    assert!(verified_assertion.user_verified);

    // Counter regression (new_count <= stored_count when nonzero) is refused.
    assert!(
        verify_authentication_response(
            &good_assertion,
            &auth_challenge,
            rp_id,
            std::slice::from_ref(&origin),
            true,
            &cred_id_b64,
            "uh-alice",
            &verified_reg.public_key,
            2,
            true,
        )
        .is_err()
    );

    // Wrong userHandle or BE mismatch is refused.
    assert!(
        verify_authentication_response(
            &good_assertion,
            &auth_challenge,
            rp_id,
            std::slice::from_ref(&origin),
            true,
            &cred_id_b64,
            "uh-other",
            &verified_reg.public_key,
            1,
            true,
        )
        .is_err()
    );
    assert!(
        verify_authentication_response(
            &good_assertion,
            &auth_challenge,
            rp_id,
            std::slice::from_ref(&origin),
            true,
            &cred_id_b64,
            "uh-alice",
            &verified_reg.public_key,
            1,
            false,
        )
        .is_err()
    );
}

#[test]
fn ed25519_and_rsa256_cose_keys_verify_signatures() {
    let rng = SystemRandom::new();
    let msg = b"authenticator-data-plus-client-data-hash";

    // Ed25519 (OKP)
    let ed_pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let ed_pair = Ed25519KeyPair::from_pkcs8(ed_pkcs8.as_ref()).unwrap();
    let ed_cbor = decode_cbor(&cose_okp_cbor(ed_pair.public_key().as_ref())).unwrap();
    let ed_cose = CosePublicKey::from_cbor(&ed_cbor).unwrap();
    let ed_sig = ed_pair.sign(msg);
    ed_cose.verify_signature(msg, ed_sig.as_ref()).unwrap();
    assert!(
        ed_cose
            .verify_signature(b"tampered", ed_sig.as_ref())
            .is_err()
    );

    // RS256 (RSA 2048)
    let rsa_pair = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    let pub_rsa = rsa_pair.public_key();
    let n = pub_rsa.modulus().big_endian_without_leading_zero();
    let e = pub_rsa.exponent().big_endian_without_leading_zero();
    let rsa_cbor = decode_cbor(&cose_rsa_cbor(n, e)).unwrap();
    let rsa_cose = CosePublicKey::from_cbor(&rsa_cbor).unwrap();
    let mut rsa_sig = vec![0u8; rsa_pair.public_modulus_len()];
    rsa_pair
        .sign(&RSA_PKCS1_SHA256, &rng, msg, &mut rsa_sig)
        .unwrap();
    rsa_cose.verify_signature(msg, &rsa_sig).unwrap();
    assert!(rsa_cose.verify_signature(b"tampered", &rsa_sig).is_err());
}

/// The registration challenge verifies only for the realm and person it was issued to, untampered
/// and before it expires (K11, WebAuthn L3 §7.1 step 8).
#[test]
fn registration_challenge_is_bound_to_realm_person_and_time() {
    let kek = kaisatsu_crypto::Kek::generate_for_tests();
    let now = 1_700_000_000;
    let c = issue_registration_challenge(&kek, "dev", "u1", now);
    assert!(verify_registration_challenge(&kek, "dev", "u1", &c, now));
    assert!(verify_registration_challenge(
        &kek,
        "dev",
        "u1",
        &c,
        now + 299
    ));
    assert!(
        !verify_registration_challenge(&kek, "dev", "u1", &c, now + 300),
        "expired"
    );
    assert!(
        !verify_registration_challenge(&kek, "dev", "u2", &c, now),
        "other person"
    );
    assert!(
        !verify_registration_challenge(&kek, "prod", "u1", &c, now),
        "other realm"
    );
    let other_kek = kaisatsu_crypto::Kek::generate_for_tests();
    assert!(
        !verify_registration_challenge(&other_kek, "dev", "u1", &c, now),
        "other key"
    );
    assert_ne!(
        c,
        issue_registration_challenge(&kek, "dev", "u1", now),
        "random part"
    );
    let mut raw = URL_SAFE_NO_PAD.decode(&c).unwrap();
    raw[20] ^= 1; // push the expiry
    assert!(!verify_registration_challenge(
        &kek,
        "dev",
        "u1",
        &URL_SAFE_NO_PAD.encode(&raw),
        now
    ));
    for bad in [
        "",
        "not base64 !",
        "c2hvcnQ",
        &URL_SAFE_NO_PAD.encode([0u8; 57]),
    ] {
        assert!(
            !verify_registration_challenge(&kek, "dev", "u1", bad, now),
            "{bad}"
        );
    }
}
