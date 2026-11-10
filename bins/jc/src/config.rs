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
    /// The edge's mesh identity (`l5d-client-id`); once named, a trusted proxy's `X-Forwarded-*`
    /// count only on requests carrying it. Repeat or comma-separate.
    #[arg(
        long = "trusted-proxy-identity",
        env = "JCBIN_TRUSTED_PROXY_IDENTITIES",
        value_delimiter = ','
    )]
    pub trusted_proxy_identities: Vec<String>,
    /// Log format: `json` or `text`.
    #[arg(long, env = "JCBIN_LOG_FORMAT")]
    pub log_format: Option<LogFormat>,
    /// kaisatsu: file holding the 32-byte key-encryption key (raw or 64 hex characters).
    #[arg(long, env = "JCBIN_KAISATSU_KEK_FILE")]
    pub kaisatsu_kek_file: Option<PathBuf>,
    /// kaisatsu: realm files (JSON) applied at start.
    #[arg(long, env = "JCBIN_KAISATSU_IMPORT_DIR")]
    pub kaisatsu_import_dir: Option<PathBuf>,
    /// kaisatsu: the only directory a realm file's `$(file:...)` secret may name (a realm file
    /// reads no environment inside `jc`).
    #[arg(long, env = "JCBIN_KAISATSU_IMPORT_SECRETS_DIR")]
    pub kaisatsu_import_secrets_dir: Option<PathBuf>,
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
    /// ekiben: the directory holding each project's pipeline client secret as a file named by
    /// the client, `<project>-pipelines` (default: kaisatsu's import secrets directory, where a
    /// realm file's `$(file:...)` names the same file). Handed to streams as
    /// `JC_CLIENT_SECRET_<PROJECT>`, read once at start.
    #[arg(long, env = "JCBIN_EKIBEN_CLIENT_SECRETS_DIR")]
    pub ekiben_client_secrets_dir: Option<PathBuf>,
    /// ekiben: a resources file (Bento's `streams -r`): its caches, rate limits and processors
    /// are shared by every stream, as the deployment's `components/pipeline-runner/streams/
    /// resources.yaml` (`helsinki__gtfs`, `zilina_opendata`, `events_tail`, ...).
    #[arg(long, env = "JCBIN_EKIBEN_RESOURCES_FILE")]
    pub ekiben_resources_file: Option<PathBuf>,
    /// ekiben: the pipeline client the pipeline-test harness runs as (`JC_CLIENT_ID` and
    /// `JC_CLIENT_SECRET`), one of the client secrets directory's files.
    #[arg(long, env = "JCBIN_EKIBEN_HARNESS_CLIENT")]
    pub ekiben_harness_client: Option<String>,
    /// antares: file holding the `/q` admin token (required: the hosted broker never serves its
    /// admin surface without a credential). Everything else of Antares is its own `ANTARES_*`
    /// environment, as for the standalone broker.
    #[arg(long, env = "JCBIN_ANTARES_ADMIN_TOKEN_FILE")]
    pub antares_admin_token_file: Option<PathBuf>,
    /// Directory (0700) for the secrets `jc` generates on first start, one 0600 file each
    /// (default `jc-state` in the working directory).
    #[arg(long, env = "JCBIN_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// The realm the Portal signs in through, the gateway's issuer and the Endpoint scope
    /// linker use (default `jc`; a migrated deployment names its Keycloak realm, e.g. `dev`).
    #[arg(long, env = "JCBIN_IDENTITY_REALM")]
    pub identity_realm: Option<String>,
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
    /// dataroom: file holding the CKAN API token the Portal publishes with (default: made per
    /// boot, in memory).
    #[arg(long, env = "JCBIN_DATAROOM_PORTAL_TOKEN_FILE")]
    pub dataroom_portal_token_file: Option<PathBuf>,
    /// daiya: file holding the password of the Portal's server administrator (default: made per
    /// boot, in memory).
    #[arg(long, env = "JCBIN_DAIYA_PORTAL_PASSWORD_FILE")]
    pub daiya_portal_password_file: Option<PathBuf>,
    /// Portal: file holding the age identity that decrypts each project's SOPS secrets
    /// (`projects/{project}/secrets/*.sops.yaml`), so a `secretRef` resolves inside `jc`.
    #[arg(long, env = "JCBIN_PORTAL_AGE_KEY_FILE")]
    pub portal_age_key_file: Option<PathBuf>,
    /// assistant: file holding the model provider's API key its proxy calls the model with (a
    /// file only: `JCBIN_MODEL_KEY` and `JC_MODEL_KEY` are refused). Without it the assistant
    /// stays off unless `JC_ASSISTANT_PROXY_URL` names a proxy outside `jc`.
    #[arg(long, env = "JCBIN_MODEL_KEY_FILE")]
    pub model_key_file: Option<PathBuf>,
    /// `production` warns about every secret given as an environment value (it shows in
    /// `docker inspect` and the pod spec); `development` (default) does not.
    #[arg(long, env = "JCBIN_PROFILE", value_enum)]
    pub profile: Option<Profile>,
}

/// How strict the start report is about where secrets come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Development,
    Production,
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Text,
}

/// An explicit mount of one module (`[mounts.<module>]` in the file, or one entry of
/// `[[mounts.<module>]]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct MountOverride {
    pub host: Option<String>,
    pub path_prefix: Option<String>,
}

/// The mounts of one module in the file: one table (`[mounts.<module>]`) or an array of them
/// (`[[mounts.<module>]]`). The first entry is the module's own address; `plan_mounts` refuses
/// the bad ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountEntries(pub Vec<MountOverride>);

impl<'de> Deserialize<'de> for MountEntries {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = MountEntries;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a mount table or an array of mount tables")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let one =
                    MountOverride::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(MountEntries(vec![one]))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
                    .map(MountEntries)
            }
        }
        de.deserialize_any(Visitor)
    }
}

/// `[kaisatsu]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KaisatsuSection {
    pub kek_file: Option<PathBuf>,
    pub import_dir: Option<PathBuf>,
    pub import_secrets_dir: Option<PathBuf>,
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
    /// Each project's pipeline client secret, one file per client `<project>-pipelines`.
    pub client_secrets_dir: Option<PathBuf>,
    /// The pipeline client the pipeline-test harness runs as (`JC_CLIENT_ID`).
    pub harness_client: Option<String>,
    /// The shared resources every stream may name (Bento's `streams -r`).
    pub resources_file: Option<PathBuf>,
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
    pub portal_password_file: Option<PathBuf>,
    pub database: Option<DatabaseSection>,
    /// Grafana provisioning root (`datasources/`, `dashboards/`) loaded at every start, with
    /// `DAIYA_VAR_*` for its `$NAME` values. Its datasources alone may reach `jc`'s own
    /// listener, named by its exact address (`http://127.0.0.1:<port>/...`).
    pub provisioning_dir: Option<PathBuf>,
    /// Organizations held from the start under their Grafana numbers (`{ id = 4, name =
    /// "bbsk" }`), so the provisioning files can name each by `orgId`.
    #[serde(default)]
    pub organizations: Vec<daiya_server::config::NumberedOrganization>,
    /// The one address organization 1's Prometheus datasources may reach over http on a
    /// non-public network (`http://<host>:<port>`, the instance's label-enforcing proxy on a
    /// shared cluster); every other datasource keeps the default outbound policy.
    pub prometheus_reach: Option<String>,
}

