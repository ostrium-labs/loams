//! The signed index as the operator and one-click deploy consume it (design
//! 86, OP2-B Task 3, D2609, D2624, D2626): both files verified under a trust
//! anchor before anything is parsed, then one listing version looked up,
//! refused when yanked, `disable`-revoked or holding a package its tier may
//! not use, and its `depends_on` resolved into an install order.

use std::collections::HashSet;

use semver::Version;

use crate::index::{self, Index, IndexEntry, IndexError, Tier};
use crate::listing_v2::{ListingV2, Package};
use crate::revocations::{Revocation, RevocationList, Severity};
use crate::sources::OFFICIAL_IDENTITY;
use crate::verify::{
    GITHUB_ACTIONS_ISSUER, Identity, KeyVerifier, SignatureError, SignatureVerifier,
    SigstoreVerifier,
};

/// The index file's name in errors (whichever index version it holds).
const INDEX_FILE: &str = "index.v2.json";
const REVOCATIONS_FILE: &str = "revocations.json";

/// How deep `depends_on` may go below the root (OP2-A's limit).
const MAX_DEPENDENCY_DEPTH: usize = 8;

/// An index, its revocation list and the Sigstore bundle beside each, as
/// fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexFiles {
    pub index: Vec<u8>,
    pub index_bundle: Vec<u8>,
    pub revocations: Vec<u8>,
    pub revocations_bundle: Vec<u8>,
}

/// Who must have signed an index (Decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustAnchor {
    /// A Sigstore keyless signer: certificate identity and OIDC issuer.
    Keyless(Identity),
    /// An ECDSA P-256 public key (PEM SPKI), checked against cosign
    /// `sign-blob --key` bundles: private indexes and the dry-run's key.
    PublicKey { pem: String },
}

impl TrustAnchor {
    /// The official index: `loams-marketplace`'s publish workflow on `main`,
    /// signed keylessly from GitHub Actions.
    pub fn official() -> TrustAnchor {
        TrustAnchor::Keyless(Identity {
            identity: OFFICIAL_IDENTITY.into(),
            issuer: GITHUB_ACTIONS_ISSUER.into(),
        })
    }
}

/// An index (v1 or v2) and revocation list whose signatures verified.
#[derive(Debug, Clone)]
pub struct SignedIndex {
    index: Index,
    revocations: RevocationList,
}

/// A listing version that may be installed.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedListing {
    pub listing: ListingV2,
    pub tier: Tier,
    pub listing_sha256: String,
    /// A `warn` revocation covering this version, for the caller to show.
    pub warning: Option<Revocation>,
}

impl VerifiedListing {
    pub fn listing(&self) -> &ListingV2 {
        &self.listing
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("{file}: {error}")]
    Signature {
        file: &'static str,
        error: SignatureError,
    },
    #[error(transparent)]
    Index(IndexError),
    #[error("no listing {0} in the index")]
    NoListing(String),
    #[error("{id} has no version {version} in the index")]
    NoVersion { id: String, version: String },
    #[error("{id}@{version} is yanked: {reason}")]
    Yanked {
        id: String,
        version: String,
        reason: String,
    },
    #[error("{id}@{version} is revoked: {reason}")]
    Revoked {
        id: String,
        version: String,
        reason: String,
    },
    #[error("{id} is {tier:?}, which may not install a {package} package")]
    PackageNotAllowed {
        id: String,
        tier: Tier,
        package: &'static str,
    },
    #[error("dependency {id}: {reason}")]
    Dependency { id: String, reason: String },
}

