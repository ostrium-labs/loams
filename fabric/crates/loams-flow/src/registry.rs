//! The connector registry: every manifest under `connectors/registry/`, indexed by
//! id and by category.
//!
//! D352 makes the registry "one versioned manifest per connector, loaded at start,
//! served by `FlowService.ListConnectors`/`DescribeConnector`, and checked by CI".
//! This module is the load: it reads every `*.yaml` in one directory, validates each
//! against `connectors/schema/connector.schema.json` and CN1 plan Task 1's semantic
//! rules, and refuses to return a registry with any manifest missing.
//!
//! Two things about a load are deliberate:
//!
//! * **Errors are collected, not the first one.** A registry whose 200 manifests
//!   disagree with the design in twelve places should say so in one run, so
//!   [`Registry::load`] gathers every manifest's errors, sorted by file name, and
//!   [`RegistryError`] renders all of them.
//! * **`config.$ref` resolves against the directory's parent**, which is the
//!   `connectors/` root: the manifests write `schemas/kafka.config.json` (CN1
//!   Ruling 5 made every one of the 200 resolve, the 179 generated stubs included).

use std::path::Path;

use indexmap::IndexMap;

use crate::manifest::{
    Category, ConnectorSpec, ManifestError, ManifestErrors, Priority, RuntimeKind, Status,
    load_manifest_with_schema,
};
use crate::validate::{ManifestSchema, SchemaError, render_manifest_errors};

/// Every manifest under one registry directory, indexed by id and by category.
///
/// Insertion order is the load order, which is the directory's file order sorted by
/// name, so `list` and `by_category` are deterministic.
#[derive(Debug)]
pub struct Registry {
    by_id: IndexMap<String, ConnectorSpec>,
    by_category: IndexMap<Category, Vec<String>>,
}

impl Registry {
    /// Every manifest in `dir`, validated. `dir` is `connectors/registry`; its parent
    /// is the `connectors/` root that `config.$ref` is relative to.
    ///
    /// The manifest schema is this repository's
    /// [`crate::validate::default_manifest_schema_path`]; use
    /// [`Registry::load_with_schema`] to point at another one.
    pub fn load(dir: &Path) -> Result<Self, RegistryError> {
        Self::load_with_schema(dir, &crate::validate::default_manifest_schema_path())
    }

    /// [`Registry::load`] against a manifest schema at `schema_path`, compiled once
    /// for the whole directory rather than once per manifest.
    pub fn load_with_schema(dir: &Path, schema_path: &Path) -> Result<Self, RegistryError> {
        let schema = ManifestSchema::load(schema_path)?;
        let root = dir.parent().unwrap_or(dir);
        Self::load_compiled(dir, root, &schema)
    }

    /// [`Registry::load_with_schema`] against an already-compiled schema, which is
    /// what the two entry points above are for.
    pub fn load_compiled(
        dir: &Path,
        root: &Path,
        schema: &ManifestSchema,
    ) -> Result<Self, RegistryError> {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|error| RegistryError::Unreadable {
                dir: dir.display().to_string(),
                message: error.to_string(),
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|suffix| suffix == "yaml"))
            .collect();
        files.sort();

        let mut by_id: IndexMap<String, ConnectorSpec> = IndexMap::with_capacity(files.len());
        let mut failures: Vec<ManifestErrors> = Vec::new();
        for path in &files {
            let file = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            match load_manifest_with_schema(path, root, schema) {
                Ok(spec) => {
                    tracing::debug!(
                        connector = %spec.id,
                        runtime = %spec.runtime.kind,
                        "loaded a connector manifest"
                    );
                    if by_id.insert(spec.id.clone(), spec).is_some() {
                        failures.push(ManifestErrors {
                            file,
                            errors: vec![ManifestError::new(
                                "id",
                                "another manifest already declares this id",
                            )],
                        });
                    }
                }
                Err(errors) => {
                    tracing::debug!(file = %file, problems = errors.len(), "a manifest did not load");
                    failures.push(ManifestErrors { file, errors });
                }
            }
        }
        if by_id.is_empty() && failures.is_empty() {
            return Err(RegistryError::NoManifests {
                dir: dir.display().to_string(),
            });
        }
        if !failures.is_empty() {
            return Err(RegistryError::Manifests(failures));
        }

