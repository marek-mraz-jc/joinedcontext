//! `jc doctor domains` (evidence/T-6991/design.md §1, §5, §6, §9; T-6975 §9): reads jc.toml and
//! says, per Organization, which origins it has and which the configuration merges with what that
//! loses, the records the customer adds, what the authoritative name servers answer against the
//! edge, CAA, the HTTP-01 path, the certificates the CT log lists for each registered domain in the
//! last 7 days, whether its sign-in is still on its working host and, with `--kubeconfig`, each
//! Certificate's Ready condition. Exit 1 on a missing or wrong record or any other error.
//!
//! Only host names are read from jc.toml: no secret, token file or database address is printed.
//! The network sits behind [`World`], so the report is tested on a stub resolver and CT answer.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use serde::Deserialize;

use crate::config::MountEntries;

/// Let's Encrypt: new certificates per registered domain per 7 days (T-6975 §6).
const WEEKLY_LIMIT: usize = 50;
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Parser)]
#[command(
    name = "jc doctor domains",
    about = "Check the records, certificates and origins of jc.toml's hosts"
)]
struct Args {
    /// The jc.toml to read.
    #[arg(long)]
    config: PathBuf,
    /// The edge's public address every record must reach; repeat for several. Default: the
    /// addresses of `base_domain` on its authoritative name servers.
    #[arg(long)]
    edge: Vec<IpAddr>,
    /// The operator's validation zone for the Apps wildcards (deployment
    /// global.ingress.dns01.validationZone), e.g. `acme.example.net`.
    #[arg(long)]
    validation_zone: Option<String>,
    /// Also read each cert-manager Certificate's Ready condition (kubectl with this file).
    #[arg(long)]
    kubeconfig: Option<PathBuf>,
}

/// The keys of jc.toml this command reads; every other key is left to jc's own parser.
#[derive(Debug, Default, Deserialize)]
struct DomainsFile {
    base_domain: Option<String>,
    base_host: Option<String>,
    apps_domain: Option<String>,
    login_host: Option<String>,
    #[serde(default)]
    organizations: Vec<Org>,
    #[serde(default)]
    mounts: BTreeMap<String, MountEntries>,
}

#[derive(Debug, Deserialize)]
struct Org {
    name: String,
    host: Option<String>,
    hosts: Option<Hosts>,
    #[serde(default)]
    same_site_apps: bool,
}

#[derive(Debug, Deserialize)]
struct Hosts {
    portal: String,
    opendata: Option<String>,
    api: Option<String>,
    mcp: Option<String>,
    apps: Option<String>,
    #[serde(default)]
    portals: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
}

/// One cert-manager Certificate as kubectl lists it.
#[derive(Debug, Clone)]
pub struct CertificateState {
    pub name: String,
    pub dns_names: Vec<String>,
    pub ready: bool,
    pub not_after: Option<String>,
}

/// What the report asks of the network.
pub trait World {
    /// The addresses `host` resolves to on its zone's authoritative name servers, CNAMEs
    /// followed; empty when there is no record.
    fn addresses(&self, host: &str) -> Result<Vec<IpAddr>, String>;
    /// The CAA records of exactly `name` as `<tag> <value>` (`issue letsencrypt.org`).
    fn caa(&self, name: &str) -> Result<Vec<String>, String>;
    /// The HTTP status `http://<host>/.well-known/acme-challenge/<probe>` answers.
    fn http01(&self, host: &str) -> Result<u16, String>;
    /// Certificates the CT log lists for `domain` and its subdomains, issued in the last 7 days.
    fn issued_last_week(&self, domain: &str) -> Result<usize, String>;
    /// cert-manager's Certificates; `None` without `--kubeconfig`.
    fn certificates(&self) -> Option<Result<Vec<CertificateState>, String>>;
}

/// One origin of an Organization.
#[derive(Debug, PartialEq, Eq)]
struct Origin {
    role: &'static str,
    host: String,
    /// A record the customer adds (false: the installation's host or a path on it).
    record: bool,
}

#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<(Level, String)>,
}

impl Report {
    fn say(&mut self, level: Level, text: impl Into<String>) {
        self.lines.push((level, text.into()));
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.lines.iter().any(|(l, _)| *l == Level::Error)
    }
}

/// `jc doctor <args>`: 0 all good, 1 a missing or wrong record or another error, 2 bad usage.
#[must_use]
pub fn command(args: &[String]) -> u8 {
    if args.first().map(String::as_str) != Some("domains") {
        eprintln!(
            "usage: jc doctor domains --config <jc.toml> [--edge <address>] [--validation-zone <zone>] [--kubeconfig <file>]"
        );
        return 2;
    }
    let args = match Args::try_parse_from(args) {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return 2;
        }
    };
    let file = match std::fs::read_to_string(&args.config)
        .map_err(|e| e.to_string())
        .and_then(|text| toml::from_str::<DomainsFile>(&text).map_err(|e| e.to_string()))
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("jc doctor: {}: {e}", args.config.display());
            return 2;
        }
    };
    let world = match Net::new(args.kubeconfig.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("jc doctor: {e}");
            return 1;
        }
    };
    let report = report(&file, &args.edge, args.validation_zone.as_deref(), &world);
    for (level, text) in &report.lines {
        let tag = match level {
            Level::Ok => "ok   ",
            Level::Info => "     ",
            Level::Warn => "warn ",
            Level::Error => "ERROR",
        };
        println!("{tag} {text}");
    }
    u8::from(report.failed())
}

