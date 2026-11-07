//! Per-organization credentials for the artifact store (PF-29, PF-31, PF-32, ADR-N-015).
//!
//! The store ships with one generated root credential in the Secret `artifact-store-root`,
//! because a Helm chart cannot mint a scoped one. Sharing that key with every writer and reader
//! would make one organization's credential a key to every other organization's artifacts, so
//! the reconciler — the only component that holds the root credential — mints a pair per
//! organization instead: a writer that may `PutObject` under that organization's five prefixes
//! and nothing else, and a reader that may `GetObject` there and nothing else (PF-32).
//!
//! **Nothing is stored.** The Portal has no secret store to write to (`sync::remote` refuses a
//! `secretRef` for the same reason, and OpenBao is T-0423), so the secret key is *derived* from
//! the root secret with HMAC-SHA256. The same root secret and organization always give the same
//! pair: the reconciler can re-issue or verify a credential at any time without keeping one,
//! rotating the root secret rotates every organization's credential at once, and a credential
//! never lands in Git, a log, an image or a task file (CC-06).

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// The prefixes one organization owns, from Architecture/17 §2. Every key in the bucket starts
/// with one of these and the organization's name, which is what makes a per-organization policy
/// expressible at all (PF-31). `endpoints/` and `filecache/` are keyed by endpoint slug rather
/// than by organization and belong to no organization's credential.
pub const OWNED_PREFIXES: [&str; 5] = ["schemas", "mappings", "dumps", "exports", "apps"];

/// What a credential may do. There are two roles and there is no third: a writer that publishes
/// and a reader that serves (Architecture/17 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `jcctl` and CI, publishing rendered artifacts.
    Writer,
    /// The Context Gateway and the Portal API, serving them.
    Reader,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Writer => "writer",
            Role::Reader => "reader",
        }
    }

    /// Both roles, in the order they are created: the policy a writer needs exists before the
    /// user that carries it.
    pub const BOTH: [Role; 2] = [Role::Writer, Role::Reader];
}

/// One S3 credential. `Debug` prints no secret: this type travels through error paths and logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub access_key: String,
    pub secret_key: String,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("access_key", &self.access_key)
            .field("secret_key", &"[redacted]")
            .finish()
    }
}

impl Credential {
    /// The pair an organization's role gets, derived and not drawn: see the module comment for
    /// why nothing is stored. The context string is versioned so a future derivation can change
    /// without silently handing out the old keys under the new rule.
    pub fn derive(root_secret: &str, org: &str, role: Role) -> Self {
        let mut mac = HmacSha256::new_from_slice(root_secret.as_bytes())
            .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
        mac.update(
            format!(
                "joinedcontext/artifact-store/v1/{org}/{role}",
                role = role.as_str()
            )
            .as_bytes(),
        );
        let digest = mac.finalize().into_bytes();
        let mut secret_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        // 40 characters is the length of an AWS secret key, which is what every S3 client and
        // every operator expects to see; the remaining bits of the digest add nothing.
        secret_key.truncate(40);
        Self {
            access_key: name(org, role),
            secret_key,
        }
    }
}

impl Credential {
    /// The pair a WASM shard gets (AP-158), derived like an organization's, under its own
    /// versioned context so no shard's key can equal an organization's.
    pub fn derive_shard(root_secret: &str, shard: u32) -> Self {
        let mut mac = HmacSha256::new_from_slice(root_secret.as_bytes())
            .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
        mac.update(format!("joinedcontext/apps-store/v1/shard/{shard}").as_bytes());
        let mut secret_key =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        secret_key.truncate(40);
        Self {
            access_key: shard_name(shard),
            secret_key,
        }
    }
}

/// The user and policy name of a shard's key, `jc-apps-shard-<shard>`.
pub fn shard_name(shard: u32) -> String {
    format!("jc-apps-shard-{shard}")
}

/// What one shard's key may do in the Apps bucket (AP-158, ADR-N-044 §2.5): read, write and
/// delete its own Apps' objects under `apps/<shard>/`, list that prefix and no other, and read
/// the components every shard loads. Not a word about another shard's prefix: the store itself
/// refuses it.
pub fn shard_policy_document(bucket: &str, shard: u32) -> serde_json::Value {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
                "Resource": [format!("arn:aws:s3:::{bucket}/apps/{shard}/*")],
            },
            {
                "Effect": "Allow",
                "Action": ["s3:ListBucket"],
                "Resource": [format!("arn:aws:s3:::{bucket}")],
                "Condition": { "StringLike": { "s3:prefix": [format!("apps/{shard}/*")] } },
            },
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject"],
                "Resource": [format!("arn:aws:s3:::{bucket}/components/*")],
            },
        ],
    })
}

