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
/// The Portal's sign-in client (confidential, code + PKCE, service account for client writes).
pub const PORTAL_CLIENT: &str = "portal";
/// The Portal's silent sign-in check client (public, PKCE).
pub const PORTAL_UI_CLIENT: &str = "portal-ui";
/// The Portal's people-administration client (service account only).
pub const PORTAL_ADMIN_CLIENT: &str = "portal-admin";
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// One generated (or kept) secret: its file and its value. `Debug` never prints the value.
#[derive(Clone)]
pub struct Held {
    pub path: PathBuf,
    pub value: String,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("path", &self.path)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// The state directory and the secrets held for the modules in the role set.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    pub dir: PathBuf,
    held: BTreeMap<&'static str, Held>,
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

fn random_hex(bytes: usize) -> Result<String, StartError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| err(format!("no randomness: {e}")))?;
    Ok(hex::encode(buf))
}

/// Reads the secret in `path`, or writes a fresh one there first (0600, created exclusively, so
/// two starts never write two values).
fn read_or_create(path: &Path, bytes: usize) -> Result<String, StartError> {
    if !path.exists() {
        let value = random_hex(bytes)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(mut file) => {
                file.write_all(value.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|e| err(format!("cannot write {}: {e}", path.display())))?;
                return Ok(value);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(format!("cannot create {}: {e}", path.display()))),
        }
    }
    crate::runtime::read_secret_file(path).map_err(err)
}

impl Setup {
    /// Opens `dir` (0700) and holds every secret the modules in `roles` need, creating the
    /// missing ones.
    pub fn prepare(dir: &Path, roles: &[Module]) -> Result<Self, StartError> {
        private_dir(dir)?;
        let mut held = BTreeMap::new();
        for secret in Secret::ALL {
            if !secret.needs().iter().all(|m| roles.contains(m)) {
                continue;
            }
            let path = dir.join(secret.file_name());
            let value = read_or_create(&path, secret.random_bytes())?;
            held.insert(secret.file_name(), Held { path, value });
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            held,
        })
    }

    /// The secret, when its module is in the role set.
    #[must_use]
    pub fn get(&self, secret: Secret) -> Option<&Held> {
        self.held.get(secret.file_name())
    }

    /// The file of the secret, when its module is in the role set.
    #[must_use]
    pub fn path(&self, secret: Secret) -> Option<&Path> {
        self.get(secret).map(|h| h.path.as_path())
    }

    /// The value of the secret, when its module is in the role set.
    #[must_use]
    pub fn value(&self, secret: Secret) -> Option<&str> {
        self.get(secret).map(|h| h.value.as_str())
    }
}

/// The importer variables the generated realm file names (`$(env:NAME)`), filled from memory.
#[must_use]
pub fn realm_env(setup: &Setup) -> BTreeMap<String, String> {
    [
        ("JC_SETUP_PORTAL_CLIENT_SECRET", Secret::PortalClientSecret),
        (
            "JC_SETUP_PORTAL_ADMIN_CLIENT_SECRET",
            Secret::PortalAdminClientSecret,
        ),
        ("JC_SETUP_FIRST_ADMIN_PASSWORD", Secret::FirstAdminPassword),
    ]
    .into_iter()
    .filter_map(|(name, secret)| Some((name.to_owned(), setup.value(secret)?.to_owned())))
    .collect()
}

