#![forbid(unsafe_code)]
//! The `jc` binary: parse flags, install the process-wide singletons, run until SIGTERM/Ctrl-C.

use clap::Parser;
use std::process::ExitCode;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

async fn stop_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}

fn main() -> ExitCode {
    let cli = jc::Cli::parse();
    let cfg = match jc::Config::resolve(cli) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("jc: {e}");
            return ExitCode::from(2);
        }
    };
    jc::runtime::init_tracing(cfg.log_format);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("jc: tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(jc::run_until(cfg, stop_signal())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "jc stopped");
            eprintln!("jc: {e}");
            ExitCode::FAILURE
        }
    }
}
