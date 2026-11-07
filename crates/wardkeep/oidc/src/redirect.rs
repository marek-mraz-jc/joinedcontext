#![forbid(unsafe_code)]
//! Strict redirect URI and post-logout redirect URI validation (W1, W12):
//! - Parsed with `url::Url` to reject userinfo (`@`), backslashes, relative paths, or fragment tricks.
//! - Exact match on scheme, host, and port (RFC 8252 exception: `http://localhost` and
//!   `http://127.0.0.1` registered without an explicit port accept any loopback port on the exact
//!   registered path, matching `mcp-local` in `components/context-gateway/keycloak-clients.yaml`).
//! - Trailing `/*` in a registered pattern is accepted ONLY as a path prefix wildcard on the exact
//!   registered origin (scheme + host + port), never in host or scheme, and refuses `..` traversal.
//! - Post-logout redirect URI supports `+` (same as registered redirect URIs) and `##`-separated lists.

use url::Url;
use wardkeep_core::ClientRepresentation;

fn has_traversal_or_backslash(raw: &str, path: &str) -> bool {
    if raw.contains('\\') || raw.chars().any(char::is_control) {
        return true;
    }
    let decoded = path.to_ascii_lowercase();
    decoded.contains("%2e")
        || decoded.contains("%2f")
        || decoded.contains("%5c")
        || path.split('/').any(|seg| seg == "." || seg == "..")
}

/// Validates `candidate` against the client's registered `redirect_uris` (W1, W12).
#[must_use]
pub fn validate_redirect_uri(registered_patterns: &[String], candidate: &str) -> bool {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return false;
    }
    let Ok(cand_url) = Url::parse(candidate) else {
        return false;
    };
    if !cand_url.username().is_empty()
        || cand_url.password().is_some()
        || cand_url.fragment().is_some()
        || has_traversal_or_backslash(candidate, cand_url.path())
    {
        return false;
    }

    for pattern in registered_patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern == "*" {
            continue;
        }
        if let Some(prefix_base) = pattern.strip_suffix("/*") {
            let Ok(pat_url) = Url::parse(&format!("{prefix_base}/")) else {
                continue;
            };
            if pat_url.scheme() != cand_url.scheme()
                || pat_url.host_str() != cand_url.host_str()
                || pat_url.port() != cand_url.port()
            {
                continue;
            }
            // Also ensure raw explicit `:443` vs implicit port matches byte-for-byte on origin
            let cand_origin_raw = candidate.split('/').take(3).collect::<Vec<_>>().join("/");
            let pat_origin_raw = prefix_base.split('/').take(3).collect::<Vec<_>>().join("/");
            if !cand_origin_raw.eq_ignore_ascii_case(&pat_origin_raw) {
                continue;
            }
            let base_path = pat_url.path().trim_end_matches('/');
            if base_path.is_empty()
                || cand_url.path() == base_path
                || cand_url.path().starts_with(&format!("{base_path}/"))
            {
                return true;
            }
        } else {
            let Ok(pat_url) = Url::parse(pattern) else {
                continue;
            };
            let is_loopback = pat_url.scheme() == "http"
                && matches!(pat_url.host_str(), Some("localhost" | "127.0.0.1"))
                && pat_url.port().is_none();
            if is_loopback {
                if cand_url.scheme() == "http"
                    && cand_url.host_str() == pat_url.host_str()
                    && cand_url.path() == pat_url.path()
                    && cand_url.query() == pat_url.query()
                {
                    return true;
                }
            } else if candidate == pattern {
                return true;
            }
        }
    }
    false
}

/// Validates `post_logout_redirect_uri` against the client's `post.logout.redirect.uris` attribute
/// (supporting `+` and `##`-separated patterns, W1, W12).
#[must_use]
pub fn validate_post_logout_redirect_uri(client: &ClientRepresentation, candidate: &str) -> bool {
    let raw_attr = client
        .attributes
        .get("post.logout.redirect.uris")
        .map_or("+", String::as_str);
    let mut patterns: Vec<String> = Vec::new();
    for entry in raw_attr
        .split("##")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if entry == "+" {
            patterns.extend(client.redirect_uris.iter().cloned());
        } else {
            patterns.push(entry.to_owned());
        }
    }
    validate_redirect_uri(&patterns, candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_exact_and_path_wildcard_and_loopback_and_bypass_corpus() {
        let registered = vec![
            "https://portal.example.test/api/v1/auth/callback".to_owned(),
            "https://app.example.test/apps/*".to_owned(),
            "http://localhost/callback".to_owned(),
        ];

        assert!(validate_redirect_uri(
            &registered,
            "https://portal.example.test/api/v1/auth/callback"
        ));
        assert!(validate_redirect_uri(
            &registered,
            "https://app.example.test/apps/my-app/cb"
        ));
        assert!(validate_redirect_uri(
            &registered,
            "http://localhost:54321/callback"
        ));

        // W12 attack cases: added path segment on exact URI, other port, userinfo trick, path traversal, scheme downgrade
        for attack in [
            "https://portal.example.test/api/v1/auth/callback/extra",
            "https://portal.example.test:8443/api/v1/auth/callback",
            "https://portal.example.test:443/api/v1/auth/callback",
            "https://portal.example.test@evil.example/api/v1/auth/callback",
            "https://app.example.test/apps/../../evil",
            "https://app.example.test/apps/%2e%2e/evil",
            "https://app.example.test/other",
            "http://portal.example.test/api/v1/auth/callback",
            "http://localhost:54321/other",
        ] {
            assert!(
                !validate_redirect_uri(&registered, attack),
                "attack URI {attack} must be refused"
            );
        }
    }
}
