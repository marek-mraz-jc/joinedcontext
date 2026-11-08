//! First-start setup: the secrets `jc` generates for its modules and the kaisatsu realm the Portal
//! signs in through.
//!
//! Every generated secret is one 0600 file in one 0700 directory (`state_dir`, `JCBIN_STATE_DIR`).
//! A file that exists is read, never rewritten, so a restart keeps every secret (idempotent). A
//! secret value is never logged, never put into an error and never written into a config file:
//! errors name the file only, and the realm file names its secrets as `$(env:...)` placeholders
//! the importer fills from memory.

use crate::app::StartError;
use crate::module::Module;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The realm the Portal signs in through when kaisatsu runs beside it.
pub const PORTAL_REALM: &str = "jc";
/// The Portal's sign-in client in kaisatsu's shipped realm file (confidential, code + PKCE).
pub const PORTAL_CLIENT: &str = "portal-api";
/// The Portal's people-administration client (service account only).
pub const PORTAL_ADMIN_CLIENT: &str = "portal-reconciler";
/// The first person, a member of the Portal's bootstrap group.
pub const FIRST_ADMIN: &str = "admin";
/// The Portal's default bootstrap group (`JC_PORTAL_BOOTSTRAP_ADMINS`).
pub const BOOTSTRAP_GROUP: &str = "platform-admins";
/// The shako owner of the Portal's organization repository and of its Apps.
pub const FORGE_OWNER: &str = "jc";
pub const FORGE_APPS_OWNER: &str = "jc-apps";
/// The organization repository the Portal writes manifests to.
pub const FORGE_REPO: &str = "config";
/// The daiya user the Portal signs in with (HTTP Basic, as to Grafana).
pub const DASHBOARDS_USER: &str = "portal";

/// The kinds of secret `jc` generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Secret {
    /// kaisatsu key-encryption key: 32 bytes as 64 hex characters.
    KaisatsuKek,
    PortalClientSecret,
    PortalAdminClientSecret,
    /// The Portal's cookie key: 64 random bytes as 128 hex characters (it asks for 64+ bytes).
    PortalCookieKey,
    FirstAdminPassword,
    ShakoToken,
    ForgeWebhookSecret,
    EkibenToken,
    AntaresAdminToken,
    DashboardsPassword,
    CatalogueToken,
}

impl Secret {
    pub const ALL: [Secret; 11] = [
        Secret::KaisatsuKek,
        Secret::PortalClientSecret,
        Secret::PortalAdminClientSecret,
        Secret::PortalCookieKey,
        Secret::FirstAdminPassword,
        Secret::ShakoToken,
        Secret::ForgeWebhookSecret,
        Secret::EkibenToken,
        Secret::AntaresAdminToken,
        Secret::DashboardsPassword,
        Secret::CatalogueToken,
    ];

    /// The credential the Portal calls `module` with, for the modules a role split may place in
    /// another container.
    #[must_use]
    pub fn service_credential(module: Module) -> Option<Secret> {
        match module {
            Module::Shako => Some(Secret::ShakoToken),
            Module::Ekiben => Some(Secret::EkibenToken),
            Module::Antares => Some(Secret::AntaresAdminToken),
            Module::Daiya => Some(Secret::DashboardsPassword),
            Module::Dataroom => Some(Secret::CatalogueToken),
            Module::Kaisatsu | Module::Portal => None,
        }
    }

