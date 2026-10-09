//! `loams-live-js-alloc`: the counting allocator behind Loams Live's
//! JavaScript memory limit (LV1 plan row T3-6, fix round 1 I2).
//!
//! QuickJS's own memory limit makes an allocation fail with a catchable
//! `InternalError: out of memory`, so a handler that catches it keeps
//! running. Instead, a runtime allocates through a [`LimitedAllocator`],
//! which counts every block against its [`MemoryMeter`]'s limit and, past
//! it, refuses the allocation and raises the meter's flag. The runtime's
//! interrupt handler sees the flag and stops the call uncatchably; the
//! host then classifies the call as out of memory by the flag, never by an
//! error's text, and replaces the runtime.
//!
//! Past the limit an allocation succeeds only out of the *grace* the
//! interrupt handler grants right before it stops the call
//! ([`MemoryMeter::top_up_grace`]): QuickJS must allocate the error that
//! stops it.
//!
//! This is the only crate of Loams Live's runtime that uses `unsafe`: the
//! allocator trait is `unsafe` to implement, and freeing and resizing a
//! block are `unsafe` calls into rquickjs's [`RustAllocator`].

use std::fmt;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rquickjs::allocator::{Allocator, RustAllocator};

/// The bytes [`RustAllocator`] puts in front of every block (its size
/// header, aligned to 8).
const HEADER_BYTES: usize = 8;

/// What a block of `size` bytes costs: its size rounded up to 8, plus the
/// header. `None` on overflow.
fn cost(size: usize) -> Option<usize> {
    size.checked_add(7)
        .map(|s| s & !7)
        .and_then(|s| s.checked_add(HEADER_BYTES))
}

#[derive(Debug)]
struct State {
    limit: usize,
    used: AtomicUsize,
    exceeded: AtomicBool,
    grace: AtomicUsize,
}

impl State {
    /// Counts `bytes` more, unless that passes the limit with too little
    /// grace left; passing the limit raises the flag either way.
    fn reserve(&self, bytes: usize) -> bool {
        let used = self.used.load(Ordering::Relaxed);
        let next = used.saturating_add(bytes);
        if next > self.limit {
            self.exceeded.store(true, Ordering::Relaxed);
            let grace = self.grace.load(Ordering::Relaxed);
            if grace < bytes {
                return false;
            }
            self.grace.store(grace - bytes, Ordering::Relaxed);
        }
        self.used.store(next, Ordering::Relaxed);
        true
    }

    fn release(&self, bytes: usize) {
        let used = self.used.load(Ordering::Relaxed);
        self.used
            .store(used.saturating_sub(bytes), Ordering::Relaxed);
    }
}

/// A runtime's memory account: the limit, the bytes in use, and whether an
/// allocation has gone past the limit. Cheap to clone; every clone is the
/// same account.
#[derive(Debug, Clone)]
pub struct MemoryMeter(Arc<State>);

impl MemoryMeter {
    /// A meter with `limit` bytes.
    pub fn new(limit: usize) -> Self {
        MemoryMeter(Arc::new(State {
            limit,
            used: AtomicUsize::new(0),
            exceeded: AtomicBool::new(false),
            grace: AtomicUsize::new(0),
        }))
    }

    /// An allocator that counts against this meter, for
    /// [`rquickjs::Runtime::new_with_alloc`].
    pub fn allocator(&self) -> LimitedAllocator {
        LimitedAllocator(self.0.clone())
    }

    /// Whether an allocation has gone past the limit. Stays set: a runtime
    /// that ran out is replaced.
    pub fn exceeded(&self) -> bool {
        self.0.exceeded.load(Ordering::Relaxed)
    }

    /// The bytes in use, block headers included.
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Relaxed)
    }

    /// The limit.
    pub fn limit(&self) -> usize {
        self.0.limit
    }

    /// Sets the bytes that may still be allocated past the limit to
    /// `bytes` (not added: repeated stops never accumulate grace). The
    /// interrupt handler calls it right before it stops a call, so QuickJS
    /// can allocate the error that stops it.
    pub fn top_up_grace(&self, bytes: usize) {
        self.0.grace.store(bytes, Ordering::Relaxed);
    }
}

/// The allocator a runtime of Loams Live allocates through: rquickjs's
/// [`RustAllocator`], with every block counted against a [`MemoryMeter`].
pub struct LimitedAllocator(Arc<State>);

impl fmt::Debug for LimitedAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedAllocator")
            .field("limit", &self.0.limit)
            .field("used", &self.0.used.load(Ordering::Relaxed))
            .finish()
    }
}

