//! A closed window of `_meter.windows` (§63 §8.2), as the window exporters
//! (Lago, the CloudEvents webhook) read it with `FINAL`.

use crate::Quantity;

/// `_meter.windows.window_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowSize {
    Minute,
    Hour,
    Day,
}

impl WindowSize {
    pub fn as_str(self) -> &'static str {
        match self {
            WindowSize::Minute => "minute",
            WindowSize::Hour => "hour",
            WindowSize::Day => "day",
        }
    }

    /// The window's length in seconds.
    pub fn seconds(self) -> i64 {
        match self {
            WindowSize::Minute => 60,
            WindowSize::Hour => 3600,
            WindowSize::Day => 86_400,
        }
    }
}

/// One row of `_meter.windows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    /// The meter slug.
    pub meter: String,
    pub subject: String,
    pub window_size: WindowSize,
    /// The window's start, Unix seconds UTC.
    pub window_start: i64,
    /// The canonical JSON of the group-by values (`{}` or empty for none).
    pub group_key: String,
    pub value: Quantity,
    /// Events in the window.
    pub events: u64,
    /// The row's version: a later close run wins.
    pub close_run: u64,
}

impl WindowRow {
    /// The group-by key, empty when the meter has none.
    pub fn group(&self) -> &str {
        match self.group_key.trim() {
            "" | "{}" => "",
            g => g,
        }
    }
}
