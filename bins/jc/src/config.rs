//! `jc` configuration: defaults, then an optional TOML file, then `JCBIN_*` environment variables,
//! then command-line flags (each later source wins).
//!
//! No key of `jc` itself starts with `JC_`: the Portal reads bare `JC_*` variables as its own
//! configuration, so `jc` keeps to `JCBIN_*` and the file.

use crate::module::{Module, RoleError, parse_roles};
use crate::pg::DatabaseSection;
use clap::Parser;
use ipnet::IpNet;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// How modules are placed on hosts and paths when the config gives no explicit mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    /// One domain, a path slug per module (`https://city.example/id/...`).
    #[default]
    Slug,
    /// A host per module (`https://id.city.example/...`).
    Host,
}

/// Command line of `jc`. Every flag also reads a `JCBIN_*` environment variable.
#[derive(Debug, Default, Parser)]
#[command(
    name = "jc",
    version,
    about = "joinedcontext: every module in one binary"
)]
pub struct Cli {
    /// TOML configuration file.
    #[arg(long, env = "JCBIN_CONFIG")]
    pub config: Option<PathBuf>,
    /// Modules to run: `all` or a comma-separated list (kaisatsu,dataroom,daiya,...).
    #[arg(long, env = "JCBIN_ROLE")]
    pub role: Option<String>,
    /// Address of the one public listener.
    #[arg(long, env = "JCBIN_LISTEN")]
    pub listen: Option<SocketAddr>,
    /// Mount mode for modules without an explicit mount.
    #[arg(long, env = "JCBIN_MODE", value_enum)]
    pub mode: Option<MountMode>,
    /// Public host: the one domain in slug mode, the parent domain in host mode.
    #[arg(long, env = "JCBIN_BASE_HOST")]
    pub base_host: Option<String>,
    /// Public scheme (`https` or `http`).
    #[arg(long, env = "JCBIN_SCHEME")]
    pub scheme: Option<String>,
    /// Optional internal listener for `/metrics` and `/healthz` (keep it on loopback or a
    /// cluster-internal address; it is never mounted on the public listener).
    #[arg(long, env = "JCBIN_ADMIN_LISTEN")]
    pub admin_listen: Option<SocketAddr>,
    /// File holding the bearer token `/metrics` of the admin listener asks for (required with
    /// `--admin-listen`: it carries every module's series, Antares' among them, which the
    /// standalone broker serves only behind its admin token).
    #[arg(long, env = "JCBIN_ADMIN_TOKEN_FILE")]
    pub admin_token_file: Option<PathBuf>,
    /// Proxy network (CIDR) whose `X-Forwarded-*` headers are trusted; repeat or comma-separate.
    #[arg(
        long = "trusted-proxy",
        env = "JCBIN_TRUSTED_PROXIES",
        value_delimiter = ','
    )]
    pub trusted_proxies: Vec<IpNet>,
    /// Log format: `json` or `text`.
    #[arg(long, env = "JCBIN_LOG_FORMAT")]
    pub log_format: Option<LogFormat>,
    /// kaisatsu: file holding the 32-byte key-encryption key (raw or 64 hex characters).
    #[arg(long, env = "JCBIN_KAISATSU_KEK_FILE")]
    pub kaisatsu_kek_file: Option<PathBuf>,
    /// kaisatsu: realm files (JSON) applied at start.
    #[arg(long, env = "JCBIN_KAISATSU_IMPORT_DIR")]
    pub kaisatsu_import_dir: Option<PathBuf>,
    /// kaisatsu: file holding the PostgreSQL connection URL without password (secrets mount).
    #[arg(long, env = "JCBIN_KAISATSU_DATABASE_URL_FILE")]
    pub kaisatsu_database_url_file: Option<PathBuf>,
    /// kaisatsu: file holding the PostgreSQL password (secrets mount).
    #[arg(long, env = "JCBIN_KAISATSU_DB_PASSWORD_FILE")]
    pub kaisatsu_db_password_file: Option<PathBuf>,
    /// kaisatsu: PostgreSQL connection pool size (default 5).
    #[arg(long, env = "JCBIN_KAISATSU_DB_MAX_CONNECTIONS")]
    pub kaisatsu_db_max_connections: Option<u32>,
    /// ekiben: file holding the API bearer token (required: hosted ekiben never runs without auth).
    #[arg(long, env = "JCBIN_EKIBEN_TOKEN_FILE")]
    pub ekiben_token_file: Option<PathBuf>,
    /// ekiben: the only directory `file` inputs and outputs may touch (default: a private
    /// directory under the system temp dir).
    #[arg(long, env = "JCBIN_EKIBEN_FILE_ROOT")]
    pub ekiben_file_root: Option<PathBuf>,
    /// antares: file holding the `/q` admin token (required: the hosted broker never serves its
    /// admin surface without a credential). Everything else of Antares is its own `ANTARES_*`
    /// environment, as for the standalone broker.
    #[arg(long, env = "JCBIN_ANTARES_ADMIN_TOKEN_FILE")]
    pub antares_admin_token_file: Option<PathBuf>,
    /// Directory (0700) for the secrets `jc` generates on first start, one 0600 file each
    /// (default `jc-state` in the working directory).
    #[arg(long, env = "JCBIN_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// Whether the Portal's settings default to its sibling modules in this `jc` (default
    /// true; `false` keeps the Portal on its own `JC_*` environment alone).
    #[arg(long, env = "JCBIN_PORTAL_SIBLINGS")]
    pub portal_siblings: Option<bool>,
    /// shako: directory holding bare Git repositories and package blobs.
    #[arg(long, env = "JCBIN_SHAKO_REPO_ROOT")]
    pub shako_repo_root: Option<PathBuf>,
    /// shako: file holding the ES256 signing key for registry tokens and image signatures.
    #[arg(long, env = "JCBIN_SHAKO_SIGNING_KEY_FILE")]
    pub shako_signing_key_file: Option<PathBuf>,
    /// shako: optional file holding a static service bearer token.
    #[arg(long, env = "JCBIN_SHAKO_TOKEN_FILE")]
    pub shako_token_file: Option<PathBuf>,
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Text,
}

