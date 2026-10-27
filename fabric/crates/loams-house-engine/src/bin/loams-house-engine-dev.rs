//! `loams-house-engine-dev [--listen 127.0.0.1:28126] [--lake <dir>]`: the
//! Rust engine behind a loopback HTTP server, for the oracle (plan HE1 Task 23,
//! D1804). `--lake` reads the lake tables under a directory (Task 26). Never
//! part of a release image; refuses any address that is not loopback.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let mut listen: SocketAddr = ([127, 0, 0, 1], 28126).into();
    let mut lakes: Vec<std::path::PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("--listen", Some(v)) => match v.parse() {
                Ok(addr) => listen = addr,
                Err(e) => {
                    eprintln!("loams-house-engine-dev: --listen {v}: {e}");
                    return std::process::ExitCode::from(2);
                }
            },
            ("--lake", Some(v)) => lakes.push(v.into()),
            _ => {
                eprintln!("usage: loams-house-engine-dev [--listen 127.0.0.1:PORT] [--lake DIR]");
                return std::process::ExitCode::from(2);
            }
        }
    }
    if !listen.ip().is_loopback() {
        eprintln!(
            "loams-house-engine-dev: {listen} is not a loopback address; the dev server is never exposed"
        );
        return std::process::ExitCode::from(2);
    }
    let listener = match TcpListener::bind(listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("loams-house-engine-dev: binding {listen}: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    let engine = Arc::new(loams_house_engine::Engine::new());
    if !lakes.is_empty() {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("loams-house-engine-dev: {e}");
                return std::process::ExitCode::from(1);
            }
        };
        for dir in &lakes {
            match runtime.block_on(loams_house_engine::lake::attach(&engine, dir)) {
                Ok(names) => eprintln!(
                    "loams-house-engine-dev: lake tables from {}: {}",
                    dir.display(),
                    names.join(", ")
                ),
                Err(e) => {
                    eprintln!("loams-house-engine-dev: --lake {}: {e}", dir.display());
                    return std::process::ExitCode::from(1);
                }
            }
        }
    }
    eprintln!("loams-house-engine-dev: listening on http://{listen}/");
    match loams_house_engine::server::serve(engine, listener) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams-house-engine-dev: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
