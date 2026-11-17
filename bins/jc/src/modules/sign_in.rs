//! The Organizations' own sign-in hosts at run time (T-7180, evidence/T-7129/design.md §5): each
//! Organization with an `idm.` host signs in there, in the realm named like it, under the issuer
//! `{scheme}://{host}/realms/{organization}` from this set, never from a request's `Host`.
//!
//! jc starts on the jc.toml set ([`from_config`]). The host registry's watcher
//! (`modules::portal`) then [`publish`]es the registry's `served` `signin` rows ([`from_registry`])
//! after every rebuild, and each module that holds an issuer table (kaisatsu's pins, the
//! gateway's and the proxy's realms, the Portal's sign-in realms) replaces it from the set it is
//! handed, so a served sign-in host answers and a removed one is refused without a restart.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One Organization's own sign-in host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub organization: String,
    /// Lowercase, without a port.
    pub host: String,
}

/// Each `[[organizations]]` row with `hosts.idm`.
pub fn from_config(orgs: &[crate::config::OrganizationSection]) -> Vec<SignIn> {
    orgs.iter()
        .filter_map(|org| {
            Some(SignIn {
                organization: org.name.clone(),
                host: jc_router::registry::bare(org.hosts.idm.as_deref()?).to_ascii_lowercase(),
            })
        })
        .collect()
}

/// One registry row as [`from_registry`] reads it: host, Organization, role, state.
pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The registry's `served` `signin` rows of the Organizations jc runs (`known`), one per
/// Organization (the first row wins; the registry's check refuses a second). A row of an
/// Organization jc does not run is left out: none of its modules could serve it.
pub fn from_registry<'a>(rows: impl IntoIterator<Item = Row<'a>>, known: &[String]) -> Vec<SignIn> {
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
        });
    }
    out.sort_by(|a, b| a.organization.cmp(&b.organization));
    out
}

type Follower = Arc<dyn Fn(&[SignIn]) + Send + Sync>;

#[derive(Default)]
struct Followers {
    /// The set the registry served last; `None` before its first rebuild.
    last: Option<Vec<SignIn>>,
    /// Keyed by module, so a module built again replaces its follower.
    by_module: BTreeMap<&'static str, Follower>,
}

static FOLLOWERS: std::sync::LazyLock<Mutex<Followers>> =
    std::sync::LazyLock::new(|| Mutex::new(Followers::default()));

fn followers() -> std::sync::MutexGuard<'static, Followers> {
    FOLLOWERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Calls `follow` with every set the registry serves from now on, and at once with the last one
/// when the registry was read before this module was built.
pub fn follow(module: &'static str, follow: impl Fn(&[SignIn]) + Send + Sync + 'static) {
    let follow: Follower = Arc::new(follow);
    let last = {
        let mut held = followers();
        held.by_module.insert(module, Arc::clone(&follow));
        held.last.clone()
    };
    if let Some(last) = last {
        follow(&last);
    }
}

/// Hands `set` to every follower when it differs from the last one; `true` when it did.
pub fn publish(set: Vec<SignIn>) -> bool {
    let to_call: Vec<Follower> = {
        let mut held = followers();
        if held.last.as_ref() == Some(&set) {
            return false;
        }
        held.last = Some(set.clone());
        held.by_module.values().cloned().collect()
    };
    for follow in to_call {
        follow(&set);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};

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
                host: "idm.bbsk.sk".into()
            }]
        );
        let set = from_registry(
            [
                ("idm.hel.fi", "hel", "signin", "served"),
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
                host: "idm.hel.fi".into()
            }],
            "a removed row, another role, an unknown Organization and a second row are left out"
        );
    }

    #[test]
    fn a_follower_gets_the_last_set_at_once_and_each_change_once() {
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        let one = vec![SignIn {
            organization: "hel".into(),
            host: "idm.hel.fi".into(),
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