    /// File name inside the state directory.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Secret::KaisatsuKek => "kaisatsu-kek",
            Secret::PortalClientSecret => "portal-client-secret",
            Secret::PortalAdminClientSecret => "portal-admin-client-secret",
            Secret::PortalCookieKey => "portal-cookie-key",
            Secret::FirstAdminPassword => "first-admin-password",
            Secret::ShakoToken => "shako-token",
            Secret::ForgeWebhookSecret => "forge-webhook-secret",
            Secret::EkibenToken => "ekiben-token",
            Secret::AntaresAdminToken => "antares-admin-token",
            Secret::DashboardsPassword => "dashboards-password",
            Secret::CatalogueToken => "catalogue-token",
        }
    }

    fn random_bytes(self) -> usize {
        match self {
            Secret::PortalCookieKey => 64,
            _ => 32,
        }
    }

    /// The modules that must all be in the role set for `jc` to generate this secret: the
    /// module that holds it, and for a Portal credential the Portal too.
    #[must_use]
    pub fn needs(self) -> &'static [Module] {
        match self {
            Secret::KaisatsuKek => &[Module::Kaisatsu],
            Secret::PortalClientSecret
            | Secret::PortalAdminClientSecret
            | Secret::PortalCookieKey
            | Secret::FirstAdminPassword => &[Module::Kaisatsu, Module::Portal],
            Secret::ShakoToken => &[Module::Shako],
            Secret::ForgeWebhookSecret => &[Module::Shako, Module::Portal],
            Secret::EkibenToken => &[Module::Ekiben],
            Secret::AntaresAdminToken => &[Module::Antares],
            Secret::DashboardsPassword => &[Module::Daiya, Module::Portal],
            Secret::CatalogueToken => &[Module::Dataroom, Module::Portal],
        }
    }

    /// Whether the secret lives in a 0600 file of the state directory, kept across restarts.
    /// The service tokens between modules of one process (the Portal's calls to shako, ekiben,
    /// daiya and dataroom) are made per boot and held in memory only; so is the Antares admin
    /// token while no Portal runs beside it (the Portal reads that one from a file,
    /// `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE`). Keys, client secrets kaisatsu stores, the cookie
    /// key and the webhook secret shako stores with its hooks must survive a restart.
    #[must_use]
    pub fn kept_on_disk(self, roles: &[Module]) -> bool {
        match self {
            Secret::ShakoToken
            | Secret::EkibenToken
            | Secret::DashboardsPassword
            | Secret::CatalogueToken => false,
            Secret::AntaresAdminToken => roles.contains(&Module::Portal),
            _ => true,
        }
    }
}

/// One secret setup holds: its value, where it came from and, when it lives in a file, the
/// file. `Debug` never prints the value.
#[derive(Debug, Clone)]
pub struct Held {
    pub path: Option<PathBuf>,
    pub value: jc_secret::Secret,
    pub source: jc_secret::Source,
}

/// A secret the operator gave (config file, `NAME` or `NAME_FILE`), which setup uses instead of
/// generating one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Given {
    pub secret: jc_secret::Secret,
    pub source: jc_secret::Source,
    /// The file it was read from, when it came from one.
    pub path: Option<PathBuf>,
}

/// The state directory and the secrets held for the modules in the role set.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    pub dir: PathBuf,
    held: BTreeMap<&'static str, Held>,
    /// The age identity that decrypts the projects' SOPS secrets, checked at start: the Portal
    /// reads it per sync to resolve each project's `secretRef`s.
    pub age_key_file: Option<PathBuf>,
}

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::Setup(e.to_string())
}

/// Creates `dir` (and parents) as 0700, or tightens an existing one to 0700.
pub fn private_dir(dir: &Path) -> Result<(), StartError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|e| err(format!("cannot create state dir {}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| err(format!("cannot make {} private: {e}", dir.display())))?;
    }
    Ok(())
}

/// Reads the secret in `path`, or writes a fresh one there first (0600, created exclusively, so
/// two starts never write two values).
fn read_or_create(path: &Path, bytes: usize) -> Result<jc_secret::Secret, StartError> {
    if !path.exists() {
        let value = jc_secret::Secret::generate(bytes).map_err(err)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(mut file) => {
                file.write_all(value.expose().as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|e| err(format!("cannot write {}: {e}", path.display())))?;
                return Ok(value);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(format!("cannot create {}: {e}", path.display()))),
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    jc_secret::read_file(name.as_deref().unwrap_or("secret"), path).map_err(err)
}

/// Removes a file an older `jc` wrote for a secret that is now held in memory only, so no stale
/// credential stays on disk.
fn remove_stale(path: &Path) -> Result<(), StartError> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            tracing::info!(file = %path.display(), "removed a secret file that is now held in memory");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(err(format!("cannot remove {}: {e}", path.display()))),
    }
}

