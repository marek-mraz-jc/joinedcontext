//! Reading another organisation's schema surface over HTTP (DM-48, DM-49).
//!
//! `jcctl::foreign_models` makes every judgement about what a peer publishes and refuses a
//! plaintext base, a model name that is not a label and a document whose digest does not match
//! the peer's own `index.json`. This is the hop it is handed, and its whole job is to not open
//! a way round any of that:
//!
//! - `https://` only, checked here as well as there, because a redirect is a second URL the
//!   module above never sees;
//! - a public address only, by the rule of `super::guard` (named, not linked: the module is
//!   private and a public item's documentation may not link into one): a `SharedSpaceReference` is a URL
//!   somebody typed, this runs inside the cluster, and without the rule a peer's schema base
//!   naming `169.254.169.254` or a `*.svc.cluster.local` Service makes the mirror a probe for
//!   whoever can get a reference approved (T-1705);
//! - a redirect is followed only to another public `https://` address, and only a few times;
//! - a timeout per request, so one slow partner cannot hold a reconcile open;
//! - a ceiling on the body, because a peer's `index.json` is a few kilobytes and a mirror that
//!   streams a gigabyte into memory is a peer's denial of service on us.
//!
//! No credential goes out with these requests and none is logged: a schema surface is the
//! public half of an endpoint (EP-46).

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use jcctl::foreign_models::{FetchError, SchemaApi};

use super::guard::{https_redirects, internal_host};
use jc_core::outbound::PublicOnly;

/// How long one document may take. A peer that cannot answer in this is a peer whose mirror
/// waits until the next run.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The largest document a mirror reads. The seven artifacts of a schema surface are text;
/// four megabytes is a generous LinkML model and nowhere near a memory problem.
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// How many `https` redirects a document may hide behind.
const MAX_REDIRECTS: usize = 3;

/// The peer half of [`jcctl::foreign_models::mirror`].
pub struct PeerSchemaApi {
    http: reqwest::blocking::Client,
}

impl PeerSchemaApi {
    /// A client that refuses to leave `https`, times out and follows a few redirects.
    ///
    /// Blocking on purpose: `SchemaApi` is a synchronous trait, because the module above it is
    /// a decision and not a process, and the reconciler runs the whole mirror on a blocking
    /// task. An async client bridged with `block_on` would be the same wait with a way to
    /// deadlock the runtime in it.
    pub fn new() -> Result<Self, reqwest::Error> {
        // A refused hop is stopped, not failed with a reason: a peer learns nothing about the
        // inside of this cluster from the shape of the answer.
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(https_redirects(MAX_REDIRECTS))
            .dns_resolver(Arc::new(PublicOnly))
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self { http })
    }
}

impl SchemaApi for PeerSchemaApi {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
        if !url.starts_with("https://") {
            return Err(FetchError::Unavailable(format!(
                "{url} is not https, and a schema surface is not fetched in the clear"
            )));
        }
        if let Some(ip) = internal_host(url) {
            return Err(FetchError::Unavailable(format!(
                "{url} names {ip}, which is not a public address; a schema surface is read from \
                 public hosts only"
            )));
        }

        let response = self
            .http
            .get(url)
            .send()
            .map_err(|err| FetchError::Unavailable(reason(err)))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(FetchError::Refused {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        if response.content_length().is_some_and(|len| len > MAX_BODY) {
            return Err(FetchError::Unavailable(format!(
                "{url} is larger than the {MAX_BODY} bytes a schema document may have"
            )));
        }

        // Read one byte past the ceiling rather than trusting Content-Length: a chunked
        // response declares no length at all, and that is the shape a slow drip arrives in.
        let mut body = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|err| FetchError::Unavailable(format!("{url}: {err}")))?;
        if body.len() as u64 > MAX_BODY {
            return Err(FetchError::Unavailable(format!(
                "{url} is larger than the {MAX_BODY} bytes a schema document may have"
            )));
        }
        Ok(Some(body))
    }
}

/// What went wrong, without the URL's credentials — there are none on this hop, and this keeps
/// it that way if one is ever added.
fn reason(err: reqwest::Error) -> String {
    if err.is_timeout() {
        return "the peer did not answer in time".to_owned();
    }
    if err.is_redirect() {
        return "the peer redirected somewhere a mirror does not follow".to_owned();
    }
    if err.is_connect() {
        return "the peer could not be reached".to_owned();
    }
    err.without_url().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plaintext_surface_is_refused_before_anything_is_requested() {
        let api = PeerSchemaApi::new().expect("a client");
        let error = api
            .get("http://peer.example/schema/index.json")
            .unwrap_err();
        assert!(
            matches!(error, FetchError::Unavailable(ref why) if why.contains("not https")),
            "{error}"
        );
    }

    /// DM-49, MF-28 (T-1705): the peer's base is a URL somebody typed into a manifest, and the
    /// mirror runs inside the cluster. An address of the cluster's own network is refused
    /// before a socket is opened, and the answer never says what was there.
    #[test]
    fn a_surface_inside_the_cluster_is_refused_before_a_socket_is_opened() {
        let api = PeerSchemaApi::new().expect("a client");
        for url in [
            "https://169.254.169.254/latest/meta-data/",
            "https://10.42.0.9/schema/index.json",
            "https://127.0.0.1/schema/index.json",
            "https://[::1]/schema/index.json",
            "https://[fd00::1]/schema/index.json",
        ] {
            let error = api.get(url).expect_err("{url} was fetched");
            assert!(
                matches!(error, FetchError::Unavailable(ref why) if why.contains("public address")),
                "{url}: {error}"
            );
        }
    }

    /// DM-49 (T-1705): a scheme that is not HTTPS never reaches the client, whichever of the
    /// ones an SSRF reaches for it is.
    #[test]
    fn a_surface_that_is_not_https_is_refused_whatever_scheme_it_names() {
        let api = PeerSchemaApi::new().expect("a client");
        for url in [
            "file:///etc/passwd",
            "gopher://peer.example/1/schema",
            "ftp://peer.example/schema/index.json",
            "http://peer.example/schema/index.json",
        ] {
            let error = api.get(url).expect_err("{url} was fetched");
            assert!(
                matches!(error, FetchError::Unavailable(ref why) if why.contains("not https")),
                "{url}: {error}"
            );
        }
    }

    #[test]
    fn the_ceiling_is_small_enough_to_be_a_ceiling() {
        // A schema surface is seven text documents. If this ever grows past a few megabytes,
        // it has stopped being a ceiling and the growth was not deliberate.
        const { assert!(MAX_BODY <= 8 * 1024 * 1024) };
    }
}
