//! DSHL binary entry point.
//!
//! All real logic lives in `dshl_core` + `dshl_cli` (the same kernel the
//! plugin-track cdylib links against). This shell only:
//!   1. forwards the process-local windows-subsystem attribute,
//!   2. invokes `dshl_cli::run_cli()` which never calls `process::exit`,
//!   3. maps the returned result into a process exit code and exits.
//
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    match dshl_cli::run_cli() {
        Ok(Ok(())) => std::process::exit(0),
        Ok(Err(outcome)) => std::process::exit(match outcome {
            dshl_cli::RunOutcome::HelpPrinted => {
                // `run_cli` returns HelpPrinted without printing (so the napi
                // wrapper can capture it). The binary shell is responsible for
                // actually writing it to stdout.
                print!("{}", dshl_cli::USAGE);
                0
            }
            dshl_cli::RunOutcome::VersionPrinted => {
                println!("dshl {}", env!("CARGO_PKG_VERSION"));
                0
            }
            dshl_cli::RunOutcome::ArgsError(msg) => {
                eprintln!("error: {msg}");
                2
            }
            dshl_cli::RunOutcome::AlreadyRunning => {
                // Single-instance: we already notified the other instance and
                // waited for it to foreground itself. Exit cleanly.
                0
            }
        }),
        Err(e) => {
            eprintln!("dshl: fatal: {e}");
            std::process::exit(1)
        }
    }
}