/// An explicit mount of one module (`[mounts.<module>]` in the file).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct MountOverride {
    pub host: Option<String>,
    pub path_prefix: Option<String>,
}

/// `[kaisatsu]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KaisatsuSection {
    pub kek_file: Option<PathBuf>,
    pub import_dir: Option<PathBuf>,
    pub database_url_file: Option<PathBuf>,
    pub db_password_file: Option<PathBuf>,
    pub db_max_connections: Option<u32>,
    /// Postgres (the alternative to `database_url_file`; not both).
    pub database: Option<DatabaseSection>,
}

/// `[ekiben]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EkibenSection {
    pub token_file: Option<PathBuf>,
    pub file_root: Option<PathBuf>,
    /// Postgres for the stream definitions (else they live only in memory).
    pub database: Option<DatabaseSection>,
}

/// `[antares]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AntaresSection {
    pub admin_token_file: Option<PathBuf>,
    /// Postgres in place of `ANTARES_DATABASE_URL` (the broker migrates as this role itself).
    pub database: Option<DatabaseSection>,
}

/// `[shako]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShakoSection {
    /// Bare Git repositories and package blobs (default `<state_dir>/shako`).
    pub repo_root: Option<PathBuf>,
    pub signing_key_file: Option<PathBuf>,
    pub token_file: Option<PathBuf>,
    pub database: Option<DatabaseSection>,
}

/// `[daiya]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaiyaSection {
    pub database: Option<DatabaseSection>,
}

/// `[dataroom]` section (a key left out keeps its default).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DataroomSection {
    pub org_id: String,
    pub catalogue_id: String,
    pub database: Option<DatabaseSection>,
}

impl Default for DataroomSection {
    fn default() -> Self {
        Self {
            org_id: "00000000-0000-7000-8000-000000000001".into(),
            catalogue_id: "00000000-0000-7000-8000-000000000010".into(),
            database: None,
        }
    }
}

