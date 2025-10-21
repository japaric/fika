//! A fixed-capacity, single-producer, single-consumer (SPSC) channel

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{self, AtomicUsize};

/// A fixed-capacity, single-producer, single-consumer (SPSC) channel
pub struct Channel<T, const N: usize> {
    inner: Inner<[UnsafeCell<MaybeUninit<T>>; N]>,
}

impl<T, const N: usize> Channel<T, N> {
    /// Creates a new channel
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        const {
            assert!(N > 0, "capacity must be at least one");
        }

        Self {
            inner: Inner {
                read: AtomicUsize::new(0),
                write: AtomicUsize::new(0),
                buf: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            },
        }
    }

    /// Splits this statically allocated channel into sender and receiver parts
    ///
    /// This operation consumes `self`
    pub fn split(&'static mut self) -> (Sender<T>, Receiver<T>) {
        let inner = NonNull::from(&mut self.inner);

        (Sender { inner }, Receiver { inner })
    }
}

/// The sender side of a channel
pub struct Sender<T> {
    inner: NonNull<Inner<[UnsafeCell<MaybeUninit<T>>]>>,
}

impl<T> Sender<T> {
    /// Sends data through the channel
    ///
    /// Returns an `Err` if the channel is observed as being full
    pub fn send(&self, value: T) -> Result<(), T> {
        // SAFETY: valid static allocation due to `split` API
        let sender = unsafe { self.inner.as_ref() };

        // SAFETY: `split` API ensures SPSC property
        unsafe { sender.send(value) }
    }
}

/// The receiver side of a channel
pub struct Receiver<T> {
    inner: NonNull<Inner<[UnsafeCell<MaybeUninit<T>>]>>,
}

impl<T> Receiver<T> {
    /// Receives data through the channel
    ///
    /// Returns `None` if the channel is observed as being empty
    pub fn recv(&self) -> Option<T> {
        // SAFETY: valid static allocation due to `split` API
        let receiver = unsafe { self.inner.as_ref() };

        // SAFETY: `split` API ensures SPSC property
        unsafe { receiver.recv() }
    }
}

struct Inner<T: ?Sized> {
    read: AtomicUsize,
    write: AtomicUsize,
    buf: T,
}

impl<T> Inner<[UnsafeCell<MaybeUninit<T>>]> {
    /// # Safety
    /// - Caller must ensure that the SPSC property holds
    unsafe fn send(&self, value: T) -> Result<(), T> {
        let current_write = self.write.load(atomic::Ordering::Relaxed);
        let capacity = self.buf.len();

        // Acquire: all operations AFTER the barrier cannot be reordered to BEFORE it
        // this synchronizes with the Release `read` store in `recv` ensuring that
        // the `slot` read in `recv` is completed before the `slot` write that happens below
        let acquired_read = self.read.load(atomic::Ordering::Acquire);
        let current_len = current_write.wrapping_sub(acquired_read);
        if current_len == capacity {
            // full
            return Err(value);
        }

        // SAFETY: within bounds due to modulo operation
        let slot = unsafe { self.buf.get_unchecked(current_write % capacity) };

        // SAFETY: SPSC, atomic fences and `if` condition ensure no data race with `recv` operation
        unsafe {
            slot.get().cast::<T>().write(value);
        }

        // Release: operations that PRECEDE this barrier cannot be reordered to AFTER it
        self.write
            .store(current_write.wrapping_add(1), atomic::Ordering::Release);

        Ok(())
    }

    /// # Safety
    /// - Caller must ensure that the SPSC property holds
    unsafe fn recv(&self) -> Option<T> {
        let current_read = self.read.load(atomic::Ordering::Relaxed);
        let capacity = self.buf.len();

        // Acquire: all operations AFTER the barrier cannot be reordered to BEFORE it
        // this synchronizes with the Release `write` store in `send` ensuring that
        // the `slot` write in `send` is completed before the `slot` read that happens below
        let acquired_write = self.write.load(atomic::Ordering::Acquire);
        if current_read == acquired_write {
            // empty
            return None;
        }

        // SAFETY: within bounds due to modulo operation
        let slot = unsafe { self.buf.get_unchecked(current_read % capacity) };
        // SAFETY: valid allocation; known to be initialized due to state of `write` cursor;
        // SPSC, atomic fences and `if` condition ensure no data race with `send` operation
        let value = unsafe { slot.get().cast::<T>().read() };

        // Release: operations that PRECEDE this barrier cannot be reordered to AFTER it
        self.read
            .store(current_read.wrapping_add(1), atomic::Ordering::Release);

        Some(value)
    }
}

// SAFETY: allowing the handle to move to another thread, allows sending values to another thread;
// therefore the value must be Send as well
unsafe impl<T> Send for Sender<T> where T: Send {}

// SAFETY: allowing the handle to move to another thread, allows sending values to another thread;
// therefore the value must be Send as well
unsafe impl<T> Send for Receiver<T> where T: Send {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_one_works() {
        let channel = Box::leak(Box::new(Channel::<i32, 1>::new()));
        let (sender, receiver) = channel.split();

        let value = 42;
        assert_eq!(None, receiver.recv());
        assert_eq!(Ok(()), sender.send(value));
        assert_eq!(Err(value), sender.send(value));
        assert_eq!(Some(value), receiver.recv());
        assert_eq!(None, receiver.recv());
    }

    #[test]
    fn fifo_order() {
        let channel = Box::leak(Box::new(Channel::<i32, 2>::new()));
        let (sender, receiver) = channel.split();

        let value1 = 42;
        let value2 = 24;
        assert_eq!(Ok(()), sender.send(value1));
        assert_eq!(Ok(()), sender.send(value2));

        assert_eq!(Some(value1), receiver.recv());
        assert_eq!(Some(value2), receiver.recv());
    }

    #[test]
    fn works_with_non_power_of_two() {
        let channel = Box::leak(Box::new(Channel::<i32, 3>::new()));
        let (sender, receiver) = channel.split();

        let value1 = 42;
        let value2 = 24;
        let value3 = 123;

        assert_eq!(None, receiver.recv());

        assert_eq!(Ok(()), sender.send(value1));
        assert_eq!(Ok(()), sender.send(value2));
        assert_eq!(Ok(()), sender.send(value3));
        assert_eq!(Err(value3), sender.send(value3));

        assert_eq!(Some(value1), receiver.recv());
        assert_eq!(Some(value2), receiver.recv());
        assert_eq!(Some(value3), receiver.recv());
        assert_eq!(None, receiver.recv());
    }

    #[test]
    fn cursor_wrap_around() {
        let channel = Box::leak(Box::new(Channel::<i32, 2>::new()));
        for cursor in [&channel.inner.read, &channel.inner.write] {
            cursor.store(usize::MAX, atomic::Ordering::SeqCst);
        }
        let (sender, receiver) = channel.split();

        let value1 = 42;
        let value2 = 24;
        assert_eq!(None, receiver.recv());
        assert_eq!(Ok(()), sender.send(value1));
        assert_eq!(Ok(()), sender.send(value2));
        assert_eq!(Err(value1), sender.send(value1));
        assert_eq!(Some(value1), receiver.recv());
        assert_eq!(Some(value2), receiver.recv());
        assert_eq!(None, receiver.recv());
    }

    #[test]
    fn check_sender_is_send() {
        is_send::<Sender<i32>>();
    }

    #[test]
    fn check_receiver_is_send() {
        is_send::<Receiver<i32>>();
    }

    fn is_send<T>()
    where
        T: Send,
    {
    }
}
