//! Which adapters this install runs (D2014).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::Serialize;

use crate::{EngineError, EngineKind, ManagedEngine, SizeTier, SqlDialect, Stability};

/// One adapter per [`EngineKind`]. The binary that hosts the facade registers the adapters
/// whose profiles are on; a kind with none answers 501 `engine_not_enabled`.
#[derive(Clone, Default)]
pub struct EngineRegistry {
    engines: BTreeMap<EngineKind, Arc<dyn ManagedEngine>>,
}

impl fmt::Debug for EngineRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineRegistry")
            .field("kinds", &self.engines.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an adapter.
    ///
    /// # Panics
    ///
    /// When an adapter for the same kind is already registered: that is a wiring bug.
    #[must_use]
    #[allow(clippy::panic)]
    pub fn with(mut self, engine: Arc<dyn ManagedEngine>) -> Self {
        let kind = engine.kind();
        if self.engines.insert(kind, engine).is_some() {
            panic!("two adapters registered for engine kind {kind}");
        }
        self
    }

    pub fn get(&self, kind: EngineKind) -> Result<&Arc<dyn ManagedEngine>, EngineError> {
        self.engines.get(&kind).ok_or_else(|| {
            EngineError::NotEnabled(format!(
                "{} is not enabled on this install (turn on its compose profile)",
                kind.label()
            ))
        })
    }

    pub fn kinds(&self) -> impl Iterator<Item = EngineKind> + '_ {
        self.engines.keys().copied()
    }

    /// `GET /v1/engines`: every kind Loams knows, enabled or not.
    pub fn catalog(&self) -> EngineCatalog {
        let engines = EngineKind::ALL
            .into_iter()
            .map(|kind| match self.engines.get(&kind) {
                Some(e) => {
                    let c = e.capabilities();
                    EngineInfo {
                        kind,
                        label: kind.label(),
                        stability: c.stability,
                        enabled: true,
                        versions: c.versions,
                        default_version: Some(c.default_version),
                        sql_dialect: c.sql_dialect,
                        sessions: c.sessions,
                        jobs: c.jobs,
                        suspend: c.suspend,
                    }
                }
                None => EngineInfo {
                    kind,
                    label: kind.label(),
                    stability: Stability::Preview,
                    enabled: false,
                    versions: Vec::new(),
                    default_version: None,
                    sql_dialect: None,
                    sessions: false,
                    jobs: false,
                    suspend: false,
                },
            })
            .collect();
        let sizes = SizeTier::ALL
            .into_iter()
            .map(|tier| {
                let (vcpu_milli, memory_mib) = tier.shape();
                SizeInfo {
                    tier,
                    vcpu_milli,
                    memory_mib,
                }
            })
            .collect();
        EngineCatalog { engines, sizes }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EngineCatalog {
    pub engines: Vec<EngineInfo>,
    pub sizes: Vec<SizeInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EngineInfo {
    pub kind: EngineKind,
    pub label: &'static str,
    pub stability: Stability,
    pub enabled: bool,
    pub versions: Vec<String>,
    pub default_version: Option<String>,
    pub sql_dialect: Option<SqlDialect>,
    pub sessions: bool,
    pub jobs: bool,
    pub suspend: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SizeInfo {
    pub tier: SizeTier,
    pub vcpu_milli: u32,
    pub memory_mib: u32,
}
