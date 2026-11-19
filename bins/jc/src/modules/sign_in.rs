//! The Organizations' own sign-in hosts at run time (T-7180, evidence/T-7129/design.md §5): each
//! Organization with an `idm.` host signs in there, in the realm named like it, under the issuer
//! `{scheme}://{host}/realms/{organization}` from this set, never from a request's `Host`.
//!
//! jc starts on the jc.toml set ([`from_config`]). The host registry's watcher
//! (`modules::portal`) then [`publish`]es the registry's `served` `signin` rows ([`from_registry`])
//! after every rebuild, and each module that holds an issuer table (kaisatsu's pins, the
//! gateway's and the proxy's realms, the Portal's sign-in realms) replaces it from the set it is
//! handed, so a served sign-in host answers and a removed one is refused without a restart.

/// One Organization's own sign-in host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub organization: String,
    /// Lowercase, without a port.
    pub host: String,
    /// The Organization's working hosts, the first its Portal's, each lowercase without a port:
    /// its realm's clients redirect to these alone (T-7284). Empty while none is served, and then
    /// the realm is neither applied nor pinned.
    pub portals: Vec<String>,
}

/// Each `[[organizations]]` row with `hosts.idm`.
pub fn from_config(orgs: &[crate::config::OrganizationSection]) -> Vec<SignIn> {
    orgs.iter()
        .filter_map(|org| {
            Some(SignIn {
                organization: org.name.clone(),
                host: jc_router::registry::bare(org.hosts.idm.as_deref()?).to_ascii_lowercase(),
                portals: std::iter::once(&org.hosts.portal)
                    .chain(&org.hosts.portals)
                    .map(|h| jc_router::registry::bare(h).to_ascii_lowercase())
                    .collect(),
            })
        })
        .collect()
}

/// One registry row as [`from_registry`] reads it: host, Organization, role, state.
#[cfg(feature = "portal")]
pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The registry's `served` `signin` rows of the Organizations jc runs (`known`), one per
/// Organization (the first row wins; the registry's check refuses a second), each with the
/// working hosts the registry serves for it (`app::registry_hosts`: the first working row, then
/// each `portal-site`). A row of an Organization jc does not run is left out: none of its modules
/// could serve it.
#[cfg(feature = "portal")]
pub fn from_registry<'a>(rows: impl IntoIterator<Item = Row<'a>>, known: &[String]) -> Vec<SignIn> {
    let rows: Vec<Row<'a>> = rows.into_iter().collect();
    let served = crate::app::registry_hosts(
        rows.iter()
            .filter(|(_, _, _, state)| *state == "served")
            .map(|&(host, organization, role, _)| (host, organization, role)),
    );
    let mut out: Vec<SignIn> = Vec::new();
    for (host, organization, role, state) in rows {
        if role != jc_router::HostRole::SignIn.as_str()
            || state != "served"
            || !known.iter().any(|o| o == organization)
            || out.iter().any(|s| s.organization == organization)
        {
            continue;
        }
        out.push(SignIn {
            organization: organization.to_owned(),
            host: jc_router::registry::bare(host).to_ascii_lowercase(),
            portals: served.get(organization).map_or_else(Vec::new, |h| {
                std::iter::once(&h.portal)
                    .filter(|p| !p.is_empty())
                    .chain(&h.portals)
                    .cloned()
                    .collect()
            }),
        });
    }
    out.sort_by(|a, b| a.organization.cmp(&b.organization));
    out
}

static SIGN_IN: super::follow::Published<Vec<SignIn>> = super::follow::Published::new();

/// Calls `follow` with every set the registry serves from now on, and at once with the last one
/// (`follow::Published::follow`).
pub fn follow(module: &'static str, follow: impl Fn(&[SignIn]) + Send + Sync + 'static) {
    SIGN_IN.follow(module, move |set: &Vec<SignIn>| follow(set));
}

/// Hands `set` to every follower when it differs from the last one; `true` when it did.
#[cfg(feature = "portal")]
pub fn publish(set: Vec<SignIn>) -> bool {
    SIGN_IN.publish(set)
}

#[cfg(all(test, feature = "portal"))]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};
    use std::sync::{Arc, Mutex};

    fn orgs() -> Vec<crate::config::OrganizationSection> {
        let text = "base_host = \"city.example\"\n\
            [[organizations]]\nname = \"bbsk\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a1\"\n\
            hosts = { portal = \"smartcity.bbsk.sk\", idm = \"IDM.bbsk.sk\" }\n\
            [[organizations]]\nname = \"hel\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a2\"\n\
            hosts = { portal = \"smartcity.hel.fi\" }\n";
        let file = Config::parse_file(std::path::Path::new("jc.toml"), text).expect("parses");
        Config::merge(Cli::default(), file)
            .expect("two Organizations")
            .organizations
    }

    #[test]
    fn the_registry_set_is_its_served_sign_in_rows_of_known_organizations() {
        let orgs = orgs();
        assert_eq!(
            from_config(&orgs),
            [SignIn {
                organization: "bbsk".into(),
                host: "idm.bbsk.sk".into(),
                portals: vec!["smartcity.bbsk.sk".into()],
            }]
        );
        let set = from_registry(
            [
                ("idm.hel.fi", "hel", "signin", "served"),
                ("smartcity.hel.fi", "hel", "working", "served"),
                ("portal.hel.fi", "hel", "portal-site", "served"),
                ("old.hel.fi", "hel", "portal-site", "removed"),
                ("idm.bbsk.sk", "bbsk", "signin", "removed"),
                ("smartcity.bbsk.sk", "bbsk", "portal-site", "served"),
                ("idm.zilina.sk", "zilina", "signin", "served"),
                ("idm2.hel.fi", "hel", "signin", "served"),
                ("idm.ba.sk", "ba", "signin", "dns-ok"),
            ],
            &["bbsk".to_owned(), "hel".to_owned()],
        );
        assert_eq!(
            set,
            [SignIn {
                organization: "hel".into(),
                host: "idm.hel.fi".into(),
                portals: vec!["smartcity.hel.fi".into(), "portal.hel.fi".into()],
            }],
            "a removed row, another role, an unknown Organization and a second row are left out; \
             the served working hosts go with the sign-in host"
        );
    }

    #[test]
    fn a_follower_gets_the_last_set_at_once_and_each_change_once() {
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        let one = vec![SignIn {
            organization: "hel".into(),
            host: "idm.hel.fi".into(),
            portals: Vec::new(),
        }];
        publish(one.clone());
        let held = Arc::clone(&seen);
        follow("test-follower", move |set| {
            held.lock().unwrap().push(set.len());
        });
        assert!(!publish(one), "the same set again is not handed out");
        assert!(publish(Vec::new()));
        assert_eq!(*seen.lock().unwrap(), [1, 0]);
    }
}
