//! Crypto methods of Bento 1.22: `encrypt_aes`, `decrypt_aes`, `compare_argon2`,
//! `compare_bcrypt`, `parse_jwt_*` and `sign_jwt_*` (HS, RS and ES at 256/384/512).
//!
//! RSA runs on crypto-bigint's constant-time Montgomery exponentiation: the `rsa` crate
//! carries RUSTSEC-2023-0071 with no fixed release, and JWT needs only PKCS#1 v1.5.

use std::collections::BTreeMap;

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes_gcm::aead::Aead;
use base64::Engine;
use crypto_bigint::modular::{BoxedMontyForm, BoxedMontyParams};
use crypto_bigint::{BoxedUint, Odd};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::get_arg;
use crate::value::Value;

/// Largest argon2 memory cost accepted from a hash (KiB); Bento has none, this keeps one
/// mapping from taking the host's memory.
const MAX_ARGON2_MEMORY_KIB: u32 = 1024 * 1024;

pub(crate) fn eval_crypto_family(
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    let res = match name {
        "encrypt_aes" | "decrypt_aes" => aes_method(target_val, name == "encrypt_aes", args, ctx),
        "compare_argon2" => string_arg(args, ctx, "hashed_secret")
            .and_then(|hash| compare_argon2(&target_val.get_string()?, &hash)),
        "compare_bcrypt" => string_arg(args, ctx, "hashed_secret").and_then(|hash| {
            bcrypt::verify(target_val.get_string()?, &hash)
                .map(Value::Bool)
                .map_err(|e| e.to_string())
        }),
        _ => {
            let (op, alg) = name
                .strip_prefix("parse_jwt_")
                .map(|a| (false, a))
                .or_else(|| name.strip_prefix("sign_jwt_").map(|a| (true, a)))?;
            let alg = Alg::from_name(alg)?;
            string_arg(args, ctx, "signing_secret").and_then(|secret| {
                if op {
                    let Value::Object(claims) = target_val else {
                        return Err(format!(
                            "expected object value, got {}",
                            target_val.type_name()
                        ));
                    };
                    sign_jwt(alg, claims, &secret).map(Value::String)
                } else {
                    parse_jwt(alg, &target_val.get_string()?, &secret)
                }
            })
        }
    };
    Some(res)
}

fn string_arg(args: &ArgList, ctx: &mut EvalContext<'_, '_>, name: &str) -> Result<String, String> {
    ctx.eval_expr(get_arg(args, name, 0)?)?.get_string()
}

fn bytes_arg(
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
    name: &str,
    pos: usize,
) -> Result<Vec<u8>, String> {
    match ctx.eval_expr(get_arg(args, name, pos)?)? {
        Value::String(s) => Ok(s.into_bytes()),
        Value::Bytes(b) => Ok(b),
        other => Err(format!("expected string value, got {}", other.type_name())),
    }
}

// ---------------------------------------------------------------- AES

enum AnyAes {
    A128(aes::Aes128),
    A192(aes::Aes192),
    A256(aes::Aes256),
}

impl AnyAes {
    fn new(key: &[u8]) -> Result<Self, String> {
        let bad = |_| format!("crypto/aes: invalid key size {}", key.len());
        match key.len() {
            16 => aes::Aes128::new_from_slice(key)
                .map(Self::A128)
                .map_err(bad),
            24 => aes::Aes192::new_from_slice(key)
                .map(Self::A192)
                .map_err(bad),
            32 => aes::Aes256::new_from_slice(key)
                .map(Self::A256)
                .map_err(bad),
            n => Err(format!("crypto/aes: invalid key size {n}")),
        }
    }

    fn encrypt(&self, block: &mut [u8; 16]) {
        let b: &mut aes::Block = block.into();
        match self {
            Self::A128(c) => c.encrypt_block(b),
            Self::A192(c) => c.encrypt_block(b),
            Self::A256(c) => c.encrypt_block(b),
        }
    }

    fn decrypt(&self, block: &mut [u8; 16]) {
        let b: &mut aes::Block = block.into();
        match self {
            Self::A128(c) => c.decrypt_block(b),
            Self::A192(c) => c.decrypt_block(b),
            Self::A256(c) => c.decrypt_block(b),
        }
    }

