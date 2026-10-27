//! Consistency tokens (design §44 §7.4, D609; runtime contract R4 as amended
//! by SDKP §2.1).
//!
//! The encoding is the server's (`crates/loams-collection/src/token.rs`):
//!
//! ```text
//! v1:s<stream>/p<partition>@<offset>,s<stream>/p<partition>@<offset>,…
//! ```
//!
//! Each item is "the next offset after the write" for one `(stream,
//! partition)`. Merging two tokens keeps, per pair, the larger offset, so a
//! read held to the merge waits for both writes. Every number is canonical
//! decimal (no sign, no leading zeros), so a token has exactly one spelling,
//! and a malformed token is `invalid_argument` here, before any network call.
//!
//! The type is the server's, re-stated: the SDK cannot depend on
//! `loams-collection`, and `sdks/fixtures/tokens.json` is what keeps the two
//! agreeing (`rust_consistency_token_merge`).

use std::fmt;
use std::str::FromStr;

use connectrpc::ErrorCode as Code;

use crate::error::{ErrorKind, LoamsError};

const PREFIX: &str = "v1:";

/// One item of a token: the next offset after a write, for one partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenPosition {
    /// The stream id.
    pub stream: u64,
    /// The partition within the stream.
    pub partition: u32,
    /// The next offset after the write.
    pub offset: u64,
}

/// A parsed consistency token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ConsistencyToken(pub Vec<TokenPosition>);

impl ConsistencyToken {
    /// The token that names nothing: `v1:`.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parses the text form.
    ///
    /// # Errors
    ///
    /// An `invalid_argument` [`LoamsError`] naming the token when it is not the
    /// `v1:` form.
    pub fn parse(text: &str) -> Result<Self, LoamsError> {
        text.parse()
    }

    /// Sorted by `(stream, partition)`, one item per pair, the max offset kept.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.0.sort_unstable();
        let mut items: Vec<TokenPosition> = Vec::with_capacity(self.0.len());
        for item in self.0 {
            match items.last_mut() {
                Some(last) if (last.stream, last.partition) == (item.stream, item.partition) => {
                    *last = item;
                }
                _ => items.push(item),
            }
        }
        ConsistencyToken(items)
    }

    /// Both tokens' offsets; a pair both name keeps the larger. Normalised.
    #[must_use]
    pub fn merge(&self, other: &ConsistencyToken) -> ConsistencyToken {
        let mut items = self.0.clone();
        items.extend_from_slice(&other.0);
        ConsistencyToken(items).normalized()
    }

    /// The offset for `(stream, partition)`, the max if listed twice.
    #[must_use]
    pub fn offset(&self, stream: u64, partition: u32) -> Option<u64> {
        self.0
            .iter()
            .filter(|p| p.stream == stream && p.partition == partition)
            .map(|p| p.offset)
            .max()
    }

    /// Whether the two name the same positions once normalised.
    #[must_use]
    pub fn same_as(&self, other: &ConsistencyToken) -> bool {
        self.clone().normalized() == other.clone().normalized()
    }

    /// Whether the token names nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for ConsistencyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        for (i, p) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "s{}/p{}@{}", p.stream, p.partition, p.offset)?;
        }
        Ok(())
    }
}

impl FromStr for ConsistencyToken {
    type Err = LoamsError;

    fn from_str(text: &str) -> Result<Self, LoamsError> {
        let invalid = || {
            LoamsError::new(
                ErrorKind::InvalidArgument,
                Code::InvalidArgument,
                format!("invalid consistency token {text:?}"),
            )
            .with_client_reason(crate::error::INVALID_CONSISTENCY_TOKEN)
        };
        let body = text.strip_prefix(PREFIX).ok_or_else(invalid)?;
        if body.is_empty() {
            return Ok(ConsistencyToken::default());
        }
        body.split(',')
            .map(|item| parse_item(item).ok_or_else(invalid))
            .collect::<Result<_, _>>()
            .map(ConsistencyToken)
    }
}

/// `s<stream>/p<partition>@<offset>`.
fn parse_item(item: &str) -> Option<TokenPosition> {
    let (stream, rest) = item.strip_prefix('s')?.split_once("/p")?;
    let (partition, offset) = rest.split_once('@')?;
    Some(TokenPosition {
        stream: canonical_u64(stream)?,
        partition: u32::try_from(canonical_u64(partition)?).ok()?,
        offset: canonical_u64(offset)?,
    })
}

/// A decimal `u64` with no sign and no leading zeros (except `"0"`).
fn canonical_u64(s: &str) -> Option<u64> {
    let canonical =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    canonical.then(|| s.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_keeps_the_max_and_prints_normalised() {
        let a: ConsistencyToken = "v1:s7/p3@10,s1/p0@5".parse().expect("a token");
        let b: ConsistencyToken = "v1:s7/p3@4,s7/p1@2,s1/p0@9".parse().expect("a token");
        assert_eq!(a.merge(&b).to_string(), "v1:s1/p0@9,s7/p1@2,s7/p3@10");
        assert_eq!(a.merge(&b).offset(7, 3), Some(10));
    }
}