impl SignedIndex {
    /// Verifies both bundles under `anchor`, then parses the index (v1 or
    /// v2) and the revocation list. Nothing is parsed before both
    /// signatures verify (MK0 Task 4 ruling 2).
    pub fn verify(files: &IndexFiles, anchor: &TrustAnchor) -> Result<SignedIndex, VerifyError> {
        let (verifier, signer): (Box<dyn SignatureVerifier>, Identity) = match anchor {
            TrustAnchor::Keyless(identity) => (
                Box::new(SigstoreVerifier::production().map_err(|error| {
                    VerifyError::Signature {
                        file: INDEX_FILE,
                        error,
                    }
                })?),
                identity.clone(),
            ),
            TrustAnchor::PublicKey { pem } => (
                Box::new(
                    KeyVerifier::from_pem(pem).map_err(|error| VerifyError::Signature {
                        file: INDEX_FILE,
                        error,
                    })?,
                ),
                // A key has no identity; `KeyVerifier` ignores it.
                Identity {
                    identity: String::new(),
                    issuer: String::new(),
                },
            ),
        };
        for (file, bytes, bundle) in [
            (INDEX_FILE, &files.index, &files.index_bundle),
            (
                REVOCATIONS_FILE,
                &files.revocations,
                &files.revocations_bundle,
            ),
        ] {
            if bundle.is_empty() {
                return Err(VerifyError::Signature {
                    file,
                    error: SignatureError::Missing(format!("{file}.sigstore.json is empty")),
                });
            }
            verifier
                .verify_blob(bytes, bundle, &signer)
                .map_err(|error| VerifyError::Signature { file, error })?;
        }

        let index = index::parse(&files.index).map_err(VerifyError::Index)?;
        let revocations = RevocationList::parse(&files.revocations)
            .map_err(|e| VerifyError::Index(IndexError::Revocations(e.to_string())))?;
        Ok(SignedIndex { index, revocations })
    }

    pub fn revocations(&self) -> &RevocationList {
        &self.revocations
    }

    /// The commit of the index repository the index was built from.
    pub fn source_commit(&self) -> Option<&str> {
        self.index.source_commit.as_deref()
    }

    fn entry(&self, id: &str) -> Option<&IndexEntry> {
        self.index.listings.iter().find(|e| e.id == id)
    }
}

/// The package type's name, lowercase, as the listing writes it.
fn package_name(package: &Package) -> &'static str {
    match package {
        Package::Oci { .. } => "oci",
        Package::Helm { .. } => "helm",
        Package::Kustomize { .. } => "kustomize",
        Package::Wasm { .. } => "wasm",
        Package::Binary { .. } => "binary",
    }
}

/// Decision 5 (D2626): Community may use `oci` or no package; the others
/// need Verified or FirstParty.
fn package_allowed(tier: Tier, package: Option<&Package>) -> bool {
    match tier {
        Tier::FirstParty | Tier::Verified => true,
        Tier::Community => matches!(package, None | Some(Package::Oci { .. })),
    }
}

/// Finds `id@version` and checks that it may be installed: not yanked, not
/// `disable`-revoked, and its package allowed for its tier. A `warn`
/// revocation comes back as [`VerifiedListing::warning`].
pub fn verify_listing(
    index: &SignedIndex,
    id: &str,
    version: &Version,
) -> Result<VerifiedListing, VerifyError> {
    let entry = index
        .entry(id)
        .ok_or_else(|| VerifyError::NoListing(id.into()))?;
    let found = entry
        .versions
        .iter()
        .find(|v| v.typed.version == *version)
        .ok_or_else(|| VerifyError::NoVersion {
            id: id.into(),
            version: version.to_string(),
        })?;
    if let Some(reason) = &found.yanked {
        return Err(VerifyError::Yanked {
            id: id.into(),
            version: version.to_string(),
            reason: reason.clone(),
        });
    }
    let revocation = index.revocations.for_version(id, &version.to_string());
    if let Some(r) = revocation
        && r.severity == Severity::Disable
    {
        return Err(VerifyError::Revoked {
            id: id.into(),
            version: version.to_string(),
            reason: r.reason.clone(),
        });
    }
    let package = found.typed.package.as_ref();
    if !package_allowed(found.tier, package) {
        return Err(VerifyError::PackageNotAllowed {
            id: id.into(),
            tier: found.tier,
            package: package.map_or("none", package_name),
        });
    }
    Ok(VerifiedListing {
        listing: found.typed.clone(),
        tier: found.tier,
        listing_sha256: found.listing_sha256.clone(),
        warning: revocation.cloned(),
    })
}

