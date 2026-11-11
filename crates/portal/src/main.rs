use joinedcontext_portal::config::Config;
use joinedcontext_portal::{server, telemetry};

/// jemalloc, not glibc's malloc (T-5283): each reconcile reads the runner's whole /metrics text
/// per project with a Live stream (13 MB on dev, 30 MB on prod) and frees it; glibc kept such
/// freed blocks resident (474 Mi on dev against a 320 Mi request). `_RJEM_MALLOC_CONF` in the
/// Dockerfile starts jemalloc's background thread, which returns them, as in the gateway (T-4782).
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() {
    // OPS-15: JSON lines on stdout, nothing on disk.
    if let Err(err) = tracing::subscriber::set_global_default(telemetry::log_subscriber(
        std::io::stdout,
        telemetry::log_filter(),
    )) {
        eprintln!("Logging error: {err}");
        std::process::exit(1);
    }

    // `joinedcontext-portal migrate`: bring `JC_PORTAL_DATABASE_URL` up to this release's schema
    // and exit, without the rest of the configuration. The upgrade drill runs the next release's
    // image this way against a copy of the database (T-2802, OPS-12).
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        std::process::exit(migrate().await);
    }

    // `joinedcontext-portal validate --as-portal --repo-dir <org> [--project-dir <slug>=<dir>]...
    // [--apex <zone>]`: judge a checkout offline as this Portal's load would (P-15), with the
    // operator's bounds from `JC_PORTAL_ORGANIZATION_BOUNDS_FILE`. Exit 1 on any finding, 2 on
    // a usage or configuration error.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("validate") {
        std::process::exit(validate(&args[1..]));
    }

    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Configuration error: {err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = server::serve(config).await {
        eprintln!("Server error: {err}");
        std::process::exit(1);
    }
}

/// The exit code of `migrate`. The URL holds the database password, so no message names it.
async fn migrate() -> i32 {
    let Some(url) = std::env::var("JC_PORTAL_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("migrate: JC_PORTAL_DATABASE_URL is not set: name the database to migrate");
        return 1;
    };
    match joinedcontext_portal::db::connect(&url).await {
        Ok(_) => {
            tracing::info!("the database is at this release's schema");
            0
        }
        Err(err) => {
            eprintln!("migrate: {err}");
            1
        }
    }
}

/// The exit code of `validate --as-portal`.
fn validate(args: &[String]) -> i32 {
    let usage = || {
        eprintln!(
            "usage: joinedcontext-portal validate --as-portal --repo-dir <org-dir> \
             [--project-dir <slug>=<dir>]... [--apex <zone>]"
        );
        2
    };
    let mut rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let (Some(&"--as-portal"), Some(&"--repo-dir"), Some(org_dir)) =
        (rest.first(), rest.get(1), rest.get(2).copied())
    else {
        return usage();
    };
    rest.drain(..3);
    let (mut projects, mut apex) = (std::collections::BTreeMap::new(), None);
    while let [flag, value, tail @ ..] = rest.as_slice() {
        match *flag {
            "--project-dir" => match value.split_once('=') {
                Some((slug, dir)) => {
                    projects.insert(slug.to_owned(), std::path::PathBuf::from(dir));
                }
                None => return usage(),
            },
            "--apex" => apex = Some((*value).to_owned()),
            _ => return usage(),
        }
        rest = tail.to_vec();
    }
    if !rest.is_empty() {
        return usage();
    }
    let bounds = match joinedcontext_portal::config::organization_bounds(&|key| {
        std::env::var(key).ok()
    }) {
        Ok(bounds) => bounds,
        Err(err) => {
            eprintln!("Configuration error: {err}");
            return 2;
        }
    };
    let report = joinedcontext_portal::judge::validate(
        std::path::Path::new(org_dir),
        projects,
        (&bounds, apex.as_deref(), None),
    );
    for finding in &report.findings {
        eprintln!("{finding}");
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if report.is_valid() {
        println!("{} manifests valid", report.checked);
        0
    } else {
        eprintln!(
            "{} manifests valid, {} invalid",
            report.checked,
            report.findings.len()
        );
        1
    }
}
