//! `yulius login` and `yulius import` (client side of SPEC-001 IMP-001..006).

use std::path::PathBuf;
use std::process::ExitCode;

use yulius_import::{
    DeviceLoginOptions, ImportOptions, TerminalIo, TokenStore, Url, device_login, run_import,
};

fn install_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn parse_server(server: &str) -> Result<Url, String> {
    Url::parse(server.trim_end_matches('/')).map_err(|e| format!("invalid --server URL: {e}"))
}

/// `yulius login --server <url>`.
pub fn login(server: &str) -> ExitCode {
    install_tls();
    let result = (|| -> Result<(), String> {
        let url = parse_server(server)?;
        let store = TokenStore::default_location().map_err(|e| e.to_string())?;
        let token = device_login(
            &url,
            &mut |start| {
                println!(
                    "Open {} and confirm the code {}",
                    start.verification_url, start.user_code
                );
            },
            &DeviceLoginOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        store.save(&url, &token).map_err(|e| e.to_string())?;
        println!("Signed in to {url}.");
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius login: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `yulius import [path] --server <url> [--since 14d] [--yes]`.
pub fn import(path: PathBuf, server: &str, since: Option<String>, assume_yes: bool) -> ExitCode {
    install_tls();
    let result = (|| -> Result<(), String> {
        let url = parse_server(server)?;
        let store = TokenStore::default_location().map_err(|e| e.to_string())?;
        let mut io = TerminalIo::new(store);
        let outcome = run_import(
            &ImportOptions {
                path,
                server: url,
                since,
                assume_yes,
            },
            &mut io,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "Imported: {} tasks created, {} sessions already present. Open {}",
            outcome.tasks_created, outcome.sessions_skipped_existing, outcome.sandbox_url
        );
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius import: {err}");
            ExitCode::FAILURE
        }
    }
}
