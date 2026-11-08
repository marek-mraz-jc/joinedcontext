#![forbid(unsafe_code)]
//! Every refusal of the WebAuthn verifier (W3C WebAuthn L3 §6.1, §6.5.1, §7.1, §7.2, K11): CBOR
//! edge cases, COSE_Key parameters, authenticator data layout, clientDataJSON checks, and each
//! registration and assertion step. Malformed input is refused with an error, never a panic.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use kaisatsu_crypto::webauthn::*;
use serde_json::json;
use sha2::{Digest, Sha256};

const RP: &str = "idm.example.test";
const ORIGIN: &str = "https://idm.example.test";

fn head(major: u8, n: u64) -> Vec<u8> {
    let tag = major << 5;
    if n < 24 {
        vec![tag | n as u8]
    } else if n <= 0xff {
        vec![tag | 24, n as u8]
    } else if n <= 0xffff {
        let b = (n as u16).to_be_bytes();
        vec![tag | 25, b[0], b[1]]
    } else {
        let b = (n as u32).to_be_bytes();
        vec![tag | 26, b[0], b[1], b[2], b[3]]
    }
}

fn bytes(b: &[u8]) -> Vec<u8> {
    let mut out = head(2, b.len() as u64);
    out.extend_from_slice(b);
    out
}