/// The TOML file. Unknown keys are refused so a typo never silently keeps a default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub role: Option<String>,
    pub listen: Option<SocketAddr>,
    pub mode: Option<MountMode>,
    pub base_host: Option<String>,
    pub scheme: Option<String>,
    pub admin_listen: Option<SocketAddr>,
    pub admin_token_file: Option<PathBuf>,
    #[serde(default)]
    pub trusted_proxies: Vec<IpNet>,
    pub log_format: Option<LogFormat>,
    pub state_dir: Option<PathBuf>,
    pub portal_siblings: Option<bool>,
    #[serde(default)]
    pub mounts: BTreeMap<String, MountOverride>,
    #[serde(default)]
    pub kaisatsu: KaisatsuSection,
    #[serde(default)]
    pub dataroom: Option<DataroomSection>,
    #[serde(default)]
    pub daiya: DaiyaSection,
    #[serde(default)]
    pub ekiben: EkibenSection,
    #[serde(default)]
    pub antares: AntaresSection,
    #[serde(default)]
    pub shako: ShakoSection,
    #[serde(default)]
    pub daiya: DaiyaSection,
}

/// Errors while assembling the configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("config file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error(transparent)]
    Role(#[from] RoleError),
    #[error("[mounts.{0}] names no module of this jc build")]
    UnknownMount(String),
    #[error("scheme must be http or https, not '{0}'")]
    Scheme(String),
    #[error("base host must be a plain host name, not '{0}'")]
    BaseHost(String),
    #[error("the admin listener needs a token file (JCBIN_ADMIN_TOKEN_FILE or admin_token_file)")]
    AdminToken,
    #[error("[{module}.database]: {message}")]
    Database { module: Module, message: String },
}

/// The resolved configuration `jc` runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub roles: Vec<Module>,
    pub listen: SocketAddr,
    pub mode: MountMode,
    pub base_host: String,
    pub scheme: String,
    pub admin_listen: Option<SocketAddr>,
    pub admin_token_file: Option<PathBuf>,
    pub trusted_proxies: Vec<IpNet>,
    pub log_format: LogFormat,
    pub state_dir: PathBuf,
    pub portal_siblings: bool,
    pub mounts: BTreeMap<Module, MountOverride>,
    pub kaisatsu: KaisatsuSection,
    pub dataroom: DataroomSection,
    pub daiya: DaiyaSection,
    pub ekiben: EkibenSection,
    pub antares: AntaresSection,
    pub shako: ShakoSection,
    pub daiya: DaiyaSection,
}

impl Config {
    /// Default listener: loopback only, so a bare `jc` never listens on every interface.
    pub const DEFAULT_LISTEN: &'static str = "127.0.0.1:8080";
    /// Default directory of the generated secrets.
    pub const DEFAULT_STATE_DIR: &'static str = "jc-state";

