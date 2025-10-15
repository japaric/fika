//! A box pool
//!
//! Similar to the object pool but the box must be initialized when created and its contents are
//! destroyed when it goes out of scope

use core::mem::MaybeUninit;
use core::{fmt, ops};

use crate::treiber::{self, OwningNodePtr, Stack};

/// A pool of boxes
pub struct BoxPool<T>
where
    T: 'static,
{
    stack: Stack<Inner<T>>,
}

impl<T> BoxPool<T>
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
    pub fn request(&'static self, value: T) -> Result<Box<T>, T> {
        if let Some(mut slot) = self.stack.pop() {
            slot.data.write(value);
            Ok(Box { inner: slot })
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
/// Must be placed in a `BoxPool` before it can be used
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
}

/// A boxed object managed by a `BoxPool`
pub struct Box<T>
where
    T: 'static,
{
    inner: OwningNodePtr<Inner<T>>,
}

impl<T> fmt::Debug for Box<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        T::fmt(self, f)
    }
}

impl<T> PartialEq for Box<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        T::eq(self, other)
    }
}

impl<T> ops::Deref for Box<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: while live, the box contents are initialized
        unsafe { &*self.inner.data.as_ptr() }
    }
}

impl<T> ops::DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: while live, the box contents are initialized
        unsafe { &mut *self.inner.data.as_mut_ptr() }
    }
}

impl<T> Drop for Box<T> {
    fn drop(&mut self) {
        if let Some(stack) = self.inner.stack {
            // SAFETY: data is currently initialized and after we run the
            // destructor, `Box::deref*` cannot be used
            unsafe {
                core::ptr::drop_in_place(self.inner.data.as_mut_ptr());
            }
            // SAFETY: this is the destructor so the original pointer cannot be used by the caller
            let owning_ptr = unsafe { self.inner.copy() };
            stack.push(owning_ptr);
        } else {
            #[cfg(debug_assertions)]
            unreachable!()
        }
    }
}

// SAFETY: moving a box transfers ownership so if the contents are Send then the Box is also Send
unsafe impl<T> Send for Box<T> where T: Send {}

// SAFETY: moving a box transfers ownership so if the contents were Sync then the Box is also Sync.
// the box does not add synchronization of its own
unsafe impl<T> Sync for Box<T> where T: Sync {}

#[cfg(test)]
mod tests {
    use super::*;

    use core::sync::atomic::{self, AtomicBool};
    use std::boxed::Box as StdBox;

    #[test]
    fn request_from_empty_pool() {
        static POOL: BoxPool<i32> = BoxPool::new();

        let value = 42;
        assert_eq!(Err(value), POOL.request(value));
    }

    #[test]
    fn it_works() {
        static POOL: BoxPool<i32> = BoxPool::new();

        let value = 42;
        let slot = StdBox::leak(StdBox::new(Slot::new()));
        POOL.manage(slot);

        let maybe_box = POOL.request(value);
        assert!(maybe_box.is_ok());

        let boxed = maybe_box.unwrap();
        assert_eq!(value, *boxed);

        let exhausted = POOL.request(value);
        assert_eq!(Err(value), exhausted);

        // returns the object to the pool
        drop(boxed);

        let maybe_object = POOL.request(value);
        assert_eq!(Ok(&value), maybe_object.as_deref());
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

        static POOL: BoxPool<Evil> = BoxPool::new();

        let slot = StdBox::leak(StdBox::new(Slot::new()));
        POOL.manage(slot);

        let boxed = POOL.request(Evil).ok().unwrap();

        // still live
        assert!(!DESTROYED.load(atomic::Ordering::Relaxed));

        drop(boxed);

        assert!(DESTROYED.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn check_box_is_send() {
        is_send::<Box<i32>>();
    }

    #[test]
    fn check_box_is_sync() {
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
