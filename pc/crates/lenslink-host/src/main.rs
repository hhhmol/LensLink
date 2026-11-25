//! LensLink Windows host.

mod adb;
mod stream;
mod video;

use std::process::ExitCode;

const APP_ID: &str = "io.github.lenslink";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("stream") => stream::parse(rest).and_then(|o| stream::run(&o)),
        Some("probe") => probe(rest),
        Some("version") => {
            println!(
                "lenslink-host {} (protocol v{})",
                env!("CARGO_PKG_VERSION"),
                lenslink_protocol::VERSION
            );
            Ok(())
        }
        _ => {
            usage();
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nerror: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("usage: lenslink-host <command> [options]");
    eprintln!();
    eprintln!("  stream [options]           stream the phone camera to a window and/or a file");
    eprintln!("  probe [-s <id>] [--json]   print the camera report saved by the LensLink app");
    eprintln!("  version                    print versions");
    eprintln!();
    eprintln!("{}", stream::USAGE);
}

/// Reads the report straight from the app's external files directory.
fn probe(args: &[String]) -> Result<(), String> {
    let mut serial = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "-s" | "--serial" => serial = it.next().map(String::as_str),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let file = if json { "camera-report.json" } else { "camera-report.txt" };
    let path = format!("/sdcard/Android/data/{APP_ID}/files/{file}");

    let output = adb::run(serial, &["exec-out", "cat", &path])?;
    if !output.status.success() || output.stdout.is_empty() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "no report on the phone at {path}. Connect the phone with USB debugging and open \
             the LensLink app once. {}",
            err.trim()
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}
