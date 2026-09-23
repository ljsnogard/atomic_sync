//! 协作式读写锁的共享内核：状态字 + 等待队列 + 放行流程。
//!
//! # 并发模型
//!
//! - **状态字**（`CoopRwState`）是无锁的，快速路径直接在其上做比较交换；
//! - **等待队列**（`WaitQueue`）由一把自旋锁保护。凡是需要判断"我是不是
//!   队首 / 谁该被放行"的地方都必须持有这把锁；
//! - **升级等待槽**（`upgrade_wait_`）由另一把自旋锁单独保护，且与队列锁
//!   **绝不同时持有**（先解锁一个再取另一个），避免锁序成环；
//! - 任何自旋锁内**只允许**做内存操作与收集 waker，**绝不允许**调用
//!   `Waker::wake`，否则在单线程执行器上会自死锁
//!   （见 `dev-notes/rwlock-20260923-2204.md` §4.4）。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{borrow::BorrowMut, task::Waker};

use funty::Unsigned;

use atomex::{
    x_deps::funty,
    StrictOrderings, TrAtomicData, TrCmpxchOrderings,
};

use super::{
    state_::CoopRwState,
    wait_::{SpinLock, WaitKind, WaitNode, WaitQueue},
};

/// 一次获取动作的当前进展。
pub(super) enum AcqProgress {
    /// 已经拿到许可。
    Acquired,
    /// 仍在队列里等待；future 需要记住这两个值以便重试与取消。
    Waiting {
        node_: Arc<WaitNode>,
        slot_: usize,
    },
}

/// 读写锁的共享内核。
///
/// 它不承载被保护的资源 `T`（资源内联在 `CooperativeRwLock` 外壳里），
/// 因此与 `T` 的类型和大小都无关，可以稳定地放在 `Arc` 里共享。
pub(super) struct RwCore<D, B, O = StrictOrderings>
where
    D: TrAtomicData + Unsigned,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    stat_: CoopRwState<D, B, O>,
    queue_: SpinLock<WaitQueue>,
    /// 升级等待槽：全锁至多一个可升级读者，因此至多一个等待者。
    upgrade_wait_: SpinLock<Option<Waker>>,
}

