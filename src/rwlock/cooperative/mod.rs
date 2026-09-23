//! 协作式（cooperative）读写锁。
//!
//! 本模块实现 `abs_sync` v0.3.0 的 [`TrAsyncRwLock`]：一种异步运行时无关的
//! 读写锁。实现只使用 `core::task::Waker`，不依赖任何具体的异步运行时。
//!
//! [`TrAsyncRwLock`]: abs_sync::async_rwlock::TrAsyncRwLock
//!
//! # 结构总览
//!
//! ```text
//! CooperativeRwLock<T>              // 栈上的外壳，内联持有资源
//! ├── _pin_  : PhantomPinned
//! ├── core_  : Arc<RwCore<D, B, O>>  // 共享同步状态，Sized 且与 T 无关
//! └── data_  : UnsafeCell<T>
//!
//! RwCore<D, B, O>
//! ├── stat_         : CoopRwState<D, B, O>     // 写者/可升级读者/读者计数/队列标记
//! ├── queue_        : SpinLock<WaitQueue>      // 等待队列（同质合并的等待节点）
//! └── upgrade_wait_ : SpinLock<Option<Waker>>  // 升级专用等待槽（不进队列）
//! ```
//!
//! 资源 `T` 内联在外壳里，而 `RwCore` 完全不泛型于 `T`：它只承载状态字与
//! 等待队列，因此自身 `Sized` 且大小固定。Guard 通过"借用会话、会话借用
//! 外壳"的路径访问数据，全程类型安全。
//!
//! # 并发模型
//!
//! - 快速路径（无人排队时）直接比较交换状态字，无锁、无分配；
//! - 一旦发生竞争，等待者入队并注册 waker；`pass()` 只负责**唤醒**，
//!   真正的许可领取始终由等待者自己比较交换完成，因此取消不会泄漏许可；
//! - 等待队列的取消回收是**主动**的：future 的 `Drop` 会标记取消并重跑
//!   `pass()`，这是活性所必需的（详见开发日志 §4.4）；
//! - 升级**不进 FIFO 队列**（否则会与排队写者互相死锁），而是走独立等待槽，
//!   并以状态字里的栅栏位挡住新读者，保证升级最终必定成功（详见 §4.5）；
//! - 任何自旋锁内都不调用 `Waker::wake`，避免单线程执行器上的自死锁（§4.6）。
//!
//! 设计讨论与决策因果链见 `dev-notes/rwlock-20260923-2204.md`。

mod core_;
mod error_;
mod reader_;
mod rwlock_;
mod state_;
#[cfg(test)]
mod tests_;
mod upgrade_;
mod wait_;
mod writer_;

pub use error_::CoopRwLockError;
pub use reader_::{ReadAcquireAsync, ReadAcquireFuture, ReaderGuard};
pub use rwlock_::{
    CooperativeAcqSession, CooperativeRwLock, CooperativeRwLockBorrowed,
    CooperativeRwLockOwned,
};
pub use upgrade_::{
    UpgradableReadAcquireAsync, UpgradableReadAcquireFuture,
    UpgradableReaderGuard, Upgrade, UpgradeAcquireAsync, UpgradeAcquireFuture,
};
pub use writer_::{WriteAcquireAsync, WriteAcquireFuture, WriterGuard};