/// `[dataroom]` section (a key left out keeps its default).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DataroomSection {
    pub org_id: String,
    pub catalogue_id: String,
    #[serde(default)]
    pub portal_token_file: Option<PathBuf>,
    pub database: Option<DatabaseSection>,
    /// `[dataroom.addresses]`: where the catalogue's live parts answer.
    pub addresses: DataroomAddresses,
    /// `[dataroom.aliases]`: `old = "new"` per renamed dataset (evidence/jc-dev/RENAMES.md); a
    /// read of the old name answers the new dataset.
    pub aliases: BTreeMap<String, String>,
    /// `[[dataroom.harvest]]`: the harvest sources held from the start, each run on its
    /// `schedule` by dataroom's harvest scheduler (prod's CKAN harvest CronJobs).
    pub harvest: Vec<HarvestSeed>,
    /// `[dataroom.hidden]`: `name = "reason"` per dataset the publish gate hides (prod's CKAN chart
    /// value `hidden`, T-4318; bins/jc/docs/catalogue-hidden.toml), written over the default
    /// catalogue's list on every start. Left out, the stored list stays.
    pub hidden: Option<BTreeMap<String, String>>,
}

/// One `[[dataroom.harvest]]` source, written over the stored one on every start: prod's
/// `harvest.sources` entry (deployment `components/ckan/values/catalogue/*.yaml.gotmpl`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestSeed {
    pub name: String,
    /// `nkod-sparql` (the SPARQL endpoint), `arcgis-hub` (the Hub's base), `ckan-action`, `dcat-ap`.
    pub kind: String,
    pub url: String,
    /// The CKAN organisation the datasets land in; created with the next three when missing.
    pub publisher: String,
    pub publisher_title: String,
    /// The publisher's IRI: the one NKOD lists the datasets under, and the publisher's `uri`.
    pub publisher_uri: String,
    #[serde(default)]
    pub publisher_description: Option<String>,
    /// An `[[organizations]]` name: the source belongs to that Organization's catalogue; none,
    /// the `[dataroom]` default catalogue.
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub spatial_uri: Option<String>,
    #[serde(default)]
    pub name_prefix: String,
    /// Every host a run may fetch from, the `url`'s own included (dataroom's outbound guard).
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub rewrite_hosts: BTreeMap<String, String>,
    /// Items left out (title, IRI or item id) with the reason the run log gives.
    #[serde(default)]
    pub skip: BTreeMap<String, String>,
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
    #[serde(default)]
    pub spelling: BTreeMap<String, String>,
    #[serde(default)]
    pub require_column: BTreeMap<String, String>,
    /// Five-field cron, UTC (prod: `23 3 * * *`).
    #[serde(default = "default_harvest_schedule")]
    pub schedule: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// prod's `od-datasets.json` (deployment `charts/ckan/files/`, bins/jc/docs/od-datasets.json):
    /// the converted datasets by name, a relative path read beside jc.toml. Set: only those are
    /// listed, with their Endpoint's distributions on `gateway`.
    #[serde(default)]
    pub converted_file: Option<PathBuf>,
    /// The catalogue base the file's `dataset` URLs name, replaced by this catalogue's own
    /// (prod's `harvest.datasetsBase`).
    #[serde(default)]
    pub datasets_base: Option<String>,
    /// The public base the Endpoints answer under (prod's `harvest.gateway`); its host must be
    /// in `allowed_hosts`. Unset: this instance's own gateway.
    #[serde(default)]
    pub gateway: Option<String>,
}

fn default_harvest_schedule() -> String {
    "23 3 * * *".into()
}

/// `[dataroom.addresses]`: the bases (`http(s)://host[:port][/path]`) the dataset pages frame,
/// link and read, one by one (G1). One left out comes from the module `jc` mounts for it: `viz`
/// from viz, `api` from the gateway's origin, `portal` from the Portal; `tiles` and `assistant`
/// only from here, since `jc` serves neither, so a page never points at a part nobody serves. A
/// catalogue's own branding (`addresses` in its branding JSON) wins over these, per catalogue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DataroomAddresses {
    pub viz: Option<String>,
    pub api: Option<String>,
    pub tiles: Option<String>,
    pub assistant: Option<String>,
    pub portal: Option<String>,
}

impl DataroomAddresses {
    /// Each named address with its key, for the start-up check.
    #[must_use]
    pub fn named(&self) -> Vec<(&'static str, &str)> {
        [
            ("viz", &self.viz),
            ("api", &self.api),
            ("tiles", &self.tiles),
            ("assistant", &self.assistant),
            ("portal", &self.portal),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k, v)))
        .collect()
    }
}

impl Default for DataroomSection {
    fn default() -> Self {
        Self {
            org_id: "00000000-0000-7000-8000-000000000001".into(),
            catalogue_id: "00000000-0000-7000-8000-000000000010".into(),
            portal_token_file: None,
            database: None,
            addresses: DataroomAddresses::default(),
            aliases: BTreeMap::new(),
            harvest: Vec::new(),
            hidden: None,
        }
    }
}

/// One `[[organizations]]` entry: an Organization of the deployment on its own hosts, with its
/// own dataroom catalogue (ADR-N-058). Every host is unique across the deployment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSection {
    /// The JC Organization's `metadata.name`; also the CKAN organisation of its catalogue.
    pub name: String,
    pub hosts: OrganizationHosts,
    /// The dataroom catalogue (a UUID) holding the Organization's datasets.
    pub catalogue_id: String,
    /// The CKAN API token the Portal publishes into this catalogue with; none, no Portal grant.
    #[serde(default)]
    pub portal_token_file: Option<PathBuf>,
}

/// The hosts one Organization answers on: its Portal, its open data portal and its API and
/// MCP addresses (both the gateway's).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationHosts {
    pub portal: String,
    pub opendata: String,
    pub api: String,
    pub mcp: String,
}

impl OrganizationHosts {
    /// Each host with the module that answers on it.
    #[must_use]
    pub fn by_module(&self) -> [(Module, &str); 4] {
        [
            (Module::Portal, self.portal.as_str()),
            (Module::Dataroom, self.opendata.as_str()),
            (Module::Gateway, self.api.as_str()),
            (Module::Gateway, self.mcp.as_str()),
        ]
    }
}

/// `[[organizations]]` checked: plain lowercase names and hosts, a catalogue UUID each, and no
/// name, catalogue or host twice (a host compared without its port).
fn check_organizations(orgs: &mut [OrganizationSection]) -> Result<(), ConfigError> {
    let bad = |m: String| Err(ConfigError::Organization(m));
    let mut names = std::collections::BTreeSet::new();
    let mut catalogues = std::collections::BTreeSet::new();
    let mut hosts = BTreeMap::new();
    for org in orgs.iter_mut() {
        let name = org.name.as_str();
        if name.is_empty()
            || name.len() > 63
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return bad(format!(
                "name '{name}' is not a lowercase Organization name"
            ));
        }
        if !names.insert(org.name.clone()) {
            return bad(format!("Organization '{name}' is named twice"));
        }
        org.catalogue_id = org.catalogue_id.trim().to_ascii_lowercase();
        if !is_uuid(&org.catalogue_id) {
            return bad(format!(
                "{name}: catalogue_id '{}' is not a UUID",
                org.catalogue_id
            ));
        }
        if !catalogues.insert(org.catalogue_id.clone()) {
            return bad(format!(
                "{name}: catalogue {} belongs to another Organization",
                org.catalogue_id
            ));
        }
        let h = &mut org.hosts;
        for host in [&mut h.portal, &mut h.opendata, &mut h.api, &mut h.mcp] {
            *host = host.trim().to_ascii_lowercase();
            if !valid_host(host) {
                return bad(format!("{name}: host '{host}' is not a plain host name"));
            }
            let bare = host.split(':').next().unwrap_or_default().to_owned();
            if let Some(other) = hosts.insert(bare, org.name.clone()) {
                return bad(format!("{name}: host '{host}' is also a host of {other}"));
            }
        }
    }
    Ok(())
}

