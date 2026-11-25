//! LensLink Windows host. For now: reads the camera probe report from a connected phone.

use std::process::{Command, ExitCode};

const APP_ID: &str = "io.github.lenslink";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("probe") => probe(args.get(1).map(String::as_str) == Some("--json")),
        Some("version") => {
            println!(
                "lenslink-host {} (protocol v{})",
                env!("CARGO_PKG_VERSION"),
                lenslink_protocol::VERSION
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: lenslink-host <command>");
            eprintln!();
            eprintln!("  probe [--json]   print the camera report saved by the LensLink app on the phone");
            eprintln!("  version          print versions");
            ExitCode::from(2)
        }
    }
}

/// `adb` reads the report straight from the app's external files directory.
fn probe(json: bool) -> ExitCode {
    let file = if json { "camera-report.json" } else { "camera-report.txt" };
    let path = format!("/sdcard/Android/data/{APP_ID}/files/{file}");

    let output = match Command::new("adb").args(["exec-out", "cat", &path]).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("cannot run adb ({e}). Install Android platform-tools and add them to PATH.");
            return ExitCode::FAILURE;
        }
    };
    if !output.status.success() || output.stdout.is_empty() {
        eprintln!("no report on the phone at {path}");
        eprintln!("Connect the phone with USB debugging enabled and open the LensLink app once.");
        let err = String::from_utf8_lossy(&output.stderr);
        if !err.trim().is_empty() {
            eprintln!("adb: {}", err.trim());
        }
        return ExitCode::FAILURE;
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    ExitCode::SUCCESS
}
