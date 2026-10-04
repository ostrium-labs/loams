//! Pagination (design §44 §7.4; runtime contract R6).
//!
//! AIP-158: `page_size` and `page_token` in, `next_page_token` out. The facade
//! exposes both the raw page call and an iterator that follows the tokens to the
//! end and yields **items**, not pages. One function serves every paged RPC,
//! because the binding names the two fields ([`crate::facade::Pagination`], from
//! `FacadeOptions.pagination`) rather than each list RPC carrying its own copy.
//!
//! **No RPC is paged yet.** `loams.collection.v1.CollectionService/ListCollections`
//! arrives with API1 Task 2, and with it the per-call `list_all` alias, which
//! needs a generated signature to hang on. What is here is the iterator itself,
//! driven by the two traits a generated binding would implement, and
//! `rust_pagination_iterator` pins it. The end-to-end half of that test is a
//! deliberate skip, not an omission: a fixture for an RPC the server does not
//! serve would test the stub rather than the SDK.
//!
//! In Rust the iterator is a [`futures::Stream`] rather than an async generator:
//! `while let Some(item) = loams.collections.list_all(..).next().await` is how
//! the language consumes an async sequence, and `futures::StreamExt` is what a
//! Rust caller already has.

use std::future::Future;
use std::pin::Pin;

use futures::{Stream, StreamExt, TryStreamExt, stream};

use crate::error::LoamsError;

/// The boxed future one page request returns.
pub type PageFuture<Resp> = Pin<Box<dyn Future<Output = Result<Resp, LoamsError>> + Send>>;

/// A request message a paged call can be given a page token in.
pub trait PageRequest: Clone {
    /// The request that asks for the page after `token` — the one the previous
    /// response carried, verbatim. The first page is the request unchanged.
    fn with_page_token(&self, token: &str) -> Self;
}

/// A response message of a paged call.
pub trait PageResponse {
    /// One item of the page.
    type Item: Clone;
    /// The page's items. A response that omits an empty repeated field is legal
    /// proto3 and yields nothing rather than failing.
    fn items(&self) -> &[Self::Item];
    /// The token for the next page, or `None` at the end.
    fn next_page_token(&self) -> Option<&str>;
}

/// A paged call: the raw page fetch, with the runtime already bound to it.
///
/// A generated binding builds one of these from its method and the two traits,
/// which is what makes the iterator one function rather than one per list RPC.
pub struct PagedCall<Req, Resp> {
    fetch: std::sync::Arc<dyn Fn(Req) -> PageFuture<Resp> + Send + Sync>,
}

/// Cloned explicitly rather than derived: a derive would demand `Req: Clone` and
/// `Resp: Clone`, which the paged traits do not require of the response.
impl<Req, Resp> Clone for PagedCall<Req, Resp> {
    fn clone(&self) -> Self {
        PagedCall {
            fetch: std::sync::Arc::clone(&self.fetch),
        }
    }
}

impl<Req, Resp> std::fmt::Debug for PagedCall<Req, Resp> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PagedCall").finish_non_exhaustive()
    }
}

impl<Req, Resp> PagedCall<Req, Resp> {
    /// Wraps a page fetch.
    #[must_use]
    pub fn new<F, Fut>(fetch: F) -> Self
    where
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, LoamsError>> + Send + 'static,
    {
        PagedCall {
            fetch: std::sync::Arc::new(move |request| Box::pin(fetch(request))),
        }
    }

    /// One page, the raw call. The iterator below is built on this.
    pub async fn page(&self, request: Req) -> Result<Resp, LoamsError> {
        (self.fetch)(request).await
    }
}

/// Every item of a paged call, following the tokens to the end (D617's "the
/// paging iterator").
///
/// The caller gets **items**, not pages. The raw page call is still available
/// through [`PagedCall::page`], so a caller that wants pages, or wants to stop
/// after one, does not have to use this.
///
/// A binding whose `FacadeOptions.pagination` names no fields is **not** a paged
/// call, and building one for it is refused rather than looping once against an
/// RPC that ignores `page_token`.
pub fn paginate<Req, Resp>(
    call: &PagedCall<Req, Resp>,
    request: Req,
) -> impl Stream<Item = Result<Resp::Item, LoamsError>>
where
    Req: PageRequest + Send + 'static,
    Resp: PageResponse + Send + 'static,
{
    // `(base, next token)`; `None` is the end of the iteration, which is what
    // stops the request after a response with no `next_page_token`.
    let pages = stream::unfold(Some((request, None::<String>)), move |state| {
        let call = call.clone();
        async move {
            let (base, token) = state?;
            let request = match &token {
                Some(token) => base.with_page_token(token),
                None => base.clone(),
            };
            // A failed page is yielded as one failed *item* and ends the
            // iteration: retrying here would loop against an RPC the caller has
            // already been told has failed, and the retry policy belongs to the
            // call, not to the iterator.
            let page = match call.page(request).await {
                Ok(page) => page,
                Err(error) => return Some((Err(error), None)),
            };
            let next = page
                .next_page_token()
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
            let items: Vec<Resp::Item> = page.items().to_vec();
            Some((
                Ok::<Vec<Resp::Item>, LoamsError>(items),
                next.map(|next| (base, Some(next))),
            ))
        }
    });
    // One `Result` per item, not per page: a caller iterating items sees a
    // failure at the item that failed rather than losing a whole page's worth.
    pages
        .map(|page| page.map(|items| stream::iter(items.into_iter().map(Ok))))
        .try_flatten()
}

