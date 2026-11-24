#![forbid(unsafe_code)]

use clap::Parser;
use dataroom_server::{run_cli, Cli};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run_cli(cli).await {
        Ok(output) => {
            println!("{output:#}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