fn bare(host: &str) -> &str {
    host.split(':').next().unwrap_or_default()
}

/// The registered domain: the last two labels. ponytail: a two-label public suffix (`co.uk`)
/// reads as the domain itself; the public suffix list is the upgrade when a customer has one.
fn registrable(host: &str) -> String {
    let labels: Vec<&str> = bare(host).trim_end_matches('.').rsplitn(3, '.').collect();
    match labels.as_slice() {
        [tld, sld, ..] => format!("{sld}.{tld}"),
        _ => host.to_owned(),
    }
}

struct Layout {
    base: String,
    apps: String,
    login: String,
    explicit_login: bool,
}

/// Each origin of `org` and, for every one the configuration merges, what that loses (§1).
fn origins(layout: &Layout, org: &Org) -> (Vec<Origin>, Vec<String>) {
    let o = |role, host: String, record| Origin { role, host, record };
    let base = &layout.base;
    let mut out = Vec::new();
    let mut merged = Vec::new();
    if org.host.as_deref() == Some("auto") {
        out.push(o("working", format!("{}.{base}", org.name), false));
        out.push(o(
            "open data",
            format!("opendata.{base}/{}/", org.name),
            false,
        ));
        out.push(o("API", format!("api.{base}/{}/", org.name), false));
        merged.push(
            "auto: no origin merged and no record to add, but the working host is one site with every \
             other auto Organization's: a same-site POST carries the Lax session cookie, and only the \
             __Host-jc_csrf token stops it (§6)"
                .to_owned(),
        );
    } else if let Some(h) = &org.hosts {
        out.push(o("working", h.portal.clone(), true));
        match &h.opendata {
            Some(host) => out.push(o("open data", host.clone(), true)),
            None => merged.push(format!(
                "open data merged into the working host ({}/data/): an injection in a public page (a \
                 dataset description) runs where the session cookie is; anonymous visitors and staff \
                 share one origin's rate limits and CSP",
                h.portal
            )),
        }
        match h.api.as_ref().or(h.mcp.as_ref()) {
            Some(host) => out.push(o("API", host.clone(), true)),
            None => merged.push(format!(
                "API merged into the working host ({}/api/): a script on a Portal page can call the data \
                 API with the person's bearer token (no new right, one origin less between a Portal XSS \
                 and the data); third-party browser apps read the API only on the open data host or \
                 api.{base}",
                h.portal
            )),
        }
        out.extend(
            h.portals
                .iter()
                .map(|p| o("further Portal", p.clone(), true)),
        );
    }
    out.push(o("sign-in", layout.login.clone(), false));
    match org.hosts.as_ref().and_then(|h| h.apps.as_ref()) {
        Some(apps) => out.push(o("Apps", format!("*.{apps}"), true)),
        None => {
            out.push(o("Apps", format!("*.{}", layout.apps), false));
            if org.hosts.is_some() {
                merged.push(format!(
                    "Apps on the installation's {}: they carry the installation's name, not the customer's",
                    layout.apps
                ));
            }
        }
    }
    (out, merged)
}

/// What the Organizations add to the checks: hosts to resolve with the record each needs, the
/// registered domains to count in the CT log, and every host named (each once).
struct Checks {
    hosts: Vec<(String, String)>,
    domains: BTreeSet<String>,
    names: BTreeSet<String>,
}