/// Refuses to build a paging iterator for a call that is not paged.
///
/// `loams.paginate` names a module and a call, and this is what it says when
/// the binding it found carries no `FacadeOptions.pagination` — which is the
/// case for every call today, because `ListCollections` has not landed (API1
/// Task 2).
///
/// # Errors
///
/// Returns a [`LoamsError`] naming the call.
pub fn not_paged(rpc: &str) -> LoamsError {
    LoamsError::internal(format!(
        "{rpc} is not a paged call: its facade options name no pagination"
    ))
    .with_rpc(rpc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// A response shaped like `ListCollectionsResponse`.
    #[derive(Debug, Clone)]
    struct Page {
        collections: Vec<String>,
        next: String,
    }

    impl PageResponse for Page {
        type Item = String;
        fn items(&self) -> &[String] {
            &self.collections
        }
        fn next_page_token(&self) -> Option<&str> {
            Some(self.next.as_str())
        }
    }

    /// A request shaped like `ListCollectionsRequest`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Req {
        namespace: String,
        page_token: String,
    }

    impl PageRequest for Req {
        fn with_page_token(&self, token: &str) -> Self {
            Req {
                namespace: self.namespace.clone(),
                page_token: token.to_owned(),
            }
        }
    }

    /// A server that pages: the pages in order, recording what it was asked.
    fn paged(
        pages: Vec<Page>,
    ) -> (
        PagedCall<Req, Page>,
        std::sync::Arc<std::sync::Mutex<Vec<Req>>>,
    ) {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&asked);
        let next = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let call = PagedCall::new(move |request: Req| {
            let seen = std::sync::Arc::clone(&seen);
            let next = std::sync::Arc::clone(&next);
            let pages = pages.clone();
            async move {
                seen.lock().expect("not poisoned").push(request);
                let at = {
                    let mut next = next.lock().expect("not poisoned");
                    let at = *next;
                    *next += 1;
                    at
                };
                Ok::<Page, LoamsError>(pages.get(at).cloned().unwrap_or(Page {
                    collections: vec![],
                    next: String::new(),
                }))
            }
        });
        (call, asked)
    }

    #[tokio::test]
    async fn the_iterator_follows_the_tokens_and_yields_items_not_pages() {
        let (call, asked) = paged(vec![
            Page {
                collections: vec!["col_1".into(), "col_2".into()],
                next: "p2".into(),
            },
            Page {
                collections: vec!["col_3".into()],
                next: "p3".into(),
            },
            Page {
                collections: vec!["col_4".into()],
                next: String::new(),
            },
        ]);
        let items: Vec<String> = paginate(
            &call,
            Req {
                namespace: "acme".into(),
                page_token: String::new(),
            },
        )
        .map(|item| item.expect("a page"))
        .collect()
        .await;
        assert_eq!(items, ["col_1", "col_2", "col_3", "col_4"]);
        let asked = asked.lock().expect("not poisoned").clone();
        assert_eq!(asked.len(), 3, "three requests, one per page");
        // The first carries no token; each later one carries the previous
        // response's token, verbatim.
        assert_eq!(
            asked[0],
            Req {
                namespace: "acme".into(),
                page_token: String::new()
            }
        );
        assert_eq!(asked[1].page_token, "p2");
        assert_eq!(asked[2].page_token, "p3");
        assert!(asked.iter().all(|request| request.namespace == "acme"));
    }

    #[tokio::test]
    async fn one_page_makes_one_request() {
        let (call, asked) = paged(vec![Page {
            collections: vec!["col_1".into()],
            next: String::new(),
        }]);
        let items: Vec<String> = paginate(
            &call,
            Req {
                namespace: "acme".into(),
                page_token: String::new(),
            },
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|item| item.expect("an item"))
        .collect();
        assert_eq!(items, ["col_1"]);
        assert_eq!(
            asked.lock().expect("not poisoned").len(),
            1,
            "no second request"
        );
    }

    #[tokio::test]
    async fn a_page_whose_items_field_is_absent_yields_nothing_and_keeps_going() {
        // A server that omits an empty repeated field is legal proto3. Throwing
        // would break a caller over a message the server is allowed to send.
        #[derive(Debug, Clone)]
        struct Sparse(Vec<String>);
        impl PageResponse for Sparse {
            type Item = String;
            fn items(&self) -> &[String] {
                &self.0
            }
            fn next_page_token(&self) -> Option<&str> {
                Some(if self.0.is_empty() { "" } else { "p2" })
            }
        }
        // A first page with no items and no token, so the iteration ends without
        // a second request: an absent repeated field is not an error.
        let asked = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let counter = std::sync::Arc::clone(&asked);
        let call = PagedCall::new(move |request: Req| {
            let counter = std::sync::Arc::clone(&counter);
            async move {
                *counter.lock().expect("not poisoned") += 1;
                Ok::<Sparse, LoamsError>(Sparse(vec![request.page_token]))
            }
        });
        let items: Vec<String> = paginate(
            &call,
            Req {
                namespace: "acme".into(),
                page_token: String::new(),
            },
        )
        .map(|item| item.expect("an item"))
        .collect()
        .await;
        assert!(items.is_empty());
        assert_eq!(*asked.lock().expect("not poisoned"), 1);
    }

    #[test]
    fn a_call_with_no_pagination_options_is_refused_by_name() {
        let error = not_paged("loams.instance.v1.InstanceService/GetInstance");
        assert!(error.to_string().contains("not a paged call"));
        assert_eq!(
            error.rpc(),
            Some("loams.instance.v1.InstanceService/GetInstance")
        );
    }
}
