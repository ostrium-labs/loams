//! The paged scanner (design §77 §6, D1642; KV1 plan KV0 Task 3).
//!
//! A [`Scanner`] walks `start..end` of a [`Txn`] or a [`Snap`] one page at
//! a time, forward or in reverse, holding at most one page: each page is
//! one bounded [`scan`](Txn::scan) (or [`scan_reverse`](Txn::scan_reverse))
//! from just past the last key it returned. So a scan of a large range
//! (a Redis `HGETALL` of a big hash, a CQL partition) costs a page of
//! memory, not the range.
//!
//! Every page reads at the same timestamp: the snapshot's, or the
//! transaction's start timestamp, with the transaction's own writes and
//! deletes overlaid as in [`Txn::scan`]. Both backends page the same way
//! (Ruling T3-1): the pinned `tikv-client`'s own scanner is forward only
//! (Ruling T0-8), and paging through the seam's bounded scans keeps the
//! buffer overlay and the error mapping of [`Txn::scan`].

use std::collections::VecDeque;

use crate::{Pair, Snap, Txn, TxnError};

/// What a [`Scanner`] reads from.
#[derive(Debug)]
enum Source<'a> {
    Txn(&'a mut Txn),
    Snap(&'a mut Snap),
}

/// A paged scan of `start..end` (`end` `None`: to the end of the root),
/// from [`Txn::scanner`], [`Txn::scanner_reverse`], [`Snap::scanner`] or
/// [`Snap::scanner_reverse`]. Pages hold at most `page` pairs.
#[derive(Debug)]
pub struct Scanner<'a> {
    source: Source<'a>,
    /// The next page's inclusive start.
    start: Vec<u8>,
    /// The next page's exclusive end.
    end: Option<Vec<u8>>,
    page: usize,
    reverse: bool,
    /// The range is exhausted: no page is fetched again.
    done: bool,
    /// The current page's pairs not yet returned by [`next`](Self::next).
    buffered: VecDeque<Pair>,
}

impl<'a> Scanner<'a> {
    fn new(
        source: Source<'a>,
        start: &[u8],
        end: Option<&[u8]>,
        page: usize,
        reverse: bool,
    ) -> Self {
        Scanner {
            source,
            start: start.to_vec(),
            end: end.map(<[u8]>::to_vec),
            page: page.max(1),
            reverse,
            done: false,
            buffered: VecDeque::new(),
        }
    }

    /// The page size (at least 1).
    pub fn page_size(&self) -> usize {
        self.page
    }

    /// How many pairs of the current page [`next`](Self::next) has not
    /// returned yet: never more than [`page_size`](Self::page_size).
    pub fn buffered(&self) -> usize {
        self.buffered.len()
    }

    /// The next page, in the scan's order: `None` when the range is
    /// exhausted. Pairs of the current page that [`next`](Self::next) has
    /// not returned are returned first, as a page of their own.
    pub async fn next_page(&mut self) -> Result<Option<Vec<Pair>>, TxnError> {
        if !self.buffered.is_empty() {
            return Ok(Some(self.buffered.drain(..).collect()));
        }
        let page = self.fetch().await?;
        Ok((!page.is_empty()).then_some(page))
    }

    /// The next pair, in the scan's order: `None` when the range is
    /// exhausted. Fetches a page when the current one is used up.
    pub async fn next(&mut self) -> Result<Option<Pair>, TxnError> {
        if self.buffered.is_empty() {
            self.buffered = self.fetch().await?.into();
        }
        Ok(self.buffered.pop_front())
    }

    /// Reads the next page and moves the bound past it.
    async fn fetch(&mut self) -> Result<Vec<Pair>, TxnError> {
        if self.done {
            return Ok(Vec::new());
        }
        let (start, end, limit) = (self.start.as_slice(), self.end.as_deref(), self.page);
        let pairs = match (&mut self.source, self.reverse) {
            (Source::Txn(t), false) => t.scan(start, end, limit).await?,
            (Source::Txn(t), true) => t.scan_reverse(start, end, limit).await?,
            (Source::Snap(s), false) => s.scan(start, end, limit).await?,
            (Source::Snap(s), true) => s.scan_reverse(start, end, limit).await?,
        };
        match pairs.last() {
            Some((last, _)) if pairs.len() == self.page => {
                if self.reverse {
                    // Below the last key: it is the next page's exclusive end.
                    self.end = Some(last.clone());
                } else {
                    // The smallest key after the last one.
                    let mut next = last.clone();
                    next.push(0x00);
                    self.start = next;
                }
            }
            // A short page is the range's last.
            _ => self.done = true,
        }
        Ok(pairs)
    }
}

impl Txn {
    /// A paged scan of `start..end` in key order (`end` `None`: to the end
    /// of the root), at the start timestamp, with this transaction's own
    /// writes overlaid; pages of at most `page` pairs (at least 1).
    pub fn scanner(&mut self, start: &[u8], end: Option<&[u8]>, page: usize) -> Scanner<'_> {
        Scanner::new(Source::Txn(self), start, end, page, false)
    }

    /// Like [`scanner`](Self::scanner), in descending key order.
    pub fn scanner_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        page: usize,
    ) -> Scanner<'_> {
        Scanner::new(Source::Txn(self), start, end, page, true)
    }
}

impl Snap {
    /// A paged scan of `start..end` in key order (`end` `None`: to the end
    /// of the root) at the snapshot's timestamp; pages of at most `page`
    /// pairs (at least 1).
    pub fn scanner(&mut self, start: &[u8], end: Option<&[u8]>, page: usize) -> Scanner<'_> {
        Scanner::new(Source::Snap(self), start, end, page, false)
    }

    /// Like [`scanner`](Self::scanner), in descending key order.
    pub fn scanner_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        page: usize,
    ) -> Scanner<'_> {
        Scanner::new(Source::Snap(self), start, end, page, true)
    }
}