fn report(file: &DomainsFile, edge: &[IpAddr], zone: Option<&str>, world: &dyn World) -> Report {
    let mut r = Report::default();
    let Some(base) = file
        .base_domain
        .as_deref()
        .or(file.base_host.as_deref())
        .map(|b| bare(b).to_owned())
    else {
        r.say(
            Level::Error,
            "jc.toml names no base_domain (or base_host): nothing to check",
        );
        return r;
    };
    let layout = Layout {
        apps: file
            .apps_domain
            .clone()
            .unwrap_or_else(|| format!("apps.{base}")),
        login: file
            .login_host
            .clone()
            .unwrap_or_else(|| format!("login.{base}")),
        explicit_login: file.login_host.is_some(),
        base: base.clone(),
    };
    let edge: Vec<IpAddr> = if edge.is_empty() {
        match world.addresses(&base) {
            Ok(a) if !a.is_empty() => a,
            Ok(_) => {
                r.say(Level::Error, format!("{base} has no address record: the edge address is unknown; add its A record or pass --edge"));
                return r;
            }
            Err(e) => {
                r.say(Level::Error, format!("{base}: {e}; pass --edge"));
                return r;
            }
        }
    } else {
        edge.to_vec()
    };
    let edge_list = edge
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    r.say(
        Level::Info,
        format!(
            "installation {base}: edge {edge_list}, sign-in {}, Apps *.{}",
            layout.login, layout.apps
        ),
    );
    if registrable(&layout.apps) == registrable(&base) {
        r.say(
            Level::Warn,
            format!(
                "apps_domain {} is the same site as {base}: an App can send a same-site request with a Lax \
                 session cookie; recommended an Apps domain on a registrable domain of its own (§6)",
                layout.apps
            ),
        );
    }
    let sign_in_hosts: BTreeSet<String> = file
        .mounts
        .get("kaisatsu")
        .map(|m| {
            m.0.iter()
                .filter_map(|e| e.host.as_deref().map(|h| bare(h).to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let mut acc = Checks {
        hosts: vec![
            (base.clone(), format!("{base} A <edge address>")),
            (
                layout.login.clone(),
                format!("{} CNAME {base}.", layout.login),
            ),
        ],
        domains: BTreeSet::from([registrable(&base)]),
        names: BTreeSet::from([base.clone(), layout.login.clone()]),
    };
    for org in &file.organizations {
        organization(&mut r, &layout, org, zone, &sign_in_hosts, &mut acc);
    }
    resolution(&mut r, world, &edge, &edge_list, &acc);
    certificates(&mut r, world, &acc.names);
    r
}

/// One Organization: its origins, what the merged ones lose, its records and the refusals.
fn organization(
    r: &mut Report,
    layout: &Layout,
    org: &Org,
    zone: Option<&str>,
    sign_in_hosts: &BTreeSet<String>,
    acc: &mut Checks,
) {
    let base = &layout.base;
    {
        r.say(Level::Info, format!("Organization {}", org.name));
        if org.host.as_deref().is_some_and(|h| h != "auto")
            || (org.host.is_some() == org.hosts.is_some())
        {
            r.say(
                Level::Error,
                format!(
                    "  {}: give either host = \"auto\" or hosts = {{ portal = ... }}",
                    org.name
                ),
            );
            return;
        }
        let (origins, merged) = origins(layout, org);
        let records = origins.iter().filter(|o| o.record).count();
        let setup = match (org.host.is_some(), records) {
            (true, _) => "auto, 0 records".to_owned(),
            (false, n) => {
                let apps = usize::from(origins.iter().any(|o| o.role == "Apps" && o.record));
                format!("{} customer origins, {} records", n, n + apps)
            }
        };
        r.say(Level::Info, format!("  setup: {setup}"));
        for o in &origins {
            r.say(Level::Info, format!("  {:<15} {}", o.role, o.host));
        }
        if org.host.is_some() {
            acc.hosts.push((
                origins[0].host.clone(),
                format!("*.{base} A <edge address>"),
            ));
        }
        for m in &merged {
            r.say(Level::Info, format!("  loses: {m}"));
        }
        if jc_core::names::RESERVED_LABELS.contains(&org.name.as_str()) && org.host.is_some() {
            r.say(Level::Error, format!("  {}.{base}: '{}' is reserved for the platform (a service's host or path); choose another name", org.name, org.name));
        }
        for o in origins.iter().filter(|o| o.record) {
            let host = o.host.trim_start_matches("*.");
            if let Some(label) = host.strip_suffix(&format!(".{base}"))
                && !label.contains('.')
                && jc_core::names::RESERVED_LABELS.contains(&label)
            {
                r.say(Level::Error, format!("  {host}: the label '{label}' is reserved for the platform; choose another host"));
            }
            if !acc.names.insert(host.to_owned()) {
                r.say(
                    Level::Error,
                    format!("  {host} is named twice in jc.toml; each host belongs to one origin"),
                );
            }
            acc.domains.insert(registrable(host));
            if o.role == "Apps" {
                let label = host.replace('.', "-");
                let zone = zone.unwrap_or("<validation zone>");
                acc.hosts.push((
                    format!("jc-doctor.{host}"),
                    format!("*.{host} CNAME {base}."),
                ));
                r.say(Level::Info, format!("  record: *.{host} CNAME {base}."));
                r.say(
                    Level::Info,
                    format!("  record: _acme-challenge.{host} CNAME {label}.{zone}."),
                );
                if zone == "<validation zone>" {
                    r.say(Level::Warn, format!("  *.{host}: no --validation-zone: without the deployment's dns01 zone each App gets a certificate of its own by HTTP-01"));
                }
                if registrable(host) == registrable(&origins[0].host) {
                    let level = if org.same_site_apps {
                        Level::Warn
                    } else {
                        Level::Error
                    };
                    r.say(level, format!(
                        "  *.{host} is the same site as the working host {}: an App can send a same-site POST with the session cookie; jc refuses it unless same_site_apps = true; recommended the installation's apps_domain (§6)",
                        origins[0].host
                    ));
                }
            } else {
                r.say(Level::Info, format!("  record: {host} CNAME {base}."));
                acc.hosts
                    .push((host.to_owned(), format!("{host} CNAME {base}.")));
            }
        }
        let working = bare(&origins[0].host).to_owned();
        if sign_in_hosts.contains(&working) {
            r.say(Level::Warn, format!(
                "  sign-in is still on the working host {working}: passwords and second factors are typed there; it moves to {} when the operator moves its issuer (API/15 §6)",
                layout.login
            ));
        } else if !layout.explicit_login {
            r.say(
                Level::Info,
                format!(
                    "  sign-in on {} (login_host not set: the default)",
                    layout.login
                ),
            );
        }
    }
}

/// Each host against the edge on its authoritative servers, CAA, HTTP-01, and the CT counts.
fn resolution(r: &mut Report, world: &dyn World, edge: &[IpAddr], edge_list: &str, acc: &Checks) {
    r.say(
        Level::Info,
        "records against the authoritative name servers",
    );
    for (host, record) in &acc.hosts {
        match world.addresses(host) {
            Ok(found) if found.is_empty() => {
                r.say(Level::Error, format!("{host}: no record; add {record}"))
            }
            Ok(found) if found.iter().all(|a| edge.contains(a)) => {
                r.say(Level::Ok, format!("{host} -> {}", join(&found)))
            }
            Ok(found) => r.say(
                Level::Error,
                format!(
                    "{host} -> {}, not the edge ({edge_list}); make it {record}",
                    join(&found)
                ),
            ),
            Err(e) => r.say(Level::Error, format!("{host}: {e}")),
        }
    }
    for (host, _) in acc
        .hosts
        .iter()
        .filter(|(h, _)| !h.starts_with("jc-doctor."))
    {
        caa(r, world, host);
        match world.http01(host) {
            Ok(status) => r.say(Level::Ok, format!("{host}: HTTP-01 path answers {status} on port 80")),
            Err(e) => r.say(Level::Error, format!("{host}: HTTP-01 path unreachable on port 80 ({e}); Let's Encrypt cannot prove this host")),
        }
    }
    for domain in &acc.domains {
        match world.issued_last_week(domain) {
            Ok(n) if n >= WEEKLY_LIMIT => r.say(Level::Error, format!("{domain}: {n} certificates in 7 days, at Let's Encrypt's {WEEKLY_LIMIT}: a new one waits (every instance on this domain counts)")),
            Ok(n) if n * 5 >= WEEKLY_LIMIT * 4 => r.say(Level::Warn, format!("{domain}: {n} of {WEEKLY_LIMIT} new certificates in 7 days; stage the rest")),
            // crt.sh lags and truncates (2026-11-15: 25 rows for joinedcontext.com, the newest
            // 2026-10-07), so its count is a floor, never a clearance.
            Ok(n) => r.say(Level::Info, format!("{domain}: at least {n} of {WEEKLY_LIMIT} new certificates in 7 days (crt.sh lags; kubectl get certificate is the full count)")),
            Err(e) => r.say(Level::Warn, format!("{domain}: CT log unreadable ({e}); the weekly count is unknown")),
        }
    }
}

/// With `--kubeconfig`: the Certificates naming each host, Ready or not.
fn certificates(r: &mut Report, world: &dyn World, names: &BTreeSet<String>) {
    if let Some(certs) = world.certificates() {
        match certs {
            Ok(certs) => {
                for host in names.iter().map(String::as_str) {
                    let wildcard = format!("*.{host}");
                    let found: Vec<&CertificateState> = certs
                        .iter()
                        .filter(|c| c.dns_names.iter().any(|n| n == host || *n == wildcard))
                        .collect();
                    if found.is_empty() {
                        r.say(Level::Warn, format!("{host}: no Certificate names it yet"));
                    }
                    for c in found {
                        let until = c.not_after.as_deref().unwrap_or("unknown");
                        let level = if c.ready { Level::Ok } else { Level::Error };
                        r.say(
                            level,
                            format!(
                                "{host}: Certificate {} {} (until {until})",
                                c.name,
                                if c.ready { "Ready" } else { "not Ready" }
                            ),
                        );
                    }
                }
            }
            Err(e) => r.say(Level::Error, format!("Certificates unreadable: {e}")),
        }
    }
}

fn join(a: &[IpAddr]) -> String {
    a.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// CAA (RFC 8659): the closest name from `host` up to its registered domain with CAA records
/// decides; when it has `issue` records, one must name letsencrypt.org.
fn caa(r: &mut Report, world: &dyn World, host: &str) {
    let stop = registrable(host);
    let mut name = host.to_owned();
    loop {
        match world.caa(&name) {
            Ok(records) if !records.is_empty() => {
                let issue: Vec<&String> =
                    records.iter().filter(|c| c.starts_with("issue ")).collect();
                if !issue.is_empty()
                    && !issue.iter().any(|c| {
                        c.trim_start_matches("issue ")
                            .trim_matches('"')
                            .starts_with("letsencrypt.org")
                    })
                {
                    r.say(Level::Error, format!("{host}: CAA on {name} permits no letsencrypt.org; add {name} CAA 0 issue \"letsencrypt.org\""));
                }
                return;
            }
            Ok(_) => {}
            Err(e) => {
                r.say(
                    Level::Warn,
                    format!("{host}: CAA unreadable on {name} ({e})"),
                );
                return;
            }
        }
        if name == stop {
            return;
        }
        match name.split_once('.') {
            Some((_, parent)) => name = parent.to_owned(),
            None => return,
        }
    }
}

/// The network: hickory for DNS (authoritative servers), reqwest for HTTP-01 and crt.sh, kubectl.
struct Net {
    rt: tokio::runtime::Runtime,
    system: hickory_resolver::TokioResolver,
    http: reqwest::blocking::Client,
    kubeconfig: Option<PathBuf>,
}

impl Net {
    fn new(kubeconfig: Option<PathBuf>) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let mut builder = hickory_resolver::TokioResolver::builder_tokio()
            .map_err(|e| format!("the system resolver configuration is unreadable: {e}"))?;
        builder.options_mut().timeout = TIMEOUT;
        let system = builder
            .build()
            .map_err(|e| format!("the resolver could not start: {e}"))?;
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT * 3)
            .user_agent("jc doctor domains")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rt,
            system,
            http,
            kubeconfig,
        })
    }

    /// A resolver asking only the name servers of the zone `name` is in.
    fn authoritative(&self, name: &str) -> Result<hickory_resolver::TokioResolver, String> {
        use hickory_resolver::config::{NameServerConfig, ResolverConfig};
        use hickory_resolver::proto::rr::{RData, RecordType};
        let mut zone = name.trim_end_matches('.').to_owned();
        let servers = loop {
            let fqdn = format!("{zone}.");
            match self
                .rt
                .block_on(self.system.lookup(fqdn.as_str(), RecordType::NS))
            {
                Ok(answer) => {
                    let ns: Vec<String> = answer
                        .answers()
                        .iter()
                        .filter(|rec| rec.name.to_ascii().eq_ignore_ascii_case(&fqdn))
                        .filter_map(|rec| match &rec.data {
                            RData::NS(ns) => Some(ns.0.to_ascii()),
                            _ => None,
                        })
                        .collect();
                    if !ns.is_empty() {
                        break ns;
                    }
                }
                Err(e) if e.is_no_records_found() || e.is_nx_domain() => {}
                Err(e) => return Err(format!("NS of {zone}: {e}")),
            }
            match zone.split_once('.') {
                Some((_, parent)) if parent.contains('.') => zone = parent.to_owned(),
                _ => return Err(format!("no zone with name servers above {name}")),
            }
        };
        let mut ips = Vec::new();
        for server in &servers {
            if let Ok(found) = self.rt.block_on(self.system.lookup_ip(server.as_str())) {
                ips.extend(found.iter());
            }
        }
        if ips.is_empty() {
            return Err(format!(
                "the name servers of {zone} ({}) have no address",
                servers.join(", ")
            ));
        }
        let config = ResolverConfig::from_name_servers(
            ips.into_iter().map(NameServerConfig::udp_and_tcp).collect(),
        );
        let mut builder =
            hickory_resolver::TokioResolver::builder_with_config(config, Default::default());
        builder.options_mut().timeout = TIMEOUT;
        builder.options_mut().cache_size = 0;
        builder
            .build()
            .map_err(|e| format!("resolver for {zone}: {e}"))
    }
}

impl World for Net {
    fn addresses(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        use hickory_resolver::proto::rr::{RData, RecordType};
        let mut name = host.trim_end_matches('.').to_owned();
        // A CNAME chain of at most 8 hops, each asked of its own zone's servers.
        for _ in 0..8 {
            let auth = self.authoritative(&name)?;
            let fqdn = format!("{name}.");
            let target = match self
                .rt
                .block_on(auth.lookup(fqdn.as_str(), RecordType::CNAME))
            {
                Ok(a) => a.answers().iter().find_map(|rec| match &rec.data {
                    RData::CNAME(c) => Some(c.0.to_ascii()),
                    _ => None,
                }),
                Err(e) if e.is_no_records_found() || e.is_nx_domain() => None,
                Err(e) => return Err(format!("CNAME of {name}: {e}")),
            };
            if let Some(target) = target {
                name = target.trim_end_matches('.').to_owned();
                continue;
            }
            let mut out = Vec::new();
            for kind in [RecordType::A, RecordType::AAAA] {
                match self.rt.block_on(auth.lookup(fqdn.as_str(), kind)) {
                    Ok(a) => out.extend(a.answers().iter().filter_map(|rec| match &rec.data {
                        RData::A(a) => Some(IpAddr::V4(a.0)),
                        RData::AAAA(a) => Some(IpAddr::V6(a.0)),
                        _ => None,
                    })),
                    Err(e) if e.is_no_records_found() || e.is_nx_domain() => {}
                    Err(e) => return Err(format!("{kind} of {name}: {e}")),
                }
            }
            return Ok(out);
        }
        Err(format!("{host}: a CNAME chain longer than 8"))
    }

    fn caa(&self, name: &str) -> Result<Vec<String>, String> {
        use hickory_resolver::proto::rr::{RData, RecordType};
        match self.rt.block_on(
            self.system
                .lookup(format!("{name}.").as_str(), RecordType::CAA),
        ) {
            Ok(a) => Ok(a
                .answers()
                .iter()
                .filter_map(|rec| match &rec.data {
                    // Displayed as `<flags> <tag> "<value>"`.
                    RData::CAA(c) => c
                        .to_string()
                        .split_once(' ')
                        .map(|(_, rest)| rest.to_owned()),
                    _ => None,
                })
                .collect()),
            Err(e) if e.is_no_records_found() || e.is_nx_domain() => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn http01(&self, host: &str) -> Result<u16, String> {
        let url = format!("http://{host}/.well-known/acme-challenge/jc-doctor-probe");
        self.http
            .get(url)
            .send()
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    }

    fn issued_last_week(&self, domain: &str) -> Result<usize, String> {
        #[derive(Deserialize)]
        struct Entry {
            serial_number: String,
            not_before: String,
        }
        // `%.<domain>`: every subdomain; the bare name matches certificates naming exactly it, which
        // undercounts (2026-11-15: nothing after 2026-10-10 for joinedcontext.com).
        let url = format!("https://crt.sh/?q=%25.{domain}&output=json&exclude=expired");
        let body = self
            .http
            .get(url)
            // crt.sh takes 30-40 s for a busy domain and answers 502 or 429 when loaded (2026-11-15).
            .timeout(Duration::from_secs(90))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|e| e.to_string())?;
        // ponytail: crt.sh answers the whole unexpired list; 32 MiB caps it.
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::Read::take(body, 32 << 20), &mut text)
            .map_err(|e| e.to_string())?;
        let entries: Vec<Entry> =
            serde_json::from_str(&text).map_err(|e| format!("crt.sh answer: {e}"))?;
        let since = week_ago();
        Ok(entries
            .iter()
            .filter(|e| e.not_before.as_str() >= since.as_str())
            .map(|e| &e.serial_number)
            .collect::<BTreeSet<_>>()
            .len())
    }

    fn certificates(&self) -> Option<Result<Vec<CertificateState>, String>> {
        let kubeconfig = self.kubeconfig.as_ref()?;
        Some(kubectl_certificates(kubeconfig))
    }
}

/// `YYYY-MM-DDTHH:MM:SS` seven days ago (UTC), comparable as text with crt.sh's `not_before`.
fn week_ago() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    civil(now.saturating_sub(7 * 86_400))
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SS` (H. Hinnant's days-from-civil, inverted).
fn civil(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or_default();
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn kubectl_certificates(kubeconfig: &std::path::Path) -> Result<Vec<CertificateState>, String> {
    let out = jc_secret::spawn::command("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args([
            "get",
            "certificates.cert-manager.io",
            "--all-namespaces",
            "-o",
            "json",
        ])
        .output()
        .map_err(|e| format!("kubectl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "kubectl: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_certificates(&out.stdout)
}

fn parse_certificates(json: &[u8]) -> Result<Vec<CertificateState>, String> {
    let list: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let items = list
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or("kubectl: no items")?;
    Ok(items
        .iter()
        .map(|c| {
            let s = |p: &str| {
                c.pointer(p)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            };
            let ready = c
                .pointer("/status/conditions")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|cs| {
                    cs.iter()
                        .any(|x| x["type"] == "Ready" && x["status"] == "True")
                });
            CertificateState {
                name: format!(
                    "{}/{}",
                    s("/metadata/namespace").unwrap_or_default(),
                    s("/metadata/name").unwrap_or_default()
                ),
                dns_names: c
                    .pointer("/spec/dnsNames")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|n| n.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                ready,
                not_after: s("/status/notAfter"),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const EDGE: &str = "203.0.113.7";

    fn report_for(text: &str, edge: &[IpAddr], zone: Option<&str>, world: &dyn World) -> Report {
        report(&toml::from_str(text).unwrap(), edge, zone, world)
    }

    /// A resolver, CAA, HTTP-01 and CT answer from tables.
    #[derive(Default)]
    struct Stub {
        dns: HashMap<String, Vec<IpAddr>>,
        caa: HashMap<String, Vec<String>>,
        ct: HashMap<String, usize>,
        certs: Option<Vec<CertificateState>>,
    }

    impl Stub {
        fn pointed(hosts: &[&str]) -> Self {
            let mut s = Self::default();
            for h in hosts {
                s.dns.insert((*h).to_owned(), vec![EDGE.parse().unwrap()]);
            }
            s
        }
    }

    impl World for Stub {
        fn addresses(&self, host: &str) -> Result<Vec<IpAddr>, String> {
            Ok(self.dns.get(host).cloned().unwrap_or_default())
        }
        fn caa(&self, name: &str) -> Result<Vec<String>, String> {
            Ok(self.caa.get(name).cloned().unwrap_or_default())
        }
        fn http01(&self, host: &str) -> Result<u16, String> {
            if self.dns.contains_key(host) {
                Ok(404)
            } else {
                Err("connection refused".into())
            }
        }
        fn issued_last_week(&self, domain: &str) -> Result<usize, String> {
            Ok(self.ct.get(domain).copied().unwrap_or_default())
        }
        fn certificates(&self) -> Option<Result<Vec<CertificateState>, String>> {
            self.certs.clone().map(Ok)
        }
    }

    const FIVE: &str = r#"
base_domain = "city.example"
apps_domain = "city-apps.example"
login_host = "login.city.example"
secret_like = "never printed"

[[organizations]]
name = "bbsk"
catalogue_id = "00000000-0000-7000-8000-000000000010"
portal_token_file = "/secrets/ckan-token"
hosts = { portal = "smartcity.bbsk.sk", opendata = "opendata.bbsk.sk", api = "api.bbsk.sk", apps = "bbsk-apps.example" }

[[organizations]]
name = "zilina"
catalogue_id = "00000000-0000-7000-8000-000000000011"
host = "auto"

[[organizations]]
name = "nitra"
catalogue_id = "00000000-0000-7000-8000-000000000012"
hosts = { portal = "smartcity.nitra.sk" }
"#;

    const ALL: &[&str] = &[
        "city.example",
        "login.city.example",
        "zilina.city.example",
        "smartcity.bbsk.sk",
        "opendata.bbsk.sk",
        "api.bbsk.sk",
        "jc-doctor.bbsk-apps.example",
        "smartcity.nitra.sk",
    ];

    fn text(r: &Report) -> String {
        r.lines
            .iter()
            .map(|(l, t)| format!("{l:?} {t}\n"))
            .collect()
    }

    fn errors(r: &Report) -> Vec<&str> {
        r.lines
            .iter()
            .filter(|(l, _)| *l == Level::Error)
            .map(|(_, t)| t.as_str())
            .collect()
    }

    #[test]
    fn every_record_on_the_edge_passes_and_prints_the_records_and_origins() {
        let r = report_for(FIVE, &[], Some("acme.example.net"), &Stub::pointed(ALL));
        let out = text(&r);
        assert!(!r.failed(), "{out}");
        for line in [
            "record: smartcity.bbsk.sk CNAME city.example.",
            "record: opendata.bbsk.sk CNAME city.example.",
            "record: api.bbsk.sk CNAME city.example.",
            "record: *.bbsk-apps.example CNAME city.example.",
            "record: _acme-challenge.bbsk-apps.example CNAME bbsk-apps-example.acme.example.net.",
            "setup: 4 customer origins, 5 records",
            "setup: auto, 0 records",
            "zilina.city.example",
            "opendata.city.example/zilina/",
            "Ok smartcity.bbsk.sk -> 203.0.113.7",
            "HTTP-01 path answers 404",
            "Info bbsk.sk: at least 0 of 50",
        ] {
            assert!(out.contains(line), "missing {line:?} in\n{out}");
        }
        // Only host names are read: nothing else of jc.toml is printed.
        assert!(
            !out.contains("never printed") && !out.contains("/secrets/"),
            "{out}"
        );
    }

    #[test]
    fn a_merged_configuration_says_what_it_loses() {
        let r = report_for(FIVE, &[], None, &Stub::pointed(ALL));
        let out = text(&r);
        let nitra = &out[out.find("Organization nitra").unwrap()..];
        assert!(
            nitra.contains("setup: 1 customer origins, 1 records"),
            "{nitra}"
        );
        assert!(
            nitra.contains(
                "loses: open data merged into the working host (smartcity.nitra.sk/data/)"
            ),
            "{nitra}"
        );
        assert!(
            nitra.contains("loses: API merged into the working host"),
            "{nitra}"
        );
        assert!(
            nitra.contains("loses: Apps on the installation's city-apps.example"),
            "{nitra}"
        );
        let bbsk =
            &out[out.find("Organization bbsk").unwrap()..out.find("Organization zilina").unwrap()];
        assert!(!bbsk.contains("loses:"), "{bbsk}");
        assert!(out.contains("loses: auto: no origin merged"), "{out}");
    }

    #[test]
    fn a_missing_record_fails_with_the_record_to_add() {
        let pointed: Vec<&str> = ALL
            .iter()
            .copied()
            .filter(|h| *h != "opendata.bbsk.sk")
            .collect();
        let r = report_for(FIVE, &[], None, &Stub::pointed(&pointed));
        assert!(r.failed());
        assert!(
            errors(&r)
                .contains(&"opendata.bbsk.sk: no record; add opendata.bbsk.sk CNAME city.example."),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn a_record_pointing_elsewhere_fails() {
        let mut stub = Stub::pointed(ALL);
        stub.dns
            .insert("api.bbsk.sk".into(), vec!["198.51.100.1".parse().unwrap()]);
        let r = report_for(FIVE, &[], None, &stub);
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.starts_with("api.bbsk.sk -> 198.51.100.1, not the edge (203.0.113.7)")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn the_edge_flag_wins_over_the_base_domains_address() {
        let r = report_for(
            FIVE,
            &["198.51.100.9".parse().unwrap()],
            None,
            &Stub::pointed(ALL),
        );
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.contains("not the edge (198.51.100.9)")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn caa_without_letsencrypt_fails_and_with_it_passes() {
        let mut stub = Stub::pointed(ALL);
        stub.caa
            .insert("bbsk.sk".into(), vec!["issue \"digicert.com\"".into()]);
        let r = report_for(FIVE, &[], None, &stub);
        assert!(errors(&r).iter().any(|e| e.contains("smartcity.bbsk.sk: CAA on bbsk.sk permits no letsencrypt.org")), "{}", text(&r));
        stub.caa
            .insert("bbsk.sk".into(), vec!["issue \"letsencrypt.org\"".into()]);
        // The closest name decides: the host's own CAA overrides its domain's.
        stub.caa
            .insert("api.bbsk.sk".into(), vec!["issue \"digicert.com\"".into()]);
        let r = report_for(FIVE, &[], None, &stub);
        let errs = errors(&r);
        assert!(
            errs.iter()
                .any(|e| e.contains("api.bbsk.sk: CAA on api.bbsk.sk")),
            "{}",
            text(&r)
        );
        assert!(
            !errs.iter().any(|e| e.contains("smartcity.bbsk.sk: CAA")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn the_ct_count_warns_near_and_fails_at_the_weekly_limit() {
        let mut stub = Stub::pointed(ALL);
        stub.ct.insert("city.example".into(), 42);
        let r = report_for(FIVE, &[], None, &stub);
        assert!(
            r.lines
                .iter()
                .any(|(l, t)| *l == Level::Warn && t.starts_with("city.example: 42 of 50")),
            "{}",
            text(&r)
        );
        stub.ct.insert("city.example".into(), 50);
        let r = report_for(FIVE, &[], None, &stub);
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.starts_with("city.example: 50 certificates in 7 days")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn apps_under_the_organizations_own_site_are_refused_unless_allowed() {
        let same = FIVE.replace("apps = \"bbsk-apps.example\"", "apps = \"apps.bbsk.sk\"");
        let pointed = [ALL, &["jc-doctor.apps.bbsk.sk"]].concat();
        let r = report_for(&same, &[], None, &Stub::pointed(&pointed));
        assert!(
            errors(&r).iter().any(|e| e
                .contains("*.apps.bbsk.sk is the same site as the working host smartcity.bbsk.sk")),
            "{}",
            text(&r)
        );
        let allowed = same.replace("name = \"bbsk\"", "name = \"bbsk\"\nsame_site_apps = true");
        let r = report_for(&allowed, &[], None, &Stub::pointed(&pointed));
        assert!(!r.failed(), "{}", text(&r));
        assert!(
            r.lines
                .iter()
                .any(|(l, t)| *l == Level::Warn && t.contains("same site as the working host"))
        );
    }

    #[test]
    fn an_apps_domain_on_the_installations_site_warns() {
        let same = FIVE.replace(
            "apps_domain = \"city-apps.example\"",
            "apps_domain = \"apps.city.example\"",
        );
        let r = report_for(&same, &[], None, &Stub::pointed(ALL));
        assert!(
            r.lines.iter().any(|(l, t)| *l == Level::Warn
                && t.starts_with("apps_domain apps.city.example is the same site")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn reserved_labels_are_refused_for_every_derived_host() {
        let auto = FIVE.replace("name = \"zilina\"", "name = \"api\"");
        let r = report_for(&auto, &[], None, &Stub::pointed(ALL));
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.contains("api.city.example: 'api' is reserved")),
            "{}",
            text(&r)
        );
        let own = FIVE.replace(
            "portal = \"smartcity.nitra.sk\"",
            "portal = \"idm.city.example\"",
        );
        let r = report_for(
            &own,
            &[],
            None,
            &Stub::pointed(&[ALL, &["idm.city.example"]].concat()),
        );
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.contains("idm.city.example: the label 'idm' is reserved")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn a_host_named_twice_is_refused() {
        let twice = FIVE.replace(
            "portal = \"smartcity.nitra.sk\"",
            "portal = \"api.bbsk.sk\"",
        );
        let r = report_for(&twice, &[], None, &Stub::pointed(ALL));
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.contains("api.bbsk.sk is named twice")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn sign_in_still_on_the_working_host_is_named() {
        let old = format!(
            "{FIVE}\n[mounts.kaisatsu]\nhost = \"smartcity.nitra.sk\"\npath_prefix = \"/id\"\n"
        );
        let r = report_for(&old, &[], None, &Stub::pointed(ALL));
        let out = text(&r);
        assert!(out.contains("Warn   sign-in is still on the working host smartcity.nitra.sk: passwords and second factors are typed there"), "{out}");
        assert!(
            !out.contains("sign-in is still on the working host smartcity.bbsk.sk"),
            "{out}"
        );
    }

    #[test]
    fn certificates_from_kubectl_report_ready_and_missing() {
        let json = br#"{"items":[
            {"metadata":{"namespace":"jc-prod","name":"jc-org-bbsk-api-bbsk-sk"},"spec":{"dnsNames":["api.bbsk.sk"]},
             "status":{"notAfter":"2027-01-10T00:00:00Z","conditions":[{"type":"Ready","status":"True"}]}},
            {"metadata":{"namespace":"jc-prod","name":"jc-org-bbsk-apps"},"spec":{"dnsNames":["*.bbsk-apps.example"]},
             "status":{"conditions":[{"type":"Ready","status":"False"}]}}]}"#;
        let stub = Stub {
            certs: Some(parse_certificates(json).unwrap()),
            ..Stub::pointed(ALL)
        };
        let r = report_for(FIVE, &[], None, &stub);
        let out = text(&r);
        assert!(out.contains("Ok api.bbsk.sk: Certificate jc-prod/jc-org-bbsk-api-bbsk-sk Ready (until 2027-01-10T00:00:00Z)"), "{out}");
        assert!(
            out.contains("Error bbsk-apps.example: Certificate jc-prod/jc-org-bbsk-apps not Ready"),
            "{out}"
        );
        assert!(
            out.contains("Warn opendata.bbsk.sk: no Certificate names it yet"),
            "{out}"
        );
    }

    #[test]
    fn host_and_hosts_together_are_refused() {
        let both = FIVE.replace(
            "host = \"auto\"",
            "host = \"auto\"\nhosts = { portal = \"z.example\" }",
        );
        let r = report_for(&both, &[], None, &Stub::pointed(ALL));
        assert!(
            errors(&r)
                .iter()
                .any(|e| e.contains("zilina: give either host = \"auto\" or hosts")),
            "{}",
            text(&r)
        );
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), "1970-01-01T00:00:00");
        assert_eq!(civil(1_763_164_800), "2025-11-15T00:00:00");
        assert_eq!(civil(951_782_400 + 3_661), "2000-02-29T01:01:01");
    }

    #[test]
    fn registrable_is_the_last_two_labels() {
        assert_eq!(registrable("smartcity.bbsk.sk"), "bbsk.sk");
        assert_eq!(registrable("bbsk.sk"), "bbsk.sk");
        assert_eq!(registrable("portal.city.example:8443"), "city.example");
    }
}
