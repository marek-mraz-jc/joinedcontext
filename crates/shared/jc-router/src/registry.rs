//! The host registry's rules (evidence/T-7129/design.md §1): one check for a host an Organization
//! takes, called by jc on the jc.toml seed and by the Portal on every registry write, so a host
//! the seed refuses is refused at the operator's door too.
//!
//! Two layers. [`check_labels`] holds for every host whoever adds it: a plain lowercase host, no
//! reserved label right under the installation's domain, `idm.` only as a sign-in host, never
//! the installation's sign-in host. [`check`] adds what a write through the Portal needs on top:
//! a host outside the installation's domain is under the Organization's minted domain, compared
//! label by label, and no host belongs to two Organizations.

/// Why the registry refuses a host; the Portal answers [`Refusal::status`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The host itself is not allowed (`422`).
    #[error("{0}")]
    Invalid(String),
    /// The host is taken (`409`).
    #[error("{0}")]
    Conflict(String),
}

impl Refusal {
    /// The HTTP status the Portal answers the refusal with.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) => 422,
            Self::Conflict(_) => 409,
        }
    }
}

/// The installation's side of the rules.
#[derive(Debug, Clone, Copy)]
pub struct Rules<'a> {
    /// The installation's domain (`jcdev.dev.joinedcontext.com`), without a port.
    pub base_domain: &'a str,
    /// The installation's sign-in host (`login_host`), without a port.
    pub login_host: &'a str,
    /// Labels and path segments the platform keeps (`jc_core::names::RESERVED_LABELS`).
    pub reserved: &'a [&'a str],
}

/// One registry row as the check reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The host, lowercase, without a port.
    pub host: &'a str,
    /// The Organization's name.
    pub organization: &'a str,
    /// Whether the host is the Organization's sign-in host (`hosts.idm`).
    pub sign_in: bool,
}

/// The host without a port, e.g. `idm.localhost:8080` -> `idm.localhost`.
#[must_use]
pub fn bare(host: &str) -> &str {
    host.rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(name, _)| name)
}

/// Whether `host` is `domain` or lies below it, label by label: `a.bbsk.sk` is under `bbsk.sk`,
/// `xbbsk.sk` and `bbsk.sk.evil.com` are not.
#[must_use]
pub fn under_domain(host: &str, domain: &str) -> bool {
    let domain = domain.trim_end_matches('.');
    !domain.is_empty()
        && (host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1))
}

/// The rules every host holds, seed and Portal alike.
pub fn check_labels(rules: &Rules<'_>, entry: &Entry<'_>) -> Result<(), Refusal> {
    let host = entry.host;
    let invalid = |m: String| Err(Refusal::Invalid(m));
    let plain = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    if !plain || host.len() > 253 {
        return invalid(format!(
            "host '{host}' is not a plain lowercase host name, e.g. smartcity.bbsk.sk"
        ));
    }
    if host == bare(rules.login_host) {
        return invalid(format!(
            "host '{host}' is the installation's sign-in host (login_host)"
        ));
    }
    let base = bare(rules.base_domain);
    if let Some(label) = host
        .strip_suffix(base)
        .and_then(|rest| rest.strip_suffix('.'))
        .and_then(|rest| rest.rsplit('.').next())
        && rules.reserved.contains(&label)
    {
        return invalid(format!(
            "host '{host}': the label '{label}' under {base} is reserved for the platform; choose another host"
        ));
    }
    if host.split('.').next() == Some("idm") && !entry.sign_in {
        return invalid(format!(
            "host '{host}': idm. names a sign-in host; give it as the Organization's sign-in host or choose another"
        ));
    }
    Ok(())
}

