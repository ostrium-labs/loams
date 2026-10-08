//! The image pins of `release/sqldb-images.toml` (§47 §4; SQ1 Task 1).
//!
//! Every runtime pulls `<image>@<digest>`, never a tag. The file is embedded
//! at build time, so a binary cannot drift from the pins it was built with.

use std::path::Path;

use serde::Deserialize;

/// `release/sqldb-images.toml`, as built.
const PINNED: &str = include_str!("../../../release/sqldb-images.toml");

/// Why an images file was refused.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// The TOML did not parse, or an image or field is missing.
    #[error("sqldb-images.toml: {0}")]
    Parse(#[from] toml::de::Error),
    /// The file could not be read.
    #[error("sqldb-images.toml: {0}")]
    Io(#[from] std::io::Error),
    /// The entry is not pinned by a `sha256:` digest, or names a tag.
    #[error("image {name} is not pinned by digest: {why}")]
    NotPinned {
        /// The table name (`tidb`, `pd`, …).
        name: &'static str,
        /// What is wrong.
        why: String,
    },
}

/// One image, pinned by digest. `tag` is informational.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImagePin {
    /// The repository, e.g. `docker.io/pingcap/tidb`, without tag or digest.
    pub image: String,
    /// The release the digest was taken from, e.g. `v8.5.8`.
    pub tag: String,
    /// `sha256:<64 hex>`.
    pub digest: String,
}

impl ImagePin {
    /// `<image>@<digest>`: the only form a runtime pulls or runs.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.image, self.digest)
    }

    fn check(&self, name: &'static str) -> Result<(), ImageError> {
        let refuse = |why: String| Err(ImageError::NotPinned { name, why });
        let hex_ok = self.digest.strip_prefix("sha256:").is_some_and(|h| {
            h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        });
        if !hex_ok {
            return refuse(format!(
                "digest {:?} is not sha256:<64 lowercase hex>",
                self.digest
            ));
        }
        let last = self.image.rsplit('/').next().unwrap_or_default();
        if self.image.is_empty() || self.image.contains('@') || last.contains(':') {
            return refuse(format!(
                "image {:?} must name a repository only",
                self.image
            ));
        }
        if self.tag.is_empty() {
            return refuse("tag is empty".into());
        }
        Ok(())
    }
}

/// The five pinned images.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Images {
    tidb: ImagePin,
    tikv: ImagePin,
    pd: ImagePin,
    ticdc: ImagePin,
    br: ImagePin,
}

impl Images {
    /// The pins of `release/sqldb-images.toml` this binary was built with.
    pub fn load() -> Result<Self, ImageError> {
        Self::parse(PINNED)
    }

    /// Reads and checks another images file (tests, operators' overrides).
    pub fn from_path(path: &Path) -> Result<Self, ImageError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    /// Parses and checks an images file: every entry pinned by digest.
    pub fn parse(text: &str) -> Result<Self, ImageError> {
        let images: Self = toml::from_str(text)?;
        for (name, pin) in images.all() {
            pin.check(name)?;
        }
        Ok(images)
    }

    /// Every pin, by table name.
    pub fn all(&self) -> [(&'static str, &ImagePin); 5] {
        [
            ("tidb", &self.tidb),
            ("tikv", &self.tikv),
            ("pd", &self.pd),
            ("ticdc", &self.ticdc),
            ("br", &self.br),
        ]
    }

    /// `pingcap/tidb`.
    pub fn tidb(&self) -> &ImagePin {
        &self.tidb
    }
    /// `pingcap/tikv`.
    pub fn tikv(&self) -> &ImagePin {
        &self.tikv
    }
    /// `pingcap/pd`.
    pub fn pd(&self) -> &ImagePin {
        &self.pd
    }
    /// `pingcap/ticdc`.
    pub fn ticdc(&self) -> &ImagePin {
        &self.ticdc
    }
    /// `pingcap/br`.
    pub fn br(&self) -> &ImagePin {
        &self.br
    }
}