/// The Secret a shard mounts its key from, `apps-host-<shard>-store` (AP-158); the host reads
/// each value from a file (`JC_WASM_S3_KEY_FILE`, `JC_WASM_S3_SECRET_FILE`), never its environment.
pub fn shard_secret(namespace: &str, shard: u32, key: &Credential) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": format!("apps-host-{shard}-store"),
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "joinedcontext.com/wasm-shard": shard.to_string(),
            },
        },
        "type": "Opaque",
        "stringData": { "access-key": key.access_key, "secret-key": key.secret_key },
    })
}

/// The name of the user and of the policy that carries its rights. One name for both: a user
/// and a canned policy live in different namespaces in the store, and reading `jc-hel-reader`
/// in either place should name the same thing.
pub fn name(org: &str, role: Role) -> String {
    format!("jc-{org}-{role}", role = role.as_str())
}

/// The policy one role of one organization gets, in the AWS shape the store parses.
///
/// The resources are the organization's own five prefixes and nothing else. A policy naming
/// `arn:aws:s3:::{bucket}/*` would hand this organization every other organization's artifacts,
/// which is the one thing this module exists to prevent (PF-31, PF-32). The writer gets no
/// delete action, because a published artifact is object-locked and is replaced by a new key,
/// never overwritten (PF-33); the reader gets no write action at all.
pub fn policy_document(bucket: &str, org: &str, role: Role) -> serde_json::Value {
    let actions: &[&str] = match role {
        // `s3:PutObject` covers the multipart upload of a large dump; aborting an upload the
        // writer itself started is the one cleanup it needs.
        Role::Writer => &["s3:PutObject", "s3:AbortMultipartUpload"],
        Role::Reader => &["s3:GetObject"],
    };
    let resources: Vec<serde_json::Value> = OWNED_PREFIXES
        .iter()
        .map(|prefix| serde_json::json!(format!("arn:aws:s3:::{bucket}/{prefix}/{org}/*")))
        .collect();
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": resources,
        }],
    })
}

/// Where the store is and how to sign for it. From the deployment, never from a manifest: which
/// store answers is an installation's setting, and a manifest carries names, not endpoints.
#[derive(Clone)]
pub struct Settings {
    /// Base URL inside the mesh, e.g. `http://artifact-store.jc-system.svc.cluster.local:9000`.
    pub endpoint: String,
    pub bucket: String,
    /// The store ignores the region; SigV4 does not, and it has to match what the signer used.
    pub region: String,
    pub root_access_key: String,
    pub root_secret_key: String,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("root_access_key", &self.root_access_key)
            .field("root_secret_key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the artifact store endpoint is not a usable URL: {0}")]
    Endpoint(String),
    /// The body is deliberately not carried: an admin response can echo the request, and the
    /// request carries a secret key.
    #[error("the artifact store refused {operation} with HTTP {status}")]
    Refused {
        operation: &'static str,
        status: u16,
    },
    #[error("{operation} did not reach the artifact store: {source}")]
    Transport {
        operation: &'static str,
        #[source]
        source: reqwest::Error,
    },
}

/// The reconciler's client for the store's admin API.
pub struct Client {
    settings: Settings,
    http: reqwest::Client,
    host: String,
}