/// `root`'s dependencies in install order (each after its own
/// dependencies), duplicates removed, `root` excluded. Each dependency is
/// the highest version that matches its range, is not a prerelease, and
/// that [`verify_listing`] accepts. A cycle, a missing listing, no
/// acceptable version, or a chain deeper than 8 is
/// [`VerifyError::Dependency`].
///
/// One version per id: a listing reached again (a diamond) keeps the version
/// chosen when it was first reached, which must match the later range too;
/// otherwise that is [`VerifyError::Dependency`] as well.
pub fn resolve_depends_on(
    index: &SignedIndex,
    root: &VerifiedListing,
) -> Result<Vec<VerifiedListing>, VerifyError> {
    let mut resolver = Resolver {
        index,
        stack: vec![root.listing.id.clone()],
        done: HashSet::new(),
        order: Vec::new(),
    };
    resolver.visit(&root.listing, 0)?;
    Ok(resolver.order)
}

struct Resolver<'a> {
    index: &'a SignedIndex,
    /// The ids on the current path, root first.
    stack: Vec<String>,
    /// Ids already in `order`.
    done: HashSet<String>,
    order: Vec<VerifiedListing>,
}

impl Resolver<'_> {
    fn visit(&mut self, listing: &ListingV2, depth: usize) -> Result<(), VerifyError> {
        for dep in &listing.depends_on {
            if self.stack.contains(&dep.id) {
                return Err(VerifyError::Dependency {
                    id: dep.id.clone(),
                    reason: format!("a cycle: {} -> {}", self.stack.join(" -> "), dep.id),
                });
            }
            if depth + 1 > MAX_DEPENDENCY_DEPTH {
                return Err(VerifyError::Dependency {
                    id: dep.id.clone(),
                    reason: format!("deeper than {MAX_DEPENDENCY_DEPTH} levels"),
                });
            }
            if self.done.contains(&dep.id) {
                // One version per id: the one chosen first must satisfy
                // every later range too.
                if let Some(earlier) = self.order.iter().find(|v| v.listing.id == dep.id)
                    && !dep.version.matches(&earlier.listing.version)
                {
                    return Err(VerifyError::Dependency {
                        id: dep.id.clone(),
                        reason: format!(
                            "{} needs {}, but {} was chosen",
                            listing.id, dep.version, earlier.listing.version
                        ),
                    });
                }
                continue;
            }
            let chosen = self.choose(&dep.id, &dep.version)?;
            self.stack.push(dep.id.clone());
            self.visit(&chosen.listing, depth + 1)?;
            self.stack.pop();
            self.done.insert(dep.id.clone());
            self.order.push(chosen);
        }
        Ok(())
    }

    fn choose(&self, id: &str, req: &semver::VersionReq) -> Result<VerifiedListing, VerifyError> {
        let entry = self
            .index
            .entry(id)
            .ok_or_else(|| VerifyError::Dependency {
                id: id.into(),
                reason: "no such listing in the index".into(),
            })?;
        let mut candidates: Vec<&Version> = entry
            .versions
            .iter()
            .map(|v| &v.typed.version)
            .filter(|v| v.pre.is_empty() && req.matches(v))
            .collect();
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        let mut refused = None;
        for version in candidates {
            match verify_listing(self.index, id, version) {
                Ok(found) => return Ok(found),
                Err(e) => {
                    refused.get_or_insert(e);
                }
            }
        }
        Err(VerifyError::Dependency {
            id: id.into(),
            reason: match refused {
                Some(e) => format!("no installable version matches {req} ({e})"),
                None => format!("no released version matches {req}"),
            },
        })
    }
}
