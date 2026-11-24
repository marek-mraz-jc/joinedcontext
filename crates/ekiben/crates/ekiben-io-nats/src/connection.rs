use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use ekiben_core::{CoreError, OrgContext, SecretString};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NatsAuthConfig {
    pub nkey_file: Option<String>,
    pub user_credentials_file: Option<String>,
    pub user_jwt: Option<SecretString>,
    pub user_nkey_seed: Option<SecretString>,
}

/// One `tls.client_certs` entry: a PEM certificate and key, inline or from files.
#[derive(Clone, Debug)]
pub struct NatsClientCert {
    pub cert: Option<String>,
    pub key: Option<SecretString>,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
}

/// The `tls` block Bento 1.22 takes. The server certificate is always verified:
/// `skip_cert_verify: true` and `enable_renegotiation: true` are refused at build.
#[derive(Clone, Debug, Default)]
pub struct NatsTlsConfig {
    pub enabled: bool,
    pub root_cas: Option<String>,
    pub root_cas_file: Option<String>,
    pub client_certs: Vec<NatsClientCert>,
}

#[derive(Clone)]
pub struct NatsConnectionDetails {
    pub urls: Vec<String>,
    pub auth: NatsAuthConfig,
    pub tls_enabled: bool,
    pub tls: NatsTlsConfig,
    /// The organization whose outbound rule every server address must pass.
    pub org: Arc<OrgContext>,
}

impl std::fmt::Debug for NatsConnectionDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NatsConnectionDetails")
            .field("urls", &self.urls)
            .field("auth", &self.auth)
            .field("tls", &self.tls)
            .field("org", &self.org.org_id)
            .finish()
    }
}

/// Confines a `*_file` option under `file_root` when the host set one (a hosted stream may not read
/// another module's key file).
fn confine(
    path: Option<String>,
    file_root: Option<&std::path::Path>,
) -> Result<Option<String>, CoreError> {
    match (path, file_root) {
        (Some(p), Some(_)) => ekiben_io_file::path_guard::sanitize_file_path(&p, file_root)
            .map(|safe| Some(safe.to_string_lossy().into_owned())),
        (p, _) => Ok(p),
    }
}

fn opt_str(obj: &serde_json::Map<String, JsonValue>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn parse_tls(conf: Option<&JsonValue>) -> Result<NatsTlsConfig, CoreError> {
    let Some(obj) = conf.and_then(JsonValue::as_object) else {
        return Ok(NatsTlsConfig::default());
    };
    let flag = |k: &str| obj.get(k).and_then(JsonValue::as_bool).unwrap_or(false);
    if flag("skip_cert_verify") {
        return Err(CoreError::config(
            "tls.skip_cert_verify: true is refused: ekiben always verifies the NATS server certificate; give its CA in tls.root_cas or tls.root_cas_file",
        ));
    }
    if flag("enable_renegotiation") {
        return Err(CoreError::config(
            "tls.enable_renegotiation: true is refused: TLS renegotiation is not supported",
        ));
    }
    let mut client_certs = Vec::new();
    for (i, c) in obj
        .get("client_certs")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let c = c
            .as_object()
            .ok_or_else(|| CoreError::config(format!("tls.client_certs[{i}] must be an object")))?;
        if opt_str(c, "password").is_some() {
            return Err(CoreError::config(format!(
                "tls.client_certs[{i}].password: encrypted keys are not supported, give an unencrypted PKCS#8 key from a secret file"
            )));
        }
        let cert = NatsClientCert {
            cert: opt_str(c, "cert"),
            key: opt_str(c, "key").map(SecretString::new),
            cert_file: opt_str(c, "cert_file"),
            key_file: opt_str(c, "key_file"),
        };
        let has_cert = cert.cert.is_some() ^ cert.cert_file.is_some();
        let has_key = cert.key.is_some() ^ cert.key_file.is_some();
        if !has_cert || !has_key {
            return Err(CoreError::config(format!(
                "tls.client_certs[{i}] needs exactly one of cert or cert_file and one of key or key_file"
            )));
        }
        client_certs.push(cert);
    }
    Ok(NatsTlsConfig {
        enabled: flag("enabled"),
        root_cas: opt_str(obj, "root_cas"),
        root_cas_file: opt_str(obj, "root_cas_file"),
        client_certs,
    })
}