impl<D, B, O> RwCore<D, B, O>
where
    D: TrAtomicData + Unsigned,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) const fn new(cell: B) -> Self {
        RwCore {
            stat_: CoopRwState::new(cell),
            queue_: SpinLock::new(VecDeque::new()),
            upgrade_wait_: SpinLock::new(None),
        }
    }

    #[inline]
    pub(super) fn reader_count(&self) -> D {
        self.stat_.reader_count()
    }

    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----
    // 快速路径：队列为空时无需分配、无需加锁
    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----

    #[inline]
    pub(super) fn try_read_fast(&self) -> bool {
        self.stat_.try_read_fast()
    }

    #[inline]
    pub(super) fn try_write_fast(&self) -> bool {
        self.stat_.try_write_fast()
    }

    #[inline]
    pub(super) fn try_upgradable_read_fast(&self) -> bool {
        self.stat_.try_upgradable_read_fast()
    }

    /// 按等待者种类调用对应的"已被放行"领取动作。
    #[inline]
    fn try_claim_(&self, kind: WaitKind) -> bool {
        match kind {
            WaitKind::Read => self.stat_.try_read_queued(),
            WaitKind::UpgradableRead => self.stat_.try_upgradable_read_queued(),
            WaitKind::Write => self.stat_.try_write_queued(),
        }
    }

    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----
    // 获取：入队 / 领取 / 注册 waker 必须在同一段队列锁临界区内完成
    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----

    /// 进入等待队列并尝试领取许可。
    ///
    /// 三步（入队、尝试领取、注册 waker）在同一段临界区内完成，这是避免
    /// "丢唤醒"的关键：临界区之外没有任何时刻能让别人替我跑 `pass()`。
    pub(super) fn acquire(&self, kind: WaitKind, waker: &Waker) -> AcqProgress {
        let mut q = self.queue_.lock();
        self.prune_front_(&mut q);
        let (node, slot) = self.enqueue_(&mut q, kind);
        // 队列由空变非空只需置位一次；已置位时省掉一次原子读改写。
        if !self.stat_.waiter_queued() {
            self.stat_.mark_waiter_queued();
        }

        let claimed = self.is_head_(&q, &node) && self.try_claim_(kind);
        if claimed {
            node.mark_acquired(slot);
        } else {
            node.set_waker(slot, waker);
        }
        let wakers = self.pass_(&mut q);
        drop(q);
        Self::wake_wakers_(wakers, Some(waker));

        if claimed {
            AcqProgress::Acquired
        } else {
            AcqProgress::Waiting {
                node_: node,
                slot_: slot,
            }
        }
    }

    /// 已被唤醒的等待者重试领取。
    ///
    /// 返回 `true` 表示拿到许可；否则刷新 waker 并继续等待。
    pub(super) fn poll_acquire(
        &self,
        node: &Arc<WaitNode>,
        slot: usize,
        waker: &Waker,
    ) -> bool {
        let mut q = self.queue_.lock();
        self.prune_front_(&mut q);

        let claimed = self.is_head_(&q, node) && self.try_claim_(node.kind());
        if claimed {
            node.mark_acquired(slot);
        } else {
            node.set_waker(slot, waker);
        }
        let wakers = self.pass_(&mut q);
        drop(q);
        Self::wake_wakers_(wakers, Some(waker));
        claimed
    }

    /// 等待者被取消（future 被丢弃）。
    ///
    /// 这一步是**活性所必需**的：队首的等待者被唤醒后立刻取消时，必须由它
    /// 自己重跑一次放行流程，否则排在它后面的等待者将永远无人唤醒。
    pub(super) fn cancel_wait(&self, node: &Arc<WaitNode>, slot: usize) {
        let mut q = self.queue_.lock();
        node.mark_cancelled(slot);
        let wakers = self.pass_(&mut q);
        drop(q);
        Self::wake_wakers_(wakers, None);
    }

    fn enqueue_(&self, q: &mut WaitQueue, kind: WaitKind) -> (Arc<WaitNode>, usize) {
        // 只有读节点允许同质合并；且只与队尾合并，保证 FIFO 次序不被破坏。
        if kind == WaitKind::Read
            && let Some(tail) = q.back()
            && tail.kind() == WaitKind::Read
        {
            let slot = tail.push_slot();
            return (tail.clone(), slot);
        }
        let (node, slot) = WaitNode::new_with_slot(kind);
        let node = Arc::new(node);
        q.push_back(node.clone());
        (node, slot)
    }

    fn is_head_(&self, q: &WaitQueue, node: &Arc<WaitNode>) -> bool {
        q.front().is_some_and(|f| Arc::ptr_eq(f, node))
    }

    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----
    // 放行流程
    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----

    /// 剪掉队首已经"没有任何在等的人"的节点。
    fn prune_front_(&self, q: &mut WaitQueue) {
        while q.front().is_some_and(|f| f.pending_count() == 0) {
            q.pop_front();
        }
    }

    /// 决定谁可以被唤醒。
    ///
    /// 这里**只负责唤醒**，不预先分配任何许可：真正的领取始终由等待者在
    /// `poll` 里自己比较交换完成，因此"被唤醒后立刻取消"不会泄漏许可。
    fn pass_(&self, q: &mut WaitQueue) -> Vec<Waker> {
        self.prune_front_(q);
        if q.is_empty() {
            self.stat_.clear_waiter_queued();
            return Vec::new();
        }

        let snap = self.stat_.snapshot();
        // 写者持有、或升级请求已挂起（升级栅栏）时，任何读者都不放行。
        if snap.writer_active || snap.upgrade_waiting {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut upgrade_taken = snap.upgrade_active;
        let Some(front) = q.front() else {
            return out;
        };

        match front.kind() {
            WaitKind::Read | WaitKind::UpgradableRead => {
                // 写者未持有，读类等待者可以成段放行。
                for node in q.iter() {
                    match node.kind() {
                        WaitKind::Read => node.collect_waiting_wakers(&mut out),
                        WaitKind::UpgradableRead => {
                            // 全锁至多一个可升级读者。
                            if upgrade_taken {
                                break;
                            }
                            node.collect_waiting_wakers(&mut out);
                            upgrade_taken = true;
                        }
                        WaitKind::Write => break,
                    }
                }
            }
            WaitKind::Write => {
                if snap.reader_count == D::ZERO && !upgrade_taken {
                    front.collect_waiting_wakers(&mut out);
                }
            }
        }
        out
    }

    /// 跑一次放行流程，并在**释放队列锁之后**唤醒收集到的 waker。
    fn run_pass_(&self) {
        let mut q = self.queue_.lock();
        let wakers = self.pass_(&mut q);
        drop(q);
        Self::wake_wakers_(wakers, None);
    }

    fn wake_wakers_(wakers: Vec<Waker>, skip: Option<&Waker>) {
        for w in wakers {
            if skip.is_some_and(|s| s.will_wake(&w)) {
                continue;
            }
            w.wake();
        }
    }

    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----
    // 升级：不进 FIFO 队列的专用等待槽
    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----

    /// 同步尝试升级；成功时顺带撤销升级栅栏并放行被栅栏挡住的读者。
    pub(super) fn try_upgrade_to_write(&self) -> bool {
        if !self.stat_.try_upgrade_to_write() {
            return false;
        }
        self.run_pass_();
        true
    }

    /// 异步升级：挂起栅栏、注册 waker、再尝试升级。
    ///
    /// 顺序必须是"先注册 waker 再尝试"，否则存在丢唤醒窗口：
    /// 若先尝试失败、再由被释放的读者检查等待槽，就会两头落空。
    /// 栅栏一旦置位，新读者无法进入，读者计数只会减少，
    /// 因此"检查到条件成立"之后升级必定成功，不会出现拿到 waker 却升不了级的空转。
    pub(super) fn upgrade_or_wait(&self, waker: &Waker) -> bool {
        let ok = {
            let mut slot = self.upgrade_wait_.lock();
            self.stat_.mark_upgrade_waiting();
            *slot = Some(waker.clone());
            let ok = self.stat_.try_upgrade_to_write();
            if ok {
                *slot = None;
            }
            ok
        };
        if ok {
            self.run_pass_();
        }
        ok
    }

    /// 撤销升级等待：清栅栏、丢弃 waker，并放行被栅栏挡住的读者。
    pub(super) fn cancel_upgrade_wait(&self) {
        {
            let mut slot = self.upgrade_wait_.lock();
            *slot = None;
        }
        self.stat_.clear_upgrade_waiting();
        self.run_pass_();
    }

    /// 若升级条件已成立，则取走等待槽里的 waker 并唤醒。
    ///
    /// 调用者必须在状态更新之后调用它（见各 `release_*`）。
    fn wake_upgrade_if_ready_(&self) {
        if !self.stat_.can_upgrade_to_write() {
            return;
        }
        let waker = {
            let mut slot = self.upgrade_wait_.lock();
            slot.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----
    // 释放 / 降级
    //-- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----

    pub(super) fn release_reader(&self) {
        debug_assert!(self.stat_.release_reader());
        self.run_pass_();
        self.wake_upgrade_if_ready_();
    }

    pub(super) fn release_upgradable_read(&self) {
        debug_assert!(self.stat_.release_upgradable_read());
        self.run_pass_();
        self.wake_upgrade_if_ready_();
    }

    pub(super) fn release_writer(&self) {
        debug_assert!(self.stat_.release_writer());
        self.run_pass_();
        self.wake_upgrade_if_ready_();
    }

    pub(super) fn downgrade_write_to_read(&self) {
        debug_assert!(self.stat_.downgrade_write_to_read());
        self.run_pass_();
    }

    pub(super) fn downgrade_write_to_upgradable(&self) {
        debug_assert!(self.stat_.downgrade_write_to_upgradable());
        self.run_pass_();
    }

    pub(super) fn downgrade_upgradable_to_read(&self) {
        debug_assert!(self.stat_.downgrade_upgradable_to_read());
        self.run_pass_();
        self.wake_upgrade_if_ready_();
    }
}