impl Setup {
    /// Opens `dir` (0700) and holds every secret the modules in `roles` need: the one the
    /// operator gave (`given`), else the one in its state file (created on first start), else,
    /// for a secret held in memory only, a fresh one per boot.
    pub fn prepare(
        dir: &Path,
        roles: &[Module],
        given: &BTreeMap<Secret, Given>,
    ) -> Result<Self, StartError> {
        Self::prepare_with(dir, roles, given, &[])
    }

    /// As [`Setup::prepare`], also holding the service credentials in `split` although the
    /// Portal or the module is not in `roles`: in a role split the module's container holds the
    /// credential its door swaps a verified token for, and the Portal's container the one its
    /// loopback forwarder swaps for a token ([`Secret::service_credential`]).
    pub fn prepare_with(
        dir: &Path,
        roles: &[Module],
        given: &BTreeMap<Secret, Given>,
        split: &[Secret],
    ) -> Result<Self, StartError> {
        private_dir(dir)?;
        let mut held = BTreeMap::new();
        for secret in Secret::ALL {
            let path = dir.join(secret.file_name());
            let on_disk = secret.kept_on_disk(roles);
            if !on_disk {
                remove_stale(&path)?;
            }
            if !secret.needs().iter().all(|m| roles.contains(m)) && !split.contains(&secret) {
                continue;
            }
            let entry = match given.get(&secret) {
                Some(g) => {
                    if secret == Secret::AntaresAdminToken && on_disk && g.path.is_none() {
                        return Err(err(
                            "the Portal reads the Antares admin token from a file: give it as \
                             JCBIN_ANTARES_ADMIN_TOKEN_FILE, not JCBIN_ANTARES_ADMIN_TOKEN",
                        ));
                    }
                    Held {
                        path: g.path.clone(),
                        value: g.secret.clone(),
                        source: g.source,
                    }
                }
                None if on_disk => Held {
                    value: read_or_create(&path, secret.random_bytes())?,
                    path: Some(path),
                    source: jc_secret::Source::Generated,
                },
                None => Held {
                    path: None,
                    value: jc_secret::Secret::generate(secret.random_bytes()).map_err(err)?,
                    source: jc_secret::Source::Generated,
                },
            };
            held.insert(secret.file_name(), entry);
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            held,
            age_key_file: None,
        })
    }

    /// The secret, when its module is in the role set.
    #[must_use]
    pub fn get(&self, secret: Secret) -> Option<&Held> {
        self.held.get(secret.file_name())
    }

    /// The file of the secret, when its module is in the role set and it lives in one.
    #[must_use]
    pub fn path(&self, secret: Secret) -> Option<&Path> {
        self.get(secret).and_then(|h| h.path.as_deref())
    }

    /// The value of the secret, when its module is in the role set.
    #[must_use]
    pub fn value(&self, secret: Secret) -> Option<&str> {
        self.get(secret).map(|h| h.value.expose())
    }

    /// Name and source of every secret held, for the start report.
    pub fn report(&self) -> impl Iterator<Item = (&'static str, jc_secret::Source)> + '_ {
        self.held.iter().map(|(name, h)| (*name, h.source))
    }
}

/// The importer variables the realm files name (`$(NAME)`), filled from memory.
#[must_use]
pub fn realm_env(setup: &Setup) -> BTreeMap<String, String> {
    [
        ("CLIENT_SECRET_PORTAL_API", Secret::PortalClientSecret),
        (
            "CLIENT_SECRET_PORTAL_RECONCILER",
            Secret::PortalAdminClientSecret,
        ),
        ("JC_SETUP_FIRST_ADMIN_PASSWORD", Secret::FirstAdminPassword),
    ]
    .into_iter()
    .filter_map(|(name, secret)| Some((name.to_owned(), setup.value(secret)?.to_owned())))
    .collect()
}

/// kaisatsu's shipped realm file for the Portal (`crates/kaisatsu/import/realms/jc-portal.json`).
const SHIPPED_REALM: &str = include_str!("../../../crates/kaisatsu/import/realms/jc-portal.json");
/// The Portal base the shipped realm file is written for.
const SHIPPED_HOST: &str = "https://portal.jc.localhost";