    fn gcm(&self, key: &[u8], encrypt: bool, nonce: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
        let nonce: &aes_gcm::Nonce<aes_gcm::aead::consts::U12> = nonce
            .try_into()
            .map_err(|_| "crypto/cipher: incorrect nonce length given to GCM".to_string())?;
        let fail = |_| "cipher: message authentication failed".to_string();
        macro_rules! run {
            ($t:ty) => {{
                let c = <$t>::new_from_slice(key).map_err(|_| "creating gcm failed".to_string())?;
                if encrypt {
                    c.encrypt(nonce, data).map_err(fail)
                } else {
                    c.decrypt(nonce, data).map_err(fail)
                }
            }};
        }
        match self {
            Self::A128(_) => run!(aes_gcm::Aes128Gcm),
            Self::A192(_) => run!(aes_gcm::AesGcm<aes::Aes192, aes_gcm::aead::consts::U12>),
            Self::A256(_) => run!(aes_gcm::Aes256Gcm),
        }
    }
}

/// Go `cipher.NewCTR`: the whole IV is a big-endian counter.
fn ctr(c: &AnyAes, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut counter = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut ks = counter;
        c.encrypt(&mut ks);
        out.extend(chunk.iter().zip(ks).map(|(a, b)| a ^ b));
        for byte in counter.iter_mut().rev() {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }
    out
}

fn ofb(c: &AnyAes, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut state = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        c.encrypt(&mut state);
        out.extend(chunk.iter().zip(state).map(|(a, b)| a ^ b));
    }
    out
}

fn cbc(c: &AnyAes, iv: &[u8; 16], data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    if !data.len().is_multiple_of(16) {
        return Err(if encrypt {
            "plaintext is not a multiple of the block size"
        } else {
            "ciphertext is not a multiple of the block size"
        }
        .to_string());
    }
    let mut prev = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        if encrypt {
            block.iter_mut().zip(prev).for_each(|(a, b)| *a ^= b);
            c.encrypt(&mut block);
            prev = block;
        } else {
            let cipher_block = block;
            c.decrypt(&mut block);
            block.iter_mut().zip(prev).for_each(|(a, b)| *a ^= b);
            prev = cipher_block;
        }
        out.extend_from_slice(&block);
    }
    Ok(out)
}

fn aes_method(
    target: &Value,
    encrypt: bool,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let scheme = ctx.eval_expr(get_arg(args, "scheme", 0)?)?.get_string()?;
    let key = bytes_arg(args, ctx, "key", 1)?;
    let iv = bytes_arg(args, ctx, "iv", 2)?;
    let cipher = AnyAes::new(&key)?;
    let data = match target {
        Value::String(s) => s.as_bytes().to_vec(),
        Value::Bytes(b) => b.clone(),
        other => return Err(format!("expected string value, got {}", other.type_name())),
    };
    let block_iv = || -> Result<[u8; 16], String> {
        iv.as_slice()
            .try_into()
            .map_err(|_| "the key must match the initialisation vector size".to_string())
    };
    let out = match scheme.as_str() {
        "ctr" => ctr(&cipher, &block_iv()?, &data),
        "ofb" => ofb(&cipher, &block_iv()?, &data),
        "cbc" => cbc(&cipher, &block_iv()?, &data, encrypt)?,
        "gcm" => cipher.gcm(&key, encrypt, &iv, &data)?,
        other if encrypt => return Err(format!("unrecognized encryption type: {other}")),
        other => return Err(format!("unrecognized decryption type: {other}")),
    };
    // Bento's encrypt_aes returns a Go string of raw bytes, which a Rust String cannot hold
    // without loss; bytes keep the ciphertext intact for encode() and friends.
    Ok(Value::Bytes(out))
}

// ---------------------------------------------------------------- argon2

