//! FIxed KApacity stuff

#![deny(missing_docs)]
#![cfg_attr(not(test), no_std)]
#![deny(clippy::missing_safety_doc)]
#![deny(clippy::undocumented_unsafe_blocks)]

#[cfg(target_arch = "arm")]
pub mod arc_pool;
#[cfg(target_arch = "arm")]
pub mod box_pool;
#[cfg(target_arch = "arm")]
pub mod object_pool;
pub mod spsc;
#[cfg(target_arch = "arm")]
mod treiber;
pub mod vec;
