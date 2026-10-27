//! `system.settings` (plan HE1 Task 38), from Task 37's settings
//! (`crate::settings::system_rows`): every ClickHouse setting, at the
//! statement's value, sorted by name. `changed` is whether the statement set
//! a value other than the default.
//!
//! `min`, `max`, `alias_for`, `is_obsolete` and `tier` are not in chDB's
//! defaults table; they print their defaults, never invented (R38.4).

use super::build::{Cell, Rows};
use crate::settings::{Settings, system_rows};

/// One row of `system.settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    /// ClickHouse's name.
    pub name: String,
    /// The value the statement runs with.
    pub value: String,
    /// Whether the statement set a value other than the default.
    pub changed: bool,
    /// The engine's class and why (Task 37).
    pub description: String,
    /// The lowest value, when ClickHouse has one.
    pub min: Option<String>,
    /// The highest value, when ClickHouse has one.
    pub max: Option<String>,
    /// Whether the House refuses any other value.
    pub readonly: bool,
    /// ClickHouse's setting type.
    pub ty: String,
    /// The default.
    pub default: String,
    /// The setting it is an alias of.
    pub alias_for: String,
    /// Whether ClickHouse marks it obsolete.
    pub is_obsolete: bool,
    /// ClickHouse's tier: 0 Production, 4 Obsolete, 8 Experimental, 12
    /// Beta, 16 PrivatePreview.
    pub tier: i8,
}

/// The rows for a statement running with `settings` (T37's body, from
/// `he1-t37-t38-notes.md`).
pub fn rows(settings: &Settings) -> Vec<SettingRow> {
    system_rows(settings)
        .into_iter()
        .map(|r| SettingRow {
            name: r.name,
            value: r.value,
            changed: r.changed,
            description: r.description,
            min: None,
            max: None,
            readonly: r.readonly,
            ty: r.ty.to_string(),
            default: r.default,
            alias_for: String::new(),
            is_obsolete: false,
            tier: 0,
        })
        .collect()
}

/// [`rows`] as `system.settings`' filled columns.
pub fn fill(rows: &[SettingRow]) -> Rows {
    let mut out = Rows::new(&[
        "name",
        "value",
        "changed",
        "description",
        "min",
        "max",
        "readonly",
        "type",
        "default",
        "alias_for",
        "is_obsolete",
        "tier",
    ]);
    for r in rows {
        out.push(vec![
            Cell::from(r.name.as_str()),
            Cell::from(r.value.as_str()),
            Cell::from(r.changed),
            Cell::from(r.description.as_str()),
            Cell::from(r.min.clone()),
            Cell::from(r.max.clone()),
            Cell::from(r.readonly),
            Cell::from(r.ty.as_str()),
            Cell::from(r.default.as_str()),
            Cell::from(r.alias_for.as_str()),
            Cell::from(r.is_obsolete),
            Cell::Int(i64::from(r.tier)),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_settings_are_changed() {
        let s = Settings::resolve(&[("max_threads".to_string(), "2".to_string())]).unwrap();
        let rows = rows(&s);
        let r = rows.iter().find(|r| r.name == "max_threads").unwrap();
        assert!(r.changed);
        assert_eq!(r.value, "2");
        assert!(
            rows.iter()
                .filter(|r| r.name != "max_threads")
                .all(|r| !r.changed)
        );
        let table = fill(&rows);
        let batch = super::super::build::batch(super::super::shapes::SETTINGS, &table).unwrap();
        assert_eq!(batch.num_rows(), rows.len());
    }
}