/// Bento's `compare_argon2`: an empty input is `false`; the hash's own parameters
/// (`$argon2id$v=19$m=4096,t=3,p=1$salt$key`) derive the key compared in constant time.
fn compare_argon2(input: &str, hashed: &str) -> Result<Value, String> {
    if input.is_empty() {
        return Ok(Value::Bool(false));
    }
    let invalid = |why: &str| format!("invalid argon2 hash: {why}");
    let parts: Vec<&str> = hashed.split('$').collect();
    let [_, format, version, params, salt, key] = parts.as_slice() else {
        return Err("invalid argon2 hash".to_string());
    };
    let alg = match *format {
        "argon2i" => argon2::Algorithm::Argon2i,
        "argon2id" => argon2::Algorithm::Argon2id,
        _ => return Err(invalid("unrecognised argon2 format")),
    };
    if version.strip_prefix("v=").is_none_or(str::is_empty) {
        return Err(invalid("failed to parse version"));
    }
    let mut nums = [0u32; 3];
    let fields: Vec<&str> = params.split(',').collect();
    for ((field, prefix), slot) in fields.iter().zip(["m=", "t=", "p="]).zip(nums.iter_mut()) {
        *slot = field
            .strip_prefix(prefix)
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| invalid("failed to parse parameters"))?;
    }
    if fields.len() != 3 {
        return Err(invalid("failed to parse parameters"));
    }
    let [memory, iterations, parallelism] = nums;
    if memory > MAX_ARGON2_MEMORY_KIB {
        return Err(invalid("memory cost above 1 GiB"));
    }
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD;
    let salt = b64
        .decode(salt)
        .map_err(|_| invalid("failed to parse base64 salt"))?;
    let key = b64
        .decode(key)
        .map_err(|_| invalid("failed to parse base64 key"))?;
    let params = argon2::Params::new(memory, iterations, parallelism, Some(key.len()))
        .map_err(|e| invalid(&e.to_string()))?;
    let mut derived = vec![0u8; key.len()];
    argon2::Argon2::new(alg, argon2::Version::V0x13, params)
        .hash_password_into(input.as_bytes(), &salt, &mut derived)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(Value::Bool(constant_time_eq(&derived, &key)))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------- JWT

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Hs,
    Rs,
    Es,
}

#[derive(Clone, Copy)]
struct Alg {
    family: Family,
    bits: u16,
}

impl Alg {
    fn from_name(name: &str) -> Option<Self> {
        let family = match name.get(..2)? {
            "hs" => Family::Hs,
            "rs" => Family::Rs,
            "es" => Family::Es,
            _ => return None,
        };
        let bits = match name.get(2..)? {
            "256" => 256,
            "384" => 384,
            "512" => 512,
            _ => return None,
        };
        Some(Self { family, bits })
    }

    fn header_name(self) -> String {
        let f = match self.family {
            Family::Hs => "HS",
            Family::Rs => "RS",
            Family::Es => "ES",
        };
        format!("{f}{}", self.bits)
    }

    fn digest(self, data: &[u8]) -> Vec<u8> {
        match self.bits {
            256 => Sha256::digest(data).to_vec(),
            384 => Sha384::digest(data).to_vec(),
            _ => Sha512::digest(data).to_vec(),
        }
    }

    fn hmac(self, key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
        macro_rules! mac {
            ($h:ty) => {{
                let mut m = <Hmac<$h>>::new_from_slice(key).map_err(|e| e.to_string())?;
                m.update(data);
                m.finalize().into_bytes().to_vec()
            }};
        }
        Ok(match self.bits {
            256 => mac!(Sha256),
            384 => mac!(Sha384),
            _ => mac!(Sha512),
        })
    }
}