impl Client {
    pub fn new(settings: Settings) -> Result<Self, Error> {
        let url: url::Url = settings
            .endpoint
            .parse()
            .map_err(|e: url::ParseError| Error::Endpoint(e.to_string()))?;
        let host = match url.port() {
            Some(port) => format!(
                "{}:{port}",
                url.host_str()
                    .ok_or_else(|| Error::Endpoint("the endpoint names no host".to_owned()))?
            ),
            None => url
                .host_str()
                .ok_or_else(|| Error::Endpoint("the endpoint names no host".to_owned()))?
                .to_owned(),
        };
        // A store that accepts and never answers costs provisioning an error, not the reconciler
        // loop it runs in (T-1716).
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Endpoint(e.to_string()))?;
        Ok(Self {
            settings,
            http,
            host,
        })
    }

    /// The credential a consumer would be handed, computed without touching the store.
    pub fn credential(&self, org: &str, role: Role) -> Credential {
        Credential::derive(&self.settings.root_secret_key, org, role)
    }

    /// Makes both policies and both users of one organization exist, and returns the pair in the
    /// order of [`Role::BOTH`].
    ///
    /// Idempotent: `add-canned-policy` and `add-user` are upserts and the derivation is
    /// deterministic, so a second run writes the same values and changes nothing. That is what
    /// lets the reconciler call it on every sync without keeping a record of what it did.
    pub async fn ensure_organization(&self, org: &str) -> Result<Vec<Credential>, Error> {
        let mut issued = Vec::with_capacity(Role::BOTH.len());
        for role in Role::BOTH {
            let credential = self.credential(org, role);
            self.ensure_user(
                &name(org, role),
                &policy_document(&self.settings.bucket, org, role),
                &credential,
            )
            .await?;
            issued.push(credential);
        }
        Ok(issued)
    }

    /// Makes one WASM shard's policy and user exist in the Apps bucket `bucket`, and returns its
    /// key (AP-158). Idempotent, as [`Client::ensure_organization`] is.
    pub async fn ensure_shard(&self, bucket: &str, shard: u32) -> Result<Credential, Error> {
        let credential = Credential::derive_shard(&self.settings.root_secret_key, shard);
        self.ensure_user(
            &shard_name(shard),
            &shard_policy_document(bucket, shard),
            &credential,
        )
        .await?;
        Ok(credential)
    }

    /// The canned policy `policy`, the user of `credential`, and the one attached to the other.
    async fn ensure_user(
        &self,
        policy: &str,
        document: &serde_json::Value,
        credential: &Credential,
    ) -> Result<(), Error> {
        self.put(
            "/rustfs/admin/v3/add-canned-policy",
            &[("name", policy.to_owned())],
            serde_json::to_vec(document).unwrap_or_default(),
            "add-canned-policy",
        )
        .await?;
        self.put(
            "/rustfs/admin/v3/add-user",
            &[("accessKey", credential.access_key.clone())],
            serde_json::to_vec(&serde_json::json!({
                "secretKey": credential.secret_key,
                "status": "enabled",
            }))
            .unwrap_or_default(),
            "add-user",
        )
        .await?;
        self.put(
            "/rustfs/admin/v3/set-user-or-group-policy",
            &[
                ("policyName", policy.to_owned()),
                ("userOrGroup", credential.access_key.clone()),
                ("isGroup", "false".to_owned()),
            ],
            Vec::new(),
            "set-user-or-group-policy",
        )
        .await
    }

    /// One signed admin request. The body is plain JSON: the store decrypts a body only on the
    /// MinIO-compatible `/minio/admin/...` prefix, so nothing here needs madmin's envelope.
    async fn put(
        &self,
        path: &str,
        query: &[(&str, String)],
        body: Vec<u8>,
        operation: &'static str,
    ) -> Result<(), Error> {
        let now = chrono::Utc::now();
        let headers = self.sign("PUT", path, query, &body, now);

        let query_string = canonical_query(query);
        let url = format!(
            "{}{path}?{query_string}",
            self.settings.endpoint.trim_end_matches('/')
        );
        let mut request = self.http.put(&url).body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|source| Error::Transport { operation, source })?;
        if !response.status().is_success() {
            return Err(Error::Refused {
                operation,
                status: response.status().as_u16(),
            });
        }
        Ok(())
    }

    /// The four headers a signed request carries. `Host` is signed but sent by the HTTP client
    /// itself, so it is not returned here.
    fn sign(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: &[u8],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<(&'static str, String)> {
        self.sign_with(method, path, query, body, &[], now)
    }

    /// [`Client::sign`] with `extra` headers signed too, lower-case names (S3 refuses an
    /// unsigned `x-amz-*` header such as `x-amz-copy-source`).
    fn sign_with(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: &[u8],
        extra: &[(&str, String)],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<(&'static str, String)> {
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let payload_hash = hex(&Sha256::digest(body));

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in extra {
            headers.insert((*name).to_owned(), value.clone());
        }
        headers.insert("host".to_owned(), self.host.clone());
        headers.insert("x-amz-content-sha256".to_owned(), payload_hash.clone());
        headers.insert("x-amz-date".to_owned(), amz_date.clone());

        let request = canonical_request(
            method,
            path,
            &canonical_query(query),
            &headers,
            &payload_hash,
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.settings.region);
        let to_sign = string_to_sign(&amz_date, &scope, &request);
        let key = signing_key(
            &self.settings.root_secret_key,
            &date,
            &self.settings.region,
            "s3",
        );
        let signature = hex(&hmac(&key, to_sign.as_bytes()));

        vec![
            (
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
                    self.settings.root_access_key,
                    headers.keys().cloned().collect::<Vec<_>>().join(";"),
                ),
            ),
            ("x-amz-content-sha256", payload_hash),
            ("x-amz-date", amz_date),
        ]
    }
}

