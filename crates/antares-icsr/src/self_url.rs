// SPDX-License-Identifier: EUPL-1.2
//! "This broker", as the deployment names it (ADR-0025 rule 1): the base
//! URLs under which it is reachable, compared exactly, never resolved.

use url::Url;

/// The self URLs of `ANTARES_SELF_URLS`, each normalised the way the HTTP
/// client normalises a forward's target: scheme and host lowercased, a
/// default port dropped, no trailing `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelfUrls(Vec<String>);

impl SelfUrls {
    /// The comma-separated list `raw`; blank entries are skipped, so an
    /// empty value is an empty list (every forward stays HTTP).
    ///
    /// # Errors
    /// The first entry that is not an absolute `http` or `https` URL without
    /// userinfo, query or fragment: a misread self URL would send a
    /// Tenant's data somewhere else, so it stops the broker.
    ///
    /// ```
    /// let urls = antares_icsr::SelfUrls::parse("HTTP://Broker.Example:80/, https://x.example/b")
    ///     .expect("two self URLs");
    /// assert_eq!(urls.matching("http://broker.example/ngsi-ld/v1/entities"), Some("http://broker.example"));
    /// assert!(antares_icsr::SelfUrls::parse("http://user@broker.example").is_err());
    /// ```
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut out = Vec::new();
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let url = normalise(entry).ok_or_else(|| {
                format!(
                    "{}: {entry:?} is not an absolute http(s) URL without userinfo, query or fragment",
                    crate::SELF_URLS_ENV
                )
            })?;
            if !out.contains(&url) {
                out.push(url);
            }
        }
        Ok(Self(out))
    }

    /// Whether no self URL is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The self URL `url` lies under: equal to it, or it followed by `/`.
    /// `None` for anything else, an URL with userinfo included (it goes
    /// somewhere that wants credentials).
    #[must_use]
    pub fn matching(&self, url: &str) -> Option<&str> {
        let parsed = Url::parse(url).ok()?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return None;
        }
        let url = String::from(parsed);
        self.0
            .iter()
            .find(|base| {
                url.strip_prefix(base.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
            })
            .map(String::as_str)
    }
}

/// `raw` as a self URL, or `None` when it cannot be one.
fn normalise(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    // An http(s) URL always has a host: the parser refuses an empty one.
    let fine = matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    fine.then(|| String::from(url).trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::SelfUrls;

    /// Rule 1: equal, or the self URL followed by `/`; nothing that only
    /// starts with its text, no other port or scheme, never userinfo, and
    /// no name resolution (an address is not its host's name).
    #[test]
    fn self_url_matching_is_exact() {
        let urls = SelfUrls::parse("https://self.example, http://Broker.Example:80/prefix/")
            .unwrap_or_default();
        for (url, matched) in [
            (
                "https://self.example/ngsi-ld/v1/entities",
                Some("https://self.example"),
            ),
            ("https://self.example", Some("https://self.example")),
            ("HTTPS://SELF.example:443/x", Some("https://self.example")),
            (
                "https://self.example?local=true",
                Some("https://self.example"),
            ),
            (
                "http://broker.example/prefix/ngsi-ld/v1/entities",
                Some("http://broker.example/prefix"),
            ),
            ("https://self.example.evil/ngsi-ld/v1/entities", None),
            ("https://self.examplex/", None),
            ("https://self.example:8443/ngsi-ld/v1/entities", None),
            ("http://self.example/ngsi-ld/v1/entities", None),
            ("https://user@self.example/ngsi-ld/v1/entities", None),
            ("https://user:pw@self.example/ngsi-ld/v1/entities", None),
            ("https://:pw@self.example/ngsi-ld/v1/entities", None),
            (
                "http://broker.example/prefix",
                Some("http://broker.example/prefix"),
            ),
            ("http://broker.example/prefixed/ngsi-ld/v1", None),
            ("http://broker.example/ngsi-ld/v1", None),
            ("https://127.0.0.1/ngsi-ld/v1", None),
            ("not a url", None),
        ] {
            assert_eq!(urls.matching(url), matched, "{url}");
        }
    }

    #[test]
    fn an_invalid_entry_is_refused_and_blank_ones_are_skipped() {
        for bad in [
            "self.example",
            "ftp://self.example",
            "https://u@self.example",
            "https://:pw@self.example",
            "http://",
            "https://self.example/?q=1",
            "https://self.example/#f",
            "https://self.example, /relative",
        ] {
            assert!(SelfUrls::parse(bad).is_err(), "{bad}");
        }
        assert!(SelfUrls::parse(" , ").is_ok_and(|u| u.is_empty()));
        assert_eq!(
            SelfUrls::parse("https://a.example,https://A.example/").map(|u| u.0.len()),
            Ok(1)
        );
    }
}