/// `8-4-4-4-12` hex digits.
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `[identity]` section: the one realm of the deployment (`crate::setup::Realm`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySection {
    pub realm: Option<String>,
}

/// `[portal]` section: what `jc` hands the Portal beside its own `JC_*` environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalSection {
    pub age_key_file: Option<PathBuf>,
}

/// `[assistant]` section: the knowledge assistant beside its siblings. Its other settings are
/// its own `JC_ASSISTANT_*` keys (an environment value wins over what `jc` fills from siblings),
/// `ORT_DYLIB_PATH`, and `JC_MODEL_BASE`/`JC_MODEL_PROVIDER` for its proxy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantSection {
    /// The model key's file (or `JCBIN_MODEL_KEY_FILE`).
    pub model_key_file: Option<PathBuf>,
    /// The `publicId` of the deployment this `jc`'s catalogue frames: its dataroom origin joins
    /// that deployment's `allowedOrigins` (https only).
    pub catalogue_deployment: String,
    /// Postgres with pgvector, as the assistant's own role (it migrates itself, as standalone,
    /// under forced row-level security). Without it the assistant stays off.
    pub database: Option<DatabaseSection>,
    /// The shared embedder service (`JC_ASSISTANT_EMBEDDER_URL`); `jc` fills its token URL,
    /// client and secret file from kaisatsu and the `jc-assistant` client.
    pub embedder_url: Option<String>,
}

impl Default for AssistantSection {
    fn default() -> Self {
        Self {
            model_key_file: None,
            catalogue_deployment: "data".into(),
            database: None,
            embedder_url: None,
        }
    }
}

/// `[door]`: this `jc` serves its modules to callers in other containers (role split). A request
/// carrying a token kaisatsu signed for one of these clients, with audience the module's name
/// (`shako`, `ekiben`, `daiya`, `dataroom`, `antares`), is let in as the module's service
/// caller; every other request reaches the module unchanged. No client named admits nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoorSection {
    /// The issuer the tokens name (`iss`).
    pub issuer: String,
    /// Where the issuer's signing keys are (`.../protocol/openid-connect/certs`).
    pub jwks_url: String,
    /// The client ids (`azp`) admitted.
    #[serde(default)]
    pub clients: Vec<String>,
}

/// `[remote]`: modules this `jc`'s Portal calls in other containers (role split). The Portal
/// calls each on a loopback forwarder with a per-boot credential; the forwarder swaps it for a
/// client-credentials token of `client_id` with the module's name as scope and audience.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSection {
    /// kaisatsu's token endpoint.
    pub token_url: String,
    pub client_id: String,
    /// File holding the client secret (or `JCBIN_REMOTE_CLIENT_SECRET[_FILE]`).
    pub client_secret_file: Option<PathBuf>,
    /// Module name to the public base of its mount in the other container.
    #[serde(default)]
    pub modules: BTreeMap<String, String>,
}

/// The modules a role split may place behind a [`DoorSection`] or a [`RemoteSection`]: those
/// the Portal calls with a service credential.
pub const SPLIT_MODULES: [Module; 5] = [
    Module::Shako,
    Module::Ekiben,
    Module::Daiya,
    Module::Dataroom,
    Module::Antares,
];

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
    #[serde(default)]
    pub trusted_proxy_identities: Vec<String>,
    pub log_format: Option<LogFormat>,
    pub profile: Option<Profile>,
    pub state_dir: Option<PathBuf>,
    pub portal_siblings: Option<bool>,
    #[serde(default)]
    pub mounts: BTreeMap<String, MountEntries>,
    #[serde(default)]
    pub kaisatsu: KaisatsuSection,
    #[serde(default)]
    pub dataroom: Option<DataroomSection>,
    #[serde(default)]
    pub ekiben: EkibenSection,
    #[serde(default)]
    pub antares: AntaresSection,
    #[serde(default)]
    pub shako: ShakoSection,
    #[serde(default)]
    pub daiya: DaiyaSection,
    #[serde(default)]
    pub portal: PortalSection,
    #[serde(default)]
    pub assistant: AssistantSection,
    #[serde(default)]
    pub identity: IdentitySection,
    pub door: Option<DoorSection>,
    pub remote: Option<RemoteSection>,
    #[serde(default)]
    pub organizations: Vec<OrganizationSection>,
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
    #[error("[mounts.{module}]: {message}")]
    Mount { module: Module, message: String },
    #[error("scheme must be http or https, not '{0}'")]
    Scheme(String),
    #[error("base host must be a plain host name, not '{0}'")]
    BaseHost(String),
    #[error(
        "the admin listener needs a token (JCBIN_ADMIN_TOKEN_FILE, admin_token_file or JCBIN_ADMIN_TOKEN)"
    )]
    AdminToken,
    #[error("{0}")]
    Secret(#[from] jc_secret::ReadError),
    #[error("{0} is a file only: set {0}_FILE (or the config file's key) to its path")]
    FileOnly(&'static str),
    #[error(
        "{0} is refused inside jc: give the model key as a file (JCBIN_MODEL_KEY_FILE or [assistant] model_key_file)"
    )]
    ModelKeyValue(&'static str),
    #[error("[remote]: {0}")]
    Remote(String),
    #[error("[ekiben] client_secrets_dir: {0}")]
    ClientSecrets(String),
    #[error("[{module}.database]: {message}")]
    Database { module: Module, message: String },
    #[error(
        "[dataroom.addresses] {0}: '{1}' is not a base URL (http(s)://host[:port][/path], no trailing slash)"
    )]
    Address(&'static str, String),
    #[error("[identity] realm: {0}")]
    Realm(String),
    #[error("[[organizations]] {0}")]
    Organization(String),
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
    pub trusted_proxy_identities: Vec<String>,
    pub log_format: LogFormat,
    pub profile: Profile,
    pub state_dir: PathBuf,
    pub portal_siblings: bool,
    /// Every module's explicit mounts, in file order: the first is the module's own address.
    pub mounts: BTreeMap<Module, Vec<MountOverride>>,
    pub kaisatsu: KaisatsuSection,
    pub dataroom: DataroomSection,
    pub ekiben: EkibenSection,
    pub antares: AntaresSection,
    pub shako: ShakoSection,
    pub daiya: DaiyaSection,
    pub portal: PortalSection,
    pub assistant: AssistantSection,
    /// `[identity] realm`, checked.
    pub realm: crate::setup::Realm,
    pub door: Option<DoorSection>,
    pub remote: Option<RemoteSection>,
    /// `[remote.modules]`, parsed: each module and the base it is called at.
    pub remote_modules: BTreeMap<Module, String>,
    /// `[[organizations]]`, checked: each Organization on its own hosts.
    pub organizations: Vec<OrganizationSection>,
}

impl Config {
    /// Default listener: loopback only, so a bare `jc` never listens on every interface.
    pub const DEFAULT_LISTEN: &'static str = "127.0.0.1:8080";
    /// Default directory of the generated secrets.
    pub const DEFAULT_STATE_DIR: &'static str = "jc-state";

