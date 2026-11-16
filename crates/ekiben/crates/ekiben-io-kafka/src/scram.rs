//! The client side of SCRAM-SHA-256 and SCRAM-SHA-512 (RFC 5802, RFC 7677) as Kafka speaks it:
//! no channel binding, and the server's signature checked before the connection is used.

use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;

use crate::client::Mechanism;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// Kafka refuses fewer than 4096 iterations; more than this many would let a hostile broker
/// spin the runner's CPU.
const MAX_ITERATIONS: u32 = 1_000_000;

fn hmac(m: Mechanism, key: &[u8], data: &[u8]) -> Vec<u8> {
    match m {
        Mechanism::ScramSha512 => {
            // HMAC takes a key of any length, so the Err arm is never taken
            let Ok(mut h) = <Hmac<Sha512> as KeyInit>::new_from_slice(key) else {
                return Vec::new();
            };
            h.update(data);
            h.finalize().into_bytes().to_vec()
        }
        _ => {
            // HMAC takes a key of any length, so the Err arm is never taken
            let Ok(mut h) = <Hmac<Sha256> as KeyInit>::new_from_slice(key) else {
                return Vec::new();
            };
            h.update(data);
            h.finalize().into_bytes().to_vec()
        }
    }
}

fn hash(m: Mechanism, data: &[u8]) -> Vec<u8> {
    match m {
        Mechanism::ScramSha512 => Sha512::digest(data).to_vec(),
        _ => Sha256::digest(data).to_vec(),
    }
}

/// Hi() of RFC 5802: PBKDF2 with one block.
fn hi(m: Mechanism, password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(m, password, &first);
    let mut out = u.clone();
    for _ in 1..iterations {
        u = hmac(m, password, &u);
        for (o, b) in out.iter_mut().zip(&u) {
            *o ^= b;
        }
    }
    out
}

pub struct Client {
    m: Mechanism,
    bare: String,
    nonce: String,
}

/// The server signature the last message must carry.
pub struct Check(Vec<u8>);

impl Client {
    pub fn new(m: Mechanism, username: &str) -> Self {
        Self::with_nonce(m, username, uuid::Uuid::new_v4().simple().to_string())
    }

    fn with_nonce(m: Mechanism, username: &str, nonce: String) -> Self {
        let name = username.replace('=', "=3D").replace(',', "=2C");
        Self {
            m,
            bare: format!("n={name},r={nonce}"),
            nonce,
        }
    }

    pub fn first(&self) -> String {
        format!("n,,{}", self.bare)
    }

    pub fn final_message(
        &self,
        server_first: &[u8],
        password: &str,
    ) -> Result<(String, Check), String> {
        let sf = std::str::from_utf8(server_first)
            .map_err(|_| "the server's first message is not UTF-8")?;
        let (mut r, mut s, mut i) = (None, None, None);
        for attr in sf.split(',') {
            match attr.split_at_checked(2) {
                Some(("r=", v)) => r = Some(v),
                Some(("s=", v)) => s = Some(v),
                Some(("i=", v)) => i = Some(v),
                _ => {}
            }
        }
        let (Some(r), Some(s), Some(i)) = (r, s, i) else {
            return Err(
                "the server's first message lacks the nonce, salt or iteration count".into(),
            );
        };
        if !r.starts_with(&self.nonce) || r.len() == self.nonce.len() {
            return Err("the server's nonce does not extend the client's".into());
        }
        let salt = B64
            .decode(s)
            .map_err(|_| "the server's salt is not base64")?;
        let iterations: u32 = i
            .parse()
            .map_err(|_| "the iteration count is not a number")?;
        if !(4096..=MAX_ITERATIONS).contains(&iterations) {
            return Err(format!("the server asks for {iterations} iterations; 4096 to {MAX_ITERATIONS} are accepted"));
        }
        let salted = hi(self.m, password.as_bytes(), &salt, iterations);
        let client_key = hmac(self.m, &salted, b"Client Key");
        let stored = hash(self.m, &client_key);
        let without_proof = format!("c=biws,r={r}");
        let auth = format!("{},{sf},{without_proof}", self.bare);
        let sig = hmac(self.m, &stored, auth.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(&sig).map(|(a, b)| a ^ b).collect();
        let server_key = hmac(self.m, &salted, b"Server Key");
        let server_sig = hmac(self.m, &server_key, auth.as_bytes());
        Ok((
            format!("{without_proof},p={}", B64.encode(proof)),
            Check(server_sig),
        ))
    }
}

impl Check {
    pub fn verify(&self, server_final: &[u8]) -> Result<(), String> {
        let sf = std::str::from_utf8(server_final)
            .map_err(|_| "the server's last message is not UTF-8")?;
        if let Some(e) = sf.strip_prefix("e=") {
            return Err(format!("the server refused: {e}"));
        }
        let v = sf
            .split(',')
            .find_map(|a| a.strip_prefix("v="))
            .ok_or("the server's last message has no signature")?;
        let v = B64
            .decode(v)
            .map_err(|_| "the server's signature is not base64")?;
        if bool::from(v.ct_eq(&self.0)) {
            Ok(())
        } else {
            Err("the server's signature is wrong: the broker does not know the password".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7677 section 3.
    #[test]
    fn rfc_7677_exchange() {
        let c = Client::with_nonce(
            Mechanism::ScramSha256,
            "user",
            "rOprNGfwEbeRWgbNEkqO".into(),
        );
        assert_eq!(c.first(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let server_first = b"r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let (fin, check) = c.final_message(server_first, "pencil").expect("final");
        assert_eq!(
            fin,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        check
            .verify(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .expect("server signature");
        assert!(check.verify(b"v=AAAA").is_err());
        assert!(check.verify(b"e=invalid-proof").is_err());
    }

    #[test]
    fn a_hostile_server_first_is_refused() {
        let c = Client::with_nonce(Mechanism::ScramSha512, "a,b=c", "abc".into());
        assert_eq!(c.first(), "n,,n=a=2Cb=3Dc,r=abc");
        for sf in [
            "r=xyz123,s=AAAA,i=4096",    // another nonce
            "r=abc,s=AAAA,i=4096",       // nonce not extended
            "r=abcd,s=AAAA,i=999999999", // CPU spin
            "r=abcd,s=AAAA,i=1",         // too few
            "r=abcd,i=4096",             // no salt
        ] {
            assert!(c.final_message(sf.as_bytes(), "pw").is_err(), "{sf}");
        }
    }
}
