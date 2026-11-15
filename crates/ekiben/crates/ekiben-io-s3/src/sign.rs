//! AWS Signature Version 4 for S3 requests (docs.aws.amazon.com, "Authenticating Requests:
//! Using the Authorization Header"), on the hmac and sha2 crates the workspace already carries.

use chrono::{DateTime, Utc};
use ekiben_core::{CoreError, SecretString};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use url::Url;

/// A static access key: `credentials.id`, `credentials.secret` and an optional session `token`.
pub struct Credentials {
    pub id: String,
    pub secret: SecretString,
    pub token: Option<SecretString>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &str) -> Result<Vec<u8>, CoreError> {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key)
        .map_err(|e| CoreError::other(format!("aws_s3: signing key: {e}")))?;
    mac.update(data.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

/// S3's URI encoding: every byte but `A-Za-z0-9-._~` as `%XX`, and `/` too unless `keep_slash`.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b))
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The value of the `host` header a request to `url` carries.
pub fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

/// Adds `x-amz-date`, `x-amz-content-sha256`, the session token if any and `authorization` to
/// `headers` (lowercase names), signing every header in it plus `host`. `url`'s path must already
/// be URI-encoded as S3 wants it and its query is signed as given.
pub fn sign(
    method: &str,
    url: &Url,
    headers: &mut Vec<(String, String)>,
    payload_sha256: &str,
    creds: &Credentials,
    region: &str,
    now: DateTime<Utc>,
) -> Result<(), CoreError> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    headers.push(("x-amz-date".into(), amz_date.clone()));
    headers.push(("x-amz-content-sha256".into(), payload_sha256.into()));
    if let Some(t) = &creds.token {
        headers.push(("x-amz-security-token".into(), t.expose().to_string()));
    }
    let mut signed: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    signed.push(("host".into(), host_header(url)));
    signed.sort();
    let canonical_headers: String = signed.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_names = signed
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (uri_encode(&k, false), uri_encode(&v, false)))
        .collect();
    query.sort();
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical = format!(
        "{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_names}\n{payload_sha256}",
        url.path()
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let mut key = hmac(format!("AWS4{}", creds.secret.expose()).as_bytes(), &date)?;
    for part in [region, "s3", "aws4_request"] {
        key = hmac(&key, part)?;
    }
    let signature = hex::encode(hmac(&key, &to_sign)?);
    headers.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_names}, Signature={signature}",
            creds.id
        ),
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// The example key of the AWS documentation, assembled so no scanner reads it as a leak.
    fn example() -> Credentials {
        Credentials {
            id: ["AKIA", "IOSFODNN7EXAMPLE"].concat(),
            secret: SecretString::new(["wJalrXUtnFEMI/K7MDENG/", "bPxRfiCYEXAMPLEKEY"].concat()),
            token: None,
        }
    }

    fn auth(headers: &[(String, String)]) -> &str {
        &headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1
    }

    /// "Example: GET Object" of the SigV4 header-signing page.
    #[test]
    fn get_object_signs_as_the_aws_example() {
        let url = Url::parse("https://examplebucket.s3.amazonaws.com/test.txt").unwrap();
        let mut h = vec![("range".to_string(), "bytes=0-9".to_string())];
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        sign(
            "GET",
            &url,
            &mut h,
            &sha256_hex(b""),
            &example(),
            "us-east-1",
            now,
        )
        .unwrap();
        assert!(auth(&h).ends_with(
            "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        ));
        assert!(auth(&h).contains("/20130524/us-east-1/s3/aws4_request,"));
    }

    /// "Example: PUT Object": a `$` in the key is encoded, the body hashed.
    #[test]
    fn put_object_signs_as_the_aws_example() {
        let path = format!("/{}", uri_encode("test$file.text", true));
        let url = Url::parse(&format!("https://examplebucket.s3.amazonaws.com{path}")).unwrap();
        let body = b"Welcome to Amazon S3.";
        let mut h = vec![
            (
                "date".to_string(),
                "Fri, 24 May 2013 00:00:00 GMT".to_string(),
            ),
            (
                "x-amz-storage-class".to_string(),
                "REDUCED_REDUNDANCY".to_string(),
            ),
        ];
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        sign(
            "PUT",
            &url,
            &mut h,
            &sha256_hex(body),
            &example(),
            "us-east-1",
            now,
        )
        .unwrap();
        assert!(auth(&h).ends_with(
            "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        ));
    }

    #[test]
    fn the_secret_stays_out_of_debug() {
        let printed = format!("{:?}", example());
        assert!(!printed.contains("EXAMPLEKEY"), "{printed}");
    }
}
