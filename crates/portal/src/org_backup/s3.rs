//! The few S3 calls an Organization backup makes, signed with AWS Signature Version 4 (T-7154).
//!
//! Path-style requests (`{endpoint}/{bucket}/{key}`), so MinIO, Ceph and AWS answer alike. A
//! configuration bundle is manifests and schemas, held in memory and capped by
//! [`MAX_OBJECT_BYTES`]; a data archive goes up from its file in parts of [`PART_BYTES`]
//! ([`Bucket::put_file`], T-7170) and comes back into a file ([`stream_into`]), both up to
//! [`MAX_ARCHIVE_BYTES`]. Errors name the call and the status, never the credentials.

use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

/// The largest object a backup holds in memory: a configuration bundle, written or read back.
pub const MAX_OBJECT_BYTES: usize = 256 * 1024 * 1024;
/// The largest sealed data archive a run uploads and a restore reads back, both through a file:
/// a run past it fails rather than log a version no restore can read.
pub const MAX_ARCHIVE_BYTES: u64 = 1 << 30;
/// One part of a multipart upload: S3 takes 5 MiB to 5 GiB a part and 10 000 parts.
pub const PART_BYTES: usize = 64 * 1024 * 1024;
/// The longest an answer to an S3 call may take.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// Where one backup goes and the key that signs for it.
pub struct Bucket {
    pub endpoint: Url,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: Zeroizing<String>,
}

/// One version of an object, or a delete marker, as ListObjectVersions answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub key: String,
    pub version_id: String,
    pub last_modified: String,
    pub size: u64,
    pub is_latest: bool,
    pub delete_marker: bool,
}

/// The HTTP client every production call goes through: [`jc_core::outbound::client`] (public addresses only,
/// after resolution, no proxy, timeouts) without redirects, since S3 never redirects a signed call.
pub fn guarded_client() -> reqwest::Client {
    jc_core::outbound::client(Duration::from_secs(10), TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

/// RFC 3986 percent-encoding as SigV4 wants it: unreserved characters stay, `/` stays where
/// `keep_slash` (an object key in a path), everything else is `%XX`.
fn encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC takes a key of any length, so this never answers the empty signature.
    Hmac::<Sha256>::new_from_slice(key)
        .map(|mut mac| {
            mac.update(data.as_bytes());
            mac.finalize().into_bytes().to_vec()
        })
        .unwrap_or_default()
}

/// What SigV4 signs of one request. `query` is the already sorted, encoded query string;
/// `headers` are the signed headers, lower-case and sorted.
struct Canonical<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    headers: &'a [(String, String)],
    payload_hash: &'a str,
    amz_date: &'a str,
}

/// The `Authorization` header of one request (SigV4, service `s3`).
fn authorization(access_key_id: &str, secret: &str, region: &str, c: &Canonical<'_>) -> String {
    let Canonical {
        method,
        path,
        query,
        headers,
        payload_hash,
        amz_date,
    } = c;
    let date = amz_date.get(..8).unwrap_or_default();
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();
    let signed: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed = signed.join(";");
    let request =
        format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}");
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(request.as_bytes()))
    );
    let key = Zeroizing::new(format!("AWS4{secret}"));
    let k_date = hmac(key.as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex(&hmac(&k_signing, &to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed}, Signature={signature}"
    )
}

/// Why an S3 call did not do what it was asked, in words an administrator acts on.
#[derive(Debug)]
pub enum S3Error {
    /// The endpoint could not be reached or refused the address (a private one included).
    Unreachable(String),
    /// S3 answered with an error status.
    Status {
        call: &'static str,
        status: u16,
        code: String,
    },
    /// The answer was not what the call expects.
    Unreadable(&'static str),
    /// The object is larger than the reader takes.
    TooLarge { call: &'static str, cap: u64 },
    /// The scratch file the object streams into could not be written.
    Scratch,
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) => {
                write!(f, "the bucket's endpoint could not be reached: {why}")
            }
            Self::Status { call, status, code } => {
                write!(f, "{call} answered {status}")?;
                if !code.is_empty() {
                    write!(f, " ({code})")?;
                }
                Ok(())
            }
            Self::Unreadable(call) => write!(f, "{call} answered something that is not S3 XML"),
            Self::TooLarge { call, cap } => write!(
                f,
                "{call} answered an object larger than {} MiB, more than a backup of this Portal writes",
                cap >> 20
            ),
            Self::Scratch => write!(f, "the Portal's temporary directory could not be written"),
        }
    }
}

