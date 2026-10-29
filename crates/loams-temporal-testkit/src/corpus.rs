//! DX0 Task 4's corpus: `conformance/compute/temporal/corpus/`, with its
//! `manifest.json` and `histories/<lang>/<scenario>[.run<k>].binpb`.

use std::path::{Path, PathBuf};

use loams_temporal_proto::temporal::api::history::v1::History;
use prost::Message as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Why the corpus could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    /// A file could not be read.
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The manifest is not what `manifest.py build` writes.
    #[error("manifest.json: {0}")]
    Manifest(String),
    /// A history's hash differs from the manifest's.
    #[error("{0}: SHA-256 differs from manifest.json")]
    Hash(PathBuf),
    /// A history does not decode.
    #[error("{0}: {1}")]
    Decode(PathBuf, prost::DecodeError),
}

#[derive(Deserialize)]
struct Manifest {
    histories: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    binpb: String,
    scenario: String,
    run: u32,
    language: String,
    phase: String,
    sha256_binpb: String,
}

/// One history of the corpus.
#[derive(Debug, Clone)]
pub struct CorpusHistory {
    /// The scenario (`hello`, …).
    pub scenario: String,
    /// `go` or `ts`.
    pub lang: String,
    /// The run, from 1 (scenarios with several runs, such as continue-as-new).
    pub run: u32,
    /// `DX1` to `DX4`.
    pub phase: String,
    /// The history.
    pub history: History,
    /// Where it was read from.
    pub path: PathBuf,
}

/// The loaded corpus.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// The corpus directory (the one holding `manifest.json`).
    pub root: PathBuf,
    /// Every history in manifest order.
    pub histories: Vec<CorpusHistory>,
}

impl Corpus {
    /// The corpus directory: `LOAMS_TEMPORAL_CORPUS`, or the repository's
    /// `conformance/compute/temporal/corpus` (found from the running test's
    /// `CARGO_MANIFEST_DIR`, read at run time). `None` if it has no
    /// `manifest.json`.
    pub fn locate() -> Option<PathBuf> {
        let root = match std::env::var_os("LOAMS_TEMPORAL_CORPUS") {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?)
                .join("../../conformance/compute/temporal/corpus"),
        };
        root.join("manifest.json").is_file().then_some(root)
    }

    /// [`Corpus::locate`] and [`Corpus::load`]; `None` (with a note on
    /// stderr) when there is no corpus, so tests can skip.
    pub fn for_tests() -> Option<Corpus> {
        let Some(root) = Corpus::locate() else {
            eprintln!("skipped: no Temporal history corpus (set LOAMS_TEMPORAL_CORPUS)");
            return None;
        };
        Some(Corpus::load(&root).unwrap_or_else(|e| panic!("corpus at {}: {e}", root.display())))
    }

    /// Loads every history the manifest lists, checking each hash.
    pub fn load(root: &Path) -> Result<Corpus, CorpusError> {
        let manifest_path = root.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| CorpusError::Io(manifest_path.clone(), e))?;
        let manifest: Manifest =
            serde_json::from_str(&text).map_err(|e| CorpusError::Manifest(e.to_string()))?;
        let mut histories = Vec::new();
        for e in manifest.histories {
            let path = root.join(&e.binpb);
            let bytes = std::fs::read(&path).map_err(|err| CorpusError::Io(path.clone(), err))?;
            if hex::encode(Sha256::digest(&bytes)) != e.sha256_binpb {
                return Err(CorpusError::Hash(path));
            }
            let history = History::decode(bytes.as_slice())
                .map_err(|err| CorpusError::Decode(path.clone(), err))?;
            histories.push(CorpusHistory {
                scenario: e.scenario,
                lang: e.language,
                run: e.run,
                phase: e.phase,
                history,
                path,
            });
        }
        Ok(Corpus {
            root: root.to_path_buf(),
            histories,
        })
    }

    /// The runs of `scenario` in `lang`, oldest first.
    pub fn runs(&self, lang: &str, scenario: &str) -> Vec<&CorpusHistory> {
        let mut v: Vec<_> = self
            .histories
            .iter()
            .filter(|h| h.lang == lang && h.scenario == scenario)
            .collect();
        v.sort_by_key(|h| h.run);
        v
    }

    /// The single-run histories of `phase` (`"DX1"`, …).
    pub fn phase<'a>(&'a self, phase: &'a str) -> impl Iterator<Item = &'a CorpusHistory> + 'a {
        self.histories.iter().filter(move |h| h.phase == phase)
    }
}