fn read_pem(inline: Option<&str>, file: Option<&str>, what: &str) -> Result<Vec<u8>, CoreError> {
    match (inline, file) {
        (Some(s), _) => Ok(s.as_bytes().to_vec()),
        (None, Some(f)) => std::fs::read(f)
            .map_err(|e| CoreError::config(format!("failed to read {what} '{f}': {e}"))),
        (None, None) => Ok(Vec::new()),
    }
}

impl NatsTlsConfig {
    /// A rustls client config verifying the server against `root_cas`/`root_cas_file`, or the
    /// system roots when neither is set.
    pub fn client_config(&self) -> Result<async_nats::rustls::ClientConfig, CoreError> {
        use async_nats::rustls;
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let mut roots = rustls::RootCertStore::empty();
        let mut custom = read_pem(self.root_cas.as_deref(), None, "tls.root_cas")?;
        custom.extend(read_pem(
            None,
            self.root_cas_file.as_deref(),
            "tls.root_cas_file",
        )?);
        if custom.is_empty() {
            let native = rustls_native_certs::load_native_certs();
            roots.add_parsable_certificates(native.certs);
        } else {
            for cert in CertificateDer::pem_slice_iter(&custom) {
                let cert = cert
                    .map_err(|e| CoreError::config(format!("tls root CA is not valid PEM: {e}")))?;
                roots
                    .add(cert)
                    .map_err(|e| CoreError::config(format!("tls root CA refused: {e}")))?;
            }
        }
        if roots.is_empty() {
            return Err(CoreError::config(
                "tls has no root CA: set tls.root_cas or tls.root_cas_file",
            ));
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| CoreError::config(format!("tls: {e}")))?
            .with_root_certificates(roots);
        let Some(cc) = self.client_certs.first() else {
            return Ok(builder.with_no_client_auth());
        };
        let cert_pem = read_pem(
            cc.cert.as_deref(),
            cc.cert_file.as_deref(),
            "tls client cert",
        )?;
        let key_pem = read_pem(
            cc.key.as_ref().map(SecretString::expose),
            cc.key_file.as_deref(),
            "tls client key",
        )?;
        let chain = CertificateDer::pem_slice_iter(&cert_pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreError::config(format!("tls client cert is not valid PEM: {e}")))?;
        let key = PrivateKeyDer::from_pem_slice(&key_pem)
            .map_err(|_| CoreError::config("tls client key is not a valid PEM private key"))?;
        builder
            .with_client_auth_cert(chain, key)
            .map_err(|e| CoreError::config(format!("tls client certificate refused: {e}")))
    }
}

