// SPDX-License-Identifier: EUPL-1.2
//! Who is calling, from the headers the gateway forwards (README section 1).
//! Values are read, never logged: the broker prints header names only.

use crate::document::{Assignee, Kind};
use antares_api::policy::Subject;
use std::collections::BTreeSet;

/// The headers this engine reads, in the order
/// `ANTARES_POLICY_SUBJECT_HEADERS` lists them.
pub const HEADERS: [&str; 8] = [
    "x-jc-subject",
    "x-jc-groups",
    "x-jc-roles",
    "x-jc-client",
    "x-jc-restricted",
    "x-jc-did",
    "x-jc-read-only",
    "x-jc-member",
];

/// The prefix of an Endpoint caller role, `endpoint:{project}/{name}` (AP-97).
const ENDPOINT_ROLE: &str = "endpoint:";

/// The caller of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// No `x-jc-*` header at all: a call from inside the cluster.
    Internal,
    /// A caller the gateway verified, or an anonymous one.
    Platform(Platform),
}

/// A platform caller's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Platform {
    /// `x-jc-subject`.
    pub user: Option<String>,
    /// `x-jc-client`.
    pub client: Option<String>,
    /// `x-jc-groups`.
    pub groups: BTreeSet<String>,
    /// `x-jc-roles`; for an anonymous caller `public` and the Endpoint caller roles alone.
    pub roles: BTreeSet<String>,
    /// `x-jc-restricted: true`: the caller wants a narrowed answer said.
    pub restricted: bool,
    /// `x-jc-did`: the data-space participant a transfer token names (DS-02).
    pub did: Option<String>,
    /// `x-jc-read-only` present, whatever its value: a read-only connector's token, which reads
    /// and never writes (platform S-49). Only `true` is ever sent; any other value is a
    /// malformed marker and still holds the caller to reads (deny by default).
    pub read_only: bool,
    /// `x-jc-member: true`: the gateway verified that the token's `jc_org` names the
    /// Organization of the Endpoint's project (ADR-N-067 2.3). Only `true` counts (deny by
    /// default), and never for an anonymous caller or a data-space participant.
    pub member: bool,
}

impl Caller {
    /// The caller `subject` carries.
    #[must_use]
    pub fn of(subject: &Subject) -> Self {
        let get = |name: &str| {
            subject
                .headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim())
        };
        if HEADERS.iter().all(|h| get(h).is_none()) {
            return Caller::Internal;
        }
        let named = |name| get(name).is_some_and(|v| !v.is_empty());
        let one = |name| get(name).filter(|v| !v.is_empty()).and_then(decode);
        let list = |name| -> BTreeSet<String> {
            get(name)
                .into_iter()
                .flat_map(|v| v.split(','))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .filter_map(decode)
                .collect()
        };
        // A name that does not decode is nobody, never the anonymous caller. A participant holds
        // what its agreement's Policies grant and nothing else: no role, no group (DS-02).
        let participant = named("x-jc-did");
        let anonymous = !named("x-jc-subject") && !named("x-jc-client") && !participant;
        let (user, client) = (one("x-jc-subject"), one("x-jc-client"));
        Caller::Platform(Platform {
            groups: if anonymous || participant {
                BTreeSet::new()
            } else {
                list("x-jc-groups")
            },
            roles: if anonymous {
                // AP-97: the gateway resolves an Endpoint's caller role for every caller it
                // admits, the anonymous one included; no other claimed role reaches them.
                list("x-jc-roles")
                    .into_iter()
                    .filter(|r| r.strip_prefix(ENDPOINT_ROLE).is_some_and(|n| !n.is_empty()))
                    .chain(["public".to_owned()])
                    .collect()
            } else if participant {
                BTreeSet::new()
            } else {
                list("x-jc-roles")
            },
            did: one("x-jc-did"),
            restricted: get("x-jc-restricted").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            read_only: get("x-jc-read-only").is_some(),
            member: !anonymous
                && !participant
                && get("x-jc-member").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            user,
            client,
        })
    }
}