fn b64url() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// golang-jwt v5 `ParseWithClaims` with the method pinned: signature, then `exp`/`nbf`.
fn parse_jwt(alg: Alg, token: &str, secret: &str) -> Result<Value, String> {
    let fail = |why: String| format!("failed to parse JWT string: {why}");
    let segments: Vec<&str> = token.split('.').collect();
    let [header, payload, signature] = segments.as_slice() else {
        return Err(fail(
            "token is malformed: token contains an invalid number of segments".to_string(),
        ));
    };
    let decode = |seg: &str, what: &str| {
        b64url().decode(seg).map_err(|e| {
            fail(format!(
                "token is malformed: could not base64 decode {what}: {e}"
            ))
        })
    };
    let head: serde_json::Value =
        serde_json::from_slice(&decode(header, "header")?).map_err(|e| {
            fail(format!(
                "token is malformed: could not JSON decode header: {e}"
            ))
        })?;
    let claims_raw = decode(payload, "claim")?;
    let sig = decode(signature, "signature")?;
    let named = head.get("alg").and_then(|a| a.as_str()).unwrap_or_default();
    if named != alg.header_name() {
        return Err(fail(format!(
            "token is unverifiable: error while executing keyfunc: incorrect signing method: {named}"
        )));
    }
    let signed = &token[..header.len() + 1 + payload.len()];
    if !verify(alg, signed.as_bytes(), &sig, secret)? {
        return Err(fail(
            "token signature is invalid: signature is invalid".to_string(),
        ));
    }
    let claims: serde_json::Value = serde_json::from_slice(&claims_raw).map_err(|e| {
        fail(format!(
            "token is malformed: could not JSON decode claim: {e}"
        ))
    })?;
    let serde_json::Value::Object(map) = claims else {
        return Err(fail(
            "token is malformed: claims are not an object".to_string(),
        ));
    };
    let now = chrono::Utc::now().timestamp() as f64;
    let num = |k: &str| map.get(k).and_then(serde_json::Value::as_f64);
    if num("exp").is_some_and(|exp| now >= exp) {
        return Err(fail(
            "token has invalid claims: token is expired".to_string(),
        ));
    }
    if num("nbf").is_some_and(|nbf| now < nbf) {
        return Err(fail(
            "token has invalid claims: token is not valid yet".to_string(),
        ));
    }
    // golang-jwt decodes claims without UseNumber: every number is a float64.
    Ok(floats(Value::from_json_value(serde_json::Value::Object(
        map,
    ))))
}

fn floats(v: Value) -> Value {
    match v {
        Value::Int(i) => Value::Float(i as f64),
        Value::Uint(u) => Value::Float(u as f64),
        Value::Array(a) => Value::Array(a.into_iter().map(floats).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, floats(v))).collect()),
        other => other,
    }
}

fn sign_jwt(alg: Alg, claims: &BTreeMap<String, Value>, secret: &str) -> Result<String, String> {
    let header = format!(r#"{{"alg":"{}","typ":"JWT"}}"#, alg.header_name());
    let body = Value::Object(claims.clone().into()).to_go_json();
    let signing_input = format!("{}.{}", b64url().encode(header), b64url().encode(body));
    let decode_err = |e: String| format!("failed to decode signing_secret: {e}");
    let sig = match alg.family {
        Family::Hs => alg.hmac(secret.as_bytes(), signing_input.as_bytes())?,
        Family::Rs => {
            let key = RsaPrivate::from_pem(secret).map_err(decode_err)?;
            key.sign(alg, signing_input.as_bytes())
                .map_err(|e| format!("failed to sign token: {e}"))?
        }
        Family::Es => es_sign(alg, secret, signing_input.as_bytes()).map_err(decode_err)?,
    };
    Ok(format!("{signing_input}.{}", b64url().encode(sig)))
}

fn verify(alg: Alg, msg: &[u8], sig: &[u8], secret: &str) -> Result<bool, String> {
    let key_err = |e: String| format!("failed to decode signing_secret: {e}");
    match alg.family {
        Family::Hs => Ok(constant_time_eq(&alg.hmac(secret.as_bytes(), msg)?, sig)),
        Family::Rs => Ok(RsaPublic::from_pem(secret)
            .map_err(key_err)?
            .verify(alg, msg, sig)),
        Family::Es => es_verify(alg, secret, msg, sig).map_err(key_err),
    }
}

fn es_sign(alg: Alg, pem: &str, msg: &[u8]) -> Result<Vec<u8>, String> {
    use ecdsa_signer as s;
    macro_rules! sign {
        ($c:ident) => {{
            let key = $c::SecretKey::from_sec1_pem(pem)
                .or_else(|_| <$c::SecretKey as s::DecodePrivateKey>::from_pkcs8_pem(pem))
                .map_err(|_| s::NOT_EC.to_string())?;
            let sk = $c::ecdsa::SigningKey::from(key);
            let sig: $c::ecdsa::Signature = s::Signer::sign(&sk, msg);
            sig.to_bytes().to_vec()
        }};
    }
    Ok(match alg.bits {
        256 => sign!(p256),
        384 => sign!(p384),
        _ => sign!(p521),
    })
}

fn es_verify(alg: Alg, pem: &str, msg: &[u8], sig: &[u8]) -> Result<bool, String> {
    use ecdsa_signer as s;
    macro_rules! check {
        ($c:ident) => {{
            let vk = <$c::ecdsa::VerifyingKey as s::DecodePublicKey>::from_public_key_pem(pem)
                .map_err(|_| s::NOT_EC.to_string())?;
            match $c::ecdsa::Signature::from_slice(sig) {
                Ok(sig) => s::Verifier::verify(&vk, msg, &sig).is_ok(),
                Err(_) => false,
            }
        }};
    }
    Ok(match alg.bits {
        256 => check!(p256),
        384 => check!(p384),
        _ => check!(p521),
    })
}

mod ecdsa_signer {
    pub use p256::ecdsa::signature::{Signer, Verifier};
    pub use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
    pub const NOT_EC: &str = "invalid key: Key must be a PEM encoded PKCS1 or PKCS8 key";
}

// ---------------------------------------------------------------- RSA (PKCS#1 v1.5)

/// Smallest DER reader: one tag-length-value, returning (tag, content, rest).
fn der_tlv(input: &[u8]) -> Result<(u8, &[u8], &[u8]), String> {
    let bad = || "invalid key: malformed DER".to_string();
    let (&tag, rest) = input.split_first().ok_or_else(bad)?;
    let (&first, rest) = rest.split_first().ok_or_else(bad)?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return Err(bad());
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    if rest.len() < len {
        return Err(bad());
    }
    Ok((tag, &rest[..len], &rest[len..]))
}

fn der_seq(input: &[u8]) -> Result<&[u8], String> {
    match der_tlv(input)? {
        (0x30, content, _) => Ok(content),
        _ => Err("invalid key: expected a DER SEQUENCE".to_string()),
    }
}

/// Reads the INTEGERs of a SEQUENCE body, leading zero bytes stripped.
fn der_ints(mut body: &[u8], count: usize) -> Result<Vec<&[u8]>, String> {
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let (tag, content, rest) = der_tlv(body)?;
        if tag != 0x02 {
            return Err("invalid key: expected a DER INTEGER".to_string());
        }
        let first_nonzero = content
            .iter()
            .position(|b| *b != 0)
            .unwrap_or(content.len());
        out.push(&content[first_nonzero..]);
        body = rest;
    }
    Ok(out)
}