/// The rules of a write through the Portal: [`check_labels`], the minted domain and one
/// Organization per host. `existing` is every row already in the registry, removed ones
/// included (a removed host stays taken); `minted_domain` is the domain of the minted
/// Organization id the Organization links to, `None` when it has none.
pub fn check(
    rules: &Rules<'_>,
    existing: &[Entry<'_>],
    entry: &Entry<'_>,
    minted_domain: Option<&str>,
) -> Result<(), Refusal> {
    check_labels(rules, entry)?;
    let host = entry.host;
    if !under_domain(host, bare(rules.base_domain)) {
        match minted_domain {
            Some(domain) if under_domain(host, domain) => {}
            Some(domain) => {
                return Err(Refusal::Invalid(format!(
                    "host '{host}' is not under {domain}, the Organization's verified domain"
                )));
            }
            None => {
                return Err(Refusal::Invalid(format!(
                    "host '{host}' is outside {}: verify the Organization's domain first",
                    bare(rules.base_domain)
                )));
            }
        }
    }
    if let Some(row) = existing.iter().find(|row| row.host == host) {
        return Err(Refusal::Conflict(
            if row.organization == entry.organization {
                format!("host '{host}' is in the registry already")
            } else {
                format!("host '{host}' is a host of another Organization")
            },
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: Rules<'static> = Rules {
        base_domain: "jcdev.example",
        login_host: "idm.jcdev.example",
        reserved: &["api", "login", "idm", "opendata"],
    };

    fn entry(host: &str) -> Entry<'_> {
        Entry {
            host,
            organization: "bbsk",
            sign_in: false,
        }
    }

    #[test]
    fn a_host_outside_the_minted_domain_is_422() {
        for host in ["smartcity.other.sk", "xbbsk.sk", "bbsk.sk.evil.com"] {
            let refused = check(&RULES, &[], &entry(host), Some("bbsk.sk")).unwrap_err();
            assert_eq!(refused.status(), 422, "{host}: {refused}");
            assert!(
                refused.to_string().contains("not under bbsk.sk"),
                "{refused}"
            );
        }
        for host in ["bbsk.sk", "smartcity.bbsk.sk", "a.b.bbsk.sk"] {
            check(&RULES, &[], &entry(host), Some("bbsk.sk")).expect(host);
        }
        let none = check(&RULES, &[], &entry("smartcity.bbsk.sk"), None).unwrap_err();
        assert_eq!(none.status(), 422);
        assert!(
            none.to_string()
                .contains("verify the Organization's domain")
        );
    }

    #[test]
    fn a_reserved_label_under_the_installation_domain_is_422() {
        for host in [
            "api.jcdev.example",
            "login.jcdev.example",
            "x.api.jcdev.example",
        ] {
            let refused = check_labels(&RULES, &entry(host)).unwrap_err();
            assert_eq!(refused.status(), 422, "{host}");
            assert!(
                refused.to_string().contains("reserved for the platform"),
                "{refused}"
            );
        }
        check_labels(&RULES, &entry("smartcity.bbsk.jcdev.example")).expect("own label");
        check_labels(&RULES, &entry("api.bbsk.sk")).expect("a customer's own api. host");
    }

    #[test]
    fn idm_is_a_sign_in_host_alone_and_never_the_installations() {
        let idm = |sign_in| Entry {
            host: "idm.bbsk.sk",
            organization: "bbsk",
            sign_in,
        };
        assert_eq!(check_labels(&RULES, &idm(false)).unwrap_err().status(), 422);
        check_labels(&RULES, &idm(true)).expect("sign-in host");
        let login = Entry {
            host: "idm.jcdev.example",
            organization: "bbsk",
            sign_in: true,
        };
        assert!(
            check_labels(&RULES, &login)
                .unwrap_err()
                .to_string()
                .contains("login_host")
        );
    }

    #[test]
    fn a_host_of_another_organization_is_409() {
        let taken = Entry {
            host: "smartcity.hel.jcdev.example",
            organization: "hel",
            sign_in: false,
        };
        let refused = check(&RULES, &[taken], &entry(taken.host), None).unwrap_err();
        assert_eq!(refused.status(), 409);
        assert!(refused.to_string().contains("another Organization"));
        let own = check(&RULES, &[entry(taken.host)], &entry(taken.host), None).unwrap_err();
        assert_eq!(own.status(), 409);
    }

    #[test]
    fn hosts_are_plain_and_lowercase() {
        for host in [
            "Smartcity.bbsk.sk",
            "a..bbsk.sk",
            "-a.bbsk.sk",
            "a_b.bbsk.sk",
            "",
        ] {
            assert_eq!(
                check_labels(&RULES, &entry(host)).unwrap_err().status(),
                422,
                "{host}"
            );
        }
        assert_eq!(bare("idm.localhost:8080"), "idm.localhost");
        assert!(under_domain("a.bbsk.sk", "bbsk.sk."));
        assert!(!under_domain("bbsk.sk", ""));
    }
}