    /// Reads and parses a TOML config file.
    pub fn read_file(path: &Path) -> Result<FileConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse_file(path, &text)
    }

    /// Parses TOML text as a config file (`path` only names it in errors).
    pub fn parse_file(path: &Path, text: &str) -> Result<FileConfig, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.message().to_owned(),
        })
    }

    /// Resolves the command line (already merged with `JCBIN_*` by clap) over the file.
    pub fn resolve(cli: Cli) -> Result<Self, ConfigError> {
        let file = match &cli.config {
            Some(path) => Self::read_file(path)?,
            None => FileConfig::default(),
        };
        Self::merge(cli, file)
    }

    /// Merges flags over a parsed file and validates the result.
    pub fn merge(cli: Cli, file: FileConfig) -> Result<Self, ConfigError> {
        let roles = parse_roles(
            cli.role
                .as_deref()
                .or(file.role.as_deref())
                .unwrap_or("all"),
        )?;
        let scheme = cli
            .scheme
            .or(file.scheme)
            .unwrap_or_else(|| "https".into())
            .to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(ConfigError::Scheme(scheme));
        }
        let base_host = cli
            .base_host
            .or(file.base_host)
            .unwrap_or_else(|| "localhost".into())
            .trim()
            .to_ascii_lowercase();
        if !valid_host(&base_host) {
            return Err(ConfigError::BaseHost(base_host));
        }
        let mut mounts = BTreeMap::new();
        for (name, mount) in file.mounts {
            let module: Module = name
                .parse()
                .map_err(|_| ConfigError::UnknownMount(name.clone()))?;
            mounts.insert(module, mount);
        }
        let mut trusted_proxies = file.trusted_proxies;
        trusted_proxies.extend(cli.trusted_proxies);
        let kaisatsu = KaisatsuSection {
            kek_file: cli.kaisatsu_kek_file.or(file.kaisatsu.kek_file),
            import_dir: cli.kaisatsu_import_dir.or(file.kaisatsu.import_dir),
            database_url_file: cli
                .kaisatsu_database_url_file
                .or(file.kaisatsu.database_url_file),
            db_password_file: cli
                .kaisatsu_db_password_file
                .or(file.kaisatsu.db_password_file),
            db_max_connections: cli
                .kaisatsu_db_max_connections
                .or(file.kaisatsu.db_max_connections),
            database: file.kaisatsu.database,
        };
        if kaisatsu.database.is_some() && kaisatsu.database_url_file.is_some() {
            return Err(ConfigError::Database {
                module: Module::Kaisatsu,
                message: "set either [kaisatsu.database] or database_url_file, not both".into(),
            });
        }
        let ekiben = EkibenSection {
            token_file: cli.ekiben_token_file.or(file.ekiben.token_file),
            file_root: cli.ekiben_file_root.or(file.ekiben.file_root),
            database: file.ekiben.database,
        };
        let antares = AntaresSection {
            admin_token_file: cli
                .antares_admin_token_file
                .or(file.antares.admin_token_file),
            database: file.antares.database,
        };
        let shako = ShakoSection {
            repo_root: cli.shako_repo_root.or(file.shako.repo_root),
            signing_key_file: cli.shako_signing_key_file.or(file.shako.signing_key_file),
            token_file: cli.shako_token_file.or(file.shako.token_file),
            database: file.shako.database,
        };
        let daiya = file.daiya;
        let dataroom = file.dataroom.unwrap_or_default();
        for (module, db) in [
            (Module::Kaisatsu, &kaisatsu.database),
            (Module::Ekiben, &ekiben.database),
            (Module::Antares, &antares.database),
            (Module::Shako, &shako.database),
            (Module::Daiya, &daiya.database),
            (Module::Dataroom, &dataroom.database),
        ] {
            let Some(db) = db else { continue };
            db.validate()
                .map_err(|message| ConfigError::Database { module, message })?;
            if module == Module::Antares && db.migrate_url.is_some() {
                return Err(ConfigError::Database {
                    module,
                    message: "Antares migrates as its own role at start; set no migrate_url".into(),
                });
            }
        }
        let admin_listen = cli.admin_listen.or(file.admin_listen);
        let admin_token_file = cli.admin_token_file.or(file.admin_token_file);
        if admin_listen.is_some() && admin_token_file.is_none() {
            return Err(ConfigError::AdminToken);
        }
        Ok(Self {
            roles,
            listen: cli.listen.or(file.listen).unwrap_or_else(|| {
                Self::DEFAULT_LISTEN
                    .parse()
                    .expect("default listen address parses")
            }),
            mode: cli.mode.or(file.mode).unwrap_or_default(),
            base_host,
            scheme,
            admin_listen,
            admin_token_file,
            trusted_proxies,
            log_format: cli.log_format.or(file.log_format).unwrap_or_default(),
            state_dir: cli
                .state_dir
                .or(file.state_dir)
                .unwrap_or_else(|| PathBuf::from(Self::DEFAULT_STATE_DIR)),
            portal_siblings: cli.portal_siblings.or(file.portal_siblings).unwrap_or(true),
            mounts,
            kaisatsu,
            dataroom,
            ekiben,
            antares,
            shako,
            daiya,
        })
    }
}