const RSA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// Largest RSA modulus accepted (bits); bounds the exponentiation cost per call.
const MAX_RSA_BITS: usize = 8192;

fn pem_der(pem: &str) -> Result<(String, Vec<u8>), String> {
    pem_rfc7468::decode_vec(pem.trim().as_bytes())
        .map(|(label, der)| (label.to_string(), der))
        .map_err(|_| "invalid key: Key must be a PEM encoded PKCS1 or PKCS8 key".to_string())
}

/// The PKCS#1 key inside a PKCS#8/SPKI wrapper: SEQ { [version,] SEQ { rsaEncryption }, key }.
fn unwrap_rsa(der: &[u8], key_tag: u8) -> Result<Vec<u8>, String> {
    let mut body = der_seq(der)?;
    if key_tag == 0x04 {
        body = der_tlv(body)?.2; // version
    }
    let (_, alg, rest) = der_tlv(body)?;
    let (_, oid, _) = der_tlv(alg)?;
    if oid != RSA_OID {
        return Err("key is not a valid RSA key".to_string());
    }
    let (tag, key, _) = der_tlv(rest)?;
    if tag != key_tag {
        return Err("invalid key: malformed DER".to_string());
    }
    // A BIT STRING starts with its unused-bits count.
    Ok(if key_tag == 0x03 {
        key.get(1..).unwrap_or_default()
    } else {
        key
    }
    .to_vec())
}

struct RsaPublic {
    n: Vec<u8>,
    e: Vec<u8>,
}

impl RsaPublic {
    fn from_pem(pem: &str) -> Result<Self, String> {
        let (label, der) = pem_der(pem)?;
        let pkcs1 = match label.as_str() {
            "RSA PUBLIC KEY" => der,
            "PUBLIC KEY" => unwrap_rsa(&der, 0x03)?,
            _ => return Err("invalid key: Key must be a PEM encoded PKCS1 or PKCS8 key".into()),
        };
        let ints = der_ints(der_seq(&pkcs1)?, 2)?;
        Self::checked(ints[0], ints[1])
    }

    fn checked(n: &[u8], e: &[u8]) -> Result<Self, String> {
        if n.len() * 8 > MAX_RSA_BITS || n.len() < 64 || e.is_empty() || e.len() > 8 {
            return Err("key is not a valid RSA key".to_string());
        }
        Ok(Self {
            n: n.to_vec(),
            e: e.to_vec(),
        })
    }