// SAFETY: every block comes from `RustAllocator`, which meets the trait's
// contract (null or a block of at least the requested size, aligned to 8);
// this allocator only refuses (returns null) before calling it, and passes
// each pointer it is given back to `RustAllocator`, which made it.
#[allow(unsafe_code)]
unsafe impl Allocator for LimitedAllocator {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        let Some(bytes) = cost(size) else {
            return ptr::null_mut();
        };
        if !self.0.reserve(bytes) {
            return ptr::null_mut();
        }
        let block = RustAllocator.alloc(size);
        if block.is_null() {
            self.0.release(bytes);
        }
        block
    }

    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        // `RustAllocator::calloc` panics on an overflowing product, and a
        // panic must not cross into C: refuse it here.
        let Some(bytes) = count.checked_mul(size).and_then(cost) else {
            return ptr::null_mut();
        };
        if count == 0 || size == 0 || !self.0.reserve(bytes) {
            return ptr::null_mut();
        }
        let block = RustAllocator.calloc(count, size);
        if block.is_null() {
            self.0.release(bytes);
        }
        block
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: the caller passes a block this allocator returned, which
        // `RustAllocator` made, so its header holds the block's size.
        let size = unsafe { RustAllocator::usable_size(ptr) };
        self.0.release(size.saturating_add(HEADER_BYTES));
        // SAFETY: as above; the block is freed once, by its maker.
        unsafe { RustAllocator.dealloc(ptr) }
    }

    unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(new_size);
        }
        if new_size == 0 {
            // SAFETY: `ptr` is a block of this allocator (the caller's
            // contract); resizing to nothing frees it.
            unsafe { self.dealloc(ptr) };
            return ptr::null_mut();
        }
        let Some(new) = cost(new_size) else {
            return ptr::null_mut();
        };
        // SAFETY: `ptr` is a block this allocator returned (the caller's
        // contract), made by `RustAllocator`.
        let old = unsafe { RustAllocator::usable_size(ptr) }.saturating_add(HEADER_BYTES);
        if new > old && !self.0.reserve(new - old) {
            return ptr::null_mut();
        }
        // SAFETY: as above; on failure `RustAllocator` leaves the block
        // as it was, still owned by the caller.
        let block = unsafe { RustAllocator.realloc(ptr, new_size) };
        if block.is_null() {
            if new > old {
                self.0.release(new - old);
            }
        } else if new < old {
            self.0.release(old - new);
        }
        block
    }

    unsafe fn usable_size(ptr: *mut u8) -> usize
    where
        Self: Sized,
    {
        // SAFETY: the caller passes a block of this allocator, made by
        // `RustAllocator`.
        unsafe { RustAllocator::usable_size(ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_round_up_and_add_the_header() {
        assert_eq!(cost(0), Some(8));
        assert_eq!(cost(1), Some(16));
        assert_eq!(cost(8), Some(16));
        assert_eq!(cost(usize::MAX), None);
    }

    #[test]
    fn the_limit_refuses_and_flags_and_grace_lets_through() {
        let meter = MemoryMeter::new(64);
        let mut a = meter.allocator();
        let first = a.alloc(32);
        assert!(!first.is_null());
        assert_eq!(meter.used(), 40);
        assert!(!meter.exceeded());
        assert!(a.alloc(32).is_null(), "past the limit");
        assert!(meter.exceeded());
        assert_eq!(meter.used(), 40);
        meter.top_up_grace(64);
        let second = a.alloc(32);
        assert!(!second.is_null(), "out of the grace");
        meter.top_up_grace(0);
        assert!(a.alloc(1).is_null(), "the grace is not cumulative");
        #[allow(unsafe_code)]
        // SAFETY: both blocks came from `a`.
        unsafe {
            a.dealloc(first);
            a.dealloc(second);
        }
        assert_eq!(meter.used(), 0);
        assert!(meter.exceeded(), "the flag stays set");
    }

    #[test]
    fn calloc_overflow_and_realloc_are_counted() {
        let meter = MemoryMeter::new(1 << 20);
        let mut a = meter.allocator();
        assert!(a.calloc(usize::MAX, 2).is_null());
        let block = a.calloc(4, 4);
        assert_eq!(meter.used(), 24);
        #[allow(unsafe_code)]
        // SAFETY: `block` and its resized successors came from `a`.
        unsafe {
            let grown = a.realloc(block, 100);
            assert!(!grown.is_null());
            assert_eq!(meter.used(), 112);
            let shrunk = a.realloc(grown, 8);
            assert_eq!(meter.used(), 16);
            assert!(a.realloc(shrunk, 2 << 20).is_null(), "past the limit");
            assert_eq!(meter.used(), 16, "a refused resize keeps the block");
            a.dealloc(shrunk);
        }
        assert_eq!(meter.used(), 0);
    }
}
