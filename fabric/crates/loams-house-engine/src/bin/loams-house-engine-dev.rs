//! `loams-house-engine-dev [--listen 127.0.0.1:28126]`: the Rust engine behind
//! a loopback HTTP server, for the oracle (plan HE1 Task 23, D1804). Never
//! part of a release image; refuses any address that is not loopback.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let mut listen: SocketAddr = ([127, 0, 0, 1], 28126).into();
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
            _ => {
                eprintln!("usage: loams-house-engine-dev [--listen 127.0.0.1:PORT]");
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
    eprintln!("loams-house-engine-dev: listening on http://{listen}/");
    match loams_house_engine::server::serve(Arc::new(loams_house_engine::Engine::new()), listener) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams-house-engine-dev: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