impl NatsConnectionDetails {
    /// Parses the connection block of a NATS component for `org`; every `*_file` option is
    /// confined under `file_root` when one is set.
    pub fn from_json(
        conf: &JsonValue,
        org: Arc<OrgContext>,
        file_root: Option<&std::path::Path>,
    ) -> Result<Self, CoreError> {
        let obj = conf
            .as_object()
            .ok_or_else(|| CoreError::config("nats config must be an object"))?;

        let mut urls = Vec::new();
        if let Some(arr) = obj.get("urls").and_then(JsonValue::as_array) {
            for item in arr.iter().filter_map(JsonValue::as_str) {
                for part in item.split(',') {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        urls.push(trimmed.to_string());
                    }
                }
            }
        } else if let Some(s) = obj.get("urls").and_then(JsonValue::as_str) {
            for part in s.split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    urls.push(trimmed.to_string());
                }
            }
        }
        if urls.is_empty() {
            return Err(CoreError::config(
                "nats requires at least one URL in 'urls'",
            ));
        }

        let mut auth = NatsAuthConfig::default();
        if let Some(auth_obj) = obj.get("auth").and_then(JsonValue::as_object) {
            auth.nkey_file = auth_obj
                .get("nkey_file")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
            auth.user_credentials_file = auth_obj
                .get("user_credentials_file")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
            let jwt = auth_obj
                .get("user_jwt")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty());
            let seed = auth_obj
                .get("user_nkey_seed")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty());
            match (jwt, seed) {
                (Some(_), None) => {
                    return Err(CoreError::config(
                        "missing auth.user_nkey_seed config field",
                    ))
                }
                (None, Some(_)) => {
                    return Err(CoreError::config("missing auth.user_jwt config field"))
                }
                (Some(j), Some(s)) => {
                    // Validate NKey seed syntax early
                    nkeys::KeyPair::from_seed(s).map_err(|e| {
                        CoreError::config(format!("invalid NATS auth.user_nkey_seed: {e}"))
                    })?;
                    auth.user_jwt = Some(SecretString::new(j));
                    auth.user_nkey_seed = Some(SecretString::new(s));
                }
                (None, None) => {}
            }
        }

        auth.nkey_file = confine(auth.nkey_file, file_root)?;
        auth.user_credentials_file = confine(auth.user_credentials_file, file_root)?;
        let mut tls = parse_tls(obj.get("tls"))?;
        tls.root_cas_file = confine(tls.root_cas_file, file_root)?;
        for cc in &mut tls.client_certs {
            cc.cert_file = confine(cc.cert_file.take(), file_root)?;
            cc.key_file = confine(cc.key_file.take(), file_root)?;
        }

        Ok(Self {
            urls,
            auth,
            tls_enabled: tls.enabled,
            tls,
            org,
        })
    }

    pub fn is_in_memory(&self) -> bool {
        self.urls
            .first()
            .is_some_and(|u| u.starts_with("nats://memory") || u.starts_with("nats://inproc"))
    }

    /// The key of the in-memory cluster a `nats://memory*` stream uses: one per organization
    /// and URL, so two organizations naming the same URL never share subjects, streams or
    /// buckets (`from_json` refuses a config without a URL).
    pub fn cluster_name(&self) -> String {
        let url = self.urls.first().map_or("", String::as_str);
        format!("{}\n{url}", self.org.org_id.as_str())
    }

    /// The server addresses to dial, each passed through the organization's outbound rule. Without
    /// TLS the dial goes to the checked IP; with TLS it keeps the name for certificate checks.
    // ponytail: a TLS name is resolved twice (here and by async-nats), a DNS rebinding window
    // remains for TLS servers; close it with a custom connector once async-nats takes one.
    pub async fn checked_server_addrs(&self) -> Result<Vec<String>, CoreError> {
        let mut out = Vec::new();
        for raw in &self.urls {
            let with_scheme = if raw.contains("://") {
                raw.clone()
            } else {
                format!("nats://{raw}")
            };
            let mut url = url::Url::parse(&with_scheme)
                .map_err(|e| CoreError::config(format!("invalid NATS URL: {e}")))?;
            if !matches!(url.scheme(), "nats" | "tls") {
                return Err(CoreError::config(format!(
                    "refused NATS URL scheme '{}': only nats and tls are allowed",
                    url.scheme()
                )));
            }
            let host = url
                .host_str()
                .ok_or_else(|| CoreError::config("NATS URL has no host"))?
                .to_string();
            let port = url.port().unwrap_or(4222);
            let addrs =
                ekiben_io_http::outbound::resolve_host_checked(&host, port, &self.org).await?;
            if self.tls.enabled || url.scheme() == "tls" {
                out.push(with_scheme);
                continue;
            }
            for a in addrs {
                url.set_ip_host(a.ip())
                    .map_err(|()| CoreError::config("NATS URL cannot take an IP host"))?;
                url.set_port(Some(a.port()))
                    .map_err(|()| CoreError::config("NATS URL cannot take a port"))?;
                out.push(url.to_string());
            }
        }
        Ok(out)
    }

    pub async fn connect_live(&self, label: &str) -> Result<async_nats::Client, CoreError> {
        let mut opts = async_nats::ConnectOptions::new().name(label);
        if let (Some(jwt), Some(seed)) = (&self.auth.user_jwt, &self.auth.user_nkey_seed) {
            let jwt_str = jwt.expose().to_string();
            let kp = Arc::new(
                nkeys::KeyPair::from_seed(seed.expose())
                    .map_err(|e| CoreError::config(format!("nkey error: {e}")))?,
            );
            opts = opts.jwt(jwt_str, move |nonce| {
                let kp = kp.clone();
                async move { kp.sign(&nonce).map_err(async_nats::AuthError::new) }
            });
        } else if let Some(ref creds_file) = self.auth.user_credentials_file {
            opts = opts
                .credentials_file(creds_file)
                .await
                .map_err(|e| CoreError::config(format!("credentials_file error: {e}")))?;
        } else if let Some(ref nkey_file) = self.auth.nkey_file {
            let seed = std::fs::read_to_string(nkey_file)
                .map_err(|e| CoreError::config(format!("failed to read nkey_file: {e}")))?;
            opts = opts.nkey(seed.trim().to_string());
        }

        if self.tls.enabled {
            opts = opts
                .require_tls(true)
                .tls_client_config(self.tls.client_config()?);
        }

        let server_addrs = self.checked_server_addrs().await?;
        // A server's INFO may advertise other addresses (connect_urls): those never passed the check.
        opts = opts.ignore_discovered_servers();
        async_nats::connect_with_options(server_addrs.as_slice(), opts)
            .await
            .map_err(|e| CoreError::other(format!("failed to connect to NATS: {e}")))
    }
}