/// The realm files the Portal signs in through: kaisatsu's shipped `jc-portal.json` with its
/// placeholder host turned into `portal_base` (the Portal's public base with a trailing slash),
/// and the first administrator, a member of the bootstrap group. The shipped file holds the least
/// privilege the Portal needs: `portal-reconciler` has `manage-users` and `query-groups`,
/// `portal-api` `view-clients`, `query-users` and `query-groups`, no realm-admin.
pub fn portal_realm_files(portal_base: &str) -> Result<Vec<(String, Value)>, serde_json::Error> {
    let shipped = SHIPPED_REALM.replace(SHIPPED_HOST, portal_base.trim_end_matches('/'));
    let first_admin = json!({
        "realm": PORTAL_REALM,
        "users": [{
            "username": FIRST_ADMIN,
            "enabled": true,
            "email": "admin@jc.invalid",
            "emailVerified": true,
            "firstName": "First",
            "lastName": "Administrator",
            "groups": [format!("/{BOOTSTRAP_GROUP}")],
            "credentials": [{
                "type": "password",
                "value": "$(env:JC_SETUP_FIRST_ADMIN_PASSWORD)",
                "temporary": false
            }]
        }]
    });
    Ok(vec![
        (
            "10-jc-portal.json".to_owned(),
            serde_json::from_str(&shipped)?,
        ),
        ("20-jc-first-admin.json".to_owned(), first_admin),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn none() -> BTreeMap<Secret, Given> {
        BTreeMap::new()
    }

    #[test]
    fn secrets_are_private_files_in_a_private_dir_and_kept_across_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state");
        let first = Setup::prepare(&dir, &Module::ALL, &none()).unwrap();
        #[cfg(unix)]
        assert_eq!(mode(&dir), 0o700);
        for secret in Secret::ALL {
            let held = first.get(secret).unwrap();
            assert_eq!(held.value.expose().len(), secret.random_bytes() * 2);
            assert_eq!(held.source, jc_secret::Source::Generated);
            if secret.kept_on_disk(&Module::ALL) {
                #[cfg(unix)]
                assert_eq!(
                    mode(held.path.as_ref().unwrap()),
                    0o600,
                    "{}",
                    secret.file_name()
                );
            }
        }
        assert_ne!(
            first.value(Secret::ShakoToken),
            first.value(Secret::EkibenToken)
        );
        let again = Setup::prepare(&dir, &Module::ALL, &none()).unwrap();
        for secret in Secret::ALL {
            if secret.kept_on_disk(&Module::ALL) {
                assert_eq!(first.value(secret), again.value(secret), "kept on restart");
            }
        }
        // Debug never shows a value.
        let shown = format!("{first:?}");
        for secret in Secret::ALL {
            assert!(!shown.contains(first.value(secret).unwrap()), "{secret:?}");
        }
    }

    #[test]
    fn in_process_service_tokens_are_not_on_disk_and_change_per_boot() {
        let tmp = tempfile::tempdir().unwrap();
        let in_memory = [
            Secret::ShakoToken,
            Secret::EkibenToken,
            Secret::DashboardsPassword,
            Secret::CatalogueToken,
        ];
        // A file an older jc wrote is removed, never read.
        std::fs::write(tmp.path().join("shako-token"), "old-token-from-disk").unwrap();
        let first = Setup::prepare(tmp.path(), &Module::ALL, &none()).unwrap();
        let again = Setup::prepare(tmp.path(), &Module::ALL, &none()).unwrap();
        for secret in in_memory {
            assert!(first.path(secret).is_none(), "{secret:?}");
            assert!(!tmp.path().join(secret.file_name()).exists(), "{secret:?}");
            assert_ne!(
                first.value(secret),
                again.value(secret),
                "{secret:?} per boot"
            );
        }
        assert_ne!(first.value(Secret::ShakoToken), Some("old-token-from-disk"));
        // With the Portal beside it the Antares admin token is its file; without, memory only.
        assert!(first.path(Secret::AntaresAdminToken).is_some());
        let alone = Setup::prepare(tmp.path(), &[Module::Antares], &none()).unwrap();
        assert!(alone.path(Secret::AntaresAdminToken).is_none());
        assert!(!tmp.path().join("antares-admin-token").exists());
    }

    #[test]
    fn a_given_secret_is_used_and_never_generated() {
        let tmp = tempfile::tempdir().unwrap();
        let mut given = BTreeMap::new();
        given.insert(
            Secret::EkibenToken,
            Given {
                secret: jc_secret::Secret::new("operator-token".into()),
                source: jc_secret::Source::Env,
                path: None,
            },
        );
        let setup = Setup::prepare(tmp.path(), &[Module::Ekiben], &given).unwrap();
        assert_eq!(setup.value(Secret::EkibenToken), Some("operator-token"));
        assert_eq!(
            setup.report().collect::<Vec<_>>(),
            vec![("ekiben-token", jc_secret::Source::Env)]
        );
        // The Portal reads the Antares admin token from a file: an env value is refused.
        let mut given = BTreeMap::new();
        given.insert(
            Secret::AntaresAdminToken,
            Given {
                secret: jc_secret::Secret::new("q-token-0123456789abcdef0123".into()),
                source: jc_secret::Source::Env,
                path: None,
            },
        );
        let e = Setup::prepare(tmp.path(), &[Module::Antares, Module::Portal], &given)
            .unwrap_err()
            .to_string();
        assert!(e.contains("JCBIN_ANTARES_ADMIN_TOKEN_FILE"), "{e}");
        assert!(!e.contains("q-token"), "{e}");
    }

    #[test]
    fn only_the_modules_in_the_role_set_get_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(tmp.path(), &[Module::Daiya], &none()).unwrap();
        assert!(
            setup.get(Secret::DashboardsPassword).is_none(),
            "no Portal, no credential"
        );
        let setup = Setup::prepare(tmp.path(), &[Module::Daiya, Module::Portal], &none()).unwrap();
        assert!(setup.get(Secret::DashboardsPassword).is_some());
        assert!(setup.get(Secret::ShakoToken).is_none());
        assert!(!tmp.path().join("shako-token").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_open_state_dir_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        Setup::prepare(tmp.path(), &[], &none()).unwrap();
        assert_eq!(mode(tmp.path()), 0o700);
    }

    #[test]
    fn an_unreadable_secret_names_the_file_not_a_value() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("kaisatsu-kek"), "  \n").unwrap();
        let e = Setup::prepare(tmp.path(), &[Module::Kaisatsu], &none())
            .unwrap_err()
            .to_string();
        assert!(e.contains("kaisatsu-kek"), "{e}");
    }

    #[test]
    fn the_realm_holds_no_literal_secret_and_least_privilege_roles() {
        let files = portal_realm_files("https://city.example/").unwrap();
        let text = serde_json::to_string(&files).unwrap();
        assert!(!text.contains("realm-admin"), "no realm-admin");
        assert!(
            !text.contains(SHIPPED_HOST),
            "the placeholder host is replaced"
        );
        assert!(text.contains("https://city.example/api/v1/auth/callback"));
        let realm = &files[0].1;
        for client in realm["clients"].as_array().unwrap() {
            if let Some(secret) = client.get("secret") {
                assert!(secret.as_str().unwrap().starts_with("$("), "{client}");
            }
        }
        let reconciler = realm["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["serviceAccountClientId"] == PORTAL_ADMIN_CLIENT)
            .expect("the reconciler's service account");
        assert_eq!(
            reconciler["clientRoles"]["realm-management"],
            json!(["manage-users", "query-groups"])
        );
        let tmp = tempfile::tempdir().unwrap();
        let setup =
            Setup::prepare(tmp.path(), &[Module::Kaisatsu, Module::Portal], &none()).unwrap();
        let env = realm_env(&setup);
        assert_eq!(env.len(), 3);
        for name in env.keys() {
            assert!(
                text.contains(&format!("$({name})")) || text.contains(&format!("$(env:{name})")),
                "{name}"
            );
        }
    }
}
