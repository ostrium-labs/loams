//! The compute image of each Postgres major (PG2 Task 11, R10.8): read from
//! the repository's pin file, `deploy/pins/loams-postgres-images.toml`
//! (written by `scripts/nf1/pins.py`), compiled in, and overridable per
//! major (`loams pg-control --compute-image 17=<repo>@sha256:<digest>`).
//!
//! Every image is pinned by digest ([`is_pinned`]), and never one of the
//! private archive copies under `.../neon-archive/` (NF1 keeps those for
//! the record, not to run).

use std::collections::BTreeMap;

use super::is_pinned;

/// The pin file, as this build was made with it.
pub const PINS: &str = include_str!("../../../../deploy/pins/loams-postgres-images.toml");

/// Why an image or the pin file was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("the pin file: {0}")]
    Pins(String),
    #[error("{0:?} is not pinned by digest (<repository>@sha256:<64 hex>)")]
    NotPinned(String),
    #[error("{0:?} is a neon-archive copy, which is never run")]
    Archive(String),
    #[error("{0:?}: expected <major>=<repository>@sha256:<digest>")]
    Malformed(String),
}

/// `image`, if computes may run it: pinned by digest, not an archive copy.
///
/// # Errors
///
/// [`ImageError::NotPinned`], [`ImageError::Archive`].
pub fn check_image(image: &str) -> Result<(), ImageError> {
    if !is_pinned(image) {
        return Err(ImageError::NotPinned(image.to_owned()));
    }
    if image.contains("/neon-archive/") {
        return Err(ImageError::Archive(image.to_owned()));
    }
    Ok(())
}

/// The Postgres major a compute image's repository names (`...compute-v17`,
/// `...compute-node-v17`), if it names one.
pub fn image_major(image: &str) -> Option<u32> {
    let repo = image.split('@').next().unwrap_or(image);
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let name = name.split(':').next().unwrap_or(name);
    let (_, digits) = name.rsplit_once("-v")?;
    digits.parse().ok()
}

/// `compute_major_mismatch_refused` (design 62 §9.2, §51 §9): a compute never
/// attaches to another major's timeline. An image whose repository names a
/// major other than the timeline's is refused; one that names none (a test
/// image) is taken at its configured major.
pub fn check_compute_major(image: &str, timeline_major: u32) -> Result<(), String> {
    match image_major(image) {
        Some(m) if m != timeline_major => Err(format!(
            "compute_major_mismatch_refused: image {image} is Postgres {m}, the timeline is \
             Postgres {timeline_major}"
        )),
        _ => Ok(()),
    }
}

/// The `[majors]` lists whose majors run computes (design §62 §9.1): a
/// supported major's new projects, a beta major's opted-in ones, and a
/// maintenance major's existing projects, exports and upgrades.
const MAJOR_LISTS: [&str; 3] = ["supported", "beta", "maintenance"];

/// The compute image of each major in pin file `text` (`[majors]
/// supported`, `beta` and `maintenance`; each `[images.compute-v<major>]`'s
/// `repository` and `digest`). A missing `beta` or `maintenance` list is
/// empty; `supported` is required.
///
/// # Errors
///
/// [`ImageError::Pins`] for a file that does not parse or misses a
/// listed major's image; the checks of [`check_image`].
pub fn from_pins(text: &str) -> Result<BTreeMap<u32, String>, ImageError> {
    let pins: toml::Value = toml::from_str(text).map_err(|e| ImageError::Pins(e.to_string()))?;
    let majors = pins
        .get("majors")
        .ok_or_else(|| ImageError::Pins("no [majors]".into()))?;
    if majors.get("supported").is_none() {
        return Err(ImageError::Pins("no [majors] supported".into()));
    }
    let mut out = BTreeMap::new();
    for list in MAJOR_LISTS {
        let Some(entries) = majors.get(list) else {
            continue;
        };
        let entries = entries
            .as_array()
            .ok_or_else(|| ImageError::Pins(format!("[majors] {list} is not a list")))?;
        for major in entries {
            let major = major
                .as_integer()
                .and_then(|m| u32::try_from(m).ok())
                .ok_or_else(|| ImageError::Pins(format!("a {list} major is not a number")))?;
            out.insert(major, image_of(&pins, major)?);
        }
    }
    Ok(out)
}

/// `[images.compute-v<major>]`'s `repository@digest`, checked.
fn image_of(pins: &toml::Value, major: u32) -> Result<String, ImageError> {
    let entry = pins
        .get("images")
        .and_then(|i| i.get(format!("compute-v{major}")))
        .ok_or_else(|| ImageError::Pins(format!("no [images.compute-v{major}]")))?;
    let field = |name: &str| {
        entry
            .get(name)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| ImageError::Pins(format!("compute-v{major} has no {name}")))
    };
    let image = format!("{}@{}", field("repository")?, field("digest")?);
    check_image(&image)?;
    Ok(image)
}

