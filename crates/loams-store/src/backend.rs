//! Building the cloud backends (S3, GCS, Azure) from a URL, the environment
//! and explicit options.
//!
//! `object_store::parse_url_opts` starts every builder from `new()`, so it
//! never reads `AWS_*`, `GOOGLE_*` or `AZURE_*`: an `s3://` bucket opened that
//! way with only `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_ENDPOINT_URL`
//! in the environment falls back to the instance metadata service and every
//! request times out (release blocker B1). Here each builder starts from the
//! environment, as the builders' own `from_env()` would, then takes the URL,
//! then the explicit options, which win over the environment.
//!
//! Nothing here logs a value that could be a secret: only key names, the
//! bucket URL, the endpoint (without any user info) and the region.

use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::azure::{AzureConfigKey, MicrosoftAzureBuilder};
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::{ClientConfigKey, ObjectStore};

/// One backend setting: a lowercased `object_store` config key and its value.
type Setting = (String, String);

/// The process environment, skipping variables that are not UTF-8 (as the
/// builders' `from_env()` do).
pub(crate) fn process_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// The settings a backend reads from `env`, in the order `object_store`'s
/// `from_env()` reads them: every `<prefix>*` variable, lowercased, plus the
/// backend's special cases (`SERVICE_ACCOUNT` for GCS, `IDENTITY_ENDPOINT`
/// for Azure).
fn env_settings<I>(prefix: &str, env: I) -> Vec<Setting>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut out = Vec::new();
    for (key, value) in env {
        if key.starts_with(prefix) {
            out.push((key.to_ascii_lowercase(), value));
        } else if prefix == "GOOGLE_" && key == "SERVICE_ACCOUNT" {
            out.push(("google_service_account".to_string(), value));
        } else if prefix == "AZURE_" && key == "IDENTITY_ENDPOINT" {
            out.push(("azure_msi_endpoint".to_string(), value));
        }
    }
    out
}

/// Explicit options with lowercased keys.
fn explicit<I, K, V>(options: I) -> Vec<Setting>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    options
        .into_iter()
        .map(|(k, v)| (k.as_ref().to_ascii_lowercase(), v.into()))
        .collect()
}

/// `endpoint` without user info, for logs.
fn redact_endpoint(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.to_string()
        }
        Err(_) => "<unparseable endpoint>".to_string(),
    }
}

/// An S3 builder for `url`: `AWS_*` from `env`, then the URL, then `options`.
///
/// An explicit `http://` endpoint (`AWS_ENDPOINT_URL=http://rustfs:9000`, a
/// local MinIO or RustFS) turns on `allow_http` unless `AWS_ALLOW_HTTP` or the
/// `aws_allow_http` option says otherwise: naming a plain-HTTP endpoint is the
/// operator's choice already, and without it every request fails with "URL
/// scheme is not permitted".
pub(crate) fn s3_builder<E, I, K, V>(url: &str, env: E, options: I) -> AmazonS3Builder
where
    E: IntoIterator<Item = (String, String)>,
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    let mut allow_http_set = false;
    let mut apply = |b: AmazonS3Builder, settings: Vec<Setting>, from_env: bool| {
        settings.into_iter().fold(b, |b, (key, value)| {
            match key.parse::<AmazonS3ConfigKey>() {
                Ok(k) => {
                    if k == AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp) {
                        allow_http_set = true;
                    }
                    b.with_config(k, value)
                }
                Err(_) => {
                    if !from_env {
                        tracing::warn!(option = %key, "ignoring an unknown S3 store option");
                    }
                    b
                }
            }
        })
    };
    let b = apply(AmazonS3Builder::new(), env_settings("AWS_", env), true);
    let b = b.with_url(url);
    let mut b = apply(b, explicit(options), false);
    let endpoint = b.get_config_value(&AmazonS3ConfigKey::Endpoint);
    if !allow_http_set
        && endpoint
            .as_deref()
            .is_some_and(|e| e.len() >= 7 && e[..7].eq_ignore_ascii_case("http://"))
    {
        b = b.with_allow_http(true);
    }
    b
}

