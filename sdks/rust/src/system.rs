//! Feature detection and the version check (design §44 §4 and §7.4; runtime
//! contract R5, R9).
//!
//! The unavailable-service path has two halves, and an SDK needs both:
//!
//! 1. **Without calling.** `GetInstance.services[]` says which packages this
//!    binary carries and which it does not (D600). One call, no auth, cheap, so
//!    an application asks first and hides a feature it cannot use.
//!    `System::available`, `System::served`, `System::unavailable` and
//!    `System::guard` wrap it.
//! 2. **When the caller calls anyway.** A call to a package the variant does not
//!    carry answers `unimplemented` with `ErrorInfo.reason =
//!    feature_not_in_variant` and the variant in `metadata.variant` (D600). The
//!    runtime turns that into [`crate::ErrorKind::FeatureNotInVariant`], so the
//!    branch is on `err.is_feature_not_in_variant()` or on
//!    `err.reason() == Some(Reason::FeatureNotInVariant)` — never on a message,
//!    and never on the package name, which is a proto detail.
//!
//! Half 1 is the one to use. Half 2 is the safety net for a caller who skipped
//! it, or whose instance changed variant underneath a long-lived client.
//!
//! The catalogue is cached for the life of the process (an instance's services do
//! not change while it runs) and concurrent readers share **one** in-flight
//! fetch, so a cold start with twenty availability checks makes one call.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use loams_proto::loams::instance::v1::{GetInstanceResponse, ServiceStatus};

use crate::error::{ErrorKind, LoamsError};
use crate::facade::{self, MODULES};
use crate::reason::Reason;
use crate::request::variant_metadata;

/// The build variant the guard names when the catalogue does not (design §30 §9:
/// `cli`, `standard` and `full`, with `standard` the default).
pub const DEFAULT_VARIANT: &str = "standard";

/// `GetInstance.services[]`, sorted by package.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalogue {
    /// Every package the instance knows, served or not.
    pub services: Vec<ServiceStatus>,
    /// The packages this binary serves.
    pub served: Vec<String>,
    /// The packages this binary knows but does not serve, which is what makes
    /// `unavailable` a plan rather than a surprise.
    pub unavailable: Vec<String>,
}

/// What `System::version` reports (runtime contract R9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReport {
    /// The proto revision this SDK was generated from.
    pub proto_rev: String,
    /// The server's own semver.
    pub server_version: String,
    /// The proto packages the server says it serves.
    pub api_versions: Vec<String>,
    /// The SDK's packages the server does not serve.
    pub missing: Vec<String>,
    /// True when every package the SDK speaks is served.
    pub compatible: bool,
}

/// The boxed future one `GetInstance` call returns.
pub type GetInstanceFuture =
    Pin<Box<dyn std::future::Future<Output = Result<GetInstanceResponse, LoamsError>> + Send>>;

/// The one RPC the system API uses, injected so the system API is testable
/// without a client and so its cache can be asserted on.
pub type GetInstance = Arc<dyn Fn() -> GetInstanceFuture + Send + Sync>;

/// A `GetInstance` that always answers `info`, for a caller that already has one.
pub fn get_instance_from(
    info: GetInstanceResponse,
) -> impl Fn() -> GetInstanceFuture + Send + Sync {
    let info = Arc::new(info);
    move || {
        let info = Arc::clone(&info);
        Box::pin(async move { Ok(Arc::unwrap_or_clone(info)) })
    }
}

/// The catalogue cache, shared by every clone of a client. `pub(crate)` because
/// the cache is an implementation detail of [`System`] rather than something a
/// caller constructs.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    catalogue: Option<Catalogue>,
}

/// The catalogue, the version check and the guard.
#[derive(Clone)]
pub struct System {
    get_instance: GetInstance,
    cache: Arc<Mutex<Cache>>,
    endpoint: String,
}

impl std::fmt::Debug for System {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("System")
            .field("endpoint", &self.endpoint)
            .field(
                "cached",
                &self
                    .cache
                    .lock()
                    .map(|cache| cache.catalogue.is_some())
                    .unwrap_or(false),
            )
            .finish_non_exhaustive()
    }
}

impl System {
    /// Wraps one call to `GetInstance`, which is the only RPC this uses.
    #[must_use]
    pub(crate) fn new(
        get_instance: GetInstance,
        endpoint: impl Into<String>,
        cache: Arc<Mutex<Cache>>,
    ) -> Self {
        System {
            get_instance,
            cache,
            endpoint: endpoint.into(),
        }
    }

