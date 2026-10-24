//! The scenarios the binary knows.

use futures::future::BoxFuture;
use loams_kv::{Store, TxnError, TxnOptions};

use crate::{Report, Scenario};

/// Every scenario, in `list` order.
pub fn all() -> Vec<Box<dyn Scenario>> {
    vec![Box::new(CasAndScan)]
}

/// The scenario called `name`.
pub fn find(name: &str) -> Option<Box<dyn Scenario>> {
    all().into_iter().find(|s| s.name() == name)
}

/// Compare-and-set a few keys from one transaction's read, then scan them
/// forward and backward: the smallest check that two backends agree on
/// reads, conditional writes, deletes and key order.
#[derive(Debug, Clone, Copy)]
pub struct CasAndScan;

/// One compare-and-set: its name, the key, the expected and the new value.
type Step = (
    &'static str,
    &'static [u8],
    Option<&'static [u8]>,
    Option<&'static [u8]>,
);

/// Sets `key` to `new` (deletes it for `None`) when it holds `expected`, in
/// one transaction: whether it matched.
async fn cas(
    store: &Store,
    key: &'static [u8],
    expected: Option<&'static [u8]>,
    new: Option<&'static [u8]>,
) -> Result<bool, TxnError> {
    store
        .run(TxnOptions::new("gw_test.cas"), move |txn| {
            Box::pin(async move {
                if txn.get(key).await?.as_deref() != expected {
                    return Ok(false);
                }
                match new {
                    Some(v) => txn.put(key, v.to_vec()).await?,
                    None => txn.delete(key).await?,
                }
                Ok(true)
            })
        })
        .await
        .map(|c| c.value)
}

impl Scenario for CasAndScan {
    fn name(&self) -> &'static str {
        "cas-and-scan"
    }

    fn about(&self) -> &'static str {
        "conditional writes and deletes, then forward and reverse scans"
    }

    fn run<'a>(&'a self, store: &'a Store) -> BoxFuture<'a, Report> {
        Box::pin(async move {
            let mut report = Report::new();
            let steps: [Step; 7] = [
                ("create a", b"a", None, Some(b"1")),
                ("create a again", b"a", None, Some(b"x")),
                ("swap a", b"a", Some(b"1"), Some(b"2")),
                ("stale swap a", b"a", Some(b"1"), Some(b"3")),
                ("create b", b"b", None, Some(b"1")),
                ("create c", b"c\x00", None, Some(b"1")),
                ("delete b", b"b", Some(b"1"), None),
            ];
            for (i, (name, key, expected, new)) in steps.into_iter().enumerate() {
                report.fact(format!("{i}. {name}"), cas(store, key, expected, new).await);
            }
            let scans = store
                .run(TxnOptions::new("gw_test.scan"), |txn| {
                    Box::pin(async move {
                        let forward = txn.scan(b"", None, 10).await?;
                        let reverse = txn.scan_reverse(b"", None, 10).await?;
                        let bounded = txn.scan(b"a\x00", Some(b"c\x00"), 10).await?;
                        Ok((forward, reverse, bounded))
                    })
                })
                .await
                .map(|c| c.value);
            match scans {
                Ok((forward, reverse, bounded)) => {
                    report.fact("scan forward", forward);
                    report.fact("scan reverse", reverse);
                    report.fact("scan a\\x00..c\\x00", bounded);
                }
                Err(e) => {
                    report.fact("scan error", e);
                }
            }
            report
        })
    }
}
