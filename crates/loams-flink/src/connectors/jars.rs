//! The pinned connector-jar bundle (D1986).
//!
//! The canonical list is `deploy/flink/connector-jars.toml`, which the image
//! build reads (`deploy/flink/Containerfile`). [`BUNDLE`] is the same list
//! in Rust, for `loams flink jars` and for the controller to label clusters;
//! a unit test checks the two are equal.

use serde::{Deserialize, Serialize};

/// The Flink release the bundle is built for.
pub const FLINK_VERSION: &str = "2.2.1";
/// The official image, by manifest-list digest.
pub const FLINK_IMAGE: &str = "docker.io/library/flink:2.2.1-scala_2.12-java21@sha256:521951efbfbb0cec74c2826446ce1d20a051bff18e6d3716a6affe97ddec692a";
/// Bumped whenever a jar changes.
pub const BUNDLE_REVISION: u32 = 1;
/// Where the bundle is published.
pub const IMAGE_REPOSITORY: &str = "ghcr.io/ostrium-labs/loams-flink";
/// Maven Central.
pub const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2";

/// One jar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConnectorJar {
    pub group: &'static str,
    pub artifact: &'static str,
    pub version: &'static str,
    pub sha256: &'static str,
    pub license: &'static str,
    pub purpose: &'static str,
}

impl ConnectorJar {
    pub fn file_name(&self) -> String {
        format!("{}-{}.jar", self.artifact, self.version)
    }

    pub fn url(&self, base: &str) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            base.trim_end_matches('/'),
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.file_name()
        )
    }
}

/// The bundle, in image order.
pub const BUNDLE: &[ConnectorJar] = &[
    ConnectorJar {
        group: "org.apache.iceberg",
        artifact: "iceberg-flink-runtime-2.2",
        version: "1.12.0",
        sha256: "c79bbee8792b1115c321e4be99fab3fb36db0bf8fa2bba24b481e6da14416de3",
        license: "Apache-2.0",
        purpose: "Iceberg catalog and table connector (REST catalog on Lakekeeper)",
    },
    ConnectorJar {
        group: "org.apache.iceberg",
        artifact: "iceberg-aws-bundle",
        version: "1.12.0",
        sha256: "dd7e49ffa4ac9b215bed846581991259e3aec9bb2fa08d6cec029c46cd66d957",
        license: "Apache-2.0",
        purpose: "S3FileIO for Loams object storage",
    },
    ConnectorJar {
        group: "org.apache.flink",
        artifact: "flink-sql-connector-kafka",
        version: "5.0.0-2.2",
        sha256: "5605c691d11a501382c383fecba37a7a552467da5ab7ba904ef5d6f3d62c5616",
        license: "Apache-2.0",
        purpose: "kafka and upsert-kafka, for Loams streams through the Iggy Kafka gateway",
    },
    ConnectorJar {
        group: "org.apache.flink",
        artifact: "flink-connector-jdbc-core",
        version: "4.1.0-2.2",
        sha256: "f8357ab799095e6486315db4019edf63ced762bc8c2eaa63614c7b962755564b",
        license: "Apache-2.0",
        purpose: "JDBC connector and catalog",
    },
    ConnectorJar {
        group: "org.apache.flink",
        artifact: "flink-connector-jdbc-postgres",
        version: "4.1.0-2.2",
        sha256: "ae6da5769d932123606f1b9e03d90d57cde9c1d0144df6ffa64a9701afb86d6b",
        license: "Apache-2.0",
        purpose: "The JDBC catalog's Postgres dialect",
    },
    ConnectorJar {
        group: "org.postgresql",
        artifact: "postgresql",
        version: "42.7.14",
        sha256: "73914527305a40cce504b0d3d90b23caf565136912d607ae8ae7c5895512332c",
        license: "BSD-2-Clause",
        purpose: "PostgreSQL JDBC driver",
    },
    ConnectorJar {
        group: "org.apache.flink",
        artifact: "flink-sql-connector-postgres-cdc",
        version: "3.7.0-2.2",
        sha256: "25cf92be4800609126c91c1951acae5a8d138ab0e6e57dcf7aef6f1c0dce13ee",
        license: "Apache-2.0",
        purpose: "Postgres CDC (flink-cdc, pgoutput)",
    },
    ConnectorJar {
        group: "org.apache.hadoop",
        artifact: "hadoop-client-api",
        version: "3.4.2",
        sha256: "607c0299f4df33013bd0a0f37c8cf255c635bbaddd247b8fec68c210cc17d19f",
        license: "Apache-2.0",
        purpose: "Hadoop Configuration, which Iceberg's Flink catalog loads",
    },
    ConnectorJar {
        group: "org.apache.hadoop",
        artifact: "hadoop-client-runtime",
        version: "3.4.2",
        sha256: "18d071c7f72f8a946677b398a57af9c7afa2dd41a81023c310f99373fec200b3",
        license: "Apache-2.0",
        purpose: "Shaded Hadoop runtime for hadoop-client-api",
    },
];

