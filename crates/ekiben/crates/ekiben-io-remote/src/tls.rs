//! The TLS client both inputs speak: the system's roots plus an optional private CA, ring, TLS
//! 1.2 and 1.3, and the server's name checked against its certificate.

use std::sync::Arc;

use ekiben_core::CoreError;
use rustls_pki_types::{pem::PemObject, CertificateDer, ServerName};

/// The client configuration with the system roots and every certificate of `root_cas` (PEM).
pub(crate) fn client_config(
    root_cas: Option<&str>,
) -> Result<Arc<rustls::ClientConfig>, CoreError> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    if let Some(pem) = root_cas {
        let mut added = 0;
        for cert in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            let cert = cert.map_err(|e| CoreError::config(format!("root_cas: {e}")))?;
            roots
                .add(cert)
                .map_err(|e| CoreError::config(format!("root_cas: {e}")))?;
            added += 1;
        }
        if added == 0 {
            return Err(CoreError::config(
                "root_cas holds no PEM certificate (-----BEGIN CERTIFICATE-----)",
            ));
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| CoreError::other(format!("tls: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The name the server's certificate must carry.
pub(crate) fn server_name(host: &str) -> Result<ServerName<'static>, CoreError> {
    ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
    )
    .map_err(|e| CoreError::config(format!("tls: '{host}' is not a server name: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_ca_that_is_not_a_certificate_and_a_bad_server_name_are_refused() {
        assert!(client_config(None).is_ok());
        let not_pem = client_config(Some("just text")).err().expect("no PEM");
        assert!(
            not_pem.to_string().contains("holds no PEM certificate"),
            "{not_pem}"
        );
        let bad_base64 = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        let err = client_config(Some(bad_base64)).err().expect("bad base64");
        assert!(err.to_string().contains("root_cas: "), "{err}");
        // Valid base64, but not a DER certificate: the store refuses it.
        let not_der = "-----BEGIN CERTIFICATE-----\naGVsbG8gd29ybGQ=\n-----END CERTIFICATE-----\n";
        let err = client_config(Some(not_der)).err().expect("not DER");
        assert!(err.to_string().contains("root_cas: "), "{err}");

        assert!(server_name("imap.example.org").is_ok());
        assert!(server_name("[2001:db8::1]").is_ok());
        let err = server_name("a..b").err().expect("empty label");
        assert!(
            err.to_string().contains("'a..b' is not a server name"),
            "{err}"
        );
    }
}
