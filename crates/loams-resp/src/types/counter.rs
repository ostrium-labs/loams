//! Counters on strings (plan KV2 Task 4): `INCRBY` and `INCRBYFLOAT`
//! semantics. A counter is a string holding a decimal number; its TTL is
//! kept across increments, as Valkey keeps it.

use loams_kv::TxnError;

use super::string::{self, body};
use super::{Kv, WrongType};
use crate::frame::{format_float_human, parse_int};

/// Why an increment was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncrError {
    WrongType,
    /// The stored value is not an integer (or not a float).
    NotANumber,
    /// The result would overflow `i64`.
    Overflow,
    /// The result would be NaN or infinite.
    NotFinite,
}

impl From<WrongType> for IncrError {
    fn from(_: WrongType) -> Self {
        IncrError::WrongType
    }
}

/// Adds `by` to the integer at `ukey` (0 when absent) and returns the new
/// value.
pub(crate) async fn incr_by<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    by: i64,
    now_ms: u64,
) -> Result<Result<i64, IncrError>, TxnError> {
    let (found, read) = string::meta(txn, db, ukey, now_ms).await?;
    let live = match read {
        Ok(live) => live,
        Err(w) => return Ok(Err(w.into())),
    };
    let current = match &live {
        Some(m) => match parse_int(&body(txn, db, ukey, m).await?) {
            Some(n) => n,
            None => return Ok(Err(IncrError::NotANumber)),
        },
        None => 0,
    };
    let Some(next) = current.checked_add(by) else {
        return Ok(Err(IncrError::Overflow));
    };
    let expire = live.as_ref().map_or(0, |m| m.expire_at_ms);
    string::write(
        txn,
        db,
        ukey,
        next.to_string().as_bytes(),
        expire,
        found.stored.as_ref(),
    )
    .await?;
    Ok(Ok(next))
}

/// A float as `INCRBYFLOAT` reads one: decimal or exponent notation with an
/// optional sign, no spaces, no NaN. `inf` parses (the sum then fails as
/// not finite).
pub fn parse_float(text: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(text).ok()?;
    if s.is_empty() || s.trim() != s {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    (!v.is_nan()).then_some(v)
}

/// Adds `by` to the float at `ukey` (0 when absent) and returns the new
/// value's text, which is also what is stored.
pub(crate) async fn incr_by_float<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    by: f64,
    now_ms: u64,
) -> Result<Result<String, IncrError>, TxnError> {
    let (found, read) = string::meta(txn, db, ukey, now_ms).await?;
    let live = match read {
        Ok(live) => live,
        Err(w) => return Ok(Err(w.into())),
    };
    let current = match &live {
        Some(m) => match parse_float(&body(txn, db, ukey, m).await?) {
            Some(n) => n,
            None => return Ok(Err(IncrError::NotANumber)),
        },
        None => 0.0,
    };
    let next = current + by;
    if !next.is_finite() {
        return Ok(Err(IncrError::NotFinite));
    }
    let text = format_float_human(next);
    let expire = live.as_ref().map_or(0, |m| m.expire_at_ms);
    string::write(
        txn,
        db,
        ukey,
        text.as_bytes(),
        expire,
        found.stored.as_ref(),
    )
    .await?;
    Ok(Ok(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, Kind, Meta};
    use crate::types::recording::Recording;

    #[test]
    fn floats_parse_like_valkey() {
        assert_eq!(parse_float(b"1.5"), Some(1.5));
        assert_eq!(parse_float(b"-2e3"), Some(-2000.0));
        assert_eq!(parse_float(b"+.5"), Some(0.5));
        assert_eq!(parse_float(b"inf"), Some(f64::INFINITY));
        assert_eq!(parse_float(b"nan"), None);
        assert_eq!(parse_float(b" 1"), None);
        assert_eq!(parse_float(b"1 "), None);
        assert_eq!(parse_float(b""), None);
        assert_eq!(parse_float(b"1x"), None);
    }

    #[tokio::test]
    async fn incr_overflow_and_parse_errors() {
        let mut txn = Recording::default();
        assert_eq!(incr_by(&mut txn, 0, b"n", 5, 0).await.expect("incr"), Ok(5));
        assert_eq!(
            incr_by(&mut txn, 0, b"n", -7, 0).await.expect("incr"),
            Ok(-2)
        );
        assert_eq!(
            incr_by(&mut txn, 0, b"n", i64::MIN, 0).await.expect("incr"),
            Err(IncrError::Overflow)
        );
        string::write(
            &mut txn,
            0,
            b"max",
            i64::MAX.to_string().as_bytes(),
            0,
            None,
        )
        .await
        .expect("a write");
        assert_eq!(
            incr_by(&mut txn, 0, b"max", 1, 0).await.expect("incr"),
            Err(IncrError::Overflow)
        );
        for bad in [&b"abc"[..], b" 1", b"01", b"1.0", b"", b"+1"] {
            string::write(&mut txn, 0, b"bad", bad, 0, None)
                .await
                .expect("a write");
            assert_eq!(
                incr_by(&mut txn, 0, b"bad", 1, 0).await.expect("incr"),
                Err(IncrError::NotANumber),
                "{bad:?}"
            );
        }
        let hash = Meta {
            kind: Kind::Hash,
            ver: 1,
            expire_at_ms: 0,
            trailer: vec![],
        };
        txn.put(&layout::meta_key(0, b"h"), hash.encode())
            .await
            .expect("a put");
        assert_eq!(
            incr_by(&mut txn, 0, b"h", 1, 0).await.expect("incr"),
            Err(IncrError::WrongType)
        );
    }

    #[tokio::test]
    async fn incr_keeps_the_ttl() {
        let mut txn = Recording::default();
        string::write(&mut txn, 0, b"n", b"10", 5_000, None)
            .await
            .expect("a write");
        assert_eq!(
            incr_by(&mut txn, 0, b"n", 1, 0).await.expect("incr"),
            Ok(11)
        );
        let m = crate::types::read_meta(&mut txn, 0, b"n")
            .await
            .expect("a read")
            .expect("the meta");
        assert_eq!(m.expire_at_ms, 5_000);
    }

    #[tokio::test]
    async fn incrbyfloat_formatting() {
        let mut txn = Recording::default();
        for (start, by, want) in [
            ("10.50", 0.1, "10.6"),
            ("5.0e3", 200.0, "5200"),
            ("3", 1.0, "4"),
            ("0", -1.5, "-1.5"),
            ("1", 1e20, "100000000000000000000"),
        ] {
            string::write(&mut txn, 0, b"f", start.as_bytes(), 0, None)
                .await
                .expect("a write");
            assert_eq!(
                incr_by_float(&mut txn, 0, b"f", by, 0)
                    .await
                    .expect("incrbyfloat"),
                Ok(want.to_owned()),
                "{start} + {by}"
            );
            assert_eq!(
                string::get(&mut txn, 0, b"f", 0).await.expect("a get"),
                Ok(Some(want.as_bytes().to_vec()))
            );
        }
        string::write(&mut txn, 0, b"f", b"1", 0, None)
            .await
            .expect("a write");
        assert_eq!(
            incr_by_float(&mut txn, 0, b"f", f64::INFINITY, 0)
                .await
                .expect("incrbyfloat"),
            Err(IncrError::NotFinite)
        );
        string::write(&mut txn, 0, b"f", b"x", 0, None)
            .await
            .expect("a write");
        assert_eq!(
            incr_by_float(&mut txn, 0, b"f", 1.0, 0)
                .await
                .expect("incrbyfloat"),
            Err(IncrError::NotANumber)
        );
    }
}