    /// The endpoint this system describes, for error messages.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Forgets the cached catalogue, so the next check calls again.
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.catalogue = None;
        }
    }

    /// The service catalogue, cached.
    ///
    /// Cached because it is asked on a cold start, when a UI is deciding which
    /// features to show, and again on every feature check.
    pub async fn catalogue(&self) -> Result<Catalogue, LoamsError> {
        if let Some(cached) = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.catalogue.clone())
        {
            return Ok(cached);
        }
        let info = (self.get_instance)().await?;
        let catalogue = to_catalogue(&info);
        if let Ok(mut cache) = self.cache.lock() {
            cache.catalogue = Some(catalogue.clone());
        }
        Ok(catalogue)
    }

    /// Whether this binary serves a module or a proto package.
    ///
    /// Takes either, so a caller holding `loams.live()` can pass `"live"` and a
    /// caller reading a `ServiceStatus` can pass `"loams.live.v1"`.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the name is neither a generated module nor
    /// a `loams.*` package — asking about a module this SDK does not speak is a
    /// caller mistake, not an availability question.
    pub async fn available(&self, module_or_package: &str) -> Result<bool, LoamsError> {
        let name = package_of(module_or_package)?;
        Ok(self.catalogue().await?.served.contains(&name))
    }

    /// The packages this binary serves.
    ///
    /// # Errors
    ///
    /// Returns whatever the catalogue call reported.
    pub async fn served(&self) -> Result<Vec<String>, LoamsError> {
        Ok(self.catalogue().await?.served)
    }

    /// The packages it knows but does not serve.
    ///
    /// # Errors
    ///
    /// Returns whatever the catalogue call reported.
    pub async fn unavailable(&self) -> Result<Vec<String>, LoamsError> {
        Ok(self.catalogue().await?.unavailable)
    }

    /// Refuses unless this binary serves the module, with the **same** error
    /// type the server's own refusal maps to.
    ///
    /// One `catch` then covers both "the guard said no" and "the server
    /// refused", and the guard costs no RPC once the catalogue is cached.
    ///
    /// The `variant` is the one the catalogue named, when it did; a catalogue
    /// that does not name it yields the instance's default `standard`, which is
    /// what `loams dev` and the cloud both run (design §30 §9).
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] of kind [`ErrorKind::FeatureNotInVariant`] when
    /// the package is not served.
    pub async fn guard(&self, module_or_package: &str) -> Result<(), LoamsError> {
        let package = package_of(module_or_package)?;
        let catalogue = self.catalogue().await?;
        if catalogue.served.contains(&package) {
            return Ok(());
        }
        Err(LoamsError::new(
            ErrorKind::FeatureNotInVariant,
            connectrpc::ErrorCode::Unimplemented,
            format!("{package} is not in this instance's build variant"),
        )
        .with_reason(
            Reason::FeatureNotInVariant,
            variant_metadata(&package, DEFAULT_VARIANT),
            None,
        )
        .with_rpc("loams.instance.v1.InstanceService/GetInstance"))
    }

    /// The proto revision check (R9).
    ///
    /// A missing package is a **warning, not an exception**: the SDK still works
    /// for the modules that are there, and the caller decides what a missing one
    /// means. The report says which are missing; nothing here throws.
    ///
    /// # Errors
    ///
    /// Returns whatever the `GetInstance` call reported.
    pub async fn version(&self) -> Result<VersionReport, LoamsError> {
        let info = (self.get_instance)().await?;
        Ok(version_report(&info))
    }
}