    fn verify(&self, alg: Alg, msg: &[u8], sig: &[u8]) -> bool {
        if sig.len() != self.n.len() {
            return false;
        }
        match (
            emsa_pkcs1(alg, msg, self.n.len()),
            modpow(sig, &self.e, &self.n),
        ) {
            (Some(want), Some(got)) => constant_time_eq(&want, &got),
            _ => false,
        }
    }
}

struct RsaPrivate {
    n: Vec<u8>,
    d: Vec<u8>,
}

impl RsaPrivate {
    fn from_pem(pem: &str) -> Result<Self, String> {
        let (label, der) = pem_der(pem)?;
        let pkcs1 = match label.as_str() {
            "RSA PRIVATE KEY" => der,
            "PRIVATE KEY" => unwrap_rsa(&der, 0x04)?,
            _ => return Err("invalid key: Key must be a PEM encoded PKCS1 or PKCS8 key".into()),
        };
        let ints = der_ints(der_seq(&pkcs1)?, 4)?; // version, n, e, d
        let public = RsaPublic::checked(ints[1], ints[2])?;
        Ok(Self {
            n: public.n,
            d: ints[3].to_vec(),
        })
    }

    /// m^d mod n through crypto-bigint's constant-time exponentiation.
    fn sign(&self, alg: Alg, msg: &[u8]) -> Result<Vec<u8>, String> {
        let em = emsa_pkcs1(alg, msg, self.n.len())
            .ok_or_else(|| "key size too small for PKCS#1 v1.5 signature".to_string())?;
        modpow(&em, &self.d, &self.n).ok_or_else(|| "rsa exponentiation failed".to_string())
    }
}

/// EMSA-PKCS1-v1_5: 00 01 FF.. 00 DigestInfo(hash), `k` bytes long.
fn emsa_pkcs1(alg: Alg, msg: &[u8], k: usize) -> Option<Vec<u8>> {
    let prefix: &[u8] = match alg.bits {
        256 => &[
            0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x01, 0x05, 0x00, 0x04, 0x20,
        ],
        384 => &[
            0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x02, 0x05, 0x00, 0x04, 0x30,
        ],
        _ => &[
            0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x03, 0x05, 0x00, 0x04, 0x40,
        ],
    };
    let hash = alg.digest(msg);
    let t_len = prefix.len() + hash.len();
    let pad = k.checked_sub(t_len + 3).filter(|p| *p >= 8)?;
    let mut em = Vec::with_capacity(k);
    em.extend_from_slice(&[0x00, 0x01]);
    em.extend(std::iter::repeat_n(0xff, pad));
    em.push(0x00);
    em.extend_from_slice(prefix);
    em.extend_from_slice(&hash);
    Some(em)
}