/// The text of the first `<tag>…</tag>` in `xml`, entities decoded.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(unescape(&xml[start..end]))
}

/// Every `<tag>…</tag>` block of `xml`, in order.
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let body = &rest[start + open.len()..];
        let Some(end) = body.find(&close) else { break };
        out.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The `<Version>` and `<DeleteMarker>` entries of a ListObjectVersions answer.
pub fn parse_versions(xml: &str) -> Vec<Version> {
    let mut out = Vec::new();
    for (name, marker) in [("Version", false), ("DeleteMarker", true)] {
        for block in blocks(xml, name) {
            out.push(Version {
                key: tag(block, "Key").unwrap_or_default(),
                version_id: tag(block, "VersionId").unwrap_or_default(),
                last_modified: tag(block, "LastModified").unwrap_or_default(),
                size: tag(block, "Size").and_then(|s| s.parse().ok()).unwrap_or(0),
                is_latest: tag(block, "IsLatest").as_deref() == Some("true"),
                delete_marker: marker,
            });
        }
    }
    out.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    out
}

/// The next `part` bytes of `file` (fewer at its end, none past it), read off the runtime.
async fn read_part(file: std::fs::File, part: usize) -> Result<(std::fs::File, Vec<u8>), S3Error> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut body = Vec::with_capacity(part);
        let mut file = file;
        (&mut file).take(part as u64).read_to_end(&mut body)?;
        Ok::<_, std::io::Error>((file, body))
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or(S3Error::Unreadable("the archive file"))
}

/// The body of a GetObject `response` in memory, up to [`MAX_OBJECT_BYTES`].
pub async fn read_object(response: reqwest::Response) -> Result<Vec<u8>, S3Error> {
    jc_core::outbound::read_capped(response, MAX_OBJECT_BYTES)
        .await
        .map_err(|err| match err {
            jc_core::outbound::BodyError::TooLarge(_) => S3Error::TooLarge {
                call: "GetObject",
                cap: MAX_OBJECT_BYTES as u64,
            },
            _ => S3Error::Unreadable("GetObject"),
        })
}

/// `body` appended to `file` and to `digest`, off the runtime.
async fn append(
    file: std::fs::File,
    digest: Sha256,
    body: Vec<u8>,
) -> Result<(std::fs::File, Sha256), S3Error> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let (mut file, mut digest) = (file, digest);
        file.write_all(&body)?;
        digest.update(&body);
        Ok::<_, std::io::Error>((file, digest))
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or(S3Error::Scratch)
}

/// The body of a GetObject `response` streamed into `file` in writes of up to [`PART_BYTES`],
/// refused once it passes `cap` bytes: its size and SHA-256 (lower-case hex), with the file
/// rewound to its start.
pub async fn stream_into(
    mut response: reqwest::Response,
    file: std::fs::File,
    cap: u64,
) -> Result<(std::fs::File, u64, String), S3Error> {
    let too_large = S3Error::TooLarge {
        call: "GetObject",
        cap,
    };
    if response.content_length().is_some_and(|len| len > cap) {
        return Err(too_large);
    }
    let (mut file, mut digest) = (file, Sha256::new());
    let mut held = Vec::new();
    let mut size = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| S3Error::Unreachable(e.without_url().to_string()))?
    {
        size += chunk.len() as u64;
        if size > cap {
            return Err(too_large);
        }
        held.extend_from_slice(&chunk);
        if held.len() >= PART_BYTES {
            (file, digest) = append(file, digest, std::mem::take(&mut held)).await?;
        }
    }
    (file, digest) = append(file, digest, held).await?;
    use std::io::Seek;
    file.rewind().map_err(|_| S3Error::Scratch)?;
    Ok((file, size, hex(&digest.finalize())))
}