impl Client {
    /// A URL that lets its holder do `method` on `key` of `bucket` alone for `expires` seconds,
    /// signed for `origin`, the address a browser reaches the store at, which is the `Host` it
    /// sends (AP-145). With a `length`, the URL signs `Content-Length` too, so the store refuses
    /// a body of any other length (AP-170).
    pub fn presign(
        &self,
        (method, bucket, key): (&str, &str, &str),
        length: Option<u64>,
        expires: u32,
        origin: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> String {
        presigned(
            &self.settings,
            (method, &Self::object_path(bucket, key), length),
            expires,
            origin,
            now,
        )
    }
}

/// [`Client::presign`] of one encoded path (SigV4 § "Authenticating Requests: Using Query
/// Parameters"): `host` is signed, with `content-length` when a length is given, and the payload
/// is `UNSIGNED-PAYLOAD`.
fn presigned(
    settings: &Settings,
    (method, path, length): (&str, &str, Option<u64>),
    expires: u32,
    origin: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", settings.region);
    let origin = origin.trim_end_matches('/');
    let host = origin.split_once("://").map_or(origin, |(_, host)| host);
    let mut headers = BTreeMap::from([("host".to_owned(), host.to_owned())]);
    if let Some(length) = length {
        headers.insert("content-length".to_owned(), length.to_string());
    }
    let signed = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let query = canonical_query(&[
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_owned()),
        (
            "X-Amz-Credential",
            format!("{}/{scope}", settings.root_access_key),
        ),
        ("X-Amz-Date", amz_date.clone()),
        ("X-Amz-Expires", expires.to_string()),
        ("X-Amz-SignedHeaders", signed),
    ]);
    let request = canonical_request(method, path, &query, &headers, "UNSIGNED-PAYLOAD");
    let key = signing_key(&settings.root_secret_key, &date, &settings.region, "s3");
    let signature = hex(&hmac(
        &key,
        string_to_sign(&amz_date, &scope, &request).as_bytes(),
    ));
    format!("{origin}{path}?{query}&X-Amz-Signature={signature}")
}

/// One object of a bucket, as a listing names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    /// `LastModified` as the listing writes it, RFC 3339; empty when it names none.
    pub modified: String,
}

impl Client {
    /// `/{bucket}/{key}`, each segment of the key encoded as SigV4 wants it.
    fn object_path(bucket: &str, key: &str) -> String {
        let key: Vec<String> = key.split('/').map(encode).collect();
        format!("/{}/{}", encode(bucket), key.join("/"))
    }

