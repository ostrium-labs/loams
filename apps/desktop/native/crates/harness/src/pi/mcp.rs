//! Load the existing Loams Desktop delegation tools into this Pi process only.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

pub(super) fn configure(
    cmd: &mut Command,
    config: &loams_desktop_proto::McpServer,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-mcp")?;
    let extension = scratch.path().join("loams-desktop-mcp.mjs");
    std::fs::write(&extension, include_str!("mcp.mjs"))?;
    cmd.arg("--extension").arg(extension).env(
        "LOAMS_DESKTOP_PI_MCP",
        serde_json::to_string(config).expect("serializable MCP config"),
    );
    Ok(scratch)
}
