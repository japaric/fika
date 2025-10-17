//! FIxed KApacity stuff

#![deny(missing_docs)]
#![cfg_attr(not(test), no_std)]
#![deny(clippy::missing_safety_doc)]
#![deny(clippy::undocumented_unsafe_blocks)]

pub mod arc_pool;
pub mod box_pool;
pub mod object_pool;
mod treiber;
pub mod vec;
