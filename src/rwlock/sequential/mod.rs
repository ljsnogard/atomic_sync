//! This module contains the implementation of a spinning reader-writer lock
//! designed with first-in-first-out (FIFO) fairness among its contenders.

mod acquire_;
mod rwlock_;
mod reader_;
mod upgrade_;
mod writer_;

pub use rwlock_::SpinningRwLock;
pub use acquire_::Acquire;
