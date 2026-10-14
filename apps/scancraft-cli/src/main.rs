//! `scancraft-cli` — headless front door to the engine (M0 skeleton).
//! Full JSON tool table + `--script` + MCP arrive with automation (M1/M8).
//! No unwraps on user input: every failure is a message + exit code 2.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use scancraft_model::ScanProfile;
use scancraft_scan_core::{CancelToken, ScannerBackend};
use scancraft_scan_mock::MockScanner;

fn usage() -> &'static str {
    "scancraft-cli [--help] [--version] <command>\n\nCommands:\n  devices    list scanners (mock backend in M0)\n  profiles   list built-in profiles\n"
}

fn list_devices() -> Result<(), String> {
    let backend = MockScanner::default();
    let devices = backend.list_devices(&CancelToken::new()).map_err(|e| e.message())?;
    if devices.is_empty() {
        println!("no scanners found");
        return Ok(());
    }
    for d in devices {
        println!("{} [{}] {}", d.id.0, d.backend, d.display_name);
    }
    Ok(())
}

fn list_profiles() -> Result<(), String> {
    let profile = ScanProfile::default();
    profile.validate().map_err(|e| e.to_string())?;
    println!("{} driver={} dpi={} source={:?}", profile.name, profile.driver, profile.dpi, profile.paper_source);
    Ok(())
}

fn run(argv: &[String]) -> Result<(), String> {
    if argv.len() < 2 {
        return Err(usage().to_owned());
    }
    match argv[1].as_str() {
        "--help" | "-h" | "help" => {
            print!("{}", usage());
            Ok(())
        }
        "--version" | "-V" => {
            println!("scancraft-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "devices" => list_devices(),
        "profiles" => list_profiles(),
        other => Err(format!("unknown command: {other}\n{}", usage())),
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if let Err(msg) = run(&argv) {
        eprintln!("{msg}");
        std::process::exit(2);
    }
}
