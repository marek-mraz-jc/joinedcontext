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