/// The image tag the bundle builds: `<repo>:<flink>-loams<revision>`.
pub fn image_ref() -> String {
    format!("{IMAGE_REPOSITORY}:{FLINK_VERSION}-loams{BUNDLE_REVISION}")
}

/// The manifest file's shape, for checks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub flink: ManifestFlink,
    #[serde(rename = "jar")]
    pub jars: Vec<ManifestJar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestFlink {
    pub version: String,
    pub image: String,
    pub bundle: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestJar {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub sha256: String,
    pub license: String,
    pub purpose: String,
}

/// Licences the bundle may carry (D359's gate: no AGPL, BSL, SSPL, ELv2).
pub const ALLOWED_LICENSES: &[&str] = &["Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "MIT"];

/// Check a parsed manifest: sha256 shape, allowed licences, no duplicate
/// artifacts, the image pinned by digest.
pub fn check_manifest(m: &Manifest) -> Result<(), String> {
    if !m.flink.image.contains("@sha256:") {
        return Err(format!(
            "flink image {:?} is not pinned by digest",
            m.flink.image
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for j in &m.jars {
        if j.sha256.len() != 64
            || !j
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(format!(
                "{}: sha256 is not 64 lowercase hex digits",
                j.artifact
            ));
        }
        if !ALLOWED_LICENSES.contains(&j.license.as_str()) {
            return Err(format!(
                "{}: licence {} is not allowed",
                j.artifact, j.license
            ));
        }
        if !seen.insert(j.artifact.as_str()) {
            return Err(format!("{}: listed twice", j.artifact));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/flink/connector-jars.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    }

    #[test]
    fn manifest_matches_bundle() {
        let m = manifest();
        assert_eq!(m.flink.version, FLINK_VERSION);
        assert_eq!(m.flink.image, FLINK_IMAGE);
        assert_eq!(m.flink.bundle, BUNDLE_REVISION);
        let from_file: Vec<_> = m
            .jars
            .iter()
            .map(|j| {
                (
                    j.group.as_str(),
                    j.artifact.as_str(),
                    j.version.as_str(),
                    j.sha256.as_str(),
                    j.license.as_str(),
                    j.purpose.as_str(),
                )
            })
            .collect();
        let from_code: Vec<_> = BUNDLE
            .iter()
            .map(|j| {
                (
                    j.group, j.artifact, j.version, j.sha256, j.license, j.purpose,
                )
            })
            .collect();
        assert_eq!(
            from_file, from_code,
            "deploy/flink/connector-jars.toml and BUNDLE differ"
        );
    }

    #[test]
    fn manifest_passes_checks() {
        assert_eq!(check_manifest(&manifest()), Ok(()));
    }

    #[test]
    fn bundle_covers_every_connector_the_builders_emit() {
        let artifacts: Vec<_> = BUNDLE.iter().map(|j| j.artifact).collect();
        for needed in [
            "iceberg-flink-runtime-2.2",
            "iceberg-aws-bundle",
            "flink-sql-connector-kafka",
            "flink-connector-jdbc-core",
            "flink-connector-jdbc-postgres",
            "postgresql",
            "flink-sql-connector-postgres-cdc",
        ] {
            assert!(artifacts.contains(&needed), "{needed}");
        }
        // Connector artifacts built for another Flink line would not load.
        for j in BUNDLE.iter().filter(|j| j.group == "org.apache.flink") {
            assert!(j.version.ends_with("-2.2"), "{} {}", j.artifact, j.version);
        }
    }

    #[test]
    fn urls() {
        let j = BUNDLE[5];
        assert_eq!(
            j.url(MAVEN_CENTRAL),
            "https://repo1.maven.org/maven2/org/postgresql/postgresql/42.7.14/postgresql-42.7.14.jar"
        );
        assert_eq!(image_ref(), "ghcr.io/ostrium-labs/loams-flink:2.2.1-loams1");
    }
}