/// base^exp mod n, big-endian bytes left-padded to n's length; `None` for an even modulus
/// or a base not below n.
fn modpow(base: &[u8], exp: &[u8], n: &[u8]) -> Option<Vec<u8>> {
    let bits = u32::try_from(n.len().div_ceil(8) * 64).ok()?;
    let modulus = BoxedUint::from_be_slice(n, bits).ok()?;
    let odd = Option::<Odd<BoxedUint>>::from(Odd::new(modulus.clone()))?;
    let b = BoxedUint::from_be_slice(base, bits).ok()?;
    if b >= modulus {
        return None;
    }
    let e = BoxedUint::from_be_slice(exp, bits).ok()?;
    let params = BoxedMontyParams::new(odd);
    let out = BoxedMontyForm::new(b, &params)
        .pow(&e)
        .retrieve()
        .to_be_bytes();
    Some(out[out.len() - n.len()..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_ctr_carries_across_iv_bytes() {
        let c = AnyAes::new(&[7u8; 16]).expect("key");
        let iv = [0xffu8; 16];
        let data = [1u8; 40];
        let enc = ctr(&c, &iv, &data);
        assert_eq!(ctr(&c, &iv, &enc), data);
        let mut wrapped = [0u8; 16];
        c.encrypt(&mut wrapped);
        assert_eq!(
            enc[16] ^ 1,
            wrapped[0],
            "counter wraps to zero after 0xff..ff"
        );
    }

    #[test]
    fn cbc_round_trips_and_refuses_partial_blocks() {
        let c = AnyAes::new(&[3u8; 32]).expect("key");
        let enc = cbc(&c, &[9u8; 16], &[5u8; 32], true).expect("enc");
        assert_eq!(cbc(&c, &[9u8; 16], &enc, false).expect("dec"), [5u8; 32]);
        assert!(cbc(&c, &[9u8; 16], &[5u8; 3], true).is_err());
    }

    #[test]
    fn argon2_refuses_huge_memory() {
        let hash = "$argon2id$v=19$m=99999999,t=3,p=1$c2FsdA$a2V5";
        assert!(compare_argon2("x", hash).is_err());
        assert_eq!(compare_argon2("", hash).ok(), Some(Value::Bool(false)));
    }

    /// A throwaway 1024-bit test key (n, d; e = 65537), not used anywhere else.
    const TEST_MODULUS: &str = "d387dcd286bbe43a45dfe42e083b160f923ad575c2f9d0aca43539c3591b2cd45c03d9bc470a7d9f361fedc0b58eb202a402a7d51a449c6020e91f560f071e829b27bfc997739f223d0c9c56d7673a3d17ab0e227453429e987b60cd0d11b994ffc4281e1877cfd401092c251f88bade2b347afd18cef723b4351e359bfa808d";
    const TEST_PRIVATE_EXPONENT: &str = "cc21344c4591547197b4b3f26826fce711ea8ccc13d818344252cae7653a004cfe6f08d3d48a9f27e187765c9dd8929851c5ee488c0ce20b323745f23a96582d57bb0755fe56973a576abac807b5d634d6a60c52a133ce3cb1d1c0c16f7bcd4724efe47efb2c501857927ea3fe59453d8f9521a08eed42d4688bd60525b8c581";

    #[test]
    fn rsa_signatures_verify_and_tampering_fails() {
        let n = hex::decode(TEST_MODULUS).expect("hex");
        let private = RsaPrivate {
            n: n.clone(),
            d: hex::decode(TEST_PRIVATE_EXPONENT).expect("hex"),
        };
        let public = RsaPublic::checked(&n, &[0x01, 0x00, 0x01]).expect("key");
        for name in ["rs256", "rs384", "rs512"] {
            let alg = Alg::from_name(name).expect("alg");
            let sig = private.sign(alg, b"header.claims").expect("sign");
            assert!(public.verify(alg, b"header.claims", &sig), "{name}");
            assert!(
                !public.verify(alg, b"header.claimz", &sig),
                "{name} tampered"
            );
        }
    }

    #[test]
    fn es_and_hs_tokens_round_trip_and_expire() {
        use p256::pkcs8::EncodePublicKey;
        let key = p256::SecretKey::from_slice(&[7u8; 32]).expect("scalar");
        let private_pem = key.to_sec1_pem(Default::default()).expect("pem");
        let public_pem = key
            .public_key()
            .to_public_key_pem(Default::default())
            .expect("pem");
        let mut claims = BTreeMap::new();
        claims.insert("sub".to_string(), Value::String("u".into()));
        let es = Alg::from_name("es256").expect("alg");
        let token = sign_jwt(es, &claims, &private_pem).expect("sign");
        let parsed = parse_jwt(es, &token, &public_pem).expect("parse");
        assert_eq!(parsed, Value::Object(claims.clone().into()));
        let hs = Alg::from_name("hs256").expect("alg");
        assert!(parse_jwt(hs, &token, "secret")
            .unwrap_err()
            .contains("incorrect signing method: ES256"));
        claims.insert("exp".to_string(), Value::Int(1));
        let expired = sign_jwt(hs, &claims, "secret").expect("sign");
        assert!(parse_jwt(hs, &expired, "secret")
            .unwrap_err()
            .ends_with("token is expired"));
        assert!(parse_jwt(hs, &expired, "other")
            .unwrap_err()
            .contains("signature is invalid"));
    }

    #[test]
    fn modpow_small_numbers() {
        // 4^13 mod 497 = 445
        assert_eq!(modpow(&[4], &[13], &[0x01, 0xf1]), Some(vec![0x01, 0xbd]));
        assert_eq!(modpow(&[4], &[13], &[0x01, 0xf0]), None, "even modulus");
    }
}
