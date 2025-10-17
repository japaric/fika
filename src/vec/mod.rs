//! A contiguous growable array type with fixed capacity backed by a byte buffer

use core::marker::PhantomData;
use core::{fmt, mem, ops, ptr, slice};

/// A contiguous growable array type with fixed capacity backed by a byte buffer
pub struct Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    data: PhantomData<T>,
    len: usize,
    storage: S,
}

impl<T, S> Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    /// Creates a new empty vector
    pub const fn new(storage: S) -> Self {
        assert!(
            0 != mem::size_of::<T>(),
            "zero-sized types are currently not supported"
        );

        Self {
            storage,
            len: 0,
            data: PhantomData,
        }
    }

    /// Appends an element to the back of a collection
    pub fn push(&mut self, element: T) -> Result<(), T> {
        if self.len() == self.capacity() {
            return Err(element);
        }

        // SAFETY: within bounds given previous len vs capacity check
        let slot = unsafe { self.as_mut_ptr().add(self.len) };
        // SAFETY: pointer is properly aligned
        unsafe {
            slot.write(element);
        }
        self.len += 1;

        Ok(())
    }

    /// Removes the last element from a vector and returns it, or `None` if it is empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        // SAFETY: within bounds and aligned
        let value = unsafe { (self.aligned_storage_ptr()).add(self.len - 1).read() };
        self.len -= 1;

        Some(value)
    }

    /// Returns the total number of elements the vector can hold
    pub fn capacity(&self) -> usize {
        let storage = self.storage.as_ref();
        let addr = storage.as_ptr() as usize;
        let len = storage.len();

        let align = mem::size_of::<T>();
        let offset = addr % align;
        let adj = if offset == 0 { 0 } else { align - offset };

        let Some(available) = len.checked_sub(adj) else {
            return 0;
        };

        available / mem::size_of::<T>()
    }

    /// # Safety
    /// - This is allowed to point outside `storage` so a length check must be performed first
    unsafe fn aligned_storage_ptr(&self) -> *const T {
        let storage = self.storage.as_ref();
        let ptr = storage.as_ptr();

        let align = mem::size_of::<T>();
        let offset = ptr as usize % align;
        let adj = if offset == 0 { 0 } else { align - offset };

        // SAFETY: caller must have checked the storage length so this should point within `storage`
        unsafe { ptr.add(adj).cast() }
    }
}

impl<T, S> fmt::Debug for Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <[T]>::fmt(self, f)
    }
}

impl<T, S> ops::Deref for Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: `len` is trusted given that it's only changed by safe API
        unsafe { slice::from_raw_parts(self.aligned_storage_ptr(), self.len) }
    }
}

impl<T, S> ops::DerefMut for Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `len` is trusted given that it's only changed by safe API
        unsafe { slice::from_raw_parts_mut(self.aligned_storage_ptr() as *mut T, self.len) }
    }
}

impl<T, S> Drop for Vec<T, S>
where
    S: AsRef<[u8]> + AsMut<[u8]>,
{
    fn drop(&mut self) {
        // SAFETY:
        unsafe {
            ptr::drop_in_place::<[T]>(&mut **self);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic;
    use core::sync::atomic::AtomicUsize;

    use crate::object_pool::{ObjectPool, Unmanaged};

    use super::*;

    #[test]
    fn capacity_works() {
        #[repr(align(4))]
        struct Align4<T>(T);

        let mut storage = Align4([0; 5]);
        let aligned = &mut storage.0[..4];

        let vec = Vec::<u8, _>::new(&mut *aligned);
        assert_eq!(4, vec.capacity());
        drop(vec);

        let vec = Vec::<u16, _>::new(&mut *aligned);
        assert_eq!(2, vec.capacity());
        drop(vec);

        let vec = Vec::<u32, _>::new(&mut *aligned);
        assert_eq!(1, vec.capacity());
        drop(vec);

        let vec = Vec::<u64, _>::new(&mut *aligned);
        assert_eq!(0, vec.capacity());
        drop(vec);

        let unaligned = &mut storage.0[1..][..4];

        let vec = Vec::<u8, _>::new(&mut *unaligned);
        assert_eq!(4, vec.capacity());
        drop(vec);

        let vec = Vec::<u16, _>::new(&mut *unaligned);
        assert_eq!(1, vec.capacity());
        drop(vec);

        let vec = Vec::<u32, _>::new(&mut *unaligned);
        assert_eq!(0, vec.capacity());
        drop(vec);

        let vec = Vec::<u64, _>::new(&mut *unaligned);
        assert_eq!(0, vec.capacity());
        drop(vec);
    }

    #[test]
    fn push_pop_works() {
        let storage = [0; 4];
        let mut vec = Vec::new(storage);

        assert!(vec.push(1u8).is_ok());
        assert!(vec.push(2).is_ok());

        assert_eq!([1, 2], &*vec);

        assert_eq!(Some(2), vec.pop());
        assert_eq!(Some(1), vec.pop());
        assert_eq!(None, vec.pop());
    }

    #[test]
    fn contents_are_destroyed() {
        static DESTROYED: AtomicUsize = AtomicUsize::new(0);

        #[repr(C)]
        struct Evil(u8);

        impl Drop for Evil {
            fn drop(&mut self) {
                DESTROYED.fetch_add(1, atomic::Ordering::Relaxed);
            }
        }

        let storage = [0; 4];
        let mut vec = Vec::new(storage);
        assert!(vec.push(Evil(0)).is_ok());
        assert!(vec.push(Evil(1)).is_ok());

        // not yet
        assert_eq!(0, DESTROYED.load(atomic::Ordering::Relaxed));

        drop(vec);
        assert_eq!(2, DESTROYED.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn backed_by_pool() {
        const ALLOC_SIZE: usize = 128;

        static POOL: ObjectPool<[u8; 128]> = ObjectPool::new();

        POOL.manage(Box::leak(Box::new(Unmanaged::new([0; ALLOC_SIZE]))));

        let storage = POOL.request().expect("OOM");
        let words = Vec::<u32, _>::new(storage);
        // XXX unclear if we can guarantee that the `T` in `Object<T>` has a minimum alignment of
        // `mem::align_of::<usize>()`. It SHOULD given the layout of `treiber::Node` but the
        // layout of Rust structs is unspecified :shrug:
        assert_eq!(ALLOC_SIZE / mem::size_of::<u32>(), words.capacity());

        assert!(POOL.request().is_none(), "expected pool to be exhausted");

        // returns storage to the pool
        drop(words);

        assert!(POOL.request().is_some(), "expected pool to have an object");
    }
}
