//! Injected faults of an embedded store file (feature `faults`): the file
//! is opened through a redb storage backend that forwards to redb's own file
//! backend and fails the next `n` syncs on demand, so a commit's write
//! transaction fails as on a real I/O error; and a committer panic.

use std::fs::OpenOptions;
use std::io;
use std::ops::Bound;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use redb::backends::FileBackend;
use redb::{BackendError, Database, DatabaseError, StorageBackend, StorageError};

/// The switch of a file's injected faults.
#[derive(Debug, Default)]
pub(crate) struct Switch {
    fail_syncs: AtomicU32,
    panic_next_group: AtomicBool,
    held: Mutex<bool>,
    released: Condvar,
}

impl Switch {
    /// Holds the committer before it drains its next group.
    pub(crate) fn hold(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    /// Lets the committer go on.
    pub(crate) fn release(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.released.notify_all();
    }

    /// Waits while the committer is held.
    pub(crate) fn wait_released(&self) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        while *held {
            held = self.released.wait(held).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Makes the committer panic in its next group, after it allocates a
    /// commit timestamp.
    pub(crate) fn panic_next_group(&self) {
        self.panic_next_group.store(true, Ordering::SeqCst);
    }

    pub(crate) fn take_panic(&self) -> bool {
        self.panic_next_group.swap(false, Ordering::SeqCst)
    }

    /// Fails the next `n` syncs.
    pub(crate) fn fail_syncs(&self, n: u32) {
        self.fail_syncs.store(n, Ordering::SeqCst);
    }

    fn take(&self) -> bool {
        self.fail_syncs
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }
}

#[derive(Debug)]
struct Faulty {
    inner: FileBackend,
    switch: Arc<Switch>,
}

impl StorageBackend for Faulty {
    fn len(&self) -> Result<u64, io::Error> {
        self.inner.len()
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), io::Error> {
        self.inner.read(offset, out)
    }

    fn set_len(&self, len: u64) -> Result<(), io::Error> {
        self.inner.set_len(len)
    }

    fn sync_data(&self) -> Result<(), io::Error> {
        if self.switch.take() {
            return Err(io::Error::other("injected sync failure"));
        }
        self.inner.sync_data()
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
        self.inner.write(offset, data)
    }

    fn close(&self) -> Result<(), io::Error> {
        self.inner.close()
    }

    fn try_lock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<bool, BackendError> {
        self.inner.try_lock_range(start, end)
    }

    fn try_lock_shared_range(
        &self,
        start: Bound<u64>,
        end: Bound<u64>,
    ) -> Result<bool, BackendError> {
        self.inner.try_lock_shared_range(start, end)
    }

    fn lock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<(), BackendError> {
        self.inner.lock_range(start, end)
    }

    fn lock_shared_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<(), BackendError> {
        self.inner.lock_shared_range(start, end)
    }

    fn unlock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<(), BackendError> {
        self.inner.unlock_range(start, end)
    }

    fn query_lock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<bool, BackendError> {
        self.inner.query_lock_range(start, end)
    }
}

/// Opens (or creates) the file at `path` through the faulty backend.
pub(crate) fn create(path: &Path, switch: Arc<Switch>) -> Result<Database, DatabaseError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| DatabaseError::Storage(StorageError::Io(e)))?;
    Database::builder().create_with_backend(Faulty {
        inner: FileBackend::new(file)?,
        switch,
    })
}
