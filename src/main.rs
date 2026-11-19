mod hid;
mod iokit;
mod remap;
mod vhidd;

use std::env;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use vhidd::{Protocol, VirtualKeyboardClient};

static RUNNING: AtomicBool = AtomicBool::new(true);

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

unsafe extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    fn geteuid() -> u32;
}

extern "C" fn handle_signal(_sig: i32) {
    RUNNING.store(false, Ordering::Relaxed);
}

struct Config {
    timeout_ms: u64,
    protocol: Option<Protocol>,
    verbose: bool,
}

fn print_usage() {
    eprintln!(
        "controlesque {} — Caps Lock = Escape on tap, Left Control on hold\n\
         \n\
         USAGE:\n\
         \tsudo controlesque [OPTIONS]\n\
         \n\
         OPTIONS:\n\
         \t--timeout-ms <MS>    Max tap duration in ms to emit Escape (default: 500)\n\
         \t--protocol <5|7>     Force Karabiner VirtualHIDDevice protocol version (default: auto)\n\
         \t-v, --verbose        Print virtual driver & device diagnostics\n\
         \t-h, --help           Show this help\n\
         \t-V, --version        Show version",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_args() -> Result<Option<Config>, String> {
    let mut cfg = Config {
        timeout_ms: 500,
        protocol: None,
        verbose: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("controlesque {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-v" | "--verbose" => cfg.verbose = true,
            "--timeout-ms" => {
                let val = args
                    .next()
                    .ok_or_else(|| "--timeout-ms requires a number".to_string())?;
                cfg.timeout_ms = val
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout-ms value: {val}"))?;
            }
            "--protocol" => {
                let val = args
                    .next()
                    .ok_or_else(|| "--protocol requires 5 or 7".to_string())?;
                cfg.protocol = match val.as_str() {
                    "5" => Some(Protocol::V5),
                    "7" => Some(Protocol::V7),
                    _ => return Err(format!("unsupported protocol '{val}' (expected 5 or 7)")),
                };
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(Some(cfg))
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(Some(c)) => c,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            print_usage();
            return ExitCode::FAILURE;
        }
    };

    if unsafe { geteuid() } != 0 {
        eprintln!(
            "error: controlesque must be run as root (sudo) to access \
             Karabiner-VirtualHIDDevice-Daemon and seize IOKit HID keyboards."
        );
        return ExitCode::FAILURE;
    }

    unsafe {
        signal(SIGINT, handle_signal);
        signal(SIGTERM, handle_signal);
    }

    // Retry briefly on startup so boot-time LaunchDaemon ordering never races with vhidd.
    let connect_deadline = Instant::now() + Duration::from_secs(10);
    let client = loop {
        match VirtualKeyboardClient::connect(cfg.protocol, cfg.verbose) {
            Ok(c) => break c,
            Err(e)
                if Instant::now() < connect_deadline && RUNNING.load(Ordering::Relaxed) =>
            {
                if cfg.verbose {
                    eprintln!("[controlesque] waiting for vhidd ({e})...");
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                eprintln!("error: failed to connect to Karabiner-VirtualHIDDevice-Daemon: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    // Wait briefly for the virtual keyboard to come up before seizing physical keyboards.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !client.is_keyboard_ready() && Instant::now() < deadline && RUNNING.load(Ordering::Relaxed)
    {
        thread::sleep(Duration::from_millis(50));
    }

    if !client.is_keyboard_ready() && cfg.verbose {
        eprintln!(
            "[controlesque] virtual keyboard ready signal not yet received; continuing anyway"
        );
    }

    if let Err(e) = iokit::run_seize_loop(
        client,
        Duration::from_millis(cfg.timeout_ms),
        &RUNNING,
        cfg.verbose,
    ) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