impl Bucket {
    async fn call(
        &self,
        http: &reqwest::Client,
        call: &'static str,
        method: reqwest::Method,
        key: Option<&str>,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<reqwest::Response, S3Error> {
        let mut path = format!("/{}", encode(&self.bucket, false));
        if let Some(key) = key {
            path.push('/');
            path.push_str(&encode(key, true));
        }
        let base = self.endpoint.path().trim_end_matches('/');
        let path = format!("{base}{path}");
        let mut pairs: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (encode(k, false), encode(v, false)))
            .collect();
        pairs.sort();
        let query: String = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let host = match self.endpoint.port() {
            Some(port) => format!("{}:{port}", self.endpoint.host_str().unwrap_or_default()),
            None => self.endpoint.host_str().unwrap_or_default().to_owned(),
        };
        let payload_hash = hex(&Sha256::digest(&body));
        let amz_date = jc_time::TimestampTz::now()
            .0
            .strftime("%Y%m%dT%H%M%SZ")
            .to_string();
        let headers = vec![
            ("host".to_owned(), host),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), amz_date.clone()),
        ];
        let auth = authorization(
            &self.access_key_id,
            &self.secret_access_key,
            &self.region,
            &Canonical {
                method: method.as_str(),
                path: &path,
                query: &query,
                headers: &headers,
                payload_hash: &payload_hash,
                amz_date: &amz_date,
            },
        );
        let mut url = self.endpoint.clone();
        url.set_path(&path);
        url.set_query((!query.is_empty()).then_some(query.as_str()));
        let response = http
            .request(method, url)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header(reqwest::header::AUTHORIZATION, auth)
            .body(body)
            .send()
            .await
            .map_err(|e| S3Error::Unreachable(e.without_url().to_string()))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Err(S3Error::Status {
            call,
            status,
            code: tag(&text, "Code").unwrap_or_default(),
        })
    }

    async fn text(response: reqwest::Response, call: &'static str) -> Result<String, S3Error> {
        let bytes = jc_core::outbound::read_capped(response, 16 * 1024 * 1024)
            .await
            .map_err(|_| S3Error::Unreadable(call))?;
        String::from_utf8(bytes).map_err(|_| S3Error::Unreadable(call))
    }

    /// GetBucketVersioning: `Enabled`, `Suspended`, or empty for a bucket never versioned.
    pub async fn versioning(&self, http: &reqwest::Client) -> Result<String, S3Error> {
        const CALL: &str = "GetBucketVersioning";
        let response = self
            .call(
                http,
                CALL,
                reqwest::Method::GET,
                None,
                &[("versioning", "")],
                Vec::new(),
            )
            .await?;
        let xml = Self::text(response, CALL).await?;
        if !xml.contains("VersioningConfiguration") {
            return Err(S3Error::Unreadable(CALL));
        }
        Ok(tag(&xml, "Status").unwrap_or_default())
    }

    /// GetObjectLockConfiguration: whether Object Lock is on. A bucket without it answers an
    /// error, which reads as off.
    pub async fn object_lock(&self, http: &reqwest::Client) -> bool {
        let Ok(response) = self
            .call(
                http,
                "GetObjectLockConfiguration",
                reqwest::Method::GET,
                None,
                &[("object-lock", "")],
                Vec::new(),
            )
            .await
        else {
            return false;
        };
        Self::text(response, "GetObjectLockConfiguration")
            .await
            .is_ok_and(|xml| tag(&xml, "ObjectLockEnabled").as_deref() == Some("Enabled"))
    }

    /// PutObject: the new version's id (empty when the bucket gave none).
    pub async fn put(
        &self,
        http: &reqwest::Client,
        key: &str,
        body: Vec<u8>,
    ) -> Result<String, S3Error> {
        let response = self
            .call(
                http,
                "PutObject",
                reqwest::Method::PUT,
                Some(key),
                &[],
                body,
            )
            .await?;
        Ok(response
            .headers()
            .get("x-amz-version-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned())
    }

    /// ListObjectVersions under `prefix`, newest first, up to 1000 entries.
    pub async fn versions(
        &self,
        http: &reqwest::Client,
        prefix: &str,
    ) -> Result<Vec<Version>, S3Error> {
        const CALL: &str = "ListObjectVersions";
        let response = self
            .call(
                http,
                CALL,
                reqwest::Method::GET,
                None,
                &[("versions", ""), ("prefix", prefix)],
                Vec::new(),
            )
            .await?;
        let xml = Self::text(response, CALL).await?;
        if !xml.contains("ListVersionsResult") {
            return Err(S3Error::Unreadable(CALL));
        }
        Ok(parse_versions(&xml))
    }

    /// GetObject of one version: the answer, its body not read yet.
    pub async fn object(
        &self,
        http: &reqwest::Client,
        key: &str,
        version_id: &str,
    ) -> Result<reqwest::Response, S3Error> {
        self.call(
            http,
            "GetObject",
            reqwest::Method::GET,
            Some(key),
            &[("versionId", version_id)],
            Vec::new(),
        )
        .await
    }

    /// The file at `path` (`size` bytes) as the next version of `key`: one PutObject up to
    /// `part` bytes, a multipart upload in parts of `part` bytes above that, aborted when a part
    /// fails so the bucket keeps no stray parts. The new version's id.
    pub async fn put_file(
        &self,
        http: &reqwest::Client,
        key: &str,
        path: &std::path::Path,
        size: u64,
        part: usize,
    ) -> Result<String, S3Error> {
        let file =
            std::fs::File::open(path).map_err(|_| S3Error::Unreadable("the archive file"))?;
        if size <= part as u64 {
            let (_, body) = read_part(file, part).await?;
            return self.put(http, key, body).await;
        }
        const CREATE: &str = "CreateMultipartUpload";
        let created = self
            .call(
                http,
                CREATE,
                reqwest::Method::POST,
                Some(key),
                &[("uploads", "")],
                Vec::new(),
            )
            .await?;
        let upload = tag(&Self::text(created, CREATE).await?, "UploadId")
            .ok_or(S3Error::Unreadable(CREATE))?;
        let parts = self.put_parts(http, key, &upload, file, part).await;
        let completed = match parts {
            Ok(etags) => {
                let body: String = etags
                    .iter()
                    .enumerate()
                    .map(|(at, etag)| {
                        format!(
                            "<Part><PartNumber>{}</PartNumber><ETag>{etag}</ETag></Part>",
                            at + 1
                        )
                    })
                    .collect();
                self.call(
                    http,
                    "CompleteMultipartUpload",
                    reqwest::Method::POST,
                    Some(key),
                    &[("uploadId", upload.as_str())],
                    format!("<CompleteMultipartUpload>{body}</CompleteMultipartUpload>")
                        .into_bytes(),
                )
                .await
            }
            Err(err) => Err(err),
        };
        match completed {
            Ok(response) => {
                let version = response
                    .headers()
                    .get("x-amz-version-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                // S3 answers 200 with an <Error> body when the assembly fails late.
                let text = Self::text(response, "CompleteMultipartUpload").await?;
                if let Some(code) = tag(&text, "Code").filter(|_| text.contains("<Error>")) {
                    return Err(S3Error::Status {
                        call: "CompleteMultipartUpload",
                        status: 200,
                        code,
                    });
                }
                Ok(version)
            }
            Err(err) => {
                let aborted = self
                    .call(
                        http,
                        "AbortMultipartUpload",
                        reqwest::Method::DELETE,
                        Some(key),
                        &[("uploadId", upload.as_str())],
                        Vec::new(),
                    )
                    .await;
                if let Err(abort) = aborted {
                    tracing::warn!(error = %abort, "a failed multipart upload was not aborted");
                }
                Err(err)
            }
        }
    }

    /// UploadPart of each `part` bytes of `file`, in order: their ETags.
    async fn put_parts(
        &self,
        http: &reqwest::Client,
        key: &str,
        upload: &str,
        mut file: std::fs::File,
        part: usize,
    ) -> Result<Vec<String>, S3Error> {
        let mut etags = Vec::new();
        loop {
            let (rest, body) = read_part(file, part).await?;
            file = rest;
            if body.is_empty() {
                return Ok(etags);
            }
            let number = (etags.len() + 1).to_string();
            let response = self
                .call(
                    http,
                    "UploadPart",
                    reqwest::Method::PUT,
                    Some(key),
                    &[("partNumber", number.as_str()), ("uploadId", upload)],
                    body,
                )
                .await?;
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .ok_or(S3Error::Unreadable("UploadPart"))?
                .to_owned();
            etags.push(etag);
        }
    }

    /// DeleteObject. Without a version id a versioned bucket only adds a delete marker (the soft
    /// delete); with one it removes that version for good, which only retention does.
    pub async fn delete(
        &self,
        http: &reqwest::Client,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<(), S3Error> {
        let query: Vec<(&str, &str)> = version_id.map(|v| ("versionId", v)).into_iter().collect();
        self.call(
            http,
            "DeleteObject",
            reqwest::Method::DELETE,
            Some(key),
            &query,
            Vec::new(),
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A file past one part goes up as a multipart upload whose parts, in order, rebuild it, and
    /// the completed version's id comes back.
    #[tokio::test]
    async fn a_large_archive_goes_up_in_parts_that_rebuild_it() {
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer, Request, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(query_param("uploads", ""))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<InitiateMultipartUploadResult><UploadId>up-1</UploadId></InitiateMultipartUploadResult>",
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(query_param("uploadId", "up-1"))
            .respond_with(|request: &Request| {
                let number = request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "partNumber")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                ResponseTemplate::new(200).insert_header("ETag", format!("\"etag-{number}\""))
            })
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(query_param("uploadId", "up-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-amz-version-id", "v-big")
                    .set_body_string("<CompleteMultipartUploadResult/>"),
            )
            .mount(&server)
            .await;
        let bucket = Bucket {
            endpoint: server.uri().parse().expect("url"),
            region: "eu-central-1".into(),
            bucket: "b".into(),
            access_key_id: "AKID".into(),
            secret_access_key: Zeroizing::new("secret".into()),
        };
        let dir = std::env::temp_dir().join(format!("s3-parts-{}", std::process::id()));
        let file: Vec<u8> = (0..2500u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&dir, &file).expect("file");
        let version = bucket
            .put_file(
                &reqwest::Client::new(),
                "jc/a/data/archive.zip.age",
                &dir,
                2500,
                1000,
            )
            .await
            .expect("uploaded");
        std::fs::remove_file(&dir).ok();
        assert_eq!(version, "v-big");
        let received = server.received_requests().await.expect("requests");
        let parts: Vec<&Request> = received
            .iter()
            .filter(|r| r.method.as_str() == "PUT")
            .collect();
        assert_eq!(
            parts.iter().map(|r| r.body.len()).collect::<Vec<_>>(),
            vec![1000, 1000, 500]
        );
        let rebuilt: Vec<u8> = parts.iter().flat_map(|r| r.body.clone()).collect();
        assert_eq!(rebuilt, file);
        let complete = received.last().expect("complete");
        let body = String::from_utf8_lossy(&complete.body);
        assert!(
            body.contains("<PartNumber>3</PartNumber><ETag>\"etag-3\"</ETag>"),
            "{body}"
        );
    }

    /// The GET Object example of the AWS SigV4 documentation ("Examples of Signature
    /// Calculations", examplebucket, 20130524T000000Z): the published signature.
    #[test]
    fn signs_the_aws_documentation_example() {
        let headers = vec![
            (
                "host".to_owned(),
                "examplebucket.s3.amazonaws.com".to_owned(),
            ),
            ("range".to_owned(), "bytes=0-9".to_owned()),
            (
                "x-amz-content-sha256".to_owned(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
            ),
            ("x-amz-date".to_owned(), "20130524T000000Z".to_owned()),
        ];
        let auth = authorization(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            &Canonical {
                method: "GET",
                path: "/test.txt",
                query: "",
                headers: &headers,
                payload_hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                amz_date: "20130524T000000Z",
            },
        );
        assert!(
            auth.ends_with(
                "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
            ),
            "{auth}"
        );
    }

    #[test]
    fn reads_versions_and_delete_markers_newest_first() {
        let xml = r#"<ListVersionsResult><Version><Key>a/b&amp;c.zip</Key><VersionId>v1</VersionId><IsLatest>false</IsLatest><LastModified>2026-11-15T10:00:00.000Z</LastModified><Size>12</Size></Version><DeleteMarker><Key>a/b&amp;c.zip</Key><VersionId>m1</VersionId><IsLatest>true</IsLatest><LastModified>2026-11-16T10:00:00.000Z</LastModified></DeleteMarker></ListVersionsResult>"#;
        let versions = parse_versions(xml);
        assert_eq!(versions.len(), 2);
        assert!(versions[0].delete_marker && versions[0].is_latest);
        assert_eq!(versions[1].key, "a/b&c.zip");
        assert_eq!(versions[1].size, 12);
    }

    #[test]
    fn encodes_keys_as_sigv4_wants() {
        assert_eq!(encode("org/a b+c.zip", true), "org/a%20b%2Bc.zip");
        assert_eq!(encode("a/b", false), "a%2Fb");
    }

    fn bucket_at(endpoint: &str) -> Bucket {
        Bucket {
            endpoint: endpoint.parse().expect("url"),
            region: "eu-central-1".into(),
            bucket: "b".into(),
            access_key_id: "AKID".into(),
            secret_access_key: Zeroizing::new("secret".into()),
        }
    }

    /// An HTTP server that answers its one connection with `raw` and closes it: a body cut
    /// short, a length past a cap, a chunked body without a length.
    pub(crate) async fn raw_server(raw: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(raw).await;
            let _ = socket.shutdown().await;
        });
        format!("http://{address}")
    }

    async fn get(endpoint: &str) -> reqwest::Response {
        reqwest::get(endpoint).await.expect("an answer")
    }

    fn scratch(name: &str) -> (std::path::PathBuf, std::fs::File) {
        // One path per call: parallel tests of one name would truncate each other's file.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let call = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("s3-{name}-{}-{call}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("scratch");
        (path, file)
    }

    /// Each refusal reads as words an administrator acts on, the S3 code only when there is one.
    #[test]
    fn every_error_says_what_went_wrong() {
        let said = |err: S3Error| err.to_string();
        assert_eq!(
            said(S3Error::Unreachable("refused".into())),
            "the bucket's endpoint could not be reached: refused"
        );
        assert_eq!(
            said(S3Error::Status {
                call: "PutObject",
                status: 403,
                code: "AccessDenied".into()
            }),
            "PutObject answered 403 (AccessDenied)"
        );
        assert_eq!(
            said(S3Error::Status {
                call: "PutObject",
                status: 500,
                code: String::new()
            }),
            "PutObject answered 500"
        );
        assert_eq!(
            said(S3Error::Unreadable("ListObjectVersions")),
            "ListObjectVersions answered something that is not S3 XML"
        );
        assert!(said(S3Error::TooLarge {
            call: "GetObject",
            cap: 2 << 20
        })
        .contains("2 MiB"));
        assert_eq!(
            said(S3Error::Scratch),
            "the Portal's temporary directory could not be written"
        );
    }

    /// An object whose length passes the cap is refused before a byte is read; one cut short
    /// is unreadable, not a short object.
    #[tokio::test]
    async fn an_object_past_the_cap_or_cut_short_is_refused() {
        let over = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 300000000\r\n\r\nx").await;
        let refused = read_object(get(&over).await).await.expect_err("too large");
        assert!(
            matches!(refused, S3Error::TooLarge { call: "GetObject", cap } if cap == MAX_OBJECT_BYTES as u64),
            "{refused}"
        );
        let short = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort").await;
        let refused = read_object(get(&short).await).await.expect_err("cut short");
        assert!(
            matches!(refused, S3Error::Unreadable("GetObject")),
            "{refused}"
        );
        let whole = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwhole").await;
        assert_eq!(
            read_object(get(&whole).await).await.expect("read"),
            b"whole"
        );
    }

    /// A streamed object refuses a chunked body once it passes the cap and a body cut short as
    /// unreachable; one within the cap lands whole with its SHA-256, the file rewound.
    #[tokio::test]
    async fn a_streamed_object_is_capped_and_hashed() {
        let chunked = raw_server(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n",
        )
        .await;
        let (path, file) = scratch("chunked");
        let refused = stream_into(get(&chunked).await, file, 6)
            .await
            .expect_err("past the cap");
        assert!(
            matches!(
                refused,
                S3Error::TooLarge {
                    call: "GetObject",
                    cap: 6
                }
            ),
            "{refused}"
        );
        let short = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort").await;
        let (_, file) = scratch("chunked");
        let refused = stream_into(get(&short).await, file, 1000)
            .await
            .expect_err("cut short");
        assert!(matches!(refused, S3Error::Unreachable(_)), "{refused}");
        let fits = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").await;
        let (_, file) = scratch("chunked");
        let (mut file, size, sha256) = stream_into(get(&fits).await, file, 3).await.expect("fits");
        assert_eq!(size, 3);
        assert_eq!(
            sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut back = String::new();
        std::io::Read::read_to_string(&mut file, &mut back).expect("rewound");
        assert_eq!(back, "abc");
        std::fs::remove_file(path).ok();
    }

    /// An endpoint on its scheme's default port signs the bare host, and one that does not
    /// answer is unreachable, not an S3 refusal.
    #[tokio::test]
    async fn an_endpoint_that_does_not_answer_is_unreachable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let closed = listener.local_addr().expect("address").port();
        drop(listener);
        let refused = bucket_at(&format!("http://127.0.0.1:{closed}"))
            .versioning(&reqwest::Client::new())
            .await
            .expect_err("nothing listens");
        assert!(matches!(refused, S3Error::Unreachable(_)), "{refused}");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");
        let refused = bucket_at("http://127.0.0.1")
            .versioning(&http)
            .await
            .expect_err("port 80 is not an S3");
        assert!(
            matches!(refused, S3Error::Unreachable(_) | S3Error::Status { .. }),
            "{refused}"
        );
    }

    /// Answers that are not the XML a call expects are unreadable: a versioning answer without
    /// its configuration, a body cut short, a version list that is no list.
    #[tokio::test]
    async fn an_answer_that_is_not_the_calls_xml_is_unreadable() {
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("versioning", ""))
            .respond_with(ResponseTemplate::new(200).set_body_string("<Other/>"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("versions", ""))
            .respond_with(ResponseTemplate::new(200).set_body_string("<Other/>"))
            .mount(&server)
            .await;
        let bucket = bucket_at(&server.uri());
        let http = reqwest::Client::new();
        let refused = bucket
            .versioning(&http)
            .await
            .expect_err("no configuration");
        assert!(
            matches!(refused, S3Error::Unreadable("GetBucketVersioning")),
            "{refused}"
        );
        let refused = bucket.versions(&http, "jc/").await.expect_err("no list");
        assert!(
            matches!(refused, S3Error::Unreadable("ListObjectVersions")),
            "{refused}"
        );
        let short = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n<Versioning").await;
        let refused = bucket_at(&short)
            .versioning(&http)
            .await
            .expect_err("cut short");
        assert!(
            matches!(refused, S3Error::Unreadable("GetBucketVersioning")),
            "{refused}"
        );
        let binary = raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n\xff\xfe").await;
        let refused = bucket_at(&binary)
            .versioning(&http)
            .await
            .expect_err("not text");
        assert!(
            matches!(refused, S3Error::Unreadable("GetBucketVersioning")),
            "{refused}"
        );
    }

    /// Object Lock reads as on only when the bucket says Enabled; a refusal or another word
    /// reads as off.
    #[tokio::test]
    async fn object_lock_is_on_only_when_the_bucket_says_enabled() {
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let http = reqwest::Client::new();
        for (status, body, on) in [
            (
                200,
                "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled></ObjectLockConfiguration>",
                true,
            ),
            (200, "<ObjectLockConfiguration></ObjectLockConfiguration>", false),
            (404, "<Error><Code>ObjectLockConfigurationNotFoundError</Code></Error>", false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(query_param("object-lock", ""))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .mount(&server)
                .await;
            assert_eq!(bucket_at(&server.uri()).object_lock(&http).await, on, "{body}");
        }
    }

    async fn multipart(
        part: ResponseTemplate,
        abort: u16,
        complete: ResponseTemplate,
    ) -> (wiremock::MockServer, Result<String, S3Error>) {
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(query_param("uploads", ""))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<InitiateMultipartUploadResult><UploadId>up-9</UploadId></InitiateMultipartUploadResult>",
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(query_param("uploadId", "up-9"))
            .respond_with(part)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(query_param("uploadId", "up-9"))
            .respond_with(complete)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(query_param("uploadId", "up-9"))
            .respond_with(ResponseTemplate::new(abort))
            .mount(&server)
            .await;
        let (path, file) = scratch("multipart");
        std::io::Write::write_all(&mut &file, &[7u8; 25]).expect("file");
        let result = bucket_at(&server.uri())
            .put_file(
                &reqwest::Client::new(),
                "jc/a/archive.zip.age",
                &path,
                25,
                10,
            )
            .await;
        std::fs::remove_file(path).ok();
        (server, result)
    }

    async fn aborted(server: &wiremock::MockServer) -> bool {
        server
            .received_requests()
            .await
            .expect("requests")
            .iter()
            .any(|r| r.method.as_str() == "DELETE")
    }

    use wiremock::ResponseTemplate;

    /// A part S3 refuses ends the upload with that refusal and aborts it, so the bucket keeps no
    /// stray parts; an abort that fails too still answers the part's refusal.
    #[tokio::test]
    async fn a_refused_part_aborts_the_upload() {
        let ok = || ResponseTemplate::new(200).set_body_string("<CompleteMultipartUploadResult/>");
        for abort in [204, 500] {
            let (server, result) = multipart(
                ResponseTemplate::new(500).set_body_string("<Error><Code>SlowDown</Code></Error>"),
                abort,
                ok(),
            )
            .await;
            let refused = result.expect_err("the part was refused");
            assert!(
                matches!(&refused, S3Error::Status { call: "UploadPart", status: 500, code } if code == "SlowDown"),
                "{refused}"
            );
            assert!(aborted(&server).await, "aborted after {abort}");
        }
        let (server, result) = multipart(ResponseTemplate::new(200), 204, ok()).await;
        let refused = result.expect_err("a part without an ETag");
        assert!(
            matches!(refused, S3Error::Unreadable("UploadPart")),
            "{refused}"
        );
        assert!(aborted(&server).await);
    }

    /// CompleteMultipartUpload may answer 200 with an error in its body: that is a refusal.
    #[tokio::test]
    async fn a_completion_that_answers_an_error_in_its_body_is_refused() {
        let (server, result) = multipart(
            ResponseTemplate::new(200).insert_header("ETag", "\"e\""),
            204,
            ResponseTemplate::new(200).set_body_string(
                "<Error><Code>InternalError</Code><Message>retry</Message></Error>",
            ),
        )
        .await;
        let refused = result.expect_err("an error in a 200");
        assert!(
            matches!(&refused, S3Error::Status { call: "CompleteMultipartUpload", status: 200, code } if code == "InternalError"),
            "{refused}"
        );
        assert!(
            !aborted(&server).await,
            "a completion error leaves the upload to S3's lifecycle"
        );
    }

    /// HMAC takes a key of any length, the empty one included: a signature is never empty.
    #[test]
    fn hmac_of_an_empty_key_still_signs() {
        assert_eq!(hmac(b"", "data").len(), 32);
        assert_ne!(hmac(b"", "data"), hmac(b"k", "data"));
    }
}
