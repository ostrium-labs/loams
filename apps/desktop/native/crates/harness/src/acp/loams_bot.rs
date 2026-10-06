//! loams: Loams Bot as an ACP agent.
//!
//! The agent itself is `loams-desktop loams bot-acp` (crate `loams-desktop-link`): the same
//! binary that is running, so there is nothing to install and the harness
//! cannot drift from the app version. Kept in its own file so the only upstream
//! lines this feature touches in `acp/mod.rs` are `mod loams_bot;` and the
//! match arms the compiler demands elsewhere (see `LOAMS.md`).

use std::path::PathBuf;

use super::{AcpAgentSpec, AcpHarness, default_effort_values, identity_transform};
use loams_desktop_proto::{HarnessId, Model, SteeringMode};

fn loams_bot_spec() -> AcpAgentSpec {
    AcpAgentSpec {
        id: HarnessId::LoamsBot,
        display_name: "Loams Bot",
        // The launch program is always the running executable (see
        // `AcpHarness::loams_bot`); this name only labels error messages.
        executable: "loams-desktop",
        env_override: "LOAMS_BOT_EXECUTABLE",
        args: &["loams", "bot-acp"],
        npm_package: None,
        archive: None,
        extra_paths: current_exe_paths,
        cli_executable: "loams-desktop",
        cli_extra_paths: current_exe_paths,
        install_hint: "Loams Bot ships inside Loams Desktop (`loams-desktop loams bot-acp`); \
             set LOAMS_BOT_EXECUTABLE to point at a different build",
        models: || {
            vec![Model {
                id: "loams-bot".into(),
                label: "Loams Bot".into(),
                description: Some(
                    "One chat that drives the Plane, Zulip, Forgejo, GlitchTip and analytics agents".into(),
                ),
                reasoning_levels: Vec::new(),
                options: Vec::new(),
            }]
        },
        // Prompts are A2A messages: a steer is delivered at the next turn.
        steering_mode: SteeringMode::TurnBoundary,
        reasoning_levels: &[],
        prompt_transform: identity_transform,
        effort_values: default_effort_values,
        ladder_extras: &[],
        prompt_complete_extension: false,
        prompt_stall: None,
        stall_hint: "The Loams agents did not answer; check LOAMS_URL / LOAMS_BOT_URL.",
        effort_in_model_id: false,
        auth_method: None,
        skill_dirs: Vec::new,
        hidden_commands: &[],
        drops_unstarted_cancelled_prompt: false,
    }
}

/// The running executable, but only when it is the `loams-desktop` app itself. Test
/// fixtures and examples link the same registry; launching *them* with
/// `loams bot-acp` would start another copy of the fixture (a second window
/// that steals focus, which broke `macos-frame-recovery`), so for any other
/// program Loams Bot reports "not installed" instead.
fn loams_desktop_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    is_loams_desktop(&exe).then_some(exe)
}

fn is_loams_desktop(path: &std::path::Path) -> bool {
    path.file_stem()
        .is_some_and(|stem| stem == loams_desktop_brand::BINARY_NAME)
}

fn current_exe_paths() -> Vec<PathBuf> {
    loams_desktop_exe().into_iter().collect()
}

impl AcpHarness {
    /// Loams Bot (`loams-desktop loams bot-acp`): this binary, speaking ACP on stdio.
    pub fn loams_bot() -> Self {
        let harness = Self::with_spec(loams_bot_spec());
        match loams_desktop_exe() {
            Some(exe) => harness.with_executable(exe),
            None => harness,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_loams_desktop;
    use std::path::Path;

    #[cfg(windows)]
    #[test]
    fn windows_exe_suffix_is_accepted() {
        assert!(is_loams_desktop(Path::new(r"C:\Loams\loams-desktop.exe")));
    }

    #[test]
    fn only_the_loams_desktop_binary_is_a_launch_target() {
        assert!(is_loams_desktop(Path::new("/opt/loams/loams-desktop")));
        assert!(!is_loams_desktop(Path::new(
            "/tmp/Fixture.app/Contents/MacOS/fixture"
        )));
        assert!(!is_loams_desktop(Path::new(
            "/target/debug/deps/loams_desktop_engine-1a2b"
        )));
    }
}
