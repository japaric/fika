//! An object pool
//!
//! The objects managed by a pool are never destroyed, i.e. their destructor never runs

use core::ops;

use crate::treiber;
use crate::treiber::{OwningNodePtr, Stack};

/// An object pool
pub struct ObjectPool<T>
where
    T: 'static,
{
    stack: Stack<Inner<T>>,
}

impl<T> ObjectPool<T> {
    /// Creates a new, empty object pool
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            stack: Stack::new(),
        }
    }

    /// Adds an un-managed object to the pool
    pub fn manage(&'static self, unmanaged: &'static mut Unmanaged<T>) {
        unmanaged.inner.data.stack = Some(&self.stack);

        self.stack.push(OwningNodePtr::new(&mut unmanaged.inner));
    }

    /// Requests an object from the pool
    pub fn request(&'static self) -> Option<Object<T>> {
        self.stack.pop().map(|inner| Object { inner })
    }
}

/// An un-managed object
///
/// Must be placed in a pool before it can be used
pub struct Unmanaged<T>
where
    T: 'static,
{
    inner: treiber::Node<Inner<T>>,
}

impl<T> Unmanaged<T>
where
    T: 'static,
{
    /// Creates an un-managed object
    pub const fn new(data: T) -> Self {
        Self {
            inner: treiber::Node::new(Inner { stack: None, data }),
        }
    }
}

struct Inner<T>
where
    T: 'static,
{
    stack: Option<&'static Stack<Inner<T>>>,
    data: T,
}

/// An object associated to a pool
pub struct Object<T>
where
    T: 'static,
{
    inner: OwningNodePtr<Inner<T>>,
}

impl<T> ops::Deref for Object<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner.data
    }
}

impl<T> ops::DerefMut for Object<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.data
    }
}

impl<T> Drop for Object<T> {
    fn drop(&mut self) {
        if let Some(stack) = self.inner.stack {
            // SAFETY: this is the destructor so the original pointer cannot be used by the caller
            let owning_ptr = unsafe { self.inner.copy() };
            stack.push(owning_ptr);
        } else {
            #[cfg(debug_assertions)]
            unreachable!()
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{self, AtomicBool};

    use super::*;

    #[test]
    fn request_from_empty_pool() {
        static POOL: ObjectPool<()> = ObjectPool::new();

        assert!(POOL.request().is_none());
    }

    #[test]
    fn it_works() {
        static POOL: ObjectPool<i32> = ObjectPool::new();

        let value = 42;
        let unmanaged = Box::leak(Box::new(Unmanaged::new(value)));
        POOL.manage(unmanaged);

        let maybe_object = POOL.request();
        assert!(maybe_object.is_some());

        let mut object = maybe_object.unwrap();
        assert_eq!(value, *object);

        *object += 1;

        // returns the object to the pool
        drop(object);

        let maybe_object = POOL.request();
        assert!(maybe_object.is_some());

        // pool manages one object so we get the same object back
        let same_object = maybe_object.unwrap();
        assert_eq!(value + 1, *same_object);
    }

    #[test]
    fn if_managed_destructor_does_not_run() {
        struct Bomb;

        impl Drop for Bomb {
            fn drop(&mut self) {
                unreachable!("destructor must not run")
            }
        }

        static POOL: ObjectPool<Bomb> = ObjectPool::new();

        let unmanaged = Box::leak(Box::new(Unmanaged::new(Bomb)));
        POOL.manage(unmanaged);

        let maybe_object = POOL.request();
        assert!(maybe_object.is_some());

        let bomb = maybe_object.unwrap();
        // "everything will be fine; just cut the red wire ..."
        drop(bomb);
    }

    #[test]
    fn if_unmanaged_destructor_does_run() {
        static DESTROYED: AtomicBool = AtomicBool::new(false);

        struct Evil;

        impl Drop for Evil {
            fn drop(&mut self) {
                DESTROYED.store(true, atomic::Ordering::Relaxed);
            }
        }

        let unmanaged = Unmanaged::new(Evil);
        drop(unmanaged);

        // did we destroy Evil?
        assert!(DESTROYED.load(atomic::Ordering::Relaxed));
    }
}