/// A host name of letters, digits, dots and hyphens, optionally with `:port`.
fn valid_host(host: &str) -> bool {
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !name.starts_with('.')
        && !name.ends_with('.')
        && port.is_none_or(|p| p.parse::<u16>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn cli(args: &[&str]) -> Cli {
        let mut all = vec!["jc"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).expect("cli parses")
    }

    #[test]
    fn no_jc_key_starts_with_jc_underscore() {
        // The Portal reads bare JC_* keys; jc's own must never collide with them.
        let cmd = Cli::command();
        let mut seen = 0;
        for arg in cmd.get_arguments() {
            if let Some(env) = arg.get_env() {
                let env = env.to_string_lossy();
                assert!(env.starts_with("JCBIN_"), "{env} must start with JCBIN_");
                assert!(
                    !env.starts_with("JC_"),
                    "{env} collides with the Portal's JC_* keys"
                );
                seen += 1;
            }
        }
        assert!(seen >= 10, "every flag reads a JCBIN_ variable");
    }

    #[test]
    fn defaults_are_loopback_slug_https_localhost() {
        let cfg = Config::merge(cli(&["--role", "dataroom"]), FileConfig::default()).unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(cfg.mode, MountMode::Slug);
        assert_eq!(cfg.scheme, "https");
        assert_eq!(cfg.base_host, "localhost");
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert_eq!(cfg.admin_listen, None);
        assert_eq!(cfg.roles, vec![Module::Dataroom]);
        assert_eq!(cfg.dataroom, DataroomSection::default());
    }

    #[test]
    fn flags_win_over_the_file() {
        let file = Config::parse_file(
            Path::new("jc.toml"),
            r#"
role = "daiya"
listen = "127.0.0.1:1"
mode = "host"
base_host = "file.example"
scheme = "http"
admin_listen = "127.0.0.1:2"
admin_token_file = "/run/jc/admin-token"
trusted_proxies = ["10.0.0.0/8"]
log_format = "text"

[mounts.daiya]
host = "grafana.example"

[dataroom]
org_id = "00000000-0000-7000-8000-0000000000aa"
catalogue_id = "00000000-0000-7000-8000-0000000000bb"
"#,
        )
        .unwrap();
        let from_file = Config::merge(Cli::default(), file.clone()).unwrap();
        assert_eq!(from_file.roles, vec![Module::Daiya]);
        assert_eq!(from_file.mode, MountMode::Host);
        assert_eq!(from_file.scheme, "http");
        assert_eq!(from_file.base_host, "file.example");
        assert_eq!(from_file.admin_listen, Some("127.0.0.1:2".parse().unwrap()));
        assert_eq!(
            from_file.admin_token_file.as_deref(),
            Some(Path::new("/run/jc/admin-token"))
        );
        assert_eq!(from_file.log_format, LogFormat::Text);
        assert_eq!(
            from_file.mounts[&Module::Daiya].host.as_deref(),
            Some("grafana.example")
        );
        assert_eq!(
            from_file.dataroom.org_id,
            "00000000-0000-7000-8000-0000000000aa"
        );

        let flags = cli(&[
            "--role",
            "dataroom",
            "--listen",
            "127.0.0.1:3",
            "--mode",
            "slug",
            "--base-host",
            "Flag.Example",
            "--scheme",
            "HTTPS",
            "--trusted-proxy",
            "192.168.0.0/16,172.16.0.0/12",
            "--log-format",
            "json",
        ]);
        let merged = Config::merge(flags, file).unwrap();
        assert_eq!(merged.roles, vec![Module::Dataroom]);
        assert_eq!(merged.listen, "127.0.0.1:3".parse().unwrap());
        assert_eq!(merged.mode, MountMode::Slug);
        assert_eq!(merged.base_host, "flag.example");
        assert_eq!(merged.scheme, "https");
        assert_eq!(merged.log_format, LogFormat::Json);
        assert_eq!(merged.trusted_proxies.len(), 3);
    }

    #[test]
    fn unknown_file_key_is_refused() {
        let err =
            Config::parse_file(Path::new("jc.toml"), "listn = \"127.0.0.1:1\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
        assert!(err.to_string().contains("jc.toml"));
        let err = Config::parse_file(Path::new("jc.toml"), "[mounts.dataroom]\nprefix = \"/x\"\n")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    }

    #[test]
    fn unknown_mount_module_is_refused() {
        let file =
            Config::parse_file(Path::new("jc.toml"), "[mounts.keycloak]\nhost = \"x\"\n").unwrap();
        let err = Config::merge(cli(&["--role", "dataroom"]), file).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownMount(ref m) if m == "keycloak"),
            "{err}"
        );
    }

    #[test]
    fn bad_scheme_and_host_are_refused() {
        let err = Config::merge(
            cli(&["--role", "dataroom", "--scheme", "ftp"]),
            FileConfig::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Scheme(_)), "{err}");
        for bad in [
            "",
            "a/b",
            "city.example/x",
            ".city",
            "city.",
            "city:port",
            "a b",
        ] {
            let err = Config::merge(
                cli(&["--role", "dataroom", "--base-host", bad]),
                FileConfig::default(),
            )
            .unwrap_err();
            assert!(matches!(err, ConfigError::BaseHost(_)), "{bad}: {err}");
        }
        let ok = Config::merge(
            cli(&["--role", "dataroom", "--base-host", "city.example:8443"]),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(ok.base_host, "city.example:8443");
    }

    #[test]
    fn kaisatsu_without_a_kek_file_leaves_it_to_setup() {
        // No file named: first-start setup generates the key (setup.rs), never runs without one.
        let cfg = Config::merge(cli(&["--role", "kaisatsu"]), FileConfig::default()).unwrap();
        assert_eq!(cfg.kaisatsu.kek_file, None);
        assert_eq!(cfg.state_dir, PathBuf::from("jc-state"));
        let ok = Config::merge(
            cli(&["--role", "kaisatsu", "--kaisatsu-kek-file", "/run/kek"]),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(ok.kaisatsu.kek_file.as_deref(), Some(Path::new("/run/kek")));
    }

    #[test]
    fn ekiben_without_a_token_file_leaves_it_to_setup() {
        let cfg = Config::merge(cli(&["--role", "ekiben"]), FileConfig::default()).unwrap();
        assert_eq!(cfg.ekiben.token_file, None);
        let file = Config::parse_file(
            Path::new("jc.toml"),
            "[ekiben]\ntoken_file = \"/run/tok\"\nfile_root = \"/srv/files\"\n",
        )
        .unwrap();
        let ok = Config::merge(cli(&["--role", "ekiben"]), file).unwrap();
        assert_eq!(ok.ekiben.token_file.as_deref(), Some(Path::new("/run/tok")));
        assert_eq!(
            ok.ekiben.file_root.as_deref(),
            Some(Path::new("/srv/files"))
        );
        let ok = Config::merge(
            cli(&[
                "--role",
                "ekiben",
                "--ekiben-token-file",
                "/a",
                "--ekiben-file-root",
                "/b",
            ]),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(ok.ekiben.file_root.as_deref(), Some(Path::new("/b")));
    }

    #[test]
    fn antares_without_an_admin_token_file_leaves_it_to_setup() {
        let cfg = Config::merge(
            cli(&["--role", "antares", "--state-dir", "/srv/jc"]),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(cfg.antares.admin_token_file, None);
        assert_eq!(cfg.state_dir, PathBuf::from("/srv/jc"));
        assert!(cfg.portal_siblings, "on by default");
        let file = Config::parse_file(Path::new("jc.toml"), "portal_siblings = false\n").unwrap();
        assert!(
            !Config::merge(cli(&["--role", "portal"]), file)
                .unwrap()
                .portal_siblings
        );
        let file = Config::parse_file(
            Path::new("jc.toml"),
            "[antares]\nadmin_token_file = \"/run/q\"\n",
        )
        .unwrap();
        let ok = Config::merge(cli(&["--role", "antares"]), file).unwrap();
        assert_eq!(
            ok.antares.admin_token_file.as_deref(),
            Some(Path::new("/run/q"))
        );
        let ok = Config::merge(
            cli(&["--role", "antares", "--antares-admin-token-file", "/x"]),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(
            ok.antares.admin_token_file.as_deref(),
            Some(Path::new("/x"))
        );
    }

    #[test]
    fn an_admin_listener_without_a_token_file_is_refused() {
        let err = Config::merge(
            cli(&["--role", "daiya", "--admin-listen", "127.0.0.1:0"]),
            FileConfig::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::AdminToken), "{err}");
        let file =
            Config::parse_file(Path::new("jc.toml"), "admin_listen = \"127.0.0.1:0\"\n").unwrap();
        let err = Config::merge(cli(&["--role", "daiya"]), file.clone()).unwrap_err();
        assert!(matches!(err, ConfigError::AdminToken), "{err}");
        let ok = Config::merge(
            cli(&["--role", "daiya", "--admin-token-file", "/run/a"]),
            file,
        )
        .unwrap();
        assert_eq!(ok.admin_token_file.as_deref(), Some(Path::new("/run/a")));
        // Without an admin listener no token is needed.
        let ok = Config::merge(cli(&["--role", "daiya"]), FileConfig::default()).unwrap();
        assert_eq!(ok.admin_token_file, None);
    }

    #[test]
    fn bad_role_is_refused() {
        let err = Config::merge(cli(&["--role", "nope"]), FileConfig::default()).unwrap_err();
        assert!(matches!(err, ConfigError::Role(_)), "{err}");
    }

    #[test]
    fn resolve_reads_the_file_and_reports_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jc.toml");
        std::fs::write(&path, "role = \"dataroom\"\nbase_host = \"from.file\"\n").unwrap();
        let cfg = Config::resolve(Cli {
            config: Some(path),
            ..Cli::default()
        })
        .unwrap();
        assert_eq!(cfg.base_host, "from.file");
        let err = Config::resolve(Cli {
            config: Some(dir.path().join("missing.toml")),
            ..Cli::default()
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }), "{err}");
        assert!(err.to_string().contains("missing.toml"));
    }

    #[test]
    fn database_sections_parse_and_bad_ones_are_refused() {
        let text = r#"
[shako.database]
url = "postgres://shako_app@db:5432/jc"
password_file = "/run/secrets/shako"
max_connections = 4
migrate_url = "postgres://shako_migrate@db:5432/jc"
migrate_password_file = "/run/secrets/shako-migrate"

[daiya.database]
url = "postgres://daiya_app@db/jc"
password_file = "/run/secrets/daiya"

[dataroom.database]
url = "postgres://dataroom_app@db/jc"
password_file = "/run/secrets/dataroom"

[ekiben.database]
url = "postgres://ekiben_app@db/jc"
password_file = "/run/secrets/ekiben"

[antares.database]
url = "postgres://antares_app@db/jc"
password_file = "/run/secrets/antares"

[kaisatsu.database]
url = "postgres://kaisatsu_app@db/jc"
password_file = "/run/secrets/kaisatsu"
"#;
        let cfg = Config::merge(
            Cli::default(),
            Config::parse_file(Path::new("jc.toml"), text).unwrap(),
        )
        .unwrap();
        let shako = cfg.shako.database.as_ref().unwrap();
        assert_eq!(shako.max_connections(), 4);
        assert!(shako.migrate_url.is_some());
        assert_eq!(cfg.daiya.database.as_ref().unwrap().max_connections(), 5);
        for db in [
            &cfg.dataroom.database,
            &cfg.ekiben.database,
            &cfg.antares.database,
            &cfg.kaisatsu.database,
        ] {
            assert!(db.is_some());
        }
        assert_eq!(cfg.dataroom.org_id, DataroomSection::default().org_id);
        // No section: every module stays on its memory store.
        let bare = Config::merge(Cli::default(), FileConfig::default()).unwrap();
        assert!(bare.shako.database.is_none() && bare.daiya.database.is_none());

        let refused = |text: &str| {
            Config::merge(
                Cli::default(),
                Config::parse_file(Path::new("jc.toml"), text).unwrap(),
            )
            .unwrap_err()
            .to_string()
        };
        let err = refused(
            "[daiya.database]\nurl = \"postgres://daiya_app:leaked@db/jc\"\npassword_file = \"/p\"\n",
        );
        assert!(
            err.contains("[daiya.database]") && err.contains("password"),
            "{err}"
        );
        assert!(!err.contains("leaked"), "{err}");
        let err = refused(
            "[antares.database]\nurl = \"postgres://a@db/jc\"\npassword_file = \"/p\"\nmigrate_url = \"postgres://m@db/jc\"\nmigrate_password_file = \"/m\"\n",
        );
        assert!(err.contains("Antares migrates"), "{err}");
        let err = refused(
            "[kaisatsu]\ndatabase_url_file = \"/u\"\n[kaisatsu.database]\nurl = \"postgres://k@db/jc\"\npassword_file = \"/p\"\n",
        );
        assert!(err.contains("not both"), "{err}");
        // A password key in the section is no key at all.
        assert!(
            Config::parse_file(
                Path::new("jc.toml"),
                "[shako.database]\nurl = \"postgres://s@db/jc\"\npassword_file = \"/p\"\npassword = \"x\"\n"
            )
            .is_err()
        );
    }
}
