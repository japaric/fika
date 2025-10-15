//! An arc pool
//!
//! Similar to the box pool but the "boxes" have the drop semantics of `std::sync::Arc`

use core::mem::MaybeUninit;
use core::sync::atomic::{self, AtomicUsize};
use core::{fmt, ops};

use crate::treiber::{self, OwningNodePtr, SharedNodePtr, Stack};

/// A pool of arcs
pub struct ArcPool<T>
where
    T: 'static,
{
    stack: Stack<Inner<T>>,
}

impl<T> ArcPool<T>
where
    T: 'static,
{
    /// Creates a new, empty object pool
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            stack: Stack::new(),
        }
    }

    /// Requests a memory slot from the pool
    pub fn request(&'static self, value: T) -> Result<Arc<T>, T> {
        if let Some(mut slot) = self.stack.pop() {
            slot.data.write(value);

            // XXX unclear if this should be Release. the two fences in Drop seem sufficient?
            slot.strong_count.store(1, atomic::Ordering::Relaxed);

            Ok(Arc {
                inner: slot.into_shared(),
            })
        } else {
            Err(value)
        }
    }

    /// Gives a memory slot to the pool
    pub fn manage(&'static self, slot: &'static mut Slot<T>) {
        slot.inner.data.stack = Some(&self.stack);

        self.stack.push(OwningNodePtr::new(&mut slot.inner));
    }
}

/// An un-managed memory slot
///
/// Must be placed in a `ArcPool` before it can be used
pub struct Slot<T>
where
    T: 'static,
{
    inner: treiber::Node<Inner<T>>,
}

impl<T> Slot<T>
where
    T: 'static,
{
    /// Creates an un-managed memory slot
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            inner: treiber::Node::new(Inner {
                stack: None,
                data: MaybeUninit::uninit(),
                strong_count: AtomicUsize::new(1),
            }),
        }
    }
}

struct Inner<T>
where
    T: 'static,
{
    stack: Option<&'static Stack<Inner<T>>>,
    data: MaybeUninit<T>,
    strong_count: AtomicUsize,
}

/// A referenced counted object managed by an `ArcPool`
pub struct Arc<T>
where
    T: 'static,
{
    inner: SharedNodePtr<Inner<T>>,
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        const MAX_REFCOUNT: usize = isize::MAX as usize;

        let old_count = self
            .inner
            .strong_count
            .fetch_add(1, atomic::Ordering::Relaxed);

        // FIXME should abort instead of panic
        assert!(old_count <= MAX_REFCOUNT);

        Self { inner: self.inner }
    }
}

impl<T> fmt::Debug for Arc<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        T::fmt(self, f)
    }
}

impl<T> PartialEq for Arc<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        T::eq(self, other)
    }
}

impl<T> ops::Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: while live, the box contents are initialized
        unsafe { &*self.inner.data.as_ptr() }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        if let Some(stack) = self.inner.stack {
            if self
                .inner
                .strong_count
                .fetch_sub(1, atomic::Ordering::Release)
                != 1
            {
                return;
            }

            // synchcronizes the subsequent loads that may happen in `drop_in_place` with the
            // Release fence of the preceding `fetch_sub` that happens in a *different* thread
            atomic::fence(atomic::Ordering::Acquire);

            // SAFETY: as per the above check this is the only shared pointer left
            let mut owning_ptr = unsafe { self.inner.into_owning() };

            // SAFETY: data is currently initialized and after we run the
            // destructor, `Box::deref*` cannot be used
            unsafe {
                core::ptr::drop_in_place(owning_ptr.data.as_mut_ptr());
            }
            // SAFETY: this is the destructor so the original pointer cannot be used by the caller
            stack.push(owning_ptr);
        } else {
            #[cfg(debug_assertions)]
            unreachable!()
        }
    }
}

// SAFETY: moving an Arc between threads effectively copies a reference to its contents to
// the receiver thread so the contents must be safe to share between threads (Sync). Furthermore,
// to be compatible with API that moves out of a reference, e.g. `Option::take`, the contents must
// also be Send
unsafe impl<T> Send for Arc<T> where T: Send + Sync {}

// SAFETY: the bounds on the contents must be at least as stringent as the ones in the Send impl
unsafe impl<T> Sync for Arc<T> where T: Send + Sync {}

#[cfg(test)]
mod tests {
    use super::*;

    use core::sync::atomic::{self, AtomicBool};

    #[test]
    fn request_from_empty_pool() {
        static POOL: ArcPool<i32> = ArcPool::new();

        let value = 42;
        assert_eq!(Err(value), POOL.request(value));
    }

    #[test]
    fn it_works() {
        static POOL: ArcPool<i32> = ArcPool::new();

        let value = 42;
        let slot = Box::leak(Box::new(Slot::new()));
        POOL.manage(slot);

        let maybe_arc = POOL.request(value);
        assert!(maybe_arc.is_ok());

        let arc = maybe_arc.unwrap();
        assert_eq!(value, *arc);

        let exhausted = POOL.request(value);
        assert_eq!(Err(value), exhausted);

        // returns the object to the pool
        drop(arc);

        let maybe_arc = POOL.request(value);
        assert_eq!(Ok(&value), maybe_arc.as_deref());
    }

    #[test]
    fn destructor_runs() {
        static DESTROYED: AtomicBool = AtomicBool::new(false);

        struct Evil;

        impl Drop for Evil {
            fn drop(&mut self) {
                DESTROYED.store(true, atomic::Ordering::Relaxed);
            }
        }

        static POOL: ArcPool<Evil> = ArcPool::new();

        let slot = Box::leak(Box::new(Slot::new()));
        POOL.manage(slot);

        let arc = POOL.request(Evil).ok().unwrap();
        let arc2 = Arc::clone(&arc);

        // still live
        assert!(!DESTROYED.load(atomic::Ordering::Relaxed));

        drop(arc2);

        // still live
        assert!(!DESTROYED.load(atomic::Ordering::Relaxed));

        drop(arc);

        assert!(DESTROYED.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn check_arc_is_send() {
        is_send::<Box<i32>>();
    }

    #[test]
    fn check_arc_is_sync() {
        is_sync::<Box<i32>>();
    }

    fn is_send<T>()
    where
        T: Send,
    {
    }

    fn is_sync<T>()
    where
        T: Sync,
    {
    }
}