    /// Reads and parses a TOML config file. A relative `converted_file` is read beside it.
    pub fn read_file(path: &Path) -> Result<FileConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut file = Self::parse_file(path, &text)?;
        let dir = path.parent().unwrap_or(Path::new(""));
        for seed in file.dataroom.iter_mut().flat_map(|d| d.harvest.iter_mut()) {
            if let Some(f) = seed.converted_file.as_mut().filter(|f| f.is_relative()) {
                *f = dir.join(&*f);
            }
        }
        Ok(file)
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
            mounts.insert(module, mount.0);
        }
        let mut trusted_proxies = file.trusted_proxies;
        trusted_proxies.extend(cli.trusted_proxies);
        let mut trusted_proxy_identities = file.trusted_proxy_identities;
        trusted_proxy_identities.extend(cli.trusted_proxy_identities);
        let kaisatsu = KaisatsuSection {
            kek_file: cli.kaisatsu_kek_file.or(file.kaisatsu.kek_file),
            import_dir: cli.kaisatsu_import_dir.or(file.kaisatsu.import_dir),
            import_secrets_dir: cli
                .kaisatsu_import_secrets_dir
                .or(file.kaisatsu.import_secrets_dir),
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
            client_secrets_dir: cli
                .ekiben_client_secrets_dir
                .or(file.ekiben.client_secrets_dir)
                .or_else(|| kaisatsu.import_secrets_dir.clone()),
            harness_client: cli.ekiben_harness_client.or(file.ekiben.harness_client),
            resources_file: cli.ekiben_resources_file.or(file.ekiben.resources_file),
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
        let daiya = DaiyaSection {
            portal_password_file: cli
                .daiya_portal_password_file
                .or(file.daiya.portal_password_file),
            database: file.daiya.database,
            provisioning_dir: file.daiya.provisioning_dir,
            organizations: file.daiya.organizations,
            prometheus_reach: file.daiya.prometheus_reach,
        };
        let mut assistant = file.assistant;
        assistant.model_key_file = cli.model_key_file.or(assistant.model_key_file);
        let mut dataroom = file.dataroom.unwrap_or_default();
        dataroom.portal_token_file = cli
            .dataroom_portal_token_file
            .or(dataroom.portal_token_file);
        if let Some((key, bad)) = dataroom
            .addresses
            .named()
            .into_iter()
            .find(|(_, v)| !crate::siblings::is_base_url(v))
        {
            return Err(ConfigError::Address(key, bad.to_owned()));
        }
        for (module, db) in [
            (Module::Kaisatsu, &kaisatsu.database),
            (Module::Ekiben, &ekiben.database),
            (Module::Antares, &antares.database),
            (Module::Shako, &shako.database),
            (Module::Daiya, &daiya.database),
            (Module::Dataroom, &dataroom.database),
            (Module::Assistant, &assistant.database),
        ] {
            let Some(db) = db else { continue };
            db.validate()
                .map_err(|message| ConfigError::Database { module, message })?;
            let own = match module {
                Module::Antares => Some("Antares"),
                Module::Assistant => Some("The assistant"),
                _ => None,
            };
            if let (Some(own), Some(_)) = (own, &db.migrate_url) {
                return Err(ConfigError::Database {
                    module,
                    message: format!("{own} migrates as its own role at start; set no migrate_url"),
                });
            }
        }
        let admin_listen = cli.admin_listen.or(file.admin_listen);
        let admin_token_file = cli.admin_token_file.or(file.admin_token_file);
        let portal = PortalSection {
            age_key_file: cli.portal_age_key_file.or(file.portal.age_key_file),
        };
        let realm = match cli.identity_realm.or(file.identity.realm) {
            Some(name) => crate::setup::Realm::parse(&name).map_err(ConfigError::Realm)?,
            None => crate::setup::Realm::default(),
        };
        let mut organizations = file.organizations;
        check_organizations(&mut organizations)?;
        let remote_modules = match &file.remote {
            Some(remote) => remote_modules(remote, &roles)?,
            None => BTreeMap::new(),
        };
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
            trusted_proxy_identities,
            log_format: cli.log_format.or(file.log_format).unwrap_or_default(),
            profile: cli.profile.or(file.profile).unwrap_or_default(),
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
            portal,
            assistant,
            realm,
            door: file.door,
            remote: file.remote,
            remote_modules,
            organizations,
        })
    }
}

/// `[remote.modules]` checked: a split module, not also run here, at an http(s) base.
fn remote_modules(
    remote: &RemoteSection,
    roles: &[Module],
) -> Result<BTreeMap<Module, String>, ConfigError> {
    let bad = |m: String| Err(ConfigError::Remote(m));
    if !roles.contains(&Module::Portal) {
        return bad("only a jc running the Portal calls remote modules".into());
    }
    let web = |url: &str| url.starts_with("https://") || url.starts_with("http://");
    if !web(&remote.token_url) {
        return bad("token_url must be an http(s) URL".into());
    }
    let mut out = BTreeMap::new();
    for (name, url) in &remote.modules {
        let module: Module = name
            .parse()
            .map_err(|_| ConfigError::Remote(format!("'{name}' is no module")))?;
        if !SPLIT_MODULES.contains(&module) {
            return bad(format!("{module} cannot run in another container"));
        }
        if roles.contains(&module) {
            return bad(format!("{module} runs in this jc and in another container"));
        }
        if !web(url) {
            return bad(format!("{module}: '{url}' is not an http(s) URL"));
        }
        out.insert(module, url.trim_end_matches('/').to_owned());
    }
    Ok(out)
}

/// The process environment `jc` reads: its own `JCBIN_*` keys, the `EKIBEN_VAR_*` values it
/// hands ekiben's streams and the `DAIYA_VAR_*` values daiya's provisioning files name. This and [`portal_env`] are the only readers of the environment in
/// `jc` and the modules it hosts (clippy `disallowed-methods` holds the module crates to it).
#[must_use]
#[allow(clippy::disallowed_methods)] // The one reader.
pub fn process_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("JCBIN_")
                || k.starts_with(EKIBEN_VAR_PREFIX)
                || k.starts_with("DAIYA_VAR_")
                || k == REFUSED_MODEL_KEY
        })
        // The proxy's own name for the model key is read only to refuse it: its value is dropped.
        .map(|(k, v)| {
            if k == REFUSED_MODEL_KEY {
                (k, String::new())
            } else {
                (k, v)
            }
        })
        .collect()
}

/// agent-proxy's environment name for the model key as a value, refused inside `jc`.
const REFUSED_MODEL_KEY: &str = "JC_MODEL_KEY";

/// One key of the assistant's or its proxy's own configuration, as they read it standalone:
/// `JC_ASSISTANT_*`, `JC_MODEL_BASE`, `JC_MODEL_PROVIDER`, `JC_DAILY_TOKENS_*` and
/// `ORT_DYLIB_PATH`. Never the model key, which comes from its file alone ([`Secrets`]).
#[must_use]
#[allow(clippy::disallowed_methods)] // The one reader, for the assistant's keys.
pub fn assistant_env(key: &str) -> Option<String> {
    let allowed = key.starts_with("JC_ASSISTANT_")
        || key.starts_with("JC_DAILY_TOKENS_")
        || matches!(
            key,
            "JC_MODEL_BASE" | "JC_MODEL_PROVIDER" | "ORT_DYLIB_PATH"
        );
    if !allowed {
        return None;
    }
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// One key of the Portal's or the gateway's own configuration (`JC_*`), as the standalone
/// Portal or gateway reads it.
#[must_use]
#[allow(clippy::disallowed_methods)] // The one reader, for the Portal's and gateway's keys.
pub fn portal_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// One `ANTARES_*` key, for the broker settings `jc` itself decides on (the broker reads the rest
/// of its environment as standalone).
#[must_use]
#[allow(clippy::disallowed_methods)] // The one reader, for Antares' keys.
pub fn antares_env(key: &str) -> Option<String> {
    debug_assert!(key.starts_with("ANTARES_"));
    std::env::var(key).ok()
}

/// `JC_EKIBEN_PRIVATE_HOSTS`: the in-cluster names ekiben's streams may reach although they
/// resolve to private addresses (ekiben's `private_hosts`; `name:port` opens one port), comma
/// separated: the MQTT brokers' internal listeners a DataSource reads. A variable rather than a
/// `jc.toml` key, so a deployment can name it before the image reads it.
#[must_use]
#[allow(clippy::disallowed_methods)] // The one reader, for ekiben's private hosts.
pub fn ekiben_private_hosts() -> String {
    std::env::var("JC_EKIBEN_PRIVATE_HOSTS").unwrap_or_default()
}

/// The prefix of the variables `jc` hands ekiben's streams as `${NAME}` (ekiben's own
/// convention for a shared process, `EnvAccess::AllowListed`).
pub const EKIBEN_VAR_PREFIX: &str = "EKIBEN_VAR_";

pub use crate::setup::Given;

/// Every secret the operator gave `jc`, each from `NAME` or `NAME_FILE` (or the config file's
/// key naming the file), never both. Read once at start; a module gets its own entries only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    /// The admin listener's bearer token.
    pub admin_token: Option<Given>,
    /// shako's ES256 signing key.
    pub shako_signing_key: Option<Given>,
    /// The secrets first-start setup otherwise generates.
    pub module: BTreeMap<crate::setup::Secret, Given>,
    /// `EKIBEN_VAR_<NAME>` values, by `NAME` and by the whole variable name.
    pub ekiben_vars: BTreeMap<String, jc_secret::Secret>,
    /// The pipeline client secrets of `[ekiben] client_secrets_dir`, by the variable a rendered
    /// stream names (`JC_CLIENT_SECRET_<PROJECT>`, and `JC_CLIENT_SECRET` for the harness).
    pub pipeline_client_secrets: BTreeMap<String, jc_secret::Secret>,
    /// The age identity the Portal decrypts project secrets with (a file: the Portal reads it
    /// per resolution).
    pub portal_age_key_file: Option<PathBuf>,
    /// `[remote]`'s client secret.
    pub remote_client_secret: Option<Given>,
    /// The model provider's key the assistant's proxy holds (a file only).
    pub model_key: Option<Given>,
    /// Each `[[organizations]]` entry's Portal publisher token, by Organization name.
    pub organization_tokens: BTreeMap<String, Given>,
}