/// The compute images this build was pinned with ([`PINS`]).
///
/// # Errors
///
/// As [`from_pins`].
pub fn pinned() -> Result<BTreeMap<u32, String>, ImageError> {
    from_pins(PINS)
}

/// `<major>=<image>`, checked.
///
/// # Errors
///
/// [`ImageError::Malformed`]; the checks of [`check_image`].
pub fn parse_override(s: &str) -> Result<(u32, String), ImageError> {
    let (major, image) = s
        .split_once('=')
        .ok_or_else(|| ImageError::Malformed(s.to_owned()))?;
    let major = major
        .parse()
        .map_err(|_| ImageError::Malformed(s.to_owned()))?;
    check_image(image)?;
    Ok((major, image.to_owned()))
}

#[cfg(test)]
mod tests {

    #[test]
    fn compute_major_mismatch_refused() {
        let v17 = "ghcr.io/ostrium-labs/loams-postgres-compute-v17@sha256:00";
        let v18 = "ghcr.io/ostrium-labs/loams-postgres-compute-v18@sha256:00";
        assert_eq!(image_major(v17), Some(17));
        assert_eq!(image_major("ghcr.io/x/compute-node-v16:tag"), Some(16));
        assert_eq!(image_major("localhost/test-compute@sha256:00"), None);
        assert!(check_compute_major(v17, 17).is_ok());
        let err = check_compute_major(v17, 18).unwrap_err();
        assert!(err.starts_with("compute_major_mismatch_refused"), "{err}");
        assert!(check_compute_major(v18, 17).is_err());
        assert!(check_compute_major("localhost/test-compute@sha256:00", 18).is_ok());
    }
    use super::*;

    const DIGEST: &str = "sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3";

    #[test]
    fn the_pin_file_names_a_pinned_image_per_supported_major() {
        let images = pinned().expect("the pin file parses");
        assert!(images.contains_key(&17), "{images:?}");
        for image in images.values() {
            assert!(is_pinned(image), "{image}");
            assert!(!image.contains("neon-archive"), "{image}");
        }
    }

    #[test]
    fn overrides_are_checked() {
        let image = format!("ghcr.io/ostrium-labs/loams-postgres-compute-v17@{DIGEST}");
        assert_eq!(
            parse_override(&format!("17={image}")).expect("ok"),
            (17, image)
        );
        assert!(matches!(
            parse_override("17=ghcr.io/x/compute:latest"),
            Err(ImageError::NotPinned(_))
        ));
        assert!(matches!(
            parse_override(&format!("17=ghcr.io/example/neon-archive/compute@{DIGEST}")),
            Err(ImageError::Archive(_))
        ));
        assert!(matches!(
            parse_override("seventeen=x"),
            Err(ImageError::Malformed(_))
        ));
    }

    #[test]
    fn beta_and_maintenance_majors_have_images() {
        let image = |m: u32| {
            format!(
                "[images.compute-v{m}]\nrepository = \"ghcr.io/ostrium-labs/loams-postgres-compute-v{m}\"\ndigest = \"{DIGEST}\"\n"
            )
        };
        let text = format!(
            "[majors]\nsupported = [17]\nbeta = [18]\nmaintenance = [16]\ndefault = 17\n{}{}{}",
            image(16),
            image(17),
            image(18)
        );
        let images = from_pins(&text).expect("ok");
        assert_eq!(images.keys().copied().collect::<Vec<_>>(), [16, 17, 18]);
        assert_eq!(
            images[&18],
            format!("ghcr.io/ostrium-labs/loams-postgres-compute-v18@{DIGEST}")
        );
        // A beta major without its image is refused like a supported one.
        let text = format!(
            "[majors]\nsupported = [17]\nbeta = [18]\nmaintenance = []\ndefault = 17\n{}",
            image(17)
        );
        assert!(matches!(from_pins(&text), Err(ImageError::Pins(m)) if m.contains("compute-v18")));
    }

    #[test]
    fn the_pin_file_has_an_image_per_listed_major() {
        let images = pinned().expect("the pin file parses");
        for major in [
            loams_postgres::majors::SUPPORTED,
            loams_postgres::majors::BETA,
            loams_postgres::majors::MAINTENANCE,
        ]
        .concat()
        {
            assert!(images.contains_key(&major), "{major}: {images:?}");
        }
    }

    #[test]
    fn a_pin_file_missing_an_image_is_refused() {
        let text = "[majors]\nsupported = [17]\n";
        assert!(matches!(from_pins(text), Err(ImageError::Pins(_))));
    }
}
