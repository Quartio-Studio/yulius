//! Stand-alone ptyhost entry point accepting exactly `ptyhost --run-dir <dir>`, the same argv the
//! server passes to `yulius ptyhost`. Tests spawn it like the real binary.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [command, flag, dir] if command == "ptyhost" && flag == "--run-dir" => {
            // Failures are recorded in exit.json; stdio is closed when spawned by the server.
            match yulius_ptyhost::run_host(Path::new(dir)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::from(1),
            }
        }
        _ => {
            eprintln!("usage: yulius-ptyhost ptyhost --run-dir <dir>");
            ExitCode::from(2)
        }
    }
}