impl Secrets {
    /// Reads every secret `cfg` names, with `NAME` values from the process environment.
    pub fn load(cfg: &Config) -> Result<Self, ConfigError> {
        Self::load_from(cfg, &process_env())
    }

    /// As [`Secrets::load`], with `env` standing for the process environment.
    pub fn load_from(cfg: &Config, env: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        use crate::setup::Secret as S;
        let given = |name: &str, file: Option<&Path>| -> Result<Option<Given>, ConfigError> {
            Ok(
                jc_secret::read(name, env.get(name).cloned(), file)?.map(|(secret, source)| {
                    Given {
                        secret,
                        source,
                        path: file.map(Path::to_path_buf),
                    }
                }),
            )
        };
        let mut out = Self {
            admin_token: given("JCBIN_ADMIN_TOKEN", cfg.admin_token_file.as_deref())?,
            shako_signing_key: given(
                "JCBIN_SHAKO_SIGNING_KEY",
                cfg.shako.signing_key_file.as_deref(),
            )?,
            ..Self::default()
        };
        if cfg.admin_listen.is_some() && out.admin_token.is_none() {
            return Err(ConfigError::AdminToken);
        }
        for (secret, name, file) in [
            (
                S::KaisatsuKek,
                "JCBIN_KAISATSU_KEK",
                cfg.kaisatsu.kek_file.as_deref(),
            ),
            (
                S::EkibenToken,
                "JCBIN_EKIBEN_TOKEN",
                cfg.ekiben.token_file.as_deref(),
            ),
            (
                S::AntaresAdminToken,
                "JCBIN_ANTARES_ADMIN_TOKEN",
                cfg.antares.admin_token_file.as_deref(),
            ),
            (
                S::ShakoToken,
                "JCBIN_SHAKO_TOKEN",
                cfg.shako.token_file.as_deref(),
            ),
            (
                S::CatalogueToken,
                "JCBIN_DATAROOM_PORTAL_TOKEN",
                cfg.dataroom.portal_token_file.as_deref(),
            ),
            (
                S::DashboardsPassword,
                "JCBIN_DAIYA_PORTAL_PASSWORD",
                cfg.daiya.portal_password_file.as_deref(),
            ),
        ] {
            if let Some(g) = given(name, file)? {
                out.module.insert(secret, g);
            }
        }
        for (name, value) in env {
            if let Some(bare) = name.strip_prefix(EKIBEN_VAR_PREFIX)
                && !bare.is_empty()
            {
                let value = jc_secret::Secret::new(value.clone());
                out.ekiben_vars.insert(bare.to_owned(), value.clone());
                out.ekiben_vars.insert(name.clone(), value);
            }
        }
        out.pipeline_client_secrets = pipeline_client_secrets(&cfg.ekiben)?;
        for org in &cfg.organizations {
            let name = format!("[[organizations]] {} portal_token_file", org.name);
            if let Some(g) = given(&name, org.portal_token_file.as_deref())? {
                out.organization_tokens.insert(org.name.clone(), g);
            }
        }
        if env.contains_key("JCBIN_PORTAL_AGE_KEY") {
            return Err(ConfigError::FileOnly("JCBIN_PORTAL_AGE_KEY"));
        }
        out.portal_age_key_file = cfg.portal.age_key_file.clone();
        for name in ["JCBIN_MODEL_KEY", REFUSED_MODEL_KEY] {
            if env.contains_key(name) {
                return Err(ConfigError::ModelKeyValue(name));
            }
        }
        if let Some(path) = &cfg.assistant.model_key_file {
            out.model_key = Some(Given {
                secret: jc_secret::read_file("JCBIN_MODEL_KEY_FILE", path)?,
                source: jc_secret::Source::File,
                path: Some(path.clone()),
            });
        }
        if let Some(remote) = &cfg.remote {
            let file = remote.client_secret_file.clone().or_else(|| {
                env.get("JCBIN_REMOTE_CLIENT_SECRET_FILE")
                    .map(PathBuf::from)
            });
            out.remote_client_secret = given("JCBIN_REMOTE_CLIENT_SECRET", file.as_deref())?;
            if out.remote_client_secret.is_none() {
                return Err(ConfigError::Remote(
                    "no client secret: set client_secret_file or JCBIN_REMOTE_CLIENT_SECRET_FILE"
                        .into(),
                ));
            }
        }
        Ok(out)
    }

    /// Name and source of each secret given here (setup reports the rest).
    #[must_use]
    pub fn report(&self) -> Vec<(&'static str, jc_secret::Source)> {
        let mut out = Vec::new();
        if let Some(g) = &self.admin_token {
            out.push(("admin-token", g.source));
        }
        if let Some(g) = &self.shako_signing_key {
            out.push(("shako-signing-key", g.source));
        }
        if !self.pipeline_client_secrets.is_empty() {
            out.push(("ekiben-pipeline-client-secrets", jc_secret::Source::File));
        }
        if self.portal_age_key_file.is_some() {
            out.push(("portal-age-key", jc_secret::Source::File));
        }
        if let Some(g) = &self.remote_client_secret {
            out.push(("remote-client-secret", g.source));
        }
        if let Some(g) = &self.model_key {
            out.push(("model-key", g.source));
        }
        out
    }
}

/// The suffix of a project's pipeline client, `{project}-pipelines` (the Portal's
/// `PIPELINE_ACCOUNT` behind jc-core's `keycloak_client_id`).
pub const PIPELINE_CLIENT_SUFFIX: &str = "-pipelines";

