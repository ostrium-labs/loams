//! The factory's apps: their configure forms and how each one is reached
//! (`factory/apps.ts`, ported).

use loams_agentd_proto::factory::FactoryCredentialField;

/// One factory app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AppId {
    Forgejo,
    Zulip,
    Plane,
    Glitchtip,
    Openpanel,
    Matomo,
    Langfuse,
    Signoz,
}

impl AppId {
    /// Every app, in the factory's order.
    pub const ALL: [AppId; 8] = [
        AppId::Forgejo,
        AppId::Zulip,
        AppId::Plane,
        AppId::Glitchtip,
        AppId::Openpanel,
        AppId::Matomo,
        AppId::Langfuse,
        AppId::Signoz,
    ];

    /// The wire id.
    pub fn as_str(self) -> &'static str {
        match self {
            AppId::Forgejo => "forgejo",
            AppId::Zulip => "zulip",
            AppId::Plane => "plane",
            AppId::Glitchtip => "glitchtip",
            AppId::Openpanel => "openpanel",
            AppId::Matomo => "matomo",
            AppId::Langfuse => "langfuse",
            AppId::Signoz => "signoz",
        }
    }

    /// The app with this wire id.
    pub fn parse(id: &str) -> Option<AppId> {
        AppId::ALL.into_iter().find(|a| a.as_str() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            AppId::Forgejo => "Forgejo",
            AppId::Zulip => "Zulip",
            AppId::Plane => "Plane (ItsAPlan)",
            AppId::Glitchtip => "GlitchTip",
            AppId::Openpanel => "OpenPanel",
            AppId::Matomo => "Matomo",
            AppId::Langfuse => "Langfuse",
            AppId::Signoz => "SigNoz",
        }
    }

    /// The honest status label while an app is a preview, shown in the UI:
    /// SigNoz stores its telemetry in the install's Loams House (D1666) and
    /// is a preview until it passes the House gate (D1571).
    pub fn preview(self) -> Option<&'static str> {
        match self {
            AppId::Signoz => Some("Preview: SigNoz on Loams House"),
            _ => None,
        }
    }

    /// SigNoz has its own UI only.
    pub fn has_panels(self) -> bool {
        self != AppId::Signoz
    }

    /// The configure form's fields, in order: `(key, label, secret)`.
    pub fn fields(self) -> &'static [(&'static str, &'static str, bool)] {
        const SSO: (&str, &str, bool) = ("ssoOrigin", "SSO origin (optional)", false);
        match self {
            AppId::Forgejo => &[("token", "Access token", true), SSO],
            AppId::Zulip => &[("email", "Email", false), ("apiKey", "API key", true), SSO],
            AppId::Plane => &[
                ("apiKey", "API key", true),
                ("projectKey", "Project key", false),
                // ItsAPlan's web UI and its API are separate origins: the embedded UI
                // and the browser's new tab use this.
                ("appUrl", "App URL (optional)", false),
                SSO,
            ],
            AppId::Glitchtip => &[("token", "API token", true), SSO],
            AppId::Openpanel => &[
                ("clientId", "Client ID", true),
                ("clientSecret", "Client secret", true),
                ("projectId", "Project ID", false),
                SSO,
            ],
            AppId::Matomo => &[
                ("apiToken", "API token", true),
                ("idSite", "Site ID", false),
                SSO,
            ],
            AppId::Langfuse => &[
                ("publicKey", "Public key", true),
                ("secretKey", "Secret key", true),
                SSO,
            ],
            AppId::Signoz => &[SSO],
        }
    }

    /// The form for the wire.
    pub fn credential_fields(self) -> Vec<FactoryCredentialField> {
        self.fields()
            .iter()
            .map(|(key, label, secret)| FactoryCredentialField {
                key: (*key).to_owned(),
                label: (*label).to_owned(),
                secret: *secret,
            })
            .collect()
    }

    /// Whether `key` is one of this app's secret fields.
    pub fn is_secret(self, key: &str) -> bool {
        self.fields()
            .iter()
            .any(|(k, _, secret)| *k == key && *secret)
    }

    /// Whether `key` is one of this app's fields.
    pub fn has_field(self, key: &str) -> bool {
        self.fields().iter().any(|(k, _, _)| *k == key)
    }
}