/// One percent-encoded UTF-8 value (README section 1), or `None` when an escape is broken or
/// the bytes are not UTF-8: the caller then holds that name not at all.
fn decode(value: &str) -> Option<String> {
    let mut parts = value.split('%');
    let mut bytes = parts.next().unwrap_or_default().as_bytes().to_vec();
    for part in parts {
        let hex = part
            .get(..2)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
        bytes.extend_from_slice(part.get(2..).unwrap_or_default().as_bytes());
    }
    String::from_utf8(bytes).ok()
}

impl Platform {
    /// Whether `assignee` is this caller.
    #[must_use]
    pub fn is(&self, assignee: &Assignee) -> bool {
        let id = assignee.id.as_str();
        match assignee.kind {
            Kind::User => self.user.as_deref() == Some(id),
            Kind::Group => self.groups.contains(id),
            Kind::Role => self.roles.contains(id),
            Kind::ServiceAccount => self.client.as_deref() == Some(id),
            Kind::Did => self.did.as_deref() == Some(id),
            Kind::Organization => self.member,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(headers: &[(&str, &str)]) -> Caller {
        Caller::of(&Subject {
            tenant: antares_model::TenantId::default(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        })
    }

    fn platform(c: Caller) -> Result<Platform, String> {
        match c {
            Caller::Platform(p) => Ok(p),
            Caller::Internal => Err("internal".into()),
        }
    }

    fn assignee(kind: Kind, id: &str) -> Assignee {
        Assignee {
            kind,
            id: id.to_owned(),
        }
    }

    #[test]
    fn no_header_is_internal_and_any_header_is_a_platform_caller() {
        assert_eq!(platform(caller(&[])), Err("internal".into()));
        assert_eq!(caller(&[("x-other", "1")]), Caller::Internal);
        assert!(matches!(caller(&[("X-JC-Roles", "")]), Caller::Platform(_)));
    }

    #[test]
    fn an_anonymous_caller_holds_public_and_nothing_it_claims() -> Result<(), String> {
        let p = platform(caller(&[("x-jc-roles", "admin"), ("x-jc-groups", "ops")]))?;
        assert_eq!(p.roles, BTreeSet::from(["public".to_owned()]));
        assert!(p.groups.is_empty());
        assert!(p.is(&assignee(Kind::Role, "public")));
        assert!(!p.is(&assignee(Kind::Role, "admin")));
        Ok(())
    }

    #[test]
    fn an_anonymous_caller_keeps_the_endpoint_role_the_gateway_resolved() -> Result<(), String> {
        let p = platform(caller(&[
            (
                "x-jc-roles",
                "admin,endpoint%3Ap%2Fe,endpoint:,endpointx:p/e",
            ),
            ("x-jc-groups", "ops"),
        ]))?;
        assert_eq!(
            p.roles,
            BTreeSet::from(["endpoint:p/e".to_owned(), "public".to_owned()])
        );
        assert!(p.groups.is_empty(), "AP-97 carries no group");
        assert!(p.is(&assignee(Kind::Role, "endpoint:p/e")));
        assert!(!p.is(&assignee(Kind::Role, "admin")));
        Ok(())
    }

    #[test]
    fn a_verified_caller_matches_by_each_kind() -> Result<(), String> {
        let c = caller(&[
            ("x-jc-subject", "ana@example.org"),
            ("x-jc-client", "app-1"),
            ("x-jc-groups", "ops, ,city"),
            ("x-jc-roles", "steward,endpoint:p/e"),
            ("x-jc-restricted", "TRUE"),
        ]);
        let p = platform(c)?;
        assert!(p.restricted);
        for (kind, id, yes) in [
            (Kind::User, "ana@example.org", true),
            (Kind::User, "bob@example.org", false),
            (Kind::ServiceAccount, "app-1", true),
            (Kind::ServiceAccount, "app-2", false),
            (Kind::Group, "city", true),
            (Kind::Group, "", false),
            (Kind::Role, "endpoint:p/e", true),
            (Kind::Role, "public", false),
            (Kind::Did, "did:web:x", false),
        ] {
            assert_eq!(p.is(&assignee(kind, id)), yes, "{kind:?} {id}");
        }
        let p = platform(caller(&[("x-jc-subject", "a"), ("x-jc-restricted", "yes")]))?;
        assert!(!p.restricted && p.roles.is_empty());
        Ok(())
    }

    #[test]
    fn any_read_only_marker_holds_the_caller_to_reads() -> Result<(), String> {
        let p = platform(caller(&[("x-jc-subject", "a")]))?;
        assert!(!p.read_only);
        // only `true` is sent; a malformed marker is still a marker (deny by default)
        for value in ["true", "TRUE", "false", "", "1"] {
            let p = platform(caller(&[("x-jc-subject", "a"), ("x-jc-read-only", value)]))?;
            assert!(p.read_only, "{value:?}");
        }
        // the marker alone makes a platform caller, never one from inside the cluster
        let p = platform(caller(&[("x-jc-read-only", "true")]))?;
        assert!(p.read_only && p.roles.contains("public"));
        Ok(())
    }

    #[test]
    fn a_participant_holds_its_did_and_no_role_or_group() -> Result<(), String> {
        // A transfer token establishes the participant and nothing else (DS-02): the gateway
        // sends `x-jc-did` alone, and a forged role or group beside it grants nothing.
        let p = platform(caller(&[
            ("x-jc-did", "did%3Aweb%3Aconsumer.example"),
            ("x-jc-roles", "public,steward"),
            ("x-jc-groups", "ops"),
        ]))?;
        assert_eq!(p.did.as_deref(), Some("did:web:consumer.example"));
        assert!(p.roles.is_empty() && p.groups.is_empty());
        assert!(p.is(&assignee(Kind::Did, "did:web:consumer.example")));
        assert!(!p.is(&assignee(Kind::Did, "did:web:other.example")));
        assert!(!p.is(&assignee(Kind::Role, "public")));
        let p = platform(caller(&[("x-jc-did", "did%3A%C5")]))?;
        assert_eq!(p.did, None, "an undecodable did names nobody");
        assert!(p.roles.is_empty(), "and is never the anonymous caller");
        Ok(())
    }

    #[test]
    fn percent_encoded_names_decode_per_member() -> Result<(), String> {
        let p = platform(caller(&[
            ("x-jc-subject", "jo%C5%BEo"),
            ("x-jc-groups", "doprava%2Fspr%C3%A1va,city"),
            ("x-jc-roles", "steward"),
        ]))?;
        assert_eq!(p.user.as_deref(), Some("jožo"));
        assert!(p.is(&assignee(Kind::Group, "doprava/správa")));
        assert!(p.is(&assignee(Kind::Group, "city")));
        assert!(p.is(&assignee(Kind::User, "jožo")));
        Ok(())
    }

    #[test]
    fn an_encoded_comma_stays_inside_one_name() -> Result<(), String> {
        let p = platform(caller(&[
            ("x-jc-subject", "a"),
            ("x-jc-groups", "x%2Cadmins"),
        ]))?;
        assert_eq!(p.groups, BTreeSet::from(["x,admins".to_owned()]));
        assert!(!p.is(&assignee(Kind::Group, "admins")));
        Ok(())
    }

    #[test]
    fn a_member_that_does_not_decode_is_dropped_alone() -> Result<(), String> {
        let p = platform(caller(&[
            ("x-jc-subject", "a"),
            ("x-jc-groups", "ops,bad%2,worse%ZZ,nonutf%C5,city"),
            ("x-jc-roles", "steward,%"),
        ]))?;
        assert_eq!(
            p.groups,
            BTreeSet::from(["city".to_owned(), "ops".to_owned()])
        );
        assert_eq!(p.roles, BTreeSet::from(["steward".to_owned()]));
        Ok(())
    }

    #[test]
    fn a_name_that_does_not_decode_is_nobody_and_never_anonymous() -> Result<(), String> {
        let p = platform(caller(&[
            ("x-jc-subject", "jo%C5"),
            ("x-jc-roles", "steward"),
        ]))?;
        assert_eq!(p.user, None);
        assert_eq!(p.roles, BTreeSet::from(["steward".to_owned()]));
        assert!(!p.is(&assignee(Kind::Role, "public")));
        Ok(())
    }
}