/// A GCS builder for `url`: `GOOGLE_*` (and `SERVICE_ACCOUNT`) from `env`,
/// then the URL, then `options`.
pub(crate) fn gcs_builder<E, I, K, V>(url: &str, env: E, options: I) -> GoogleCloudStorageBuilder
where
    E: IntoIterator<Item = (String, String)>,
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    let apply = |b: GoogleCloudStorageBuilder, settings: Vec<Setting>, from_env: bool| {
        settings
            .into_iter()
            .fold(b, |b, (key, value)| match key.parse::<GoogleConfigKey>() {
                Ok(k) => b.with_config(k, value),
                Err(_) => {
                    if !from_env {
                        tracing::warn!(option = %key, "ignoring an unknown GCS store option");
                    }
                    b
                }
            })
    };
    let b = apply(
        GoogleCloudStorageBuilder::new(),
        env_settings("GOOGLE_", env),
        true,
    );
    apply(b.with_url(url), explicit(options), false)
}

/// An Azure builder for `url`: `AZURE_*` (and `IDENTITY_ENDPOINT`) from
/// `env`, then the URL, then `options`.
pub(crate) fn azure_builder<E, I, K, V>(url: &str, env: E, options: I) -> MicrosoftAzureBuilder
where
    E: IntoIterator<Item = (String, String)>,
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    let apply = |b: MicrosoftAzureBuilder, settings: Vec<Setting>, from_env: bool| {
        settings
            .into_iter()
            .fold(b, |b, (key, value)| match key.parse::<AzureConfigKey>() {
                Ok(k) => b.with_config(k, value),
                Err(_) => {
                    if !from_env {
                        tracing::warn!(option = %key, "ignoring an unknown Azure store option");
                    }
                    b
                }
            })
    };
    let b = apply(
        MicrosoftAzureBuilder::new(),
        env_settings("AZURE_", env),
        true,
    );
    apply(b.with_url(url), explicit(options), false)
}

/// Logs how an S3 store was configured, without secrets, and warns when
/// nothing gives it credentials: it would then ask the instance metadata
/// service (169.254.169.254), which off EC2 makes every request time out.
fn log_s3(url: &str, b: &AmazonS3Builder) {
    let endpoint = b
        .get_config_value(&AmazonS3ConfigKey::Endpoint)
        .map(|e| redact_endpoint(&e));
    let region = b.get_config_value(&AmazonS3ConfigKey::Region);
    let has = |k: AmazonS3ConfigKey| b.get_config_value(&k).is_some_and(|v| !v.is_empty());
    let credentials = if has(AmazonS3ConfigKey::AccessKeyId) {
        "access key"
    } else if has(AmazonS3ConfigKey::SkipSignature) {
        "unsigned"
    } else if has(AmazonS3ConfigKey::WebIdentityTokenFile) {
        "web identity"
    } else if has(AmazonS3ConfigKey::ContainerCredentialsRelativeUri)
        || has(AmazonS3ConfigKey::ContainerCredentialsFullUri)
    {
        "container credentials"
    } else {
        "instance metadata"
    };
    tracing::info!(
        bucket = %url,
        endpoint = endpoint.as_deref().unwrap_or("<aws default>"),
        region = region.as_deref().unwrap_or("<default>"),
        credentials,
        "opening S3 store"
    );
    if credentials == "instance metadata" && endpoint.is_some() {
        tracing::warn!(
            bucket = %url,
            "S3 store has an endpoint but no AWS_ACCESS_KEY_ID: it will ask the \
             instance metadata service for credentials, which only answers on a \
             cloud VM"
        );
    }
}

/// Whether [`build`] handles `scheme` (S3, GCS, Azure). Other schemes go
/// through `object_store::parse_url_opts`, which reads no environment.
pub(crate) fn is_cloud(scheme: &object_store::ObjectStoreScheme) -> bool {
    use object_store::ObjectStoreScheme as S;
    matches!(
        scheme,
        S::AmazonS3 | S::GoogleCloudStorage | S::MicrosoftAzure
    )
}