/// The kaisatsu realm the Portal signs in through, as a Keycloak realm file. `portal_base` is the
/// Portal's public base with a trailing slash (`https://city.example/portal/`).
///
/// Least privilege (kaisatsu `core/src/authz.rs`, Portal `config.rs` on the admin client):
/// - `portal-admin` holds `manage-users` and `query-groups` of `realm-management`, nothing else;
/// - `portal` (sign-in) holds `view-clients`, which kaisatsu narrows to the clients the Portal
///   manages (`managed-by: joinedcontext`, K3); no realm-admin anywhere.
#[must_use]
pub fn portal_realm(portal_base: &str) -> Value {
    let groups_mapper = json!({
        "name": "groups",
        "protocol": "openid-connect",
        "protocolMapper": "oidc-group-membership-mapper",
        "config": {
            "claim.name": "groups",
            "full.path": "true",
            "id.token.claim": "true",
            "access.token.claim": "true",
            "userinfo.token.claim": "true"
        }
    });
    json!({
        "realm": PORTAL_REALM,
        "enabled": true,
        "displayName": "joinedcontext",
        "groups": [{"name": BOOTSTRAP_GROUP}],
        "clients": [
            {
                "clientId": PORTAL_CLIENT,
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": false,
                "standardFlowEnabled": true,
                "serviceAccountsEnabled": true,
                "secret": "$(env:JC_SETUP_PORTAL_CLIENT_SECRET)",
                "redirectUris": [format!("{portal_base}api/v1/auth/callback")],
                "attributes": {
                    "pkce.code.challenge.method": "S256",
                    "post.logout.redirect.uris": portal_base,
                    "standard.token.exchange.enabled": "true"
                },
                "protocolMappers": [groups_mapper]
            },
            {
                "clientId": PORTAL_UI_CLIENT,
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": true,
                "standardFlowEnabled": true,
                "redirectUris": [format!("{portal_base}api/v1/auth/sso-check/done")],
                "attributes": {"pkce.code.challenge.method": "S256"}
            },
            {
                "clientId": PORTAL_ADMIN_CLIENT,
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": false,
                "standardFlowEnabled": false,
                "serviceAccountsEnabled": true,
                "secret": "$(env:JC_SETUP_PORTAL_ADMIN_CLIENT_SECRET)"
            }
        ],
        "users": [
            {
                "username": format!("service-account-{PORTAL_ADMIN_CLIENT}"),
                "enabled": true,
                "serviceAccountClientId": PORTAL_ADMIN_CLIENT,
                "clientRoles": {"realm-management": ["manage-users", "query-groups"]}
            },
            {
                "username": format!("service-account-{PORTAL_CLIENT}"),
                "enabled": true,
                "serviceAccountClientId": PORTAL_CLIENT,
                "clientRoles": {"realm-management": ["view-clients"]}
            },
            {
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
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn secrets_are_private_files_in_a_private_dir_and_kept_across_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state");
        let first = Setup::prepare(&dir, &Module::ALL).unwrap();
        #[cfg(unix)]
        assert_eq!(mode(&dir), 0o700);
        for secret in Secret::ALL {
            let held = first.get(secret).unwrap();
            #[cfg(unix)]
            assert_eq!(mode(&held.path), 0o600, "{}", secret.file_name());
            assert_eq!(held.value.len(), secret.random_bytes() * 2);
        }
        assert_ne!(
            first.value(Secret::ShakoToken),
            first.value(Secret::EkibenToken)
        );
        let again = Setup::prepare(&dir, &Module::ALL).unwrap();
        for secret in Secret::ALL {
            assert_eq!(first.value(secret), again.value(secret), "kept on restart");
        }
        // Debug never shows a value.
        let shown = format!("{first:?}");
        assert!(!shown.contains(first.value(Secret::ShakoToken).unwrap()));
    }

    #[test]
    fn only_the_modules_in_the_role_set_get_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(tmp.path(), &[Module::Daiya]).unwrap();
        assert!(
            setup.get(Secret::DashboardsPassword).is_none(),
            "no Portal, no credential"
        );
        let setup = Setup::prepare(tmp.path(), &[Module::Daiya, Module::Portal]).unwrap();
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
        Setup::prepare(tmp.path(), &[]).unwrap();
        assert_eq!(mode(tmp.path()), 0o700);
    }

    #[test]
    fn an_unreadable_secret_names_the_file_not_a_value() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ekiben-token"), "  \n").unwrap();
        let e = Setup::prepare(tmp.path(), &[Module::Ekiben])
            .unwrap_err()
            .to_string();
        assert!(e.contains("ekiben-token"), "{e}");
    }

    #[test]
    fn the_realm_holds_no_literal_secret_and_least_privilege_roles() {
        let realm = portal_realm("https://city.example/portal/");
        let text = realm.to_string();
        assert!(!text.contains("realm-admin"), "no realm-admin");
        assert!(text.contains("https://city.example/portal/api/v1/auth/callback"));
        for client in realm["clients"].as_array().unwrap() {
            if let Some(secret) = client.get("secret") {
                assert!(secret.as_str().unwrap().starts_with("$(env:"));
            }
        }
        let admin = &realm["users"][0]["clientRoles"]["realm-management"];
        assert_eq!(admin, &json!(["manage-users", "query-groups"]));
        let tmp = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(tmp.path(), &[Module::Kaisatsu, Module::Portal]).unwrap();
        let env = realm_env(&setup);
        assert_eq!(env.len(), 3);
        for name in env.keys() {
            assert!(text.contains(&format!("$(env:{name})")), "{name}");
        }
    }
}