    /// One signed object request with the root credential (AP-150, AP-151): its status, its
    /// headers and its body. `extra` headers are sent and signed.
    pub async fn object(
        &self,
        method: reqwest::Method,
        bucket: &str,
        key: &str,
        body: Vec<u8>,
        extra: &[(&str, String)],
        operation: &'static str,
    ) -> Result<(u16, reqwest::header::HeaderMap, Vec<u8>), Error> {
        let path = Self::object_path(bucket, key);
        let headers = self.sign_with(
            method.as_str(),
            &path,
            &[],
            &body,
            extra,
            chrono::Utc::now(),
        );
        let url = format!("{}{path}", self.settings.endpoint.trim_end_matches('/'));
        let mut request = self.http.request(method, &url);
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }
        let response = request
            .send()
            .await
            .map_err(|source| Error::Transport { operation, source })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response
            .bytes()
            .await
            .map_err(|source| Error::Transport { operation, source })?;
        Ok((status, headers, bytes.to_vec()))
    }

    /// Writes `body` at `key`; any answer but 2xx is a refusal.
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Vec<u8>,
        operation: &'static str,
    ) -> Result<(), Error> {
        match self
            .object(reqwest::Method::PUT, bucket, key, body, &[], operation)
            .await?
        {
            (200..=299, _, _) => Ok(()),
            (status, _, _) => Err(Error::Refused { operation, status }),
        }
    }

    /// Copies `from` to `to` in the same bucket, inside the store.
    pub async fn copy_object(&self, bucket: &str, from: &str, to: &str) -> Result<(), Error> {
        let source = Self::object_path(bucket, from);
        match self
            .object(
                reqwest::Method::PUT,
                bucket,
                to,
                Vec::new(),
                &[("x-amz-copy-source", source)],
                "copy an object",
            )
            .await?
        {
            (200..=299, _, _) => Ok(()),
            (status, _, _) => Err(Error::Refused {
                operation: "copy an object",
                status,
            }),
        }
    }

    /// Deletes `key`; one already gone is deleted.
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), Error> {
        match self
            .object(
                reqwest::Method::DELETE,
                bucket,
                key,
                Vec::new(),
                &[],
                "delete an object",
            )
            .await?
        {
            (200..=299 | 404, _, _) => Ok(()),
            (status, _, _) => Err(Error::Refused {
                operation: "delete an object",
                status,
            }),
        }
    }

    /// Every object under `prefix`, following the listing's pages.
    pub async fn list(&self, bucket: &str, prefix: &str) -> Result<Vec<Object>, Error> {
        const OPERATION: &str = "list objects";
        let path = format!("/{}", encode(bucket));
        let mut found = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2".to_owned()), ("prefix", prefix.to_owned())];
            if let Some(token) = &token {
                query.push(("continuation-token", token.clone()));
            }
            let headers = self.sign("GET", &path, &query, &[], chrono::Utc::now());
            let url = format!(
                "{}{path}?{}",
                self.settings.endpoint.trim_end_matches('/'),
                canonical_query(&query)
            );
            let mut request = self.http.get(&url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let response = request.send().await.map_err(|source| Error::Transport {
                operation: OPERATION,
                source,
            })?;
            let status = response.status().as_u16();
            let text = response.text().await.map_err(|source| Error::Transport {
                operation: OPERATION,
                source,
            })?;
            if !(200..300).contains(&status) {
                return Err(Error::Refused {
                    operation: OPERATION,
                    status,
                });
            }
            for contents in elements(&text, "Contents") {
                let key = elements(&contents, "Key")
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                let size = elements(&contents, "Size")
                    .into_iter()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let modified = elements(&contents, "LastModified")
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                found.push(Object {
                    key,
                    size,
                    modified,
                });
            }
            let truncated = elements(&text, "IsTruncated")
                .first()
                .is_some_and(|t| t == "true");
            token = elements(&text, "NextContinuationToken").into_iter().next();
            if !truncated || token.is_none() {
                return Ok(found);
            }
        }
    }
}

/// The text of every `<tag>…</tag>` in a listing, its entities decoded.
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        out.push(
            rest[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &rest[end + close.len()..];
    }
    out
}

/// `{METHOD}\n{path}\n{query}\n{headers}\n\n{signed headers}\n{payload hash}` (SigV4 §
/// "Create a canonical request"). The admin paths contain no character that needs encoding, so
/// the path is used as it stands; the query is encoded by [`canonical_query`].
fn canonical_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &BTreeMap<String, String>,
    payload_hash: &str,
) -> String {
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
    format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}")
}

fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    )
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let mut key = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    key = hmac(&key, region.as_bytes());
    key = hmac(&key, service.as_bytes());
    hmac(&key, b"aws4_request")
}

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Query parameters sorted by name, each half encoded the way SigV4 wants: everything but
/// `A-Za-z0-9-._~` as `%XX`. An access key or a policy name with a character outside that set
/// would otherwise sign one string and send another, and the store would answer 403 with
/// nothing in it to say why.
fn canonical_query(query: &[(&str, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (encode(name), encode(value)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The Secret a workload of one organization reads its artifact-store credential from (T-0925).
///
/// The reader's pair and no other: the writer publishes, and a workload that serves artifacts has
/// no business holding a key that can replace one (PF-32). The values are `stringData`, so the
/// API server encodes them and the Portal never writes a base64 blob it would also have to read
/// back; the object is applied with the same field manager as every other object the reconciler
/// owns, so an operator's edit is corrected on the next sync (CC-18).
///
/// Derived, so this is a pure function of the root secret and the organization: re-running it
/// writes the same bytes, and rotating the root secret rewrites every organization's Secret on
/// the next sync without a migration.
pub fn reader_secret(namespace: &str, org: &str, reader: &Credential) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": reader_secret_name(org),
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "joinedcontext.com/organization": org,
                "joinedcontext.com/artifact-store-role": Role::Reader.as_str(),
            },
        },
        "type": "Opaque",
        "stringData": {
            "ACCESS_KEY_ID": reader.access_key,
            "ACCESS_SECRET_KEY": reader.secret_key,
        },
    })
}

