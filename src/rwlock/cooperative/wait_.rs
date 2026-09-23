//! 等待队列的底层构件：自旋锁、等待节点与队列。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicUsize, Ordering},
    task::Waker,
};

/// 一个极简的自旋锁。
///
/// 它只服务于本模块内部的等待队列与等待节点，临界区里**绝不允许**出现
/// 任何可能让出执行权的操作（尤其是调用 [`Waker::wake`]），否则会在
/// 单线程执行器上自死锁。
pub(super) struct SpinLock<T> {
    locked_: AtomicUsize,
    data_: UnsafeCell<T>,
}

// SAFETY: `data_` 只在持有锁（`locked_` 由 1 表示）时经 `SpinLockGuard`
// 访问，而同一时刻至多只有一个执行流能取得锁，故共享访问必然是串行的；
// `T: Send` 保证临界区数据可以跨执行流移交。
unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub(super) const fn new(data: T) -> Self {
        SpinLock {
            locked_: AtomicUsize::new(0),
            data_: UnsafeCell::new(data),
        }
    }

    /// 自旋直到取得锁。
    pub(super) fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked_
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock_: self }
    }

    fn unlock_(&self) {
        self.locked_.store(0, Ordering::Release);
    }
}

/// [`SpinLock`] 的 RAII 守卫；析构即解锁。
pub(super) struct SpinLockGuard<'a, T> {
    lock_: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: 构造 `SpinLockGuard` 的前提是已持有锁，且在守卫析构前
        // 不会释放，因此这里是该临界区的唯一可变访问路径。
        unsafe { &*self.lock_.data_.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: 同 `deref`。
        unsafe { &mut *self.lock_.data_.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock_.unlock_()
    }
}

/// 等待者的种类。
///
/// `Read` 之间可以同质合并进同一个节点（见 [`WaitNode`]）；其余种类一个
/// 节点只承载一个槽位，因为它们彼此互斥。
///
/// 注意"可升级读者想要升级为写者"不在队列里等待：它由 `RwCore` 单独的一个
/// waker 槽位承载（全锁至多一个可升级读者，因此至多一个升级等待者）。
/// 放进 FIFO 队列会和"写者等待 R==0、可升级读者等待队首"互相死锁。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitKind {
    /// 想要获得普通读许可。
    Read,
    /// 想要获得可升级读许可（此时 `UPGRADE_ACTIVE` 尚未置位）。
    UpgradableRead,
    /// 想要获得写许可。
    Write,
}

/// 单个槽位的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotState {
    /// 仍在等待放行。
    Waiting,
    /// 已被放行并成功领取许可。
    Acquired,
    /// 等待者被取消（future 被丢弃）。
    Cancelled,
}

struct WaiterSlot {
    waker_: Option<Waker>,
    state_: SlotState,
}

/// 一个等待节点。
///
/// 一个节点可以承载**多个同质的读者**（"同质合并"）：它们共享同一份排队
/// 位置，`pass()` 一次就唤醒整组。每个槽位独立记录自己的 waker 与状态，
/// 因此单个等待者的取消只影响它自己的槽位，不会波及同组的其他人。
///
/// 节点的槽位由自身的自旋锁保护；调用方（`core_`）总是**先持有队列锁、
/// 再取节点锁**，锁序固定为 `队列锁 → 节点锁`，不会成环。
pub(super) struct WaitNode {
    kind_: WaitKind,
    slots_: SpinLock<Vec<WaiterSlot>>,
}

impl WaitNode {
    /// 建立只带一个槽位的新节点，返回 (节点, 槽位下标)。
    ///
    /// 该槽位对应 `kind` 自身，因此不受"只有读节点能追加槽位"的限制。
    pub(super) fn new_with_slot(kind: WaitKind) -> (Self, usize) {
        let node = WaitNode {
            kind_: kind,
            slots_: SpinLock::new(Vec::new()),
        };
        node.slots_.lock().push(WaiterSlot {
            waker_: None,
            state_: SlotState::Waiting,
        });
        (node, 0)
    }

    pub(super) fn kind(&self) -> WaitKind {
        self.kind_
    }

    /// 追加一个槽位；调用者必须持有队列锁，且只有 `Read` 节点允许追加。
    pub(super) fn push_slot(&self) -> usize {
        debug_assert!(self.kind_ == WaitKind::Read);
        let mut slots = self.slots_.lock();
        slots.push(WaiterSlot {
            waker_: None,
            state_: SlotState::Waiting,
        });
        slots.len() - 1
    }

    /// 记录（或刷新）槽位的 waker。
    pub(super) fn set_waker(&self, slot: usize, waker: &Waker) {
        let mut slots = self.slots_.lock();
        let Some(entry) = slots.get_mut(slot) else {
            debug_assert!(false, "slot index out of range");
            return;
        };
        if entry
            .waker_
            .as_ref()
            .is_some_and(|w| w.will_wake(waker))
        {
            return;
        }
        entry.waker_ = Some(waker.clone());
    }

    /// 标注该槽位已成功领取许可。
    pub(super) fn mark_acquired(&self, slot: usize) {
        self.set_state_(slot, SlotState::Acquired);
    }

    /// 标注该槽位已被取消。
    pub(super) fn mark_cancelled(&self, slot: usize) {
        self.set_state_(slot, SlotState::Cancelled);
    }

    fn set_state_(&self, slot: usize, state: SlotState) {
        let mut slots = self.slots_.lock();
        let Some(entry) = slots.get_mut(slot) else {
            debug_assert!(false, "slot index out of range");
            return;
        };
        debug_assert!(entry.state_ == SlotState::Waiting);
        entry.state_ = state;
        // 一旦离开 `Waiting`，waker 就没有保留价值了。
        entry.waker_ = None;
    }

    /// 仍处于 [`SlotState::Waiting`] 的槽位数量。
    ///
    /// 该值为 0 时，节点的排队位置可以被回收（`pass()` 会把它从队首剪掉）。
    pub(super) fn pending_count(&self) -> usize {
        self.slots_
            .lock()
            .iter()
            .filter(|s| s.state_ == SlotState::Waiting)
            .count()
    }

    /// 把本节点所有 `Waiting` 槽位的 waker 克隆收集到 `out`。
    pub(super) fn collect_waiting_wakers(&self, out: &mut Vec<Waker>) {
        let slots = self.slots_.lock();
        for s in slots.iter() {
            if s.state_ == SlotState::Waiting
                && let Some(w) = s.waker_.as_ref()
            {
                out.push(w.clone());
            }
        }
    }
}

/// 等待队列：队首即最先到达的等待者。
pub(super) type WaitQueue = VecDeque<Arc<WaitNode>>;
