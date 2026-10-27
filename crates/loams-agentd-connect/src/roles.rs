//! The remote listener's role, `studio` (design §53 §6.3, §8.2; plan SD1
//! Task 7, Shared contracts "Daemon role").
//!
//! `studio` may call the Studio method set: every `loams.agentd.v1` method
//! ([`crate::twins::CONNECT_METHODS`]) except
//! - the harness installers and updaters, which run code on the host and
//!   are confirmed from the desktop only (Q808);
//! - the agent account logins and account switching (Q808);
//! - `Shutdown` and every secret method, which are not on the Connect surface
//!   at all (`twins::the_lifecycle_is_never_on_the_connect_surface`);
//! - the factory's configure and remove, which carry its apps' credentials:
//!   Electron main's alone on the loopback listener (SF1 amendment F4).
//!
//! The list below is a deny list over the twins table, so a §53 §6.5 gap
//! that a later SD1 task serves is a Studio method without a change here.

use crate::twins::{CONNECT_METHODS, PACKAGE};

/// The role name (Shared contracts).
pub const STUDIO_ROLE: &str = "studio";

/// The `loams.agentd.v1` methods `studio` may not call, with why.
pub const STUDIO_EXCLUDED: &[(&str, &str, &str)] = &[
    ("HarnessService", "InstallHarness", "harness install (Q808)"),
    ("HarnessService", "CancelInstall", "harness install (Q808)"),
    (
        "HarnessService",
        "ApplyHarnessUpdate",
        "harness install (Q808)",
    ),
    (
        "HarnessService",
        "CancelHarnessUpdate",
        "harness install (Q808)",
    ),
    (
        "HarnessService",
        "ActivateAgentAccount",
        "account login (Q808)",
    ),
    (
        "HarnessService",
        "ForgetAgentAccount",
        "account login (Q808)",
    ),
    ("HarnessService", "StartAgentLogin", "account login (Q808)"),
    (
        "HarnessService",
        "CompleteAgentLogin",
        "account login (Q808)",
    ),
    ("HarnessService", "PollAgentLogin", "account login (Q808)"),
    ("HarnessService", "CancelAgentLogin", "account login (Q808)"),
    (
        "FactoryService",
        "ConfigureFactoryApp",
        "factory credentials stay local (SF1 F4)",
    ),
    (
        "FactoryService",
        "RemoveFactoryApp",
        "factory credentials stay local (SF1 F4)",
    ),
];

/// What `studio` may do with a request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// A Studio method.
    Allowed,
    /// A `loams.agentd.v1` method outside the Studio set (403
    /// `forbidden_role`).
    Forbidden,
    /// Not a `loams.agentd.v1` method (404).
    Unknown,
    /// A dev-server preview under `/preview/` (Q806; SD1 Task 26, ruling
    /// T7-12): served by [`crate::preview`] behind the same origin and token
    /// checks, never by the Connect router.
    Preview,
}

/// `studio`'s access to the Connect path `path`
/// (`/loams.agentd.v1.<Service>/<Method>`; a GET's query is not part of it).
pub fn studio_access(path: &str) -> Access {
    if path == crate::preview::ROOT.trim_end_matches('/') || path.starts_with(crate::preview::ROOT)
    {
        return Access::Preview;
    }
    let Some((service, method)) = path
        .strip_prefix('/')
        .and_then(|rest| rest.strip_prefix(PACKAGE))
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.split_once('/'))
    else {
        return Access::Unknown;
    };
    if !CONNECT_METHODS
        .iter()
        .any(|row| row.service == service && row.method == method)
    {
        return Access::Unknown;
    }
    if STUDIO_EXCLUDED
        .iter()
        .any(|(s, m, _)| *s == service && *m == method)
    {
        Access::Forbidden
    } else {
        Access::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twins::twin_of;

    #[test]
    fn every_exclusion_names_a_method() {
        for (service, method, _) in STUDIO_EXCLUDED {
            assert!(
                twin_of(service, method).is_some(),
                "{service}/{method} is not a loams.agentd.v1 method"
            );
        }
    }

    #[test]
    fn a_browser_reads_the_factory_but_never_stores_its_credentials() {
        for read in ["ListFactoryApps", "TestFactoryApp", "QueryFactory"] {
            assert_eq!(
                studio_access(&format!("/loams.agentd.v1.FactoryService/{read}")),
                Access::Allowed,
                "{read}"
            );
        }
        for write in ["ConfigureFactoryApp", "RemoveFactoryApp"] {
            assert_eq!(
                studio_access(&format!("/loams.agentd.v1.FactoryService/{write}")),
                Access::Forbidden,
                "{write}"
            );
        }
    }

    #[test]
    fn paths_are_read_strictly() {
        assert_eq!(
            studio_access("/loams.agentd.v1.ThreadService/WatchThreads"),
            Access::Allowed
        );
        assert_eq!(
            studio_access("/loams.agentd.v1.HarnessService/InstallHarness"),
            Access::Forbidden
        );
        for unknown in [
            "/",
            "/rpc",
            "/loams.agentd.v1.ThreadService/Shutdown",
            "/loams.agentd.v1.ThreadService/WatchThreads/x",
            "/loams.agentd.v1ThreadService/WatchThreads",
            "loams.agentd.v1.ThreadService/WatchThreads",
            "/loams.agentd.v2.ThreadService/WatchThreads",
            "/previews/a.b/",
        ] {
            assert_eq!(studio_access(unknown), Access::Unknown, "{unknown}");
        }
        for preview in [
            "/preview",
            "/preview/",
            "/preview/a.b/x",
            "/preview/-/ticket",
        ] {
            assert_eq!(studio_access(preview), Access::Preview, "{preview}");
        }
    }
}
