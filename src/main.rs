//! DSHL binary entry point.

// Release builds are GUI apps on Windows (no console window); debug builds
// keep the console so `cargo r` can show the `--debug` runtime log.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use dshl::config;

const USAGE: &str = "\
DSHL — DeepSeek Harness web launcher

USAGE:
    dshl [OPTIONS]

OPTIONS:
    -c, --config <path>    Path to dshl.toml
    -d, --debug            Print runtime logs to stderr (also DSHL_LOG=1)
    -V, --version          Print version
    -h, --help             Print help
";

struct Cli {
    config: Option<PathBuf>,
    debug: bool,
}

fn parse_args() -> Cli {
    let mut args = std::env::args().skip(1);
    let mut cli = Cli {
        config: None,
        debug: false,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("dshl {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-c" | "--config" => match args.next() {
                Some(path) => cli.config = Some(PathBuf::from(path)),
                None => {
                    eprintln!("error: --config requires a value");
                    std::process::exit(2);
                }
            },
            "-d" | "--debug" | "-v" | "--verbose" => cli.debug = true,
            other => {
                eprintln!("error: unexpected argument '{other}'\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    cli
}

fn main() {
    // Must run before any window (WebView) is created, so high-DPI displays
    // don't bitmap-scale/blur the embedded WebView.
    dshl::platform::make_dpi_aware();

    let cli = parse_args();

    // `DSHL_LOG` (any non-empty value) is an alternative way to enable it.
    let env_debug = std::env::var("DSHL_LOG")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    dshl::debug::set_enabled(cli.debug || env_debug);

    if dshl::debug::enabled() {
        dshl::debug::emit("debug runtime logging enabled");
    }

    // Optional launcher-level single instance ([ui] single-instance): when
    // another dshl is already running, don't start a second window/instance —
    // activate the existing one (restore from tray or focus its window) and
    // exit. Distinct from [dsh] single-instance, which guards dsh itself.
    if config::load(cli.config.as_deref()).config.ui.single_instance {
        if let Some(lock) = dshl::platform::single_instance::acquire() {
            // First instance: the lock file handle must stay open for the
            // whole process lifetime or the kernel releases the lock. We
            // intentionally leak it (the OS reclaims it on exit).
            std::mem::forget(lock);
        } else {
            dshl::platform::single_instance::notify_activate();
            // Give the running instance a moment to bring its window to the
            // foreground before this one exits.
            std::thread::sleep(std::time::Duration::from_millis(500));
            std::process::exit(0);
        }
    }

    // SIGINT/SIGTERM (and console Ctrl+C on Windows) → clean shutdown, which
    // kills the supervised dsh child.
    let _ = ctrlc::set_handler(dshl::ui::request_shutdown);

    dshl::ui::setup(cli.config);
    dshl::ui::launch_flow();
    dshl::ui::run_loop();
}