use std::sync::Arc;

/// Checks if a NATS subject pattern (supporting `*` and `>`) matches a concrete message subject.
pub fn subject_matches(pattern: &str, subject: &str) -> bool {
    if pattern == ">" || pattern == subject {
        return true;
    }
    let pat_tokens: Vec<&str> = pattern.split('.').collect();
    let sub_tokens: Vec<&str> = subject.split('.').collect();
    for (i, &pt) in pat_tokens.iter().enumerate() {
        if pt == ">" {
            // `>` stands for one or more tokens: `a.>` matches `a.b`, never `a`.
            return i < sub_tokens.len();
        }
        let Some(&st) = sub_tokens.get(i) else {
            return false;
        };
        if pt != "*" && pt != st {
            return false;
        }
    }
    pat_tokens.len() == sub_tokens.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgId, OrgQuotas};

    fn org() -> Arc<OrgContext> {
        OrgContext::new(OrgId::new("t"), OrgQuotas::default())
    }

    fn tls(v: serde_json::Value) -> Result<NatsConnectionDetails, CoreError> {
        NatsConnectionDetails::from_json(
            &serde_json::json!({"urls": ["nats://h:4222"], "tls": v}),
            org(),
            None,
        )
    }

    #[test]
    fn tls_refuses_what_would_skip_verification_and_half_client_certs() {
        let msg = |v| tls(v).unwrap_err().to_string();
        assert!(
            msg(serde_json::json!({"enabled": true, "skip_cert_verify": true}))
                .contains("skip_cert_verify")
        );
        assert!(msg(serde_json::json!({"enable_renegotiation": true})).contains("renegotiation"));
        assert!(
            msg(serde_json::json!({"client_certs": [{"cert_file": "c.pem"}]}))
                .contains("client_certs[0]")
        );
        assert!(msg(
            serde_json::json!({"client_certs": [{"cert": "x", "cert_file": "c", "key": "k"}]})
        )
        .contains("exactly one of cert or cert_file"));
        assert!(msg(
            serde_json::json!({"client_certs": [{"cert": "x", "key": "k", "password": "p"}]})
        )
        .contains("password"));
        let ok = tls(serde_json::json!({"enabled": true, "root_cas_file": "/run/secrets/ca.pem",
            "client_certs": [{"cert_file": "/run/secrets/c.pem", "key_file": "/run/secrets/k.pem"}]}))
        .unwrap();
        assert!(ok.tls_enabled);
        assert_eq!(ok.tls.client_certs.len(), 1);
        assert!(
            tls(serde_json::json!({"enabled": true, "root_cas": "not pem"}))
                .unwrap()
                .tls
                .client_config()
                .is_err()
        );
    }

    fn conn(v: serde_json::Value, root: Option<&std::path::Path>) -> NatsConnectionDetails {
        NatsConnectionDetails::from_json(&v, org(), root).unwrap()
    }

    #[tokio::test]
    async fn server_addresses_pass_the_outbound_rule() {
        for refused in [
            "nats://127.0.0.1:4222",
            "nats://10.43.0.10:4222",
            "169.254.169.254",
            "nats://localhost:4222",
            "tls://[::1]:4222",
            "http://8.8.8.8:4222",
        ] {
            let c = conn(serde_json::json!({"urls": [refused]}), None);
            assert!(
                c.checked_server_addrs().await.is_err(),
                "{refused} must be refused"
            );
        }
        let c = conn(serde_json::json!({"urls": ["nats://u:p@8.8.8.8"]}), None);
        assert_eq!(
            c.checked_server_addrs().await.unwrap(),
            ["nats://u:p@8.8.8.8:4222"]
        );
    }

    #[test]
    fn file_options_stay_under_the_file_root() {
        let root = std::path::Path::new("/srv/ekiben-root");
        let c = conn(
            serde_json::json!({"urls": ["nats://8.8.8.8"],
                "auth": {"user_credentials_file": "/run/secrets/portal.creds", "nkey_file": "k.nk"},
                "tls": {"root_cas_file": "/etc/ssl/ca.pem",
                    "client_certs": [{"cert_file": "/c.pem", "key_file": "/k.pem"}]}}),
            Some(root),
        );
        assert_eq!(
            c.auth.user_credentials_file.as_deref(),
            Some("/srv/ekiben-root/run/secrets/portal.creds")
        );
        assert_eq!(c.auth.nkey_file.as_deref(), Some("/srv/ekiben-root/k.nk"));
        assert_eq!(
            c.tls.root_cas_file.as_deref(),
            Some("/srv/ekiben-root/etc/ssl/ca.pem")
        );
        assert_eq!(
            c.tls.client_certs[0].key_file.as_deref(),
            Some("/srv/ekiben-root/k.pem")
        );
        assert!(NatsConnectionDetails::from_json(
            &serde_json::json!({"urls": ["nats://8.8.8.8"], "auth": {"nkey_file": "../../proc/self/environ"}}),
            org(),
            Some(root),
        )
        .is_err());
        // Without a root (the standalone binary) the path is the operator's own.
        let free = conn(
            serde_json::json!({"urls": ["nats://8.8.8.8"], "auth": {"nkey_file": "/run/k.nk"}}),
            None,
        );
        assert_eq!(free.auth.nkey_file.as_deref(), Some("/run/k.nk"));
    }

    #[test]
    fn debug_output_names_the_org_and_never_a_secret() {
        let seed = nkeys::KeyPair::new_user().seed().unwrap();
        let c = conn(
            serde_json::json!({"urls": "nats://h:4222", "auth": {"user_jwt": "jwt-text", "user_nkey_seed": seed}}),
            None,
        );
        let text = format!("{c:?}");
        assert!(text.contains("NatsConnectionDetails") && text.contains("\"t\""), "{text}");
        assert!(!text.contains(&seed) && !text.contains("jwt-text"), "{text}");
    }

    #[test]
    fn urls_split_on_commas_and_an_empty_list_is_refused() {
        let c = conn(serde_json::json!({"urls": " nats://a:1, ,nats://b:2 "}), None);
        assert_eq!(c.urls, ["nats://a:1", "nats://b:2"]);
        let c = conn(serde_json::json!({"urls": ["nats://a:1", "  "]}), None);
        assert_eq!(c.urls, ["nats://a:1"]);
        for bad in [
            serde_json::json!({"urls": []}),
            serde_json::json!({"urls": " , "}),
            serde_json::json!({}),
        ] {
            let err = NatsConnectionDetails::from_json(&bad, org(), None).unwrap_err();
            assert!(err.to_string().contains("at least one URL"), "{bad}");
        }
        let err = NatsConnectionDetails::from_json(&serde_json::json!("nats://a"), org(), None).unwrap_err();
        assert!(err.to_string().contains("must be an object"));
    }

    #[test]
    fn jwt_auth_needs_both_halves_and_a_valid_seed() {
        let err = |auth: serde_json::Value| {
            NatsConnectionDetails::from_json(&serde_json::json!({"urls": ["nats://h"], "auth": auth}), org(), None)
                .unwrap_err()
                .to_string()
        };
        assert!(err(serde_json::json!({"user_jwt": "j"})).contains("missing auth.user_nkey_seed"));
        assert!(err(serde_json::json!({"user_jwt": "j", "user_nkey_seed": ""})).contains("missing auth.user_nkey_seed"));
        assert!(err(serde_json::json!({"user_nkey_seed": "S"})).contains("missing auth.user_jwt"));
        assert!(err(serde_json::json!({"user_jwt": "", "user_nkey_seed": "S"})).contains("missing auth.user_jwt"));
        assert!(err(serde_json::json!({"user_jwt": "j", "user_nkey_seed": "not-a-seed"})).contains("invalid NATS auth.user_nkey_seed"));
        let seed = nkeys::KeyPair::new_user().seed().unwrap();
        let ok = conn(serde_json::json!({"urls": ["nats://h"], "auth": {"user_jwt": "j", "user_nkey_seed": seed}}), None);
        assert_eq!(ok.auth.user_jwt.as_ref().map(SecretString::expose), Some("j"));
    }

    #[test]
    fn memory_clusters_are_kept_apart_per_organization() {
        let mk = |org_id: &str| {
            NatsConnectionDetails::from_json(
                &serde_json::json!({"urls": ["nats://memory/shared"]}),
                OrgContext::new(OrgId::new(org_id), OrgQuotas::default()),
                None,
            )
            .unwrap()
        };
        let (a, b) = (mk("org-a"), mk("org-b"));
        assert!(a.is_in_memory());
        assert_ne!(a.cluster_name(), b.cluster_name());
        assert_eq!(a.cluster_name(), mk("org-a").cluster_name());
    }

    #[test]
    fn subjects_match_nats_wildcards() {
        assert!(subject_matches(">", "a.b"));
        assert!(subject_matches("a.b", "a.b"));
        assert!(subject_matches("a.*", "a.b"));
        assert!(subject_matches("a.>", "a.b.c"));
        // `>` needs at least one token.
        assert!(!subject_matches("a.>", "a"));
        assert!(!subject_matches("a.*", "a"));
        assert!(!subject_matches("a.b", "a.c"));
        assert!(!subject_matches("a.*", "a.b.c"));
    }

    #[test]
    fn tls_reports_unreadable_and_invalid_certificates() {
        let msg = |v: serde_json::Value| tls(v).and_then(|c| c.tls.client_config().map(|_| ())).unwrap_err().to_string();
        assert!(msg(serde_json::json!({"enabled": true, "root_cas_file": "/nonexistent/ca.pem"})).contains("failed to read tls.root_cas_file"));
        assert!(msg(serde_json::json!({"client_certs": ["not an object"]})).contains("tls.client_certs[0] must be an object"));
        let garbage = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        assert!(msg(serde_json::json!({"enabled": true, "root_cas": garbage})).contains("tls root CA refused"));
        let broken = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        assert!(msg(serde_json::json!({"enabled": true, "root_cas": broken})).contains("tls root CA is not valid PEM"));
        // Without a custom CA the platform's roots are used (or an error says none were found).
        match tls(serde_json::json!({"enabled": true})).unwrap().tls.client_config() {
            Ok(_) => {}
            Err(e) => assert!(e.to_string().contains("root"), "{e}"),
        }
    }

    #[test]
    fn tls_client_certificates_load_from_inline_pem_or_refuse_clearly() {
        let Some((cert, key)) = self_signed() else {
            eprintln!("SKIP tls_client_certificates: openssl is not available");
            return;
        };
        let client = |c: &str, k: &str| {
            tls(serde_json::json!({"enabled": true, "root_cas": cert, "client_certs": [{"cert": c, "key": k}]}))
                .unwrap()
                .tls
                .client_config()
        };
        assert!(client(&cert, &key).is_ok());
        let err = |r: Result<async_nats::rustls::ClientConfig, CoreError>| r.unwrap_err().to_string();
        assert!(err(client("-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----\n", &key)).contains("tls client cert is not valid PEM"));
        assert!(err(client(&cert, "not a key")).contains("tls client key is not a valid PEM private key"));
        // A key that does not belong to the certificate is refused by rustls.
        let (_, other_key) = self_signed().unwrap();
        assert!(err(client(&cert, &other_key)).contains("tls client certificate refused"));
        let missing = tls(serde_json::json!({"enabled": true, "root_cas": cert,
            "client_certs": [{"cert_file": "/nonexistent/c.pem", "key_file": "/nonexistent/k.pem"}]}))
        .unwrap()
        .tls
        .client_config();
        assert!(err(missing).contains("failed to read tls client cert"));
    }

    /// A self-signed certificate and its PKCS#8 key, made by the openssl CLI.
    fn self_signed() -> Option<(String, String)> {
        let dir = std::env::temp_dir().join(format!("ekiben-nats-tls-{}-{}", std::process::id(), uuid_like()));
        std::fs::create_dir_all(&dir).ok()?;
        let (c, k) = (dir.join("c.pem"), dir.join("k.pem"));
        let ok = std::process::Command::new("openssl")
            .args(["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "1", "-subj", "/CN=localhost", "-keyout"])
            .arg(&k)
            .arg("-out")
            .arg(&c)
            .output()
            .ok()?
            .status
            .success();
        let pair = ok.then(|| Some((std::fs::read_to_string(&c).ok()?, std::fs::read_to_string(&k).ok()?))).flatten();
        let _ = std::fs::remove_dir_all(&dir);
        pair
    }

    fn uuid_like() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    #[tokio::test]
    async fn unreadable_auth_files_and_bad_urls_fail_before_dialing() {
        let creds = conn(serde_json::json!({"urls": ["nats://8.8.8.8"], "auth": {"user_credentials_file": "/nonexistent/x.creds"}}), None);
        assert!(creds.connect_live("t").await.unwrap_err().to_string().contains("credentials_file error"));
        let nkey = conn(serde_json::json!({"urls": ["nats://8.8.8.8"], "auth": {"nkey_file": "/nonexistent/x.nk"}}), None);
        assert!(nkey.connect_live("t").await.unwrap_err().to_string().contains("failed to read nkey_file"));
        let bad = conn(serde_json::json!({"urls": ["nats://[::1"]}), None);
        assert!(bad.checked_server_addrs().await.unwrap_err().to_string().contains("invalid NATS URL"));
    }
}
