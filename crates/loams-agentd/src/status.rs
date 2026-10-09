//! `loams-agentd status`: the data directory, whether a daemon holds it, and
//! whether its IPC port answers. Adapted from the fork's `auth_cli::status`,
//! without the WorkOS account lines (D781).

use loams_agentd_sessions::{EngineConfig, InstanceLock};

pub fn status(config: &EngineConfig) -> anyhow::Result<()> {
    println!("Data dir: {}", config.data_dir.display());
    match InstanceLock::holder(&config.data_dir) {
        Some(pid) => println!("Daemon:   running (pid {pid})"),
        None => println!("Daemon:   not running"),
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.ipc_port));
    let ipc = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500));
    println!(
        "IPC:      {} 127.0.0.1:{}",
        if ipc.is_ok() {
            "listening on"
        } else {
            "not listening on"
        },
        config.ipc_port
    );
    Ok(())
}
