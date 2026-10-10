//! Coalesced chat snapshots. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use loams_agentd_doc::SessionDoc;
use loams_agentd_store::DocsStore;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Doc lineage epoch stamped on every thin-lineage snapshot: thin docs are
/// epoch 2, and an older stored doc is served as it is (`DocHost::open_local`).
pub(crate) const CHAT2_DOC_EPOCH: u32 = 2;

pub(crate) struct ChatPersistence {
    doc: Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    cursor: AtomicU64,
    generation: AtomicU64,
    saved: AtomicU64,
    snapshot_bytes: AtomicUsize,
    urgent: AtomicBool,
    write: Mutex<()>,
    wake: Option<mpsc::Sender<()>>,
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    before_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl ChatPersistence {
    pub(crate) fn new(
        doc: &Arc<SessionDoc>,
        store: Arc<DocsStore>,
        chat_id: String,
        cursor: u64,
    ) -> Arc<Self> {
        let verified = store.snapshot_cursor_verified(&chat_id).unwrap_or(false);
        let runtime = tokio::runtime::Handle::try_current().ok();
        let (tx, rx) = mpsc::channel(1);
        let this = Arc::new(Self {
            doc: Arc::downgrade(doc),
            store,
            chat_id,
            // Legacy cursors can have holes. Don't certify one before the
            // one-time repair has established a contiguous applied prefix.
            cursor: AtomicU64::new(if verified { cursor } else { 0 }),
            generation: AtomicU64::new(0),
            saved: AtomicU64::new(0),
            snapshot_bytes: AtomicUsize::new(0),
            urgent: AtomicBool::new(false),
            write: Mutex::new(()),
            wake: runtime.as_ref().map(|_| tx),
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            before_export: Mutex::new(None),
        });
        if let Some(runtime) = runtime {
            runtime.spawn(Self::run(Arc::downgrade(&this), rx));
        }
        this
    }

    pub(crate) fn snapshot_bytes(&self) -> usize {
        self.snapshot_bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn dirty(&self, immediate: bool) {
        self.generation.fetch_add(1, Ordering::Release);
        if immediate {
            self.urgent.store(true, Ordering::Release);
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        } else {
            // Offline synchronous tooling/tests have no executor to schedule.
            self.flush_sync();
        }
    }

    async fn run(weak: Weak<Self>, mut rx: mpsc::Receiver<()>) {
        while rx.recv().await.is_some() {
            let deadline = tokio::time::Instant::now() + SAVE_INTERVAL;
            loop {
                let Some(this) = weak.upgrade() else { return };
                let urgent = this.urgent.swap(false, Ordering::AcqRel);
                drop(this);
                if urgent {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    message = rx.recv() => if message.is_none() { return; },
                }
            }
            let Some(this) = weak.upgrade() else { return };
            // All chat persisters sharing this SQLite connection queue
            // asynchronously, BEFORE occupying a blocking-pool thread.
            let permit = this.store.snapshot_writer.clone().lock_owned().await;
            let job = this.clone();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                job.flush_sync();
            })
            .await
            {
                tracing::error!(%error, "chat snapshot worker failed");
            }
            if this.doc.strong_count() == 0 {
                return;
            }
            if this.saved.load(Ordering::Acquire) != this.generation.load(Ordering::Acquire) {
                // Includes failed writes and changes during export. Retry on
                // the next interval even if the document has gone quiet.
                if let Some(wake) = &this.wake {
                    let _ = wake.try_send(());
                }
            }
        }
    }

    pub(crate) fn flush_sync(&self) {
        let flush = || {
            let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
            let generation = self.generation.load(Ordering::Acquire);
            if generation == self.saved.load(Ordering::Acquire) {
                return;
            }
            let Some(doc) = self.doc.upgrade() else {
                return;
            };
            // Never read the cursor AFTER exporting: a concurrent import
            // could then label an older snapshot with a newer cursor.
            let cursor = self.cursor.load(Ordering::Acquire);
            #[cfg(test)]
            if let Some(hook) = self.before_export.lock().unwrap().take() {
                hook();
            }
            let result = doc
                .export_snapshot()
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                    self.store
                        .save_verified_snapshot_with_cursor(
                            &self.chat_id,
                            &bytes,
                            cursor,
                            CHAT2_DOC_EPOCH,
                        )
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(()) => {
                    #[cfg(test)]
                    self.writes.fetch_add(1, Ordering::Relaxed);
                    self.saved.store(generation, Ordering::Release);
                }
                Err(error) => {
                    tracing::warn!(chat = %self.chat_id, %error, "chat snapshot failed; retrying")
                }
            }
        };
        // Compatibility for synchronous shutdown/eviction and command APIs.
        // Async persisters already execute this on the blocking pool.
        if tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        {
            tokio::task::block_in_place(flush);
        } else {
            flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        Arc<SessionDoc>,
        Arc<DocsStore>,
        Arc<ChatPersistence>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("whale").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "whale".into(), 0);
        (dir, doc, store, persistence)
    }

    #[tokio::test]
    async fn commit_burst_coalesces_and_flush_bypasses_debounce() {
        let (_dir, doc, store, persistence) = fixture();
        for n in 1..=1000 {
            doc.doc().get_map("test").insert("n", n as i64).unwrap();
            doc.doc().commit();
            persistence.dirty(false);
        }
        assert_eq!(
            persistence.writes.load(Ordering::Relaxed),
            0,
            "no per-commit writes"
        );
        // The debounce schedules a blocking SQLite write; a loaded CI runner
        // may not finish that write within 200 ms of the debounce deadline.
        // Wait for completion, then still require the entire burst to coalesce.
        tokio::time::timeout(Duration::from_secs(10), async {
            while persistence.writes.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("debounced snapshot should finish");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1);
        let (bytes, _, epoch) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(epoch, CHAT2_DOC_EPOCH);
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(
            restored.get_map("test").get_deep_value(),
            doc.doc().get_map("test").get_deep_value()
        );
        persistence.dirty(true);
        tokio::time::timeout(Duration::from_millis(500), async {
            while persistence.writes.load(Ordering::Relaxed) != 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        doc.doc().get_map("test").insert("last", true).unwrap();
        doc.doc().commit();
        persistence.dirty(false);
        persistence.flush_sync(); // shutdown/eviction must not await the timer
        let (bytes, _, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(
            restored.get_map("test").get_deep_value(),
            doc.doc().get_map("test").get_deep_value()
        );
    }
}
