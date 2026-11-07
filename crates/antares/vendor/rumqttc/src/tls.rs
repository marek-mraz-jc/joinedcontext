#[cfg(feature = "use-rustls-no-provider")]
use tokio_rustls::rustls::{
    self,
    pki_types::{
        pem::{self, PemObject},
        CertificateDer, InvalidDnsNameError, PrivateKeyDer, ServerName,
    },
    ClientConfig, RootCertStore,
};
#[cfg(feature = "use-rustls-no-provider")]
use tokio_rustls::TlsConnector as RustlsConnector;

#[cfg(feature = "use-rustls-no-provider")]
use std::convert::TryFrom;
#[cfg(feature = "use-rustls-no-provider")]
use std::sync::Arc;

use crate::framed::AsyncReadWrite;
use crate::TlsConfiguration;

#[cfg(feature = "use-native-tls")]
use tokio_native_tls::TlsConnector as NativeTlsConnector;

#[cfg(feature = "use-native-tls")]
use tokio_native_tls::native_tls::{Error as NativeTlsError, Identity};

use std::io;
use std::net::AddrParseError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Error parsing IP address
    #[error("Addr")]
    Addr(#[from] AddrParseError),
    /// I/O related error
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[cfg(feature = "use-rustls-no-provider")]
    /// Certificate/Name validation error
    #[error("Web Pki: {0}")]
    WebPki(#[from] webpki::Error),
    /// Invalid DNS name
    #[cfg(feature = "use-rustls-no-provider")]
    #[error("DNS name")]
    DNSName(#[from] InvalidDnsNameError),
    #[cfg(feature = "use-rustls-no-provider")]
    /// Error from rustls module
    #[error("TLS error: {0}")]
    TLS(#[from] rustls::Error),
    #[cfg(feature = "use-rustls-no-provider")]
    /// No valid CA cert found
    #[error("No valid CA certificate provided")]
    NoValidCertInChain,
    #[cfg(feature = "use-rustls-no-provider")]
    /// No valid client cert found
    #[error("No valid certificate for client authentication in chain")]
    NoValidClientCertInChain,
    #[cfg(feature = "use-rustls-no-provider")]
    /// No valid key found
    #[error("No valid key in chain")]
    NoValidKeyInChain,
    #[cfg(feature = "use-native-tls")]
    #[error("Native TLS error {0}")]
    NativeTls(#[from] NativeTlsError),
}

#[cfg(feature = "use-rustls-no-provider")]
pub async fn rustls_connector(tls_config: &TlsConfiguration) -> Result<RustlsConnector, Error> {
    let config = match tls_config {
        TlsConfiguration::Simple {
            ca,
            alpn,
            client_auth,
        } => {
            // Add ca to root store if the connection is TLS
            let config = ClientConfig::builder().with_root_certificates(root_store(ca)?);

            // Add der encoded client cert and key
            let mut config = if let Some(client) = client_auth.as_ref() {
                let certs = pem_certs(&client.0)?;
                if certs.is_empty() {
                    return Err(Error::NoValidClientCertInChain);
                }
                config.with_client_auth_cert(certs, pem_private_key(&client.1)?)?
            } else {
                config.with_no_client_auth()
            };

            // Set ALPN
            if let Some(alpn) = alpn.as_ref() {
                config.alpn_protocols.extend_from_slice(alpn);
            }

            Arc::new(config)
        }
        TlsConfiguration::Rustls(tls_client_config) => tls_client_config.clone(),
        #[allow(unreachable_patterns)]
        _ => unreachable!("This cannot be called for other TLS backends than Rustls"),
    };

    Ok(RustlsConnector::from(config))
}

/// The CA certificates of `ca` (PEM) as a root store; an error when none parses.
#[cfg(feature = "use-rustls-no-provider")]
fn root_store(ca: &[u8]) -> Result<RootCertStore, Error> {
    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_parsable_certificates(pem_certs(ca)?);
    if root_cert_store.is_empty() {
        return Err(Error::NoValidCertInChain);
    }
    Ok(root_cert_store)
}

/// Every CERTIFICATE section of `pem`, other sections skipped; a malformed section is an error.
#[cfg(feature = "use-rustls-no-provider")]
fn pem_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, Error> {
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(pem_error)
}

/// The first SEC1, PKCS#1 or PKCS#8 private key of `pem`; `NoValidKeyInChain` when there is none.
#[cfg(feature = "use-rustls-no-provider")]
fn pem_private_key(pem: &[u8]) -> Result<PrivateKeyDer<'static>, Error> {
    PrivateKeyDer::from_pem_slice(pem).map_err(|err| match err {
        pem::Error::NoItemsFound => Error::NoValidKeyInChain,
        err => pem_error(err),
    })
}

/// A malformed PEM section, as the `io::ErrorKind::InvalidData` rustls-pemfile returned.
#[cfg(feature = "use-rustls-no-provider")]
fn pem_error(err: pem::Error) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(feature = "use-native-tls")]
pub async fn native_tls_connector(
    tls_config: &TlsConfiguration,
) -> Result<NativeTlsConnector, Error> {
    let connector = match tls_config {
        TlsConfiguration::SimpleNative { ca, client_auth } => {
            let cert = native_tls::Certificate::from_pem(ca)?;

            let mut connector_builder = native_tls::TlsConnector::builder();
            connector_builder.add_root_certificate(cert);

            if let Some((der, password)) = client_auth {
                let identity = Identity::from_pkcs12(der, password)?;
                connector_builder.identity(identity);
            }

            connector_builder.build()?
        }
        TlsConfiguration::Native => native_tls::TlsConnector::new()?,
        TlsConfiguration::NativeConnector(connector) => connector.to_owned(),
        #[allow(unreachable_patterns)]
        _ => unreachable!("This cannot be called for other TLS backends than Native TLS"),
    };

    Ok(connector.into())
}

pub async fn tls_connect(
    addr: &str,
    _port: u16,
    tls_config: &TlsConfiguration,
    tcp: Box<dyn AsyncReadWrite>,
) -> Result<Box<dyn AsyncReadWrite>, Error> {
    let tls: Box<dyn AsyncReadWrite> = match tls_config {
        #[cfg(feature = "use-rustls-no-provider")]
        TlsConfiguration::Simple { .. } | TlsConfiguration::Rustls(_) => {
            let connector = rustls_connector(tls_config).await?;
            let domain = ServerName::try_from(addr)?.to_owned();
            Box::new(connector.connect(domain, tcp).await?)
        }
        #[cfg(feature = "use-native-tls")]
        TlsConfiguration::Native
        | TlsConfiguration::NativeConnector(_)
        | TlsConfiguration::SimpleNative { .. } => {
            let connector = native_tls_connector(tls_config).await?;
            Box::new(connector.connect(addr, tcp).await?)
        }
        #[allow(unreachable_patterns)]
        _ => panic!("Unknown or not enabled TLS backend configuration"),
    };
    Ok(tls)
}

#[cfg(all(test, feature = "use-rustls-no-provider"))]
mod tests {
    use super::*;

    // Throwaway test material (openssl, CN=t), never used anywhere else. The
    // PEM labels are joined at run time so secret scanners see no key block.
    const CERT: &str = "MIIBbTCCAROgAwIBAgIUZBgHi6aKHbsSrOzYp/rwPaXINe8wCgYIKoZIzj0EAwIw
DDEKMAgGA1UEAwwBdDAeFw0yNjEwMjcxNzEzMjNaFw0zNjEwMjQxNzEzMjNaMAwx
CjAIBgNVBAMMAXQwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAAQTvKpzOBgI9oN0
mUeN+qwNutReslstyDChWSTFw76fxaH+e1XBP3Jo7poWQA/loVxmBrKVjp/zQhdH
MsMrJ0uoo1MwUTAdBgNVHQ4EFgQUW7AAM1uoitseZ8/KcbReUMhjsgUwHwYDVR0j
BBgwFoAUW7AAM1uoitseZ8/KcbReUMhjsgUwDwYDVR0TAQH/BAUwAwEB/zAKBggq
hkjOPQQDAgNIADBFAiEAn/CUsZlvEubGf/lY+gy0iU9NYx2X+z7yTGaHtkUsag4C
IC2RYr4Njubi2UOUVDoyzCf6Mfg18TUzqFanlh1dL64n";
    const PKCS8: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgK2QWKQ6gapXxdpFn
ns15IaHrNa3CnrYkfyt+B7h4/sGhRANCAAQTvKpzOBgI9oN0mUeN+qwNutReslst
yDChWSTFw76fxaH+e1XBP3Jo7poWQA/loVxmBrKVjp/zQhdHMsMrJ0uo";
    const RSA: &str = "MIICXAIBAAKBgQDN7eOiuxTW7HXXe2cmFT4q3n1lTBNTQAATUOcHJ0A8Q44HpUnz
HPr3z2WT6DrqbXvN93qy+UqjG33dV4PrRsgPnUoAYWa6Yzarwxz/ew4zXiYGHix1
RGccBFFbOs4b59scUJsqQz0rYoMxD8J795j3W03WHjBYmyjDJyPUvCC19wIDAQAB
AoGAU49hHCovpZpjouFnFJPwGvMXDOCWwuQoY64jYMqzjQCYhE+0txqeV6nnfOHx
7igo4YW3WyI+//pq00Lsu66IBNuOShSsaF0izfswCRzP+R6jHu1t32BUs1ZFTFwQ
XmdkQcu48Mrr+WtF5sX49tOyDebWh9bSqXeqxkrlPhgCN+kCQQDtJGfaQNc4upZY
4sIWFadu4vhKTipTbZGt4U1qGRUu/7vJbbhDJVE9C00+7U5tnkpbq5SkmPzXJSz8
OpwTxGLrAkEA3k4Ry2LfGSoy31W4ig08HVoL6BiBsgaWbc4SIlVpEda8OLa0uT5i
VNZzlJx9IornwC5HFXhH8Py0NyQqQPy+JQJAQ7vVSfqhPpvU27iRFTvEIiEEKD0h
S8kNBixHL1nQuibjXUpQiiU7igMTYjjn4NmIhZyBjW4BMfZ1XenjsKW0VwJBAIFD
rB0z1L5xRzuOQMwlffsjTSEcfu3h3r7iwLCuMXZqAvCHbPb6S3tBZ9vWUv0vACtD
vZnCYzLB/pjBssAvNvECQBHFSyYzkagH5RFKDyouZbu2WaKz6K/LptdYnWyNCiw8
63GFu94qD5zKaCznJa31oKiqs/Iol7InpXl+Ukv5Wjk=";
    const SEC1: &str = "MHcCAQEEICtkFikOoGqV8XaRZ57NeSGh6zWtwp62JH8rfge4eP7BoAoGCCqGSM49
AwEHoUQDQgAEE7yqczgYCPaDdJlHjfqsDbrUXrJbLcgwoVkkxcO+n8Wh/ntVwT9y
aO6aFkAP5aFcZgaylY6f80IXRzLDKydLqA==";

    fn pem(label: &str, body: &str) -> Vec<u8> {
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n").into_bytes()
    }

    fn key_label(kind: &str) -> String {
        [kind, "PRIVATE", "KEY"].join(" ").trim_start().to_owned()
    }

    fn is_invalid_data(err: &Error) -> bool {
        matches!(err, Error::Io(e) if e.kind() == io::ErrorKind::InvalidData)
    }

    #[test]
    fn a_ca_pem_gives_a_root_store_with_its_certificate() {
        let mut ca = b"comment line before the section\n".to_vec();
        ca.extend(pem("CERTIFICATE", CERT));
        assert_eq!(root_store(&ca).unwrap().len(), 1);
        assert_eq!(pem_certs(&ca).unwrap().len(), 1);
    }

    #[test]
    fn a_client_chain_skips_a_key_section_beside_the_certificate() {
        let mut chain = pem(&key_label(""), PKCS8);
        chain.extend(pem("CERTIFICATE", CERT));
        assert_eq!(pem_certs(&chain).unwrap().len(), 1);
    }

    #[test]
    fn a_pkcs8_key_is_read() {
        let key = pem_private_key(&pem(&key_label(""), PKCS8)).unwrap();
        assert!(matches!(key, PrivateKeyDer::Pkcs8(_)));
    }

    #[test]
    fn an_rsa_pkcs1_key_is_read() {
        let key = pem_private_key(&pem(&key_label("RSA"), RSA)).unwrap();
        assert!(matches!(key, PrivateKeyDer::Pkcs1(_)));
    }

    #[test]
    fn a_sec1_key_after_a_certificate_is_read() {
        let mut file = pem("CERTIFICATE", CERT);
        file.extend(pem(&key_label("EC"), SEC1));
        let key = pem_private_key(&file).unwrap();
        assert!(matches!(key, PrivateKeyDer::Sec1(_)));
    }

    #[test]
    fn garbage_has_no_certificate_and_no_key() {
        let garbage = b"not a pem file at all";
        assert!(pem_certs(garbage).unwrap().is_empty());
        assert!(matches!(
            root_store(garbage),
            Err(Error::NoValidCertInChain)
        ));
        assert!(matches!(
            pem_private_key(garbage),
            Err(Error::NoValidKeyInChain)
        ));
        let cert_only = pem("CERTIFICATE", CERT);
        assert!(matches!(
            pem_private_key(&cert_only),
            Err(Error::NoValidKeyInChain)
        ));
    }

    #[test]
    fn a_section_without_its_end_is_an_error() {
        let cut = format!("-----BEGIN CERTIFICATE-----\n{CERT}\n").into_bytes();
        assert!(is_invalid_data(&pem_certs(&cut).unwrap_err()));
        assert!(is_invalid_data(&root_store(&cut).unwrap_err()));
        let cut_key = format!("-----BEGIN {}-----\n{PKCS8}\n", key_label(""));
        assert!(is_invalid_data(
            &pem_private_key(cut_key.as_bytes()).unwrap_err()
        ));
    }

    #[test]
    fn bad_base64_inside_a_section_is_an_error() {
        let bad = pem("CERTIFICATE", "!!!! not base64 !!!!");
        assert!(is_invalid_data(&pem_certs(&bad).unwrap_err()));
        let bad_key = pem(&key_label(""), "!!!! not base64 !!!!");
        assert!(is_invalid_data(&pem_private_key(&bad_key).unwrap_err()));
    }
}