        let mut by_category: IndexMap<Category, Vec<String>> = IndexMap::new();
        for spec in by_id.values() {
            by_category
                .entry(spec.category)
                .or_default()
                .push(spec.id.clone());
        }
        Ok(Self { by_id, by_category })
    }

    /// The manifest with this id.
    pub fn get(&self, id: &str) -> Option<&ConnectorSpec> {
        self.by_id.get(id)
    }

    /// Every manifest that matches `filter`, in id order.
    pub fn list(&self, filter: &Filter) -> Vec<&ConnectorSpec> {
        self.by_id
            .values()
            .filter(|spec| filter.matches(spec))
            .collect()
    }

    /// Every manifest, in id order.
    pub fn all(&self) -> impl Iterator<Item = &ConnectorSpec> {
        self.by_id.values()
    }

    /// The 21 ★ manifests of §33 §8 (D358), in id order.
    pub fn starred(&self) -> Vec<&ConnectorSpec> {
        self.list(&Filter {
            starred: Some(true),
            ..Filter::default()
        })
    }

    /// The manifests grouped by category, in first-seen order, which is the order the
    /// categories appear in the registry directory.
    pub fn by_category(&self) -> impl Iterator<Item = (&Category, Vec<&ConnectorSpec>)> {
        self.by_category.iter().map(|(category, ids)| {
            let specs = ids.iter().filter_map(|id| self.by_id.get(id)).collect();
            (category, specs)
        })
    }

    /// How many manifests the registry holds.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the registry is empty, which a load never returns.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Every category present, in first-seen order.
    pub fn categories(&self) -> impl Iterator<Item = &Category> {
        self.by_category.keys()
    }
}

/// What [`Registry::list`] filters on. Every field left `None` matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// The kind of system the connector speaks to.
    pub category: Option<Category>,
    /// The rollout phase.
    pub priority: Option<Priority>,
    /// The connector's maturity.
    pub status: Option<Status>,
    /// Whether it is one of the 21 ★ connectors.
    pub starred: Option<bool>,
    /// Which runtime runs it (§33 §5).
    pub runtime_kind: Option<RuntimeKind>,
    /// A free-text needle, matched case-insensitively against the id, the display
    /// name and the `docs` path, which is what the catalog page's search box wants.
    pub query: Option<String>,
}

impl Filter {
    /// Whether `spec` passes every field of the filter.
    pub fn matches(&self, spec: &ConnectorSpec) -> bool {
        if let Some(category) = self.category
            && spec.category != category
        {
            return false;
        }
        if let Some(priority) = self.priority
            && spec.priority != priority
        {
            return false;
        }
        if let Some(status) = self.status
            && spec.status != status
        {
            return false;
        }
        if let Some(starred) = self.starred
            && spec.starred != starred
        {
            return false;
        }
        if let Some(runtime_kind) = self.runtime_kind
            && spec.runtime.kind != runtime_kind
        {
            return false;
        }
        if let Some(query) = &self.query {
            let needle = query.to_lowercase();
            let haystacks = [
                spec.id.as_str(),
                spec.name.as_str(),
                spec.docs.as_deref().unwrap_or_default(),
            ];
            if !haystacks
                .iter()
                .any(|field| field.to_lowercase().contains(&needle))
            {
                return false;
            }
        }
        true
    }
}

/// Why a registry did not load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The directory could not be read.
    #[error("{dir}: cannot read the registry directory: {message}")]
    Unreadable {
        /// The directory's path.
        dir: String,
        /// The I/O error.
        message: String,
    },
    /// The directory holds no `*.yaml` at all, which is a deployment mistake rather
    /// than a manifest problem.
    #[error("{dir}: the registry directory holds no *.yaml manifest")]
    NoManifests {
        /// The directory's path.
        dir: String,
    },
    /// The manifest schema could not be compiled, so nothing was checked.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// One entry per manifest that did not load, sorted by file name, each with all
    /// of that manifest's problems. `Display` renders every one of them.
    #[error("{}", render_manifest_errors(.0))]
    Manifests(Vec<ManifestErrors>),
}
