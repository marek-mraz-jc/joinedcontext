//! A string whose `Debug` does not print it (CC-06, OPS-12, SEC-GAP-03).
//!
//! A configuration struct is the natural thing to `Debug`-print while chasing a startup
//! problem, and every such struct that holds a credential is one `tracing::debug!(?config)`
//! away from putting it in the cluster's logs, which `CLAUDE.md` forbids outright. Writing a
//! `Debug` by hand for each of them works until somebody adds a field, so the redaction lives
//! on the value instead: a [`Secret`] prints as `<redacted>` wherever it is nested, copied or
//! moved, and the plaintext comes out only by asking for it by name.

use std::fmt;

use zeroize::Zeroize;

/// A credential held in memory: a client secret, a token, a password.
///
/// It can be read only through [`Secret::expose`], which is the grep an auditor runs to find
/// every place a credential can escape. There is no `Display`, no `Serialize` and no
/// `Deref`, because each of those is a way for the value to reach a log or a response by
/// accident, and the buffer is wiped when the value is dropped. There is no `PartialEq` either:
/// `==` on two credentials runs in time that depends on the bytes (F-R2PLT3-018).
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wraps a plaintext credential the process already holds, from the environment or a file.
    pub fn new(plaintext: impl Into<String>) -> Self {
        Self(plaintext.into())
    }

    /// The plaintext. Every call site of this is a place where a credential can escape.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The secret of `client` in an Organization's own `realm`, derived from this, the client's
    /// installation secret (S1-04): HMAC-SHA256 keyed by the value's SHA-256 over the
    /// length-prefixed realm and client names, as hex. jc's kaisatsu writes it into the realm;
    /// the Portal and the model proxy sign in to that realm with it, each from the one value it
    /// already holds, so nothing more is stored. A secret read in one realm is worthless in any
    /// other realm and in the installation's.
    pub fn for_realm(&self, realm: &str, client: &str) -> Self {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};
        // A fixed-size key (the value's digest in a zeroed block): HMAC takes it infallibly.
        let mut key = zeroize::Zeroizing::new([0u8; 64]);
        key[..32].copy_from_slice(&Sha256::digest(self.0.as_bytes()));
        let mut mac = <Hmac<Sha256> as Mac>::new(&(*key).into());
        for part in [realm, client] {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part.as_bytes());
        }
        let digest = mac.finalize().into_bytes();
        Self(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Whether the credential is empty, for the checks that must not read it.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl From<String> for Secret {
    fn from(plaintext: String) -> Self {
        Self::new(plaintext)
    }
}

/// How a workload proves its own Keycloak client in a request to the realm (PF-47).
#[derive(Debug, Clone)]
pub enum ClientCredential {
    /// A client secret, sent with the client id.
    Secret(Secret),
    /// The pod's projected ServiceAccount token, presented as `client_assertion` to a
    /// `federated-jwt` client. The file is read at every request, because the kubelet rewrites
    /// it before the token expires.
    Assertion(std::path::PathBuf),
}

/// RFC 7523's `client_assertion_type` for a JWT.
pub const JWT_BEARER: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

impl ClientCredential {
    /// The client-authentication fields of a form posted to the realm. An assertion goes without
    /// `client_id`: Keycloak finds a `federated-jwt` client by the token's subject and refuses a
    /// request that names one beside it. A missing or empty token file is an error, so nothing
    /// is asked of the realm without a credential; the error names the path, never a value.
    pub fn form(&self, client_id: &str) -> Result<Vec<(&'static str, String)>, String> {
        match self {
            Self::Secret(secret) => Ok(vec![
                ("client_id", client_id.to_owned()),
                ("client_secret", secret.expose().to_owned()),
            ]),
            Self::Assertion(path) => {
                let token = std::fs::read_to_string(path).map_err(|e| {
                    format!(
                        "the projected token {} is not readable: {e}",
                        path.display()
                    )
                })?;
                let token = token.trim();
                if token.is_empty() {
                    return Err(format!("the projected token {} is empty", path.display()));
                }
                Ok(vec![
                    ("client_assertion_type", JWT_BEARER.to_owned()),
                    ("client_assertion", token.to_owned()),
                ])
            }
        }
    }
}

#[cfg(test)]
mod client_credential_tests {
    use super::{ClientCredential, Secret, JWT_BEARER};

    fn file(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jc-core-cred-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        let path = dir.join("token");
        std::fs::write(&path, content).expect("the token file");
        path
    }

    #[test]
    fn a_secret_goes_with_the_client_id() {
        let form = ClientCredential::Secret(Secret::new("s3cret"))
            .form("portal-api")
            .expect("a form");
        assert_eq!(
            form,
            vec![
                ("client_id", "portal-api".to_owned()),
                ("client_secret", "s3cret".to_owned())
            ]
        );
    }

    #[test]
    fn an_assertion_is_read_at_every_call_and_names_no_client() {
        let path = file("rotates", "eyJ.first\n");
        let credential = ClientCredential::Assertion(path.clone());
        let first = credential.form("portal-api").expect("a form");
        std::fs::write(&path, "eyJ.second").expect("the rotated token");
        let second = credential.form("portal-api").expect("a form");
        for (form, token) in [(first, "eyJ.first"), (second, "eyJ.second")] {
            assert_eq!(
                form,
                vec![
                    ("client_assertion_type", JWT_BEARER.to_owned()),
                    ("client_assertion", token.to_owned())
                ]
            );
        }
    }

    #[test]
    fn a_missing_or_empty_token_file_is_an_error() {
        let empty = file("empty", " \n");
        for path in [empty.clone(), empty.with_file_name("absent")] {
            let error = ClientCredential::Assertion(path)
                .form("portal-api")
                .expect_err("no form without a token");
            assert!(error.contains("projected token"), "{error}");
        }
    }

    #[test]
    fn debug_prints_no_secret() {
        let shown = format!("{:?}", ClientCredential::Secret(Secret::new("s3cret")));
        assert!(!shown.contains("s3cret"), "{shown}");
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    /// S1-04: one realm's secret is its own per realm and client, stable, and never the
    /// installation's.
    #[test]
    fn a_realm_secret_is_its_own_per_realm_and_client() {
        let installation = Secret::new("installation-client-secret-0123456789");
        let a = installation.for_realm("a", "portal-api");
        assert_eq!(
            a.expose(),
            installation.for_realm("a", "portal-api").expose()
        );
        assert_eq!(a.expose().len(), 64);
        for other in [
            installation.for_realm("b", "portal-api"),
            installation.for_realm("a", "portal-reconciler"),
            installation.for_realm("ap", "ortal-api"),
            Secret::new("another-installation").for_realm("a", "portal-api"),
            installation.clone(),
        ] {
            assert_ne!(a.expose(), other.expose());
        }
    }

    /// SEC-GAP-03: the property is the absence of the value, not the presence of a word.
    #[test]
    fn no_formatting_of_a_secret_prints_it() {
        let secret = Secret::new("s3cr3t-value-nobody-may-log");
        for rendered in [
            format!("{secret:?}"),
            format!("{:?}", Some(secret.clone())),
            format!("{:?}", ("gateway-client", secret.clone())),
            format!("{:#?}", vec![secret.clone()]),
        ] {
            assert!(
                !rendered.contains("s3cr3t"),
                "a secret reached a formatted string: {rendered}",
            );
            assert!(rendered.contains("<redacted>"), "{rendered}");
        }
        // Asking for it by name is the one way out, so the value is still usable.
        assert_eq!(secret.expose(), "s3cr3t-value-nobody-may-log");
    }

    #[test]
    fn a_secret_is_still_a_value_that_clones() {
        assert_eq!(
            Secret::new("a").clone().expose(),
            Secret::from("a".to_owned()).expose()
        );
        assert!(Secret::new("").is_empty());
        assert!(!Secret::new("a").is_empty());
    }
}
