//! A lock-free Treiber stack built on top of LL/SC instructions
//!
//! Currently only ARM is supported, i.e. "do not ask for support for other architectures"

use core::arch::asm;
use core::ptr::NonNull;
use core::sync::atomic;
use core::sync::atomic::AtomicPtr;
use core::{ops, ptr};

pub(crate) struct Stack<T> {
    top: AtomicPtr<Node<T>>,
}

impl<T> Stack<T> {
    pub const fn new() -> Self {
        Self {
            top: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub fn push(&self, mut node: OwningNodePtr<T>) {
        // XXX this feels iffy and sort of gives the impression that `self` needs to be pinned?
        let top_addr = NonNull::from(&self.top).cast::<usize>();

        loop {
            // SAFETY: non-null value
            let top = unsafe { load_link(top_addr) };

            // NOTE ordering is not important as the data dependency will maintain the order of
            // the operations
            // SAFETY: `node` is a valid pointer
            unsafe {
                node.inner
                    .as_mut()
                    .next
                    .store(top as *mut _, atomic::Ordering::Relaxed);
            }

            // SAFETY: `node` is a valid pointer
            if unsafe { store_conditional(top_addr, node.inner.addr().get()).is_ok() } {
                break;
            }
        }
    }

    pub fn pop(&self) -> Option<OwningNodePtr<T>> {
        // XXX this feels iffy and sort of gives the impression that `self` needs to be pinned?
        let top_addr = NonNull::from(&self.top).cast();

        'retry: loop {
            // SAFETY: `node` is a valid pointer
            let top = unsafe { load_link(top_addr) };

            if let Some(top) = NonNull::new(top as *mut Node<T>) {
                // SAFETY: given that is non-null, `top` is a valid pointer as only valid
                // pointers can be `push`-ed
                let next = unsafe { top.as_ref().next.load(atomic::Ordering::Relaxed) };

                // SAFETY: `top_addr` is a valid pointer
                if unsafe { store_conditional(top_addr, next as usize).is_ok() } {
                    break Some(OwningNodePtr { inner: top });
                } else {
                    continue 'retry;
                }
            } else {
                clear_load_link();

                break None;
            }
        }
    }
}

// SAFETY: if you put the `Stack` in a static then you can move nodes between threads, therefore
// the data must be `Send`
unsafe impl<T> Sync for Stack<T> where T: Send {}

/// An owning pointer into a statically allocated (`'static`) node
#[repr(transparent)]
pub(crate) struct OwningNodePtr<T> {
    inner: NonNull<Node<T>>,
}

impl<T> ops::Deref for OwningNodePtr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Given the `OwningNodePtr::new` constructor, the data is always valid (never
        // deallocated) and the owning nature ensures aliasing rules are respected.
        unsafe { &self.inner.as_ref().data }
    }
}

impl<T> ops::DerefMut for OwningNodePtr<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Given the `OwningNodePtr::new` constructor, the data is always valid (never
        // deallocated) and the owning nature ensures aliasing rules are respected.
        unsafe { &mut self.inner.as_mut().data }
    }
}

impl<T> OwningNodePtr<T> {
    pub fn new(node: &'static mut Node<T>) -> Self {
        Self {
            inner: NonNull::from(node),
        }
    }

    pub fn into_shared(self) -> SharedNodePtr<T> {
        SharedNodePtr { inner: self.inner }
    }

    /// # Safety
    /// - To prevent aliasing the original handle (`self`) must not be used after this operation
    pub unsafe fn copy(&self) -> Self {
        Self { inner: self.inner }
    }
}

/// A shared pointer into a statically allocated (`'static`) node
#[repr(transparent)]
pub(crate) struct SharedNodePtr<T> {
    inner: NonNull<Node<T>>,
}

impl<T> SharedNodePtr<T> {
    /// # Safety
    /// - Caller must ensure this is the last remaining shared instance
    pub unsafe fn into_owning(self) -> OwningNodePtr<T> {
        OwningNodePtr { inner: self.inner }
    }
}

impl<T> Copy for SharedNodePtr<T> {}

impl<T> Clone for SharedNodePtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> ops::Deref for SharedNodePtr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Given the `OwningNodePtr::new` constructor, the data is always valid (never
        // deallocated) and the owning nature ensures aliasing rules are respected.
        unsafe { &self.inner.as_ref().data }
    }
}

pub(crate) struct Node<T> {
    next: AtomicPtr<Node<T>>,
    pub data: T,
}

impl<T> Node<T> {
    pub const fn new(data: T) -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            data,
        }
    }
}

fn clear_load_link() {
    // SAFETY: cannot trigger undefined behavior
    unsafe { asm!("CLREX", options(nomem, nostack)) }
}

/// # Safety
/// - `ptr` must be a valid pointer
unsafe fn load_link(ptr: NonNull<usize>) -> usize {
    let value;
    // SAFETY: `ptr` is a valid pointer as per the caller contract
    unsafe {
        asm!("LDREX {}, [{}]",
             out(reg) value,
             in(reg) ptr.addr().get(),
             options(nostack),
        )
    }
    value
}

/// # Safety
/// - `ptr` must be a valid pointer
unsafe fn store_conditional(ptr: NonNull<usize>, value: usize) -> Result<(), ()> {
    let outcome: usize;
    // SAFETY: `ptr` is a valid pointer as per the caller contract
    unsafe {
        asm!("STREX {}, {}, [{}]",
             out(reg) outcome,
             in(reg) value,
             in(reg) ptr.addr().get(),
             options(nostack)
        );
    }
    if outcome == 0 { Ok(()) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_empty() {
        let stack = Stack::<()>::new();
        assert!(stack.pop().is_none());
    }

    #[test]
    fn one_node() {
        let value = 42;

        let stack = Stack::new();
        let a = OwningNodePtr::new(Box::leak(Box::new(Node::new(value))));
        stack.push(a);

        let a = stack.pop();
        assert!(a.is_some());
        assert_eq!(value, *a.unwrap());

        assert!(stack.pop().is_none());
    }

    #[test]
    fn two_nodes() {
        let value_a = 42;
        let value_b = 24;

        let stack = Stack::new();
        let a = OwningNodePtr::new(Box::leak(Box::new(Node::new(value_a))));
        let b = OwningNodePtr::new(Box::leak(Box::new(Node::new(value_b))));

        stack.push(a);
        stack.push(b);

        // LIFO order
        let b = stack.pop();
        assert!(b.is_some());
        assert_eq!(value_b, *b.unwrap());

        let a = stack.pop();
        assert!(a.is_some());
        assert_eq!(value_a, *a.unwrap());

        assert!(stack.pop().is_none());
    }

    #[test]
    fn can_move_the_stack_without_invalidating_it() {
        #[inline(never)]
        fn construct_and_move(value: i32) -> Stack<i32> {
            let stack = Stack::new();
            let a = OwningNodePtr::new(Box::leak(Box::new(Node::new(value))));

            stack.push(a);

            stack
        }

        let value = 42;
        let stack = construct_and_move(value);

        let a = stack.pop();
        assert!(a.is_some());
        assert_eq!(value, *a.unwrap());
    }
}
