mod impl_;
mod reader_;
mod writer_;
mod upgrade_;

#[cfg(test)]
mod tests_;

pub use impl_::{
    Acquire, SpinningRwLock, SpinningRwLockBorrowed, SpinningRwLockOwned,
};
pub use reader_::{ReadTask, ReaderGuard};
pub use writer_::{WriteTask, WriterGuard};
pub use upgrade_::{UpgradableReadTask, UpgradableReaderGuard, UpgradeTask};