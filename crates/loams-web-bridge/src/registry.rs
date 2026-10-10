use std::fmt;
use std::sync::Arc;

use crate::config::ProviderKind;
use crate::provider::{BrowserProvider, Capabilities};

/// A registry of browser providers that selects the best provider
/// for a given request based on capability requirements.
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn BrowserProvider>>,
}

impl ProviderRegistry {
    /// Creates a new, empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Registers a new provider.
    pub fn register(&mut self, provider: Arc<dyn BrowserProvider>) {
        self.providers.push(provider);
    }

    /// Finds the first provider whose capabilities satisfy all `true` requirements.
    pub fn select(&self, requirements: &Capabilities) -> Option<Arc<dyn BrowserProvider>> {
        self.providers
            .iter()
            .find(|p| {
                let caps = p.capabilities();
                (!requirements.core || caps.core)
                    && (!requirements.screenshots || caps.screenshots)
                    && (!requirements.extraction || caps.extraction)
                    && (!requirements.authenticated_sessions || caps.authenticated_sessions)
                    && (!requirements.guardrails || caps.guardrails)
                    && (!requirements.tabs || caps.tabs)
                    && (!requirements.credentials_stay_local || caps.credentials_stay_local)
            })
            .cloned()
    }

    /// Returns the first registered provider of the given kind.
    pub fn select_by_kind(&self, kind: ProviderKind) -> Option<Arc<dyn BrowserProvider>> {
        self.providers.iter().find(|p| p.kind() == kind).cloned()
    }

    /// Returns the first provider in the registry, which acts as the default fallback.
    pub fn default(&self) -> Option<Arc<dyn BrowserProvider>> {
        self.providers.first().cloned()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field(
                "providers",
                &self.providers.iter().map(|p| p.kind()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Engine;
    use crate::error::BridgeError;
    use crate::provider::{OpenRequest, PageRef};
    use crate::tool::{
        ActionOutcome, Extracted, FillValue, FindQuery, Navigation, Screenshot, Snapshot,
        SnapshotRequest, Uid, WaitOutcome, WaitRequest,
    };

    #[derive(Debug)]
    struct MockProvider {
        kind: ProviderKind,
        capabilities: Capabilities,
    }

    #[async_trait::async_trait]
    impl BrowserProvider for MockProvider {
        fn kind(&self) -> ProviderKind {
            self.kind
        }
        fn engine(&self) -> Option<Engine> {
            None
        }
        fn capabilities(&self) -> Capabilities {
            self.capabilities
        }
        async fn open(&self, _request: OpenRequest) -> Result<PageRef, BridgeError> {
            unimplemented!()
        }
        async fn navigate(
            &self,
            _page: &PageRef,
            _url: &url::Url,
        ) -> Result<Navigation, BridgeError> {
            unimplemented!()
        }
        async fn snapshot(
            &self,
            _page: &PageRef,
            _request: &SnapshotRequest,
        ) -> Result<Snapshot, BridgeError> {
            unimplemented!()
        }
        async fn find(
            &self,
            _page: &PageRef,
            _query: &FindQuery,
        ) -> Result<Vec<crate::tool::SnapshotNode>, BridgeError> {
            unimplemented!()
        }
        async fn click(&self, _page: &PageRef, _uid: &Uid) -> Result<ActionOutcome, BridgeError> {
            unimplemented!()
        }
        async fn fill(
            &self,
            _page: &PageRef,
            _uid: &Uid,
            _value: &FillValue,
        ) -> Result<ActionOutcome, BridgeError> {
            unimplemented!()
        }
        async fn wait_for(
            &self,
            _page: &PageRef,
            _request: &WaitRequest,
        ) -> Result<WaitOutcome, BridgeError> {
            unimplemented!()
        }
        async fn screenshot(&self, _page: &PageRef) -> Result<Screenshot, BridgeError> {
            unimplemented!()
        }
        async fn extract(&self, _page: &PageRef) -> Result<Extracted, BridgeError> {
            unimplemented!()
        }
        async fn close(&self, _page: &PageRef) -> Result<(), BridgeError> {
            unimplemented!()
        }
    }

    #[test]
    fn test_select() {
        let mut registry = ProviderRegistry::new();

        let local_caps = Capabilities {
            core: true,
            screenshots: true,
            extraction: true,
            authenticated_sessions: true,
            guardrails: false,
            tabs: true,
            credentials_stay_local: true,
        };
        registry.register(Arc::new(MockProvider {
            kind: ProviderKind::Local,
            capabilities: local_caps,
        }));

        let remote_caps = Capabilities {
            core: true,
            screenshots: true,
            extraction: true,
            authenticated_sessions: false,
            guardrails: true,
            tabs: false,
            credentials_stay_local: false,
        };
        registry.register(Arc::new(MockProvider {
            kind: ProviderKind::BrowserRun,
            capabilities: remote_caps,
        }));

        // Requirements matching local
        let req1 = Capabilities {
            credentials_stay_local: true,
            ..Capabilities::default()
        };
        let p1 = registry.select(&req1).unwrap();
        assert_eq!(p1.kind(), ProviderKind::Local);

        // Requirements matching remote (guardrails)
        let req2 = Capabilities {
            guardrails: true,
            ..Capabilities::default()
        };
        let p2 = registry.select(&req2).unwrap();
        assert_eq!(p2.kind(), ProviderKind::BrowserRun);

        // Requirements matching none
        let req3 = Capabilities {
            guardrails: true,
            credentials_stay_local: true,
            ..Capabilities::default()
        };
        assert!(registry.select(&req3).is_none());
    }

    #[test]
    fn test_select_by_kind_and_default() {
        let mut registry = ProviderRegistry::new();

        assert!(registry.default().is_none());
        assert!(registry.select_by_kind(ProviderKind::Local).is_none());

        let local = Arc::new(MockProvider {
            kind: ProviderKind::Local,
            capabilities: Capabilities::default(),
        });
        registry.register(local);

        assert_eq!(registry.default().unwrap().kind(), ProviderKind::Local);
        assert_eq!(
            registry.select_by_kind(ProviderKind::Local).unwrap().kind(),
            ProviderKind::Local
        );
        assert!(registry.select_by_kind(ProviderKind::BrowserRun).is_none());
    }
}
