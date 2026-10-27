//! Lake tables in a local directory, for `loams-house-engine-dev --lake
//! <dir>` (plan HE1 Task 26): `<dir>/<database>/<table>/metadata/` holds a
//! table's metadata files, and the newest (by its `<version>-` prefix) is
//! read at its current snapshot, as the House's catalog would pin it. Task 39
//! replaces this with the pins the front sends in hsw1 v2.

use std::path::Path;
use std::sync::Arc;

use datafusion::object_store::local::LocalFileSystem;
use loams_house_iceberg::TablePin;
use loams_house_types::ChError;

use crate::Engine;

fn io(path: &Path, e: impl std::fmt::Display) -> ChError {
    ChError::bad_arguments(format!("--lake {}: {e}", path.display()))
}

fn entries(dir: &Path) -> Result<Vec<std::path::PathBuf>, ChError> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| io(dir, e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    out.sort();
    Ok(out)
}

/// The pins of the tables under `dir`.
///
/// # Errors
///
/// An unreadable directory or metadata file.
pub fn pins(dir: &Path) -> Result<Vec<TablePin>, ChError> {
    let dir = dir.canonicalize().map_err(|e| io(dir, e))?;
    let mut pins = Vec::new();
    for db in entries(&dir)?.into_iter().filter(|p| p.is_dir()) {
        for table in entries(&db)?.into_iter().filter(|p| p.is_dir()) {
            let metadata_dir = table.join("metadata");
            if !metadata_dir.is_dir() {
                continue;
            }
            let newest = entries(&metadata_dir)?
                .into_iter()
                .filter(|p| p.to_string_lossy().ends_with(".metadata.json"))
                .max_by_key(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.split('-').next())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0)
                });
            let Some(file) = newest else { continue };
            let bytes = std::fs::read(&file).map_err(|e| io(&file, e))?;
            let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string());
            let (Some(db_name), Some(table_name)) = (name(&db), name(&table)) else {
                continue;
            };
            let mut pin = TablePin::from_metadata_json(
                &db_name,
                &table_name,
                &format!("file://{}", file.display()),
                &bytes,
            )?;
            pin.location = format!("file://{}", table.display());
            pins.push(pin);
        }
    }
    Ok(pins)
}

/// Adds every table under `dir` to `engine`'s catalog; answers their names.
///
/// # Errors
///
/// What [`pins`] or opening a table refuses.
pub async fn attach(engine: &Engine, dir: &Path) -> Result<Vec<String>, ChError> {
    let store = Arc::new(LocalFileSystem::new());
    let mut names = Vec::new();
    for pin in pins(dir)? {
        let provider =
            loams_house_iceberg::provider(&pin, store.clone(), engine.registry()).await?;
        engine.catalog.insert_lake(&pin.db, &pin.name, provider);
        names.push(format!("{}.{}", pin.db, pin.name));
    }
    Ok(names)
}
