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
