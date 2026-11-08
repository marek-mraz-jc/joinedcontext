//! The modules `jc` can host, their default slug (one-domain mode) and host label (host-per-module
//! mode), and the parsing of `--role all|<module>[,<module>]`.

use std::fmt;
use std::str::FromStr;

/// A module of the platform that `jc` can mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Module {
    Kaisatsu,
    Dataroom,
    Daiya,
    Ekiben,
    Antares,
    Portal,
}

impl Module {
    /// Every module in mount order.
    pub const ALL: [Module; 6] = [
        Module::Kaisatsu,
        Module::Dataroom,
        Module::Daiya,
        Module::Ekiben,
        Module::Antares,
        Module::Portal,
    ];

    /// The module's name as `--role` and the config file spell it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Module::Kaisatsu => "kaisatsu",
            Module::Dataroom => "dataroom",
            Module::Daiya => "daiya",
            Module::Ekiben => "ekiben",
            Module::Antares => "antares",
            Module::Portal => "portal",
        }
    }

    /// Default path slug in one-domain mode (`https://city.example/<slug>/...`).
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Module::Kaisatsu => "id",
            Module::Dataroom => "opendata",
            Module::Daiya => "dashboards",
            Module::Ekiben => "streams",
            Module::Antares => "broker",
            Module::Portal => "portal",
        }
    }

    /// Default host label in host-per-module mode (`https://<label>.city.example/...`).
    #[must_use]
    pub fn host_label(self) -> &'static str {
        self.slug()
    }

    /// Whether this build of `jc` carries the module (each module is a Cargo feature).
    #[must_use]
    pub fn compiled_in(self) -> bool {
        match self {
            Module::Kaisatsu => cfg!(feature = "kaisatsu"),
            Module::Dataroom => cfg!(feature = "dataroom"),
            Module::Daiya => cfg!(feature = "daiya"),
            Module::Ekiben => cfg!(feature = "ekiben"),
            Module::Antares => cfg!(feature = "antares"),
            Module::Portal => cfg!(feature = "portal"),
        }
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Error of [`Module::from_str`] and [`parse_roles`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoleError {
    #[error("unknown module '{0}' (known: kaisatsu, dataroom, daiya, ekiben, antares, portal)")]
    Unknown(String),
    #[error("module '{0}' is not compiled into this jc binary (Cargo feature '{0}')")]
    NotCompiled(String),
    #[error("--role names no module")]
    Empty,
}

impl FromStr for Module {
    type Err = RoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim().to_ascii_lowercase();
        Module::ALL
            .into_iter()
            .find(|m| m.name() == wanted)
            .ok_or_else(|| RoleError::Unknown(s.trim().to_owned()))
    }
}

/// Parses `all` or a comma-separated module list into the modules to run, deduplicated and in mount
/// order. `all` means every module compiled into the binary; naming a module that is not compiled
/// in is refused rather than silently skipped.
pub fn parse_roles(raw: &str) -> Result<Vec<Module>, RoleError> {
    let mut out: Vec<Module> = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if part.eq_ignore_ascii_case("all") {
            out.extend(Module::ALL.into_iter().filter(|m| m.compiled_in()));
            continue;
        }
        let module: Module = part.parse()?;
        if !module.compiled_in() {
            return Err(RoleError::NotCompiled(module.name().to_owned()));
        }
        out.push(module);
    }
    if out.is_empty() {
        return Err(RoleError::Empty);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_display() {
        for m in Module::ALL {
            assert_eq!(m.name().parse::<Module>().unwrap(), m);
            assert_eq!(m.to_string(), m.name());
            assert_eq!(m.host_label(), m.slug());
            assert!(!m.slug().is_empty());
        }
        assert_eq!(" Kaisatsu ".parse::<Module>().unwrap(), Module::Kaisatsu);
    }

    #[test]
    fn unknown_module_is_refused() {
        assert_eq!(
            "keycloak".parse::<Module>(),
            Err(RoleError::Unknown("keycloak".into()))
        );
        assert_eq!(
            parse_roles("kaisatsu,keycloak"),
            Err(RoleError::Unknown("keycloak".into()))
        );
    }

    #[test]
    fn empty_role_is_refused() {
        assert_eq!(parse_roles(""), Err(RoleError::Empty));
        assert_eq!(parse_roles(" , ,"), Err(RoleError::Empty));
    }

    #[test]
    fn all_means_every_compiled_module_and_lists_dedupe() {
        let all = parse_roles("all").unwrap();
        let compiled: Vec<Module> = Module::ALL
            .into_iter()
            .filter(|m| m.compiled_in())
            .collect();
        assert_eq!(all, compiled);
        assert_eq!(parse_roles("all,ALL").unwrap(), compiled);
        let first = compiled[0];
        let twice = format!("{first},{first}");
        assert_eq!(parse_roles(&twice).unwrap(), vec![first]);
    }

    #[test]
    fn a_module_not_compiled_in_is_refused() {
        if let Some(missing) = Module::ALL.into_iter().find(|m| !m.compiled_in()) {
            assert_eq!(
                parse_roles(missing.name()),
                Err(RoleError::NotCompiled(missing.name().into()))
            );
        }
        for m in Module::ALL.into_iter().filter(|m| m.compiled_in()) {
            assert_eq!(parse_roles(m.name()).unwrap(), vec![m]);
        }
    }
}