/// The proto package behind a module name or a package name.
fn package_of(module_or_package: &str) -> Result<String, LoamsError> {
    if module_or_package.starts_with("loams.") {
        return Ok(module_or_package.to_owned());
    }
    match facade::module_of(module_or_package) {
        Some(module) => Ok(module.package()),
        None => Err(LoamsError::internal(format!(
            "no generated module named {module_or_package}; this SDK speaks {}",
            MODULES
                .iter()
                .map(|m| m.name)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Splits `GetInstance.services[]` into served and not.
fn to_catalogue(info: &GetInstanceResponse) -> Catalogue {
    let mut services = info.services.clone();
    services.sort_by(|left, right| left.package.cmp(&right.package));
    Catalogue {
        served: services
            .iter()
            .filter(|s| s.available)
            .map(|s| s.package.clone())
            .collect(),
        unavailable: services
            .iter()
            .filter(|s| !s.available)
            .map(|s| s.package.clone())
            .collect(),
        services,
    }
}

/// The R9 report, from one `GetInstance` answer.
#[must_use]
pub fn version_report(info: &GetInstanceResponse) -> VersionReport {
    let api_versions = info.api_versions.clone();
    let spoken: Vec<String> = facade::PROTO_PACKAGES
        .iter()
        .filter(|name| name.starts_with("loams."))
        .map(|name| (*name).to_owned())
        .collect();
    let missing: Vec<String> = spoken
        .iter()
        .filter(|name| !api_versions.contains(name))
        .cloned()
        .collect();
    VersionReport {
        proto_rev: facade::PROTO_REV.to_owned(),
        server_version: info.server_version.clone(),
        compatible: missing.is_empty(),
        api_versions,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(package: &str, available: bool) -> ServiceStatus {
        ServiceStatus {
            package: package.to_owned(),
            version: "v1".to_owned(),
            available,
            services: vec![format!("{package}.SomeService")],
            unstable: package == "loams.live.v1",
            ..Default::default()
        }
    }

    fn info() -> GetInstanceResponse {
        GetInstanceResponse {
            instance_id: "01".to_owned(),
            name: "Loams".to_owned(),
            server_version: "0.1.0".to_owned(),
            api_versions: vec!["loams.instance.v1".to_owned()],
            services: vec![
                status("loams.instance.v1", true),
                status("loams.live.v1", false),
            ],
            ..Default::default()
        }
    }

    fn system() -> System {
        System::new(
            Arc::new(get_instance_from(info())),
            "http://127.0.0.1:8080",
            Arc::new(Mutex::new(Cache::default())),
        )
    }

    /// A `System` over a `GetInstance` that counts its calls, so the cache can be
    /// asserted on.
    fn counting_system(calls: &Arc<Mutex<usize>>) -> System {
        let answered = info();
        let counter = Arc::clone(calls);
        System::new(
            Arc::new(move || {
                let answered = answered.clone();
                let counter = Arc::clone(&counter);
                Box::pin(async move {
                    *counter.lock().expect("not poisoned") += 1;
                    Ok(answered)
                })
            }),
            "http://127.0.0.1:8080",
            Arc::new(Mutex::new(Cache::default())),
        )
    }

    #[tokio::test]
    async fn the_catalogue_splits_served_from_unavailable_and_is_cached() {
        let calls = Arc::new(Mutex::new(0usize));
        let system = counting_system(&calls);
        assert!(system.available("instance").await.expect("answered"));
        assert!(!system.available("live").await.expect("answered"));
        assert_eq!(
            system.served().await.expect("answered"),
            ["loams.instance.v1"]
        );
        assert_eq!(
            system.unavailable().await.expect("answered"),
            ["loams.live.v1"]
        );
        // Three more reads of the same catalogue: still one call.
        assert_eq!(*calls.lock().expect("not poisoned"), 1);
        system.invalidate();
        assert_eq!(system.served().await.expect("answered").len(), 1);
        assert_eq!(
            *calls.lock().expect("not poisoned"),
            2,
            "invalidate drops it"
        );
    }

    #[tokio::test]
    async fn the_guard_raises_the_same_error_type_as_the_servers_own_refusal() {
        let system = system();
        let refused = system.guard("live").await.unwrap_err();
        assert!(refused.is_feature_not_in_variant());
        assert_eq!(refused.reason(), Some(Reason::FeatureNotInVariant));
        assert_eq!(refused.code, connectrpc::ErrorCode::Unimplemented);
        assert_eq!(refused.variant(), Some("standard"));
        assert_eq!(
            refused.metadata().get("package").map(String::as_str),
            Some("loams.live.v1")
        );
        // The served module passes, and a package name works as well as a module
        // name — `loams.live` and `loams.tables` are one service.
        system.guard("instance").await.expect("served");
        system.guard("loams.instance.v1").await.expect("served");
        system.guard("loams.live.v1").await.unwrap_err();
    }

    #[tokio::test]
    async fn a_module_this_sdk_does_not_speak_is_a_caller_mistake_not_a_yes_or_no() {
        let system = system();
        let error = system.available("collections").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no generated module named collections"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_version_report_names_the_missing_packages_without_failing() {
        let report = system().version().await.expect("answered");
        assert_eq!(report.proto_rev, facade::PROTO_REV);
        assert_eq!(report.server_version, "0.1.0");
        assert_eq!(report.api_versions, ["loams.instance.v1"]);
        // `loams.live.v1` is served as unavailable in the standard variant, and
        // `api_versions` lists only what is served, so the mismatch is the
        // server's and is reported rather than raised.
        assert!(!report.compatible);
        assert!(report.missing.contains(&"loams.live.v1".to_owned()));
        assert!(!report.missing.contains(&"loams.instance.v1".to_owned()));
    }

    #[tokio::test]
    async fn a_server_serving_everything_the_sdk_speaks_is_compatible() {
        let mut answered = info();
        answered.api_versions = facade::PROTO_PACKAGES
            .iter()
            .filter(|n| n.starts_with("loams."))
            .map(|n| (*n).to_owned())
            .collect();
        let report = version_report(&answered);
        assert!(report.compatible, "{:?}", report.missing);
    }
}