/// The variable a rendered stream names for `project`'s pipeline client secret, as the Portal's
/// reconciler writes it: `zilina-mesto` becomes `JC_CLIENT_SECRET_ZILINA_MESTO`.
#[must_use]
pub fn client_secret_var(project: &str) -> String {
    let mut name = String::from("JC_CLIENT_SECRET_");
    for ch in project.chars() {
        name.push(if ch.is_ascii_alphanumeric() {
            ch.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    name
}

/// Whether `name` is a DNS-1123 label, as every project name is.
fn dns_label(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Reads every `<project>-pipelines` file of the client secrets directory (other files are left
/// alone: the directory is kaisatsu's import secrets too) and the harness client's. A symbolic
/// link is refused, as kaisatsu's loader refuses one. Errors name files, never a value.
fn pipeline_client_secrets(
    cfg: &EkibenSection,
) -> Result<BTreeMap<String, jc_secret::Secret>, ConfigError> {
    let mut out = BTreeMap::new();
    let Some(dir) = &cfg.client_secrets_dir else {
        if cfg.harness_client.is_some() {
            return Err(ConfigError::ClientSecrets(
                "harness_client needs a client secrets directory".into(),
            ));
        }
        return Ok(out);
    };
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ConfigError::ClientSecrets(format!("{}: {}", dir.display(), e.kind())))?;
    let mut clients = BTreeMap::new();
    for entry in entries {
        let entry = entry
            .map_err(|e| ConfigError::ClientSecrets(format!("{}: {}", dir.display(), e.kind())))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(project) = name.strip_suffix(PIPELINE_CLIENT_SUFFIX) else {
            continue;
        };
        if !dns_label(project) {
            continue;
        }
        let path = entry.path();
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ConfigError::ClientSecrets(format!(
                "{} is a symbolic link",
                path.display()
            )));
        }
        let secret = jc_secret::read_file(&name, &path)?;
        out.insert(client_secret_var(project), secret.clone());
        clients.insert(name, secret);
    }
    if let Some(client) = &cfg.harness_client {
        let secret = clients.get(client).ok_or_else(|| {
            ConfigError::ClientSecrets(format!(
                "harness_client {client} has no file in {}",
                dir.display()
            ))
        })?;
        out.insert("JC_CLIENT_SECRET".to_owned(), secret.clone());
    }
    Ok(out)
}

impl Config {
    /// The start report's lines for the database passwords: one per `[<module>.database]`
    /// password file (and migration password file), always a file, read at connect.
    #[must_use]
    pub fn database_report(&self) -> Vec<(&'static str, jc_secret::Source)> {
        let sections = [
            (
                "kaisatsu-db-password",
                "kaisatsu-db-migrate-password",
                &self.kaisatsu.database,
            ),
            (
                "ekiben-db-password",
                "ekiben-db-migrate-password",
                &self.ekiben.database,
            ),
            (
                "antares-db-password",
                "antares-db-migrate-password",
                &self.antares.database,
            ),
            (
                "shako-db-password",
                "shako-db-migrate-password",
                &self.shako.database,
            ),
            (
                "daiya-db-password",
                "daiya-db-migrate-password",
                &self.daiya.database,
            ),
            (
                "dataroom-db-password",
                "dataroom-db-migrate-password",
                &self.dataroom.database,
            ),
        ];
        let mut out = Vec::new();
        for (app, migrate, db) in sections {
            let Some(db) = db else { continue };
            out.push((app, jc_secret::Source::File));
            if db.migrate_password_file.is_some() {
                out.push((migrate, jc_secret::Source::File));
            }
        }
        out
    }
}

/// A host name of letters, digits, dots and hyphens, optionally with `:port`.
pub(crate) fn valid_host(host: &str) -> bool {
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

fn default_true() -> bool {
    true
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
trusted_proxy_identities = ["apisix.jc-prod.serviceaccount.identity.linkerd.cluster.local"]
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
            from_file.mounts[&Module::Daiya][0].host.as_deref(),
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
        assert_eq!(
            merged.trusted_proxy_identities,
            vec!["apisix.jc-prod.serviceaccount.identity.linkerd.cluster.local".to_owned()]
        );
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
    fn a_mount_is_one_table_or_an_array_of_tables() {
        let one = Config::parse_file(
            Path::new("jc.toml"),
            "[mounts.portal]\nhost = \"portal.city.test\"\npath_prefix = \"\"\n",
        )
        .unwrap();
        assert_eq!(
            one.mounts["portal"].0,
            vec![MountOverride {
                host: Some("portal.city.test".into()),
                path_prefix: Some(String::new()),
            }]
        );
        let many = Config::parse_file(
            Path::new("jc.toml"),
            r#"
[[mounts.gateway]]
host = "api.city.test"

[[mounts.gateway]]
host = "portal.city.test"
path_prefix = "/api/endpoint"
"#,
        )
        .unwrap();
        let merged = Config::merge(cli(&["--role", "gateway"]), many).unwrap();
        assert_eq!(
            merged.mounts[&Module::Gateway],
            vec![
                MountOverride {
                    host: Some("api.city.test".into()),
                    path_prefix: None,
                },
                MountOverride {
                    host: Some("portal.city.test".into()),
                    path_prefix: Some("/api/endpoint".into()),
                },
            ]
        );
        // An unknown key is refused inside an array entry as in a single table.
        let err = Config::parse_file(
            Path::new("jc.toml"),
            "[[mounts.gateway]]\nhost = \"a.test\"\n[[mounts.gateway]]\nprefix = \"/x\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field `prefix`"), "{err}");
        let err =
            Config::parse_file(Path::new("jc.toml"), "mounts.gateway = \"api\"\n").unwrap_err();
        assert!(err.to_string().contains("array of mount tables"), "{err}");
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
    fn an_admin_listener_without_a_token_is_refused() {
        let env = BTreeMap::new();
        let load = |cfg: &Config| Secrets::load_from(cfg, &env);
        let cfg = Config::merge(
            cli(&["--role", "daiya", "--admin-listen", "127.0.0.1:0"]),
            FileConfig::default(),
        )
        .unwrap();
        assert!(matches!(load(&cfg), Err(ConfigError::AdminToken)));
        let file =
            Config::parse_file(Path::new("jc.toml"), "admin_listen = \"127.0.0.1:0\"\n").unwrap();
        let cfg = Config::merge(cli(&["--role", "daiya"]), file.clone()).unwrap();
        assert!(matches!(load(&cfg), Err(ConfigError::AdminToken)));
        let ok = Config::merge(
            cli(&["--role", "daiya", "--admin-token-file", "/run/a"]),
            file,
        )
        .unwrap();
        assert_eq!(ok.admin_token_file.as_deref(), Some(Path::new("/run/a")));
        // Given as a value instead of a file.
        let env = BTreeMap::from([("JCBIN_ADMIN_TOKEN".to_owned(), "tok".to_owned())]);
        let secrets = Secrets::load_from(&cfg, &env).unwrap();
        assert_eq!(
            secrets.admin_token.map(|g| g.source),
            Some(jc_secret::Source::Env)
        );
        // Without an admin listener no token is needed.
        let ok = Config::merge(cli(&["--role", "daiya"]), FileConfig::default()).unwrap();
        assert_eq!(ok.admin_token_file, None);
        assert_eq!(load(&ok).unwrap().admin_token, None);
    }

    /// Every secret `jc` takes from the operator, by its `NAME`, and the flag naming its file.
    const GIVEN: [(&str, &str); 8] = [
        ("JCBIN_ADMIN_TOKEN", "--admin-token-file"),
        ("JCBIN_SHAKO_SIGNING_KEY", "--shako-signing-key-file"),
        ("JCBIN_KAISATSU_KEK", "--kaisatsu-kek-file"),
        ("JCBIN_EKIBEN_TOKEN", "--ekiben-token-file"),
        ("JCBIN_ANTARES_ADMIN_TOKEN", "--antares-admin-token-file"),
        ("JCBIN_SHAKO_TOKEN", "--shako-token-file"),
        (
            "JCBIN_DATAROOM_PORTAL_TOKEN",
            "--dataroom-portal-token-file",
        ),
        (
            "JCBIN_DAIYA_PORTAL_PASSWORD",
            "--daiya-portal-password-file",
        ),
    ];

    #[test]
    fn every_secret_comes_from_name_or_name_file_and_both_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("value");
        std::fs::write(&file, "from-the-file\n").unwrap();
        let path = file.to_str().unwrap();
        for (name, flag) in GIVEN {
            // The flag's environment variable is NAME_FILE.
            let arg = Cli::command()
                .get_arguments()
                .find(|a| a.get_long() == flag.strip_prefix("--"))
                .and_then(|a| a.get_env().map(|e| e.to_string_lossy().into_owned()))
                .unwrap();
            assert_eq!(arg, format!("{name}_FILE"));
            let with_file =
                Config::merge(cli(&["--role", "all", flag, path]), FileConfig::default()).unwrap();
            let value = BTreeMap::from([(name.to_owned(), "from-the-env".to_owned())]);
            let e = Secrets::load_from(&with_file, &value)
                .unwrap_err()
                .to_string();
            assert!(e.contains(name) && e.contains("both"), "{name}: {e}");
            assert!(!e.contains("from-the-"), "{name}: {e}");
            let given = |s: &Secrets| -> Option<Given> {
                match name {
                    "JCBIN_ADMIN_TOKEN" => s.admin_token.clone(),
                    "JCBIN_SHAKO_SIGNING_KEY" => s.shako_signing_key.clone(),
                    _ => s.module.values().next().cloned(),
                }
            };
            let from_file = Secrets::load_from(&with_file, &BTreeMap::new()).unwrap();
            let g = given(&from_file).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(
                (g.secret.expose(), g.source),
                ("from-the-file", jc_secret::Source::File)
            );
            let env_only = Config::merge(cli(&["--role", "all"]), FileConfig::default()).unwrap();
            let from_env = Secrets::load_from(&env_only, &value).unwrap();
            let g = given(&from_env).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(
                (g.secret.expose(), g.source),
                ("from-the-env", jc_secret::Source::Env)
            );
            // Debug of the whole set never shows a value.
            assert!(!format!("{from_env:?}{from_file:?}").contains("from-the-"));
        }
        // The age identity is a file only.
        let env = BTreeMap::from([(
            "JCBIN_PORTAL_AGE_KEY".to_owned(),
            "AGE-SECRET-KEY-1".to_owned(),
        )]);
        let cfg = Config::merge(cli(&["--role", "portal"]), FileConfig::default()).unwrap();
        let e = Secrets::load_from(&cfg, &env).unwrap_err().to_string();
        assert!(
            e.contains("JCBIN_PORTAL_AGE_KEY_FILE") && !e.contains("AGE-SECRET"),
            "{e}"
        );
    }

    #[test]
    fn the_model_key_is_a_file_only_and_its_value_never_shows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("model-key");
        std::fs::write(&file, "sk-or-v1-test-only\n").unwrap();
        let path = file.to_str().unwrap();
        let cfg = Config::merge(
            cli(&["--role", "all", "--model-key-file", path]),
            FileConfig::default(),
        )
        .unwrap();
        let secrets = Secrets::load_from(&cfg, &BTreeMap::new()).unwrap();
        let key = secrets.model_key.as_ref().unwrap();
        assert_eq!(
            (key.secret.expose(), key.source),
            ("sk-or-v1-test-only", jc_secret::Source::File)
        );
        assert!(!format!("{secrets:?}").contains("sk-or-v1"));
        // As a value, under jc's name or the proxy's, it is refused and the error names no value.
        for name in ["JCBIN_MODEL_KEY", "JC_MODEL_KEY"] {
            let env = BTreeMap::from([(name.to_owned(), "sk-or-v1-from-env".to_owned())]);
            let e = Secrets::load_from(&cfg, &env).unwrap_err().to_string();
            assert!(e.contains(name) && e.contains("file"), "{e}");
            assert!(!e.contains("sk-or-v1"), "{e}");
        }
        // The assistant's own lookup never answers the key.
        assert_eq!(assistant_env("JC_MODEL_KEY"), None);
        assert_eq!(assistant_env("JC_MODEL_KEY_FILE"), None);
        assert_eq!(assistant_env("JC_OIDC_CLIENT_SECRET"), None);
    }

    #[test]
    fn pipeline_client_secrets_come_from_the_directory_by_client_name() {
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str, value: &str| std::fs::write(dir.path().join(name), value).unwrap();
        at("zilina-mesto-pipelines", "s-zilina\n");
        at("praha-pipelines", "s-praha");
        // kaisatsu's other import secrets and odd names are left alone.
        at("e2e-person-password", "pw");
        at("-pipelines", "x");
        at("Upper-pipelines", "x");
        let dir_arg = dir.path().to_str().unwrap();
        let load = |args: &[&str]| {
            let mut all = vec!["--role", "ekiben"];
            all.extend_from_slice(args);
            Secrets::load_from(
                &Config::merge(cli(&all), FileConfig::default()).unwrap(),
                &BTreeMap::new(),
            )
        };
        let secrets = load(&["--ekiben-client-secrets-dir", dir_arg]).unwrap();
        let names: Vec<&str> = secrets
            .pipeline_client_secrets
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            names,
            ["JC_CLIENT_SECRET_PRAHA", "JC_CLIENT_SECRET_ZILINA_MESTO"]
        );
        assert_eq!(
            secrets.pipeline_client_secrets["JC_CLIENT_SECRET_ZILINA_MESTO"].expose(),
            "s-zilina"
        );
        assert!(
            !format!("{secrets:?}").contains("s-zilina"),
            "Debug hides it"
        );
        // The default is kaisatsu's import secrets directory, the file a realm file names.
        let shared = load(&["--kaisatsu-import-secrets-dir", dir_arg]).unwrap();
        assert_eq!(shared.pipeline_client_secrets.len(), 2);
        let harness = load(&[
            "--ekiben-client-secrets-dir",
            dir_arg,
            "--ekiben-harness-client",
            "praha-pipelines",
        ])
        .unwrap();
        assert_eq!(
            harness.pipeline_client_secrets["JC_CLIENT_SECRET"].expose(),
            "s-praha"
        );
        let missing = load(&[
            "--ekiben-client-secrets-dir",
            dir_arg,
            "--ekiben-harness-client",
            "brno-pipelines",
        ])
        .unwrap_err()
        .to_string();
        assert!(missing.contains("brno-pipelines"), "{missing}");
        assert!(load(&["--ekiben-harness-client", "praha-pipelines"]).is_err());
        // An empty file and a symbolic link are refused, naming the file only.
        at("brno-pipelines", " ");
        let empty = load(&["--ekiben-client-secrets-dir", dir_arg])
            .unwrap_err()
            .to_string();
        assert!(empty.contains("brno-pipelines"), "{empty}");
        std::fs::remove_file(dir.path().join("brno-pipelines")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hostname", dir.path().join("brno-pipelines")).unwrap();
            let link = load(&["--ekiben-client-secrets-dir", dir_arg])
                .unwrap_err()
                .to_string();
            assert!(link.contains("symbolic link"), "{link}");
        }
        assert!(load(&["--ekiben-client-secrets-dir", "/nonexistent/jc"]).is_err());
    }

    #[test]
    fn ekiben_vars_are_read_by_jc_and_handed_over_by_name() {
        let cfg = Config::merge(cli(&["--role", "ekiben"]), FileConfig::default()).unwrap();
        let env = BTreeMap::from([(
            "EKIBEN_VAR_JC_GATEWAY_URL".to_owned(),
            "http://gw".to_owned(),
        )]);
        let secrets = Secrets::load_from(&cfg, &env).unwrap();
        assert_eq!(secrets.ekiben_vars["JC_GATEWAY_URL"].expose(), "http://gw");
        assert_eq!(
            secrets.ekiben_vars["EKIBEN_VAR_JC_GATEWAY_URL"].expose(),
            "http://gw"
        );
    }

    #[test]
    fn a_remote_module_is_checked_and_its_client_secret_is_required() {
        let parse = |toml: &str| Config::parse_file(Path::new("jc.toml"), toml).unwrap();
        let remote = "[remote]\ntoken_url = \"http://id/token\"\nclient_id = \"portal-remote\"\n[remote.modules]\n";
        let merge = |role: &str, modules: &str| {
            Config::merge(cli(&["--role", role]), parse(&format!("{remote}{modules}")))
        };
        let cfg = merge("portal", "shako = \"https://git.city.test/\"").unwrap();
        assert_eq!(cfg.remote_modules[&Module::Shako], "https://git.city.test");
        for (role, modules, said) in [
            (
                "dataroom",
                "shako = \"https://g\"",
                "only a jc running the Portal",
            ),
            ("portal,shako", "shako = \"https://g\"", "runs in this jc"),
            (
                "portal",
                "kaisatsu = \"https://g\"",
                "cannot run in another",
            ),
            ("portal", "shako = \"file:///etc\"", "not an http(s) URL"),
        ] {
            let err = merge(role, modules).unwrap_err().to_string();
            assert!(err.contains(said), "{role} {modules}: {err}");
        }
        let err = Secrets::load_from(&cfg, &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("no client secret"), "{err}");
        let env = BTreeMap::from([("JCBIN_REMOTE_CLIENT_SECRET".to_owned(), "s3cret".to_owned())]);
        let secrets = Secrets::load_from(&cfg, &env).unwrap();
        assert_eq!(
            secrets.remote_client_secret.unwrap().source,
            jc_secret::Source::Env
        );
    }

    const TWO_ORGS: &str = include_str!("../tests/fixtures/two-orgs.toml");

    fn orgs(text: &str) -> Result<Config, ConfigError> {
        let file = Config::parse_file(Path::new("jc.toml"), text)?;
        Config::merge(cli(&["--role", "dataroom"]), file)
    }

    #[test]
    fn organizations_are_read_and_each_bad_one_is_refused() {
        let cfg = orgs(TWO_ORGS).unwrap();
        assert_eq!(cfg.organizations.len(), 2);
        assert_eq!(cfg.organizations[1].name, "beta");
        assert_eq!(cfg.organizations[1].hosts.mcp, "mcp.beta.test");
        // Hosts are compared lowercased: an upper-case spelling is the same host.
        let upper = TWO_ORGS.replacen("opendata.alpha.test", "OpenData.Alpha.test", 1);
        assert_eq!(
            orgs(&upper).unwrap().organizations[0].hosts.opendata,
            "opendata.alpha.test"
        );
        for (from, to, says) in [
            ("\"beta\"", "\"alpha\"", "named twice"),
            ("\"beta\"", "\"Beta\"", "not a lowercase Organization name"),
            ("\"beta\"", "\"-b\"", "not a lowercase Organization name"),
            (
                "0000000000b1",
                "0000000000a1",
                "belongs to another Organization",
            ),
            ("0000000000b1", "b1", "is not a UUID"),
            ("api.beta.test", "api.alpha.test", "also a host of alpha"),
            // The same host twice inside one Organization, and with only a port apart.
            ("mcp.beta.test", "api.beta.test", "also a host of beta"),
            ("mcp.beta.test", "api.beta.test:8443", "also a host of beta"),
            ("api.beta.test", "*.beta.test", "not a plain host name"),
            (
                "api.beta.test",
                "https://api.beta.test",
                "not a plain host name",
            ),
        ] {
            let text = TWO_ORGS.replace(from, to);
            let err = orgs(&text).expect_err(to).to_string();
            assert!(err.contains(says), "{to}: {err}");
        }
        // An unknown key and a missing host are refused by the parser.
        let err =
            orgs(&TWO_ORGS.replace("mcp = \"mcp.beta.test\"", "mcp = \"m.b\", www = \"w.b\""))
                .unwrap_err()
                .to_string();
        assert!(err.contains("unknown field `www`"), "{err}");
        let err = orgs(&TWO_ORGS.replace(", mcp = \"mcp.beta.test\"", ""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `mcp`"), "{err}");
    }

    #[test]
    fn dataroom_addresses_are_read_one_by_one_and_a_bad_one_is_refused() {
        let file = Config::parse_file(
            Path::new("jc.toml"),
            "[dataroom.addresses]\napi = \"https://api.dev.example\"\ntiles = \"https://tiles.dev.example\"\n",
        )
        .unwrap();
        let cfg = Config::merge(cli(&["--role", "dataroom"]), file).unwrap();
        assert_eq!(
            cfg.dataroom.addresses.api.as_deref(),
            Some("https://api.dev.example")
        );
        assert_eq!(cfg.dataroom.addresses.assistant, None);

        let typo = Config::parse_file(
            Path::new("jc.toml"),
            "[dataroom.addresses]\napis = \"https://x.example\"\n",
        );
        assert!(typo.is_err(), "an unknown key is refused");
        let bad = Config::parse_file(
            Path::new("jc.toml"),
            "[dataroom.addresses]\nviz = \"https://viz.example/?x=\\\"\"\n",
        )
        .unwrap();
        let err = Config::merge(cli(&["--role", "dataroom"]), bad).unwrap_err();
        assert!(matches!(err, ConfigError::Address("viz", _)), "{err}");
    }

    #[test]
    fn dataroom_aliases_are_read_as_old_to_new_names() {
        let file = Config::parse_file(
            Path::new("jc.toml"),
            "[dataroom.aliases]\ncatalog = \"zilina-catalog\"\n",
        )
        .unwrap();
        let cfg = Config::merge(cli(&["--role", "dataroom"]), file).unwrap();
        assert_eq!(
            cfg.dataroom.aliases,
            BTreeMap::from([("catalog".to_owned(), "zilina-catalog".to_owned())])
        );
        let cfg = Config::merge(cli(&["--role", "dataroom"]), FileConfig::default()).unwrap();
        assert!(cfg.dataroom.aliases.is_empty());
    }

    #[test]
    fn the_identity_realm_defaults_to_jc_and_refuses_unsafe_names() {
        let file = |text: &str| Config::parse_file(Path::new("jc.toml"), text).unwrap();
        let cfg = Config::merge(cli(&[]), FileConfig::default()).unwrap();
        assert_eq!(cfg.realm.as_str(), "jc");
        let cfg = Config::merge(cli(&[]), file("[identity]\nrealm = \"dev\"\n")).unwrap();
        assert_eq!(cfg.realm.as_str(), "dev");
        // The flag (and so JCBIN_IDENTITY_REALM) wins over the file.
        let cfg = Config::merge(
            cli(&["--identity-realm", "smartcity"]),
            file("[identity]\nrealm = \"dev\"\n"),
        )
        .unwrap();
        assert_eq!(cfg.realm.as_str(), "smartcity");
        for bad in ["", "master", "MASTER", "a/b", "../jc", "a b", ".x", "dév"] {
            let err =
                Config::merge(cli(&["--identity-realm", bad]), FileConfig::default()).unwrap_err();
            assert!(matches!(err, ConfigError::Realm(_)), "{bad:?}: {err}");
            let text = format!("[identity]\nrealm = {bad:?}\n");
            let err = Config::merge(cli(&[]), file(&text)).unwrap_err();
            assert!(matches!(err, ConfigError::Realm(_)), "{bad:?}: {err}");
        }
        assert!(
            Config::parse_file(Path::new("jc.toml"), "[identity]\nrealms = \"dev\"\n").is_err(),
            "an unknown key is refused"
        );
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
        // The start report names each password file, shako's migration one too, as a file.
        let report = cfg.database_report();
        assert_eq!(report.len(), 7, "{report:?}");
        assert!(report.contains(&("shako-db-migrate-password", jc_secret::Source::File)));
        assert!(bare.database_report().is_empty());

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
