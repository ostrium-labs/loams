//! A secret value: zeroised on drop, never printed.

use std::fmt;

use zeroize::Zeroizing;

/// A credential or other secret value. `Debug` and `Display` print
/// `<redacted>`; read it with [`Secret::expose`] where it is written out.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// The value, for the one place that writes it (an HTTP header, a
    /// child process's stdin).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_printed() {
        let s = Secret::new("hunter2-PLANTED");
        assert_eq!(format!("{s:?} {s}"), "<redacted> <redacted>");
        assert_eq!(s.expose(), "hunter2-PLANTED");
    }
}