fn text(s: &str) -> Vec<u8> {
    let mut out = head(3, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
    out
}

fn int(n: i64) -> Vec<u8> {
    if n >= 0 {
        head(0, n as u64)
    } else {
        head(1, (-1 - n) as u64)
    }
}

/// A CBOR map from (int key, encoded value) pairs.
fn map(entries: &[(i64, Vec<u8>)]) -> Vec<u8> {
    let mut out = head(5, entries.len() as u64);
    for (k, v) in entries {
        out.extend_from_slice(&int(*k));
        out.extend_from_slice(v);
    }
    out
}

fn bad(input: &[u8]) -> String {
    decode_cbor(input).expect_err("refused").to_string()
}

#[test]
fn cbor_refuses_every_malformed_head_and_length() {
    // Truncated integer heads (ai 24..27) and reserved additional information (28..30).
    for input in [
        &[0x18][..],
        &[0x19, 0x01],
        &[0x1a, 0, 0],
        &[0x1b, 0, 0, 0],
        &[0x1c],
    ] {
        decode_cbor(input).expect_err("truncated or reserved head");
    }
    assert!(bad(&[0x1c]).contains("additional information"));
    // Full-width heads decode.
    assert_eq!(
        decode_cbor(&[0x1b, 0, 0, 0, 0, 0, 0, 1, 0]).unwrap(),
        CborValue::Int(256)
    );
    assert_eq!(
        decode_cbor(&[0x1a, 0, 1, 0, 0]).unwrap(),
        CborValue::Int(65536)
    );
    // Integers outside i64.
    assert!(bad(&[0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).contains("i64"));
    assert!(bad(&[0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).contains("i64"));
    assert_eq!(
        decode_cbor(&[0x3b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
        CborValue::Int(i64::MIN)
    );
    // Byte and text strings declaring more than 64 KiB, and invalid UTF-8.
    assert!(bad(&[0x5a, 0x00, 0x01, 0x00, 0x01]).contains("64 KiB"));
    assert!(bad(&[0x7a, 0x00, 0x01, 0x00, 0x01]).contains("64 KiB"));
    assert!(bad(&[0x62, 0xc3, 0x28]).contains("UTF-8"));
    // Arrays and maps declaring more than the item cap, and item-count exhaustion.
    assert!(bad(&[0x99, 0x04, 0x01]).contains("array length"));
    assert!(bad(&[0xb9, 0x04, 0x01]).contains("map length"));
    let mut many = vec![0x99, 0x03, 0xe8]; // array(1000) of array(1) of 0: 2001 items
    for _ in 0..1000 {
        many.extend_from_slice(&[0x81, 0x00]);
    }
    assert!(bad(&many).contains("item count"));
    // A map whose value is missing, and a truncated nested key.
    decode_cbor(&[0xa1, 0x01]).expect_err("value missing");
    decode_cbor(&[0xa1, 0x61]).expect_err("key truncated");
    // Tags (major 6) and floats / unknown simple values are refused; the four simple values decode.
    assert!(bad(&[0xc0, 0x00]).contains("major type"));
    assert!(bad(&[0xf9, 0x00, 0x00]).contains("simple"));
    assert_eq!(decode_cbor(&[0xf4]).unwrap(), CborValue::Bool(false));
    assert_eq!(decode_cbor(&[0xf5]).unwrap(), CborValue::Bool(true));
    assert_eq!(decode_cbor(&[0xf6]).unwrap(), CborValue::Null);
    assert_eq!(decode_cbor(&[0xf7]).unwrap(), CborValue::Null);
    // Trailing bytes after one complete item, and the empty input.
    assert!(bad(&[0x00, 0x00]).contains("trailing"));
    assert!(bad(&[]).contains("EOF"));
    // Accessors on the wrong variant answer None.
    let v = decode_cbor(&text("x")).unwrap();
    assert_eq!(v.as_str(), Some("x"));
    assert!(v.as_i64().is_none() && v.as_bytes().is_none());
    assert!(v.map_get_int(1).is_none() && v.map_get_str("x").is_none());
}

type Entries = Vec<(i64, Vec<u8>)>;

fn cose(entries: &[(i64, Vec<u8>)]) -> Result<CosePublicKey, String> {
    CosePublicKey::from_cbor(&decode_cbor(&map(entries)).unwrap()).map_err(|e| e.to_string())
}

#[test]
fn cose_key_refuses_missing_and_out_of_range_parameters() {
    let b32 = bytes(&[7u8; 32]);
    let b31 = bytes(&[7u8; 31]);
    let cases: Vec<(Entries, &str)> = vec![
        (vec![(3, int(-7))], "kty"),
        (vec![(1, int(2))], "alg"),
        (vec![(1, int(2)), (3, int(-7))], "crv"),
        (vec![(1, int(2)), (3, int(-7)), (-1, int(2))], "P-256"),
        (
            vec![(1, int(2)), (3, int(-7)), (-1, int(1)), (-2, b31.clone())],
            "x (-2)",
        ),
        (
            vec![
                (1, int(2)),
                (3, int(-7)),
                (-1, int(1)),
                (-2, b32.clone()),
                (-3, b31.clone()),
            ],
            "y (-3)",
        ),
        (vec![(1, int(1)), (3, int(-8))], "crv"),
        (vec![(1, int(1)), (3, int(-8)), (-1, int(4))], "Ed25519"),
        (
            vec![(1, int(1)), (3, int(-8)), (-1, int(6)), (-2, b31)],
            "x (-2)",
        ),
        (
            vec![(1, int(3)), (3, int(-257)), (-1, bytes(&[1u8; 128]))],
            "modulus",
        ),
        (
            vec![
                (1, int(3)),
                (3, int(-257)),
                (-1, bytes(&[1u8; 256])),
                (-2, bytes(&[])),
            ],
            "exponent",
        ),
        (
            vec![
                (1, int(3)),
                (3, int(-257)),
                (-1, bytes(&[1u8; 256])),
                (-2, bytes(&[1u8; 9])),
            ],
            "exponent",
        ),
        // A known kty with another algorithm (EC2 with EdDSA) is unsupported.
        (vec![(1, int(2)), (3, int(-8))], "unsupported"),
    ];
    for (entries, want) in cases {
        let err = cose(&entries).expect_err(want);
        assert!(err.contains(want), "{want}: {err}");
    }
}

#[test]
fn stored_public_keys_with_bad_material_refuse_to_verify() {
    let good = URL_SAFE_NO_PAD.encode([1u8; 32]);
    let short = URL_SAFE_NO_PAD.encode([1u8; 31]);
    let keys = [
        CosePublicKey::Ec2 {
            alg: -7,
            crv: 1,
            x: "!".into(),
            y: good.clone(),
        },
        CosePublicKey::Ec2 {
            alg: -7,
            crv: 1,
            x: good.clone(),
            y: "!".into(),
        },
        CosePublicKey::Ec2 {
            alg: -7,
            crv: 1,
            x: short.clone(),
            y: good.clone(),
        },
        CosePublicKey::Okp {
            alg: -8,
            crv: 6,
            x: "!".into(),
        },
        CosePublicKey::Rsa {
            alg: -257,
            n: "!".into(),
            e: "AQAB".into(),
        },
        CosePublicKey::Rsa {
            alg: -257,
            n: good.clone(),
            e: "!".into(),
        },
    ];
    for k in &keys {
        assert!(
            matches!(
                k.verify_signature(b"msg", b"sig"),
                Err(kaisatsu_core::KaisatsuError::Crypto(_))
            ),
            "{k:?}"
        );
    }
    // Well-formed but wrong keys answer Unauthorized, never Ok.
    let wrong = [
        CosePublicKey::Ec2 {
            alg: -7,
            crv: 1,
            x: good.clone(),
            y: good.clone(),
        },
        CosePublicKey::Okp {
            alg: -8,
            crv: 6,
            x: good.clone(),
        },
        CosePublicKey::Rsa {
            alg: -257,
            n: URL_SAFE_NO_PAD.encode([0xc5u8; 256]),
            e: "AQAB".into(),
        },
    ];
    for k in &wrong {
        assert!(
            matches!(
                k.verify_signature(b"msg", &[0u8; 64]),
                Err(kaisatsu_core::KaisatsuError::Unauthorized(_))
            ),
            "{k:?}"
        );
    }
}

fn auth_data(flags: u8, count: u32, tail: &[u8]) -> Vec<u8> {
    let mut out = Sha256::digest(RP.as_bytes()).to_vec();
    out.push(flags);
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(tail);
    out
}

fn attested(cred_id: &[u8], cose_key: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 16];
    out.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
    out.extend_from_slice(cred_id);
    out.extend_from_slice(cose_key);
    out
}

fn ec2_key() -> Vec<u8> {
    map(&[
        (1, int(2)),
        (3, int(-7)),
        (-1, int(1)),
        (-2, bytes(&[9u8; 32])),
        (-3, bytes(&[9u8; 32])),
    ])
}

#[test]
fn authenticator_data_refuses_every_layout_violation() {
    const AT: u8 = AUTH_DATA_FLAG_AT | AUTH_DATA_FLAG_UP;
    let err = |b: Vec<u8>| ParsedAuthData::parse(&b).expect_err("refused").to_string();
    assert!(err(vec![0u8; 36]).contains("37 bytes"));
    assert!(err(auth_data(AUTH_DATA_FLAG_BS, 0, &[])).contains("BS=1"));
    assert!(err(auth_data(AT, 0, &[0u8; 17])).contains("header"));
    let mut zero_len = vec![0u8; 16];
    zero_len.extend_from_slice(&[0, 0]);
    assert!(err(auth_data(AT, 0, &zero_len)).contains("1..=1023"));
    let mut long_len = vec![0u8; 16];
    long_len.extend_from_slice(&1024u16.to_be_bytes());
    assert!(err(auth_data(AT, 0, &long_len)).contains("1..=1023"));
    let mut cut = vec![0u8; 16];
    cut.extend_from_slice(&8u16.to_be_bytes());
    cut.extend_from_slice(b"abc");
    assert!(err(auth_data(AT, 0, &cut)).contains("truncated credentialId"));
    // Credential id with no public key after it, and a key that is not a COSE_Key.
    assert!(err(auth_data(AT, 0, &attested(b"id", &[]))).contains("EOF"));
    assert!(err(auth_data(AT, 0, &attested(b"id", &map(&[(3, int(-7))])))).contains("kty"));
    // Extensions: missing, not a map, and a valid map.
    let ed = AUTH_DATA_FLAG_ED | AUTH_DATA_FLAG_UP;
    assert!(err(auth_data(ed, 0, &[])).contains("EOF"));
    assert!(err(auth_data(ed, 0, &int(1))).contains("CBOR map"));
    let ok = ParsedAuthData::parse(&auth_data(ed, 7, &map(&[(1, int(1))]))).unwrap();
    assert_eq!(ok.sign_count, 7);
    assert!(ok.attested.is_none() && ok.user_present() && !ok.backup_state());
    // Trailing bytes after the attested key.
    let mut tail = attested(b"id", &ec2_key());
    tail.push(0);
    assert!(err(auth_data(AT, 0, &tail)).contains("trailing"));
    let ok = ParsedAuthData::parse(&auth_data(AT, 0, &attested(b"id", &ec2_key()))).unwrap();
    assert_eq!(ok.attested.unwrap().credential_id, b"id");
}

fn client_data(value: serde_json::Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

fn att_object(entries: &[(&str, Vec<u8>)]) -> String {
    let mut out = head(5, entries.len() as u64);
    for (k, v) in entries {
        out.extend_from_slice(&text(k));
        out.extend_from_slice(v);
    }
    URL_SAFE_NO_PAD.encode(out)
}

fn registration(
    cdata: String,
    att: String,
    id: &str,
    raw_id: Option<&str>,
) -> RegistrationClientResponse {
    RegistrationClientResponse {
        id: id.into(),
        raw_id: raw_id.map(str::to_owned),
        cred_type: "public-key".into(),
        response: AuthenticatorAttestationResponsePayload {
            client_data_json: cdata,
            attestation_object: att,
            transports: Vec::new(),
        },
        authenticator_attachment: None,
        client_extension_results: None,
    }
}

fn verify_reg(
    p: &RegistrationClientResponse,
    challenge: &str,
    uv: bool,
) -> Result<VerifiedRegistration, String> {
    verify_registration_response(
        p,
        challenge,
        RP,
        &[ORIGIN.to_owned()],
        uv,
        &[COSE_ALG_ES256],
    )
    .map_err(|e| e.to_string())
}

#[test]
fn registration_refuses_each_step() {
    let ch = URL_SAFE_NO_PAD.encode([5u8; 32]);
    let cd = client_data(json!({ "type": "webauthn.create", "challenge": ch, "origin": ORIGIN }));
    let id = URL_SAFE_NO_PAD.encode(b"id");
    let good_ad = auth_data(
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_AT,
        0,
        &attested(b"id", &ec2_key()),
    );
    let att = |ad: &[u8]| {
        att_object(&[
            ("fmt", text("none")),
            ("attStmt", map(&[])),
            ("authData", bytes(ad)),
        ])
    };
    let ok = registration(cd.clone(), att(&good_ad), &id, Some(&id));
    assert!(verify_reg(&ok, &ch, false).is_ok());

    let mut wrong_type = ok.clone();
    wrong_type.cred_type = "password".into();
    assert!(
        verify_reg(&wrong_type, &ch, false)
            .unwrap_err()
            .contains("public-key")
    );

    // clientDataJSON checks: base64, UTF-8, JSON, type, empty expected challenge, origin, crossOrigin.
    let cases = [
        ("!!".to_owned(), "base64url"),
        (URL_SAFE_NO_PAD.encode([0xff, 0xfe]), "UTF-8"),
        (URL_SAFE_NO_PAD.encode("{"), "structure"),
        (
            client_data(json!({ "type": "webauthn.get", "challenge": ch, "origin": ORIGIN })),
            "type",
        ),
        (
            client_data(
                json!({ "type": "webauthn.create", "challenge": ch, "origin": "https://evil.test" }),
            ),
            "origin",
        ),
        (
            client_data(
                json!({ "type": "webauthn.create", "challenge": ch, "origin": ORIGIN, "crossOrigin": true }),
            ),
            "crossOrigin",
        ),
        (
            client_data(
                json!({ "type": "webauthn.create", "challenge": ch, "origin": ORIGIN, "topOrigin": ORIGIN }),
            ),
            "crossOrigin",
        ),
    ];
    for (cdata, want) in cases {
        let p = registration(cdata, att(&good_ad), &id, None);
        let e = verify_reg(&p, &ch, false).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    assert!(
        verify_reg(&ok, "", false)
            .unwrap_err()
            .contains("challenge")
    );
    // A UTF-8 BOM before the JSON is stripped (§7.1 step 5).
    let bom = URL_SAFE_NO_PAD.encode(format!(
        "\u{feff}{}",
        json!({ "type": "webauthn.create", "challenge": ch, "origin": ORIGIN })
    ));
    assert!(verify_reg(&registration(bom, att(&good_ad), &id, None), &ch, false).is_ok());

    // attestationObject checks.
    let att_cases = [
        ("!!".to_owned(), "attestationObject base64url"),
        (
            att_object(&[("attStmt", map(&[])), ("authData", bytes(&good_ad))]),
            "fmt",
        ),
        (
            att_object(&[("fmt", text("none")), ("attStmt", map(&[]))]),
            "authData",
        ),
        (
            att_object(&[("fmt", text("none")), ("authData", bytes(&good_ad))]),
            "attStmt",
        ),
    ];
    for (a, want) in att_cases {
        let e = verify_reg(&registration(cd.clone(), a, &id, None), &ch, false).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }

    // authData checks: rpIdHash, UP, UV, no attested data, unoffered alg, id and rawId.
    let mut other_rp = good_ad.clone();
    other_rp[0] ^= 1;
    let ad_cases: Vec<(Vec<u8>, bool, &str)> = vec![
        (other_rp, false, "rpIdHash"),
        (
            auth_data(AUTH_DATA_FLAG_AT, 0, &attested(b"id", &ec2_key())),
            false,
            "(UP)",
        ),
        (good_ad.clone(), true, "(UV)"),
        (
            auth_data(AUTH_DATA_FLAG_UP, 0, &[]),
            false,
            "attestedCredentialData",
        ),
        (
            auth_data(
                AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_AT,
                0,
                &attested(
                    b"id",
                    &map(&[
                        (1, int(1)),
                        (3, int(-8)),
                        (-1, int(6)),
                        (-2, bytes(&[1u8; 32])),
                    ]),
                ),
            ),
            false,
            "pubKeyCredParams",
        ),
    ];
    for (ad, uv, want) in ad_cases {
        let e = verify_reg(&registration(cd.clone(), att(&ad), &id, None), &ch, uv).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let e = verify_reg(
        &registration(cd.clone(), att(&good_ad), "other", None),
        &ch,
        false,
    )
    .unwrap_err();
    assert!(e.contains("credential.id"), "{e}");
    let e = verify_reg(
        &registration(cd, att(&good_ad), &id, Some("other")),
        &ch,
        false,
    )
    .unwrap_err();
    assert!(e.contains("rawId"), "{e}");
}

/// Browsers may add members to clientDataJSON (Chrome inserts `other_keys_can_be_added_here`;
/// WebAuthn L3 §5.8.1 tells relying parties to ignore unknown members). Red before the fix:
/// `deny_unknown_fields` refused every such ceremony as "invalid clientDataJSON structure".
#[test]
fn client_data_with_extra_browser_members_is_accepted() {
    let ch = URL_SAFE_NO_PAD.encode([6u8; 32]);
    let cd = client_data(json!({
        "type": "webauthn.create",
        "challenge": ch,
        "origin": ORIGIN,
        "crossOrigin": false,
        "other_keys_can_be_added_here": "do not compare clientDataJSON against a template"
    }));
    let id = URL_SAFE_NO_PAD.encode(b"id");
    let ad = auth_data(
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_AT,
        0,
        &attested(b"id", &ec2_key()),
    );
    let att = att_object(&[
        ("fmt", text("none")),
        ("attStmt", map(&[])),
        ("authData", bytes(&ad)),
    ]);
    verify_reg(&registration(cd, att, &id, None), &ch, false).expect("extra member ignored");
}

struct Signer {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
    key: CosePublicKey,
}

impl Signer {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
        let p = pair.public_key().as_ref();
        let key = CosePublicKey::Ec2 {
            alg: -7,
            crv: 1,
            x: URL_SAFE_NO_PAD.encode(&p[1..33]),
            y: URL_SAFE_NO_PAD.encode(&p[33..65]),
        };
        Self { pair, rng, key }
    }

    fn assertion(
        &self,
        ad: &[u8],
        cdata: &str,
        user_handle: Option<&str>,
    ) -> AuthenticationClientResponse {
        let raw_cd = URL_SAFE_NO_PAD.decode(cdata).unwrap_or_default();
        let mut signed = ad.to_vec();
        signed.extend_from_slice(&Sha256::digest(&raw_cd));
        let sig = self.pair.sign(&self.rng, &signed).unwrap();
        AuthenticationClientResponse {
            id: "cred".into(),
            raw_id: None,
            cred_type: "public-key".into(),
            response: AuthenticatorAssertionResponsePayload {
                client_data_json: cdata.to_owned(),
                authenticator_data: URL_SAFE_NO_PAD.encode(ad),
                signature: URL_SAFE_NO_PAD.encode(sig.as_ref()),
                user_handle: user_handle.map(str::to_owned),
            },
            authenticator_attachment: None,
            client_extension_results: None,
        }
    }

    fn verify(
        &self,
        p: &AuthenticationClientResponse,
        ch: &str,
        uv: bool,
        stored: u32,
        be: bool,
    ) -> Result<VerifiedAssertion, String> {
        verify_authentication_response(
            p,
            ch,
            RP,
            &[ORIGIN.to_owned()],
            uv,
            "cred",
            "handle-1",
            &self.key,
            stored,
            be,
        )
        .map_err(|e| e.to_string())
    }
}

#[test]
fn authentication_refuses_each_step() {
    let s = Signer::new();
    let ch = URL_SAFE_NO_PAD.encode([8u8; 32]);
    let cd = client_data(json!({ "type": "webauthn.get", "challenge": ch, "origin": ORIGIN }));
    let ad = auth_data(AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV, 5, &[]);
    let ok = s.assertion(&ad, &cd, Some("handle-1"));
    let done = s.verify(&ok, &ch, true, 4, false).unwrap();
    assert_eq!(done.new_sign_count, 5);
    assert!(done.user_verified && !done.backup_state);
    // Both counters zero: an authenticator without a counter passes.
    let zero = s.assertion(&auth_data(AUTH_DATA_FLAG_UP, 0, &[]), &cd, None);
    assert!(s.verify(&zero, &ch, false, 0, false).is_ok());

    let mut p = ok.clone();
    p.cred_type = "other".into();
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("public-key")
    );
    let mut p = ok.clone();
    p.id = "other".into();
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("stored credential")
    );
    let p = s.assertion(&ad, &cd, Some("handle-2"));
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("userHandle")
    );
    let p = s.assertion(&ad, &cd, Some(""));
    assert!(
        s.verify(&p, &ch, false, 0, false).is_ok(),
        "an empty userHandle is no claim"
    );
    let wrong_type =
        client_data(json!({ "type": "webauthn.create", "challenge": ch, "origin": ORIGIN }));
    assert!(
        s.verify(&s.assertion(&ad, &wrong_type, None), &ch, false, 0, false)
            .unwrap_err()
            .contains("type")
    );
    let mut p = ok.clone();
    p.response.authenticator_data = "!!".into();
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("authenticatorData base64url")
    );
    let with_at = auth_data(
        AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_AT,
        1,
        &attested(b"id", &ec2_key()),
    );
    assert!(
        s.verify(&s.assertion(&with_at, &cd, None), &ch, false, 0, false)
            .unwrap_err()
            .contains("must not include")
    );
    let mut other_rp = ad.clone();
    other_rp[3] ^= 1;
    assert!(
        s.verify(&s.assertion(&other_rp, &cd, None), &ch, false, 0, false)
            .unwrap_err()
            .contains("rpIdHash")
    );
    let no_up = auth_data(AUTH_DATA_FLAG_UV, 5, &[]);
    assert!(
        s.verify(&s.assertion(&no_up, &cd, None), &ch, false, 0, false)
            .unwrap_err()
            .contains("(UP)")
    );
    let no_uv = auth_data(AUTH_DATA_FLAG_UP, 5, &[]);
    assert!(
        s.verify(&s.assertion(&no_uv, &cd, None), &ch, true, 0, false)
            .unwrap_err()
            .contains("(UV)")
    );
    assert!(
        s.verify(&ok, &ch, false, 0, true)
            .unwrap_err()
            .contains("(BE)")
    );
    let mut p = ok.clone();
    p.response.signature = "!!".into();
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("signature base64url")
    );
    let mut p = ok.clone();
    p.response.signature = URL_SAFE_NO_PAD.encode([0u8; 70]);
    assert!(
        s.verify(&p, &ch, false, 0, false)
            .unwrap_err()
            .contains("signature invalid")
    );
    // Counter regression: equal, lower, and a zero after a non-zero stored counter.
    for stored in [5, 9] {
        assert!(
            s.verify(&ok, &ch, false, stored, false)
                .unwrap_err()
                .contains("regression")
        );
    }
    assert!(
        s.verify(&zero, &ch, false, 3, false)
            .unwrap_err()
            .contains("regression")
    );
}