/// Builds the store for a cloud `scheme` (see [`is_cloud`]).
pub(crate) fn build<E, I, K, V>(
    scheme: &object_store::ObjectStoreScheme,
    url: &str,
    env: E,
    options: I,
) -> Result<Box<dyn ObjectStore>, object_store::Error>
where
    E: IntoIterator<Item = (String, String)>,
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    use object_store::ObjectStoreScheme as S;
    match scheme {
        S::AmazonS3 => {
            let b = s3_builder(url, env, options);
            log_s3(url, &b);
            Ok(Box::new(b.build()?))
        }
        S::GoogleCloudStorage => Ok(Box::new(gcs_builder(url, env, options).build()?)),
        S::MicrosoftAzure => Ok(Box::new(azure_builder(url, env, options).build()?)),
        other => Err(object_store::Error::NotSupported {
            source: format!("{other:?} is not a cloud scheme").into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const NONE: [(&str, &str); 0] = [];

    #[test]
    fn s3_reads_aws_variables_from_the_environment() {
        let b = s3_builder(
            "s3://bucket/prefix",
            env(&[
                ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
                ("AWS_SECRET_ACCESS_KEY", "s3cr3t"),
                ("AWS_REGION", "eu-west-1"),
                ("AWS_ENDPOINT_URL", "https://s3.example.com"),
                ("AWS_VIRTUAL_HOSTED_STYLE_REQUEST", "true"),
                ("PATH", "/usr/bin"),
                ("REGION", "not-an-aws-variable"),
            ]),
            NONE,
        );
        let get = |k| b.get_config_value(&k);
        assert_eq!(
            get(AmazonS3ConfigKey::AccessKeyId).as_deref(),
            Some("AKIDEXAMPLE")
        );
        assert_eq!(
            get(AmazonS3ConfigKey::SecretAccessKey).as_deref(),
            Some("s3cr3t")
        );
        assert_eq!(get(AmazonS3ConfigKey::Region).as_deref(), Some("eu-west-1"));
        assert_eq!(
            get(AmazonS3ConfigKey::Endpoint).as_deref(),
            Some("https://s3.example.com")
        );
        assert_eq!(
            get(AmazonS3ConfigKey::VirtualHostedStyleRequest).as_deref(),
            Some("true")
        );
        // An https endpoint does not turn on plain HTTP.
        assert_eq!(
            get(AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp)).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn s3_accepts_the_short_endpoint_variable() {
        let b = s3_builder(
            "s3://bucket",
            env(&[("AWS_ENDPOINT", "https://minio.internal:9000")]),
            NONE,
        );
        assert_eq!(
            b.get_config_value(&AmazonS3ConfigKey::Endpoint).as_deref(),
            Some("https://minio.internal:9000")
        );
    }

    #[test]
    fn s3_explicit_options_win_over_the_environment() {
        let b = s3_builder(
            "s3://bucket",
            env(&[
                ("AWS_REGION", "eu-west-1"),
                ("AWS_ACCESS_KEY_ID", "from-env"),
            ]),
            [
                ("aws_region", "us-east-2"),
                ("AWS_ACCESS_KEY_ID", "explicit"),
            ],
        );
        assert_eq!(
            b.get_config_value(&AmazonS3ConfigKey::Region).as_deref(),
            Some("us-east-2")
        );
        assert_eq!(
            b.get_config_value(&AmazonS3ConfigKey::AccessKeyId)
                .as_deref(),
            Some("explicit")
        );
    }

    #[test]
    fn s3_http_endpoint_allows_http_unless_told_otherwise() {
        let allow = |b: &AmazonS3Builder| {
            b.get_config_value(&AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp))
        };
        let b = s3_builder(
            "s3://bucket",
            env(&[("AWS_ENDPOINT_URL", "http://rustfs:9000")]),
            NONE,
        );
        assert_eq!(allow(&b).as_deref(), Some("true"));

        let b = s3_builder(
            "s3://bucket",
            env(&[
                ("AWS_ENDPOINT_URL", "http://rustfs:9000"),
                ("AWS_ALLOW_HTTP", "false"),
            ]),
            NONE,
        );
        assert_eq!(allow(&b).as_deref(), Some("false"));

        let b = s3_builder(
            "s3://bucket",
            env(&[("AWS_ENDPOINT_URL", "http://rustfs:9000")]),
            [("aws_allow_http", "false")],
        );
        assert_eq!(allow(&b).as_deref(), Some("false"));
    }

    #[test]
    fn s3_builds_without_network_access() {
        // Building never contacts the endpoint or the metadata service.
        let b = s3_builder(
            "s3://bucket/prefix",
            env(&[
                ("AWS_ACCESS_KEY_ID", "k"),
                ("AWS_SECRET_ACCESS_KEY", "s"),
                ("AWS_ENDPOINT_URL", "http://127.0.0.1:1"),
            ]),
            NONE,
        );
        b.build().unwrap();
    }

    #[test]
    fn gcs_reads_google_variables_and_service_account() {
        let b = gcs_builder(
            "gs://bucket",
            env(&[
                ("GOOGLE_SERVICE_ACCOUNT_KEY", "{\"k\":1}"),
                ("SERVICE_ACCOUNT", "/etc/sa.json"),
                ("AWS_REGION", "ignored"),
            ]),
            NONE,
        );
        assert_eq!(
            b.get_config_value(&GoogleConfigKey::ServiceAccountKey)
                .as_deref(),
            Some("{\"k\":1}")
        );
        assert_eq!(
            b.get_config_value(&GoogleConfigKey::ServiceAccount)
                .as_deref(),
            Some("/etc/sa.json")
        );
    }

    #[test]
    fn azure_reads_azure_variables() {
        let b = azure_builder(
            "az://container",
            env(&[
                ("AZURE_STORAGE_ACCOUNT_NAME", "acct"),
                ("AZURE_STORAGE_ACCOUNT_KEY", "a2V5"),
                ("IDENTITY_ENDPOINT", "http://169.254.169.254/msi"),
            ]),
            NONE,
        );
        assert_eq!(
            b.get_config_value(&AzureConfigKey::AccountName).as_deref(),
            Some("acct")
        );
        assert_eq!(
            b.get_config_value(&AzureConfigKey::AccessKey).as_deref(),
            Some("a2V5")
        );
        assert_eq!(
            b.get_config_value(&AzureConfigKey::MsiEndpoint).as_deref(),
            Some("http://169.254.169.254/msi")
        );
    }

    /// BY0 Task 0, ruling 1: the cloud-init and `install.sh --cloud` paths give
    /// the engine no key. On Azure the engine takes the VM's managed identity
    /// from IMDS when `AZURE_CREDENTIAL_TYPE=managed_identity` (and
    /// `AZURE_STORAGE_CLIENT_ID` picks a user-assigned one).
    #[test]
    fn azure_managed_identity_needs_no_key() {
        let b = azure_builder(
            "az://container/loams",
            env(&[
                ("AZURE_STORAGE_ACCOUNT_NAME", "acct"),
                ("AZURE_CREDENTIAL_TYPE", "managed_identity"),
                (
                    "AZURE_STORAGE_CLIENT_ID",
                    "11111111-2222-3333-4444-555555555555",
                ),
            ]),
            NONE,
        );
        assert_eq!(
            b.get_config_value(&AzureConfigKey::CredentialType)
                .as_deref(),
            Some("managed_identity")
        );
        assert_eq!(
            b.get_config_value(&AzureConfigKey::ClientId).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert!(b.get_config_value(&AzureConfigKey::AccessKey).is_none());
        assert!(b.get_config_value(&AzureConfigKey::SasKey).is_none());
        // Building never contacts the metadata service.
        b.build().unwrap();
    }

    /// On Compute Engine the engine takes the VM service account's token from
    /// the metadata server when no `GOOGLE_*` key is set.
    #[test]
    fn gcs_builds_without_a_key_for_the_metadata_server() {
        let b = gcs_builder("gs://bucket/loams", env(&[("AWS_REGION", "ignored")]), NONE);
        assert!(
            b.get_config_value(&GoogleConfigKey::ServiceAccountKey)
                .is_none()
        );
        b.build().unwrap();
    }

    /// On EC2 the engine takes the instance role from IMDS when no `AWS_*` key
    /// is set; only the region is a setting.
    #[test]
    fn s3_builds_without_a_key_for_the_instance_role() {
        let b = s3_builder(
            "s3://bucket/loams",
            env(&[("AWS_REGION", "us-east-1")]),
            NONE,
        );
        assert!(
            b.get_config_value(&AmazonS3ConfigKey::AccessKeyId)
                .is_none()
        );
        assert!(
            b.get_config_value(&AmazonS3ConfigKey::SecretAccessKey)
                .is_none()
        );
        b.build().unwrap();
    }

    #[test]
    fn endpoints_are_logged_without_user_info() {
        assert_eq!(
            redact_endpoint("http://user:pass@minio:9000/"),
            "http://minio:9000/"
        );
    }
}