/// The name of that Secret. `artifactStore.credentialsSecretRef` of the workload's values names
/// the same string, which is the whole coupling between the two (Architecture/17 §4).
pub fn reader_secret_name(org: &str) -> String {
    format!("artifact-store-reader-{org}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_credential_grants_no_write_action() {
        let policy = policy_document("jc-artifacts", "hel", Role::Reader);
        let actions = policy["Statement"][0]["Action"].to_string();
        assert!(actions.contains("s3:GetObject"));
        for write in ["PutObject", "DeleteObject", "Multipart", "s3:*"] {
            assert!(!actions.contains(write), "a reader may {write}: {actions}");
        }
    }

    #[test]
    fn a_writer_credential_grants_no_read_and_no_delete() {
        let policy = policy_document("jc-artifacts", "hel", Role::Writer);
        let actions = policy["Statement"][0]["Action"].to_string();
        assert!(actions.contains("s3:PutObject"));
        for forbidden in ["GetObject", "DeleteObject", "s3:*"] {
            assert!(
                !actions.contains(forbidden),
                "a writer may {forbidden}: {actions}"
            );
        }
    }

    #[test]
    fn a_policy_names_only_the_five_prefixes_of_its_own_organization() {
        for role in Role::BOTH {
            let policy = policy_document("jc-artifacts", "hel", role);
            let resources: Vec<String> = policy["Statement"][0]["Resource"]
                .as_array()
                .expect("resources are a list")
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(resources.len(), OWNED_PREFIXES.len());
            for resource in &resources {
                assert!(
                    resource.starts_with("arn:aws:s3:::jc-artifacts/"),
                    "{resource} is not in this installation's bucket"
                );
                assert!(
                    resource.contains("/hel/"),
                    "{resource} is not confined to one organization"
                );
                assert_ne!(resource, "arn:aws:s3:::jc-artifacts/*");
            }
        }
    }

    #[test]
    fn one_organization_cannot_reach_another_whose_name_starts_the_same() {
        // `a` and `ab` are the pair a prefix rule gets wrong: `schemas/a` is a prefix of
        // `schemas/ab`, and only the separator after the organization keeps them apart.
        let reach = |org: &str| -> Vec<String> {
            policy_document("jc-artifacts", org, Role::Writer)["Statement"][0]["Resource"]
                .as_array()
                .expect("resources are a list")
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_default()
                        .trim_end_matches('*')
                        .to_owned()
                })
                .collect()
        };
        for mine in reach("a") {
            for theirs in reach("ab") {
                assert!(
                    !mine.starts_with(&theirs) && !theirs.starts_with(&mine),
                    "{mine} and {theirs} overlap"
                );
            }
        }
    }

    #[test]
    fn the_same_root_secret_always_derives_the_same_credential() {
        let first = Credential::derive("root-secret", "hel", Role::Reader);
        let again = Credential::derive("root-secret", "hel", Role::Reader);
        assert_eq!(first, again);
        assert_eq!(first.access_key, "jc-hel-reader");
    }

    #[test]
    fn a_derived_secret_differs_per_role_per_organization_and_per_root_secret() {
        let reader = Credential::derive("root-secret", "hel", Role::Reader);
        let writer = Credential::derive("root-secret", "hel", Role::Writer);
        let other_org = Credential::derive("root-secret", "bbx", Role::Reader);
        let rotated = Credential::derive("root-secret-2", "hel", Role::Reader);
        assert_ne!(reader.secret_key, writer.secret_key);
        assert_ne!(reader.secret_key, other_org.secret_key);
        assert_ne!(reader.secret_key, rotated.secret_key);
    }

    #[test]
    fn a_derived_secret_is_forty_characters_and_carries_no_part_of_the_root_secret() {
        let credential = Credential::derive("a-very-recognisable-root-secret", "hel", Role::Writer);
        assert_eq!(credential.secret_key.len(), 40);
        assert!(credential
            .secret_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(!credential.secret_key.contains("recognisable"));
    }

    #[test]
    fn the_signature_matches_the_published_sigv4_test_vector() {
        // AWS's own `get-vanilla` case: the one way to know the signer is right without a store
        // to ask. Service and signed headers are the vector's, not this module's.
        let mut headers = BTreeMap::new();
        headers.insert("host".to_owned(), "example.amazonaws.com".to_owned());
        headers.insert("x-amz-date".to_owned(), "20150830T123600Z".to_owned());
        let payload_hash = hex(&Sha256::digest(b""));
        let request = canonical_request("GET", "/", "", &headers, &payload_hash);
        let to_sign = string_to_sign(
            "20150830T123600Z",
            "20150830/us-east-1/service/aws4_request",
            &request,
        );
        let key = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "service",
        );
        assert_eq!(
            hex(&hmac(&key, to_sign.as_bytes())),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn query_parameters_are_sorted_and_encoded_the_way_the_signature_says() {
        let query = canonical_query(&[
            ("userOrGroup", "jc-hel-reader".to_owned()),
            ("policyName", "jc-hel/reader".to_owned()),
            ("isGroup", "false".to_owned()),
        ]);
        assert_eq!(
            query,
            "isGroup=false&policyName=jc-hel%2Freader&userOrGroup=jc-hel-reader"
        );
    }

    #[test]
    fn neither_the_settings_nor_a_credential_print_their_secret() {
        let settings = Settings {
            endpoint: "http://store:9000".to_owned(),
            bucket: "jc-artifacts".to_owned(),
            region: "us-east-1".to_owned(),
            root_access_key: "root".to_owned(),
            root_secret_key: "the-root-secret".to_owned(),
        };
        assert!(!format!("{settings:?}").contains("the-root-secret"));
        let credential = Credential::derive("the-root-secret", "hel", Role::Reader);
        assert!(!format!("{credential:?}").contains(&credential.secret_key));
    }

    #[test]
    fn an_endpoint_with_a_port_signs_the_host_with_it() {
        let client = Client::new(Settings {
            endpoint: "http://artifact-store.jc-system.svc.cluster.local:9000".to_owned(),
            bucket: "jc-artifacts".to_owned(),
            region: "us-east-1".to_owned(),
            root_access_key: "root".to_owned(),
            root_secret_key: "secret".to_owned(),
        })
        .expect("a URL with a host");
        assert_eq!(
            client.host,
            "artifact-store.jc-system.svc.cluster.local:9000"
        );
        assert!(Client::new(Settings {
            endpoint: "not a url".to_owned(),
            bucket: "jc-artifacts".to_owned(),
            region: "us-east-1".to_owned(),
            root_access_key: "root".to_owned(),
            root_secret_key: "secret".to_owned(),
        })
        .is_err());
    }

    #[test]
    fn a_presigned_url_is_signed_as_aws_documents_it_for_the_host_a_browser_sends() {
        // The example of SigV4's query-parameter page: the bucket's own host, no bucket segment.
        let settings = Settings {
            endpoint: "http://rustfs.store:9000".into(),
            bucket: "examplebucket".into(),
            region: "us-east-1".into(),
            root_access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            root_secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        };
        let may_24 = chrono::DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z")
            .expect("a date")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            presigned(
                &settings,
                ("GET", "/test.txt", None),
                86_400,
                "https://examplebucket.s3.amazonaws.com/",
                may_24
            ),
            "https://examplebucket.s3.amazonaws.com/test.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
        let client = Client::new(settings).expect("a client");
        let url = client.presign(
            ("PUT", "apps", "apps/0/a1/x y.png"),
            None,
            300,
            "https://files.dev.example",
            may_24,
        );
        assert!(
            url.starts_with("https://files.dev.example/apps/apps/0/a1/x%20y.png?"),
            "path style on the public origin: {url}"
        );
        // T-3661: a PUT held to a length signs it, and the signature is another one.
        let held = client.presign(
            ("PUT", "apps", "apps/0/a1/x y.png"),
            Some(1024),
            300,
            "https://files.dev.example",
            may_24,
        );
        assert!(
            held.contains("X-Amz-SignedHeaders=content-length%3Bhost"),
            "{held}"
        );
        assert_ne!(
            held.split("X-Amz-Signature=").nth(1),
            url.split("X-Amz-Signature=").nth(1)
        );
    }
}
