//! 可升级读守卫、升级会话，以及它们的异步获取。

use alloc::sync::Arc;
use core::{
    borrow::BorrowMut,
    future::Future,
    ops::Deref,
    pin::Pin,
    task::{Context, Poll},
};

use funty::Unsigned;

use atomex::{x_deps::funty, TrAtomicData, TrCmpxchOrderings};
use abs_cancel::TrCancellationToken;
use abs_sync::{
    async_rwlock::*,
    ok_or::XtOkOr,
    sync_guard::TrAcqRefGuard,
    x_deps::abs_cancel,
};
use gen_mcf2::gen_may_cancel_future;

use crate::rwlock::TrShareMut;

use super::{
    core_::AcqProgress,
    error_::CoopRwLockError,
    reader_::ReaderGuard,
    rwlock_::CooperativeAcqSession,
    wait_::{WaitKind, WaitNode},
    writer_::WriterGuard,
};

/// 可升级读守卫。
///
/// 它在状态字上同时占一个读者名额与"可升级"标志；全锁至多存在一个。
pub struct UpgradableReaderGuard<'a, 'g, T: ?Sized + 'a, D, B, O>(
    &'g mut CooperativeAcqSession<'a, T, D, B, O>,
)
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings;

impl<'a, 'g, T: ?Sized + 'a, D, B, O> UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    pub(super) fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        UpgradableReaderGuard(sess)
    }

    /// 降级为普通读守卫。
    pub fn downgrade(self) -> ReaderGuard<'a, 'g, T, D, B, O> {
        CooperativeAcqSession::downgrade_upgradable_to_reader(self)
    }

    /// 立即尝试升级为写者；失败时原样归还自身。
    #[allow(clippy::type_complexity)]
    pub fn try_upgrade(
        self,
    ) -> Result<WriterGuard<'a, 'g, T, D, B, O>, Self> {
        CooperativeAcqSession::try_upgrade_to_writer(self)
    }

    /// 取得升级会话。
    pub fn upgrade_session(self) -> Upgrade<'a, 'g, T, D, B, O> {
        Upgrade::new(self)
    }

    /// 只读地访问底层会话。
    fn sess_ref(&self) -> &CooperativeAcqSession<'a, T, D, B, O> {
        self.0
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Drop for UpgradableReaderGuard<'a, '_, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn drop(&mut self) {
        self.0.core().release_upgradable_read()
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O>
    TrShareMut<'g, CooperativeAcqSession<'a, T, D, B, O>>
    for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn share_mut(&mut self) -> &'g mut CooperativeAcqSession<'a, T, D, B, O> {
        let p = self.0 as *mut _;
        // SAFETY: 同 `WriterGuard::share_mut`。
        unsafe { &mut *p }
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Deref for UpgradableReaderGuard<'a, '_, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.deref_impl()
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrAcqRefGuard<'a, 'g, T>
    for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrReaderGuard<'a, 'g, T>
    for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Acquire = CooperativeAcqSession<'a, T, D, B, O>;
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrUpgradableReaderGuard<'a, 'g, T>
    for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type UpgradeSess = Upgrade<'a, 'g, T, D, B, O>;

    #[inline]
    fn downgrade(
        self,
    ) -> <Self::Acquire as TrAsyncRwLockAcqSess<'a, T>>::ReaderGuard<'g> {
        UpgradableReaderGuard::downgrade(self)
    }

    #[inline]
    fn upgrade_session(self) -> Self::UpgradeSess {
        UpgradableReaderGuard::upgrade_session(self)
    }
}

/// 升级会话：持有可升级读守卫，并提供同步/异步升级入口。
pub struct Upgrade<'a, 'g, T: ?Sized + 'a, D, B, O>(
    UpgradableReaderGuard<'a, 'g, T, D, B, O>,
)
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings;

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Upgrade<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    pub(super) fn new(guard: UpgradableReaderGuard<'a, 'g, T, D, B, O>) -> Self {
        Upgrade(guard)
    }

    /// 立即尝试升级；失败时返回错误，会话保持不变。
    pub fn try_upgrade<'u>(
        &'u mut self,
    ) -> Result<WriterGuard<'a, 'u, T, D, B, O>, CoopRwLockError> {
        CooperativeAcqSession::try_upgrade_mut_to_writer(self.guard_mut())
    }

    /// 可取消的异步升级。
    pub fn upgrade_async<'f>(
        &'f mut self,
    ) -> UpgradeAcquireAsync<'a, 'g, 'f, 'f, T, D, B, O>
    where
        'g: 'f,
    {
        UpgradeAcquireAsync::new(self)
    }

    /// 取回内部的可升级读守卫。
    pub fn into_guard(self) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        self.0
    }

    fn guard_mut(&mut self) -> &mut UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        &mut self.0
    }

    fn sess_ref(&self) -> &CooperativeAcqSession<'a, T, D, B, O> {
        self.0.sess_ref()
    }

    fn share_guard_sess_mut(&mut self) -> &'g mut CooperativeAcqSession<'a, T, D, B, O> {
        self.0.share_mut()
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrAsyncRwLockUpgradeSession<'a, 'g, T>
    for Upgrade<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type ParentSess = CooperativeAcqSession<'a, T, D, B, O>;

    #[inline]
    fn try_upgrade<'f>(
        &'f mut self,
    ) -> Result<
        <Self::ParentSess as TrAsyncRwLockAcqSess<'a, T>>::WriterGuard<'f>,
        CoopRwLockError,
    >
    where
        'g: 'f,
    {
        Upgrade::try_upgrade(self)
    }

    type UpgradeAsync<'f> = UpgradeAcquireAsync<'a, 'g, 'f, 'f, T, D, B, O>
    where
        'g: 'f,
        Self: 'f;

    #[inline]
    fn upgrade_async<'f>(&'f mut self) -> Self::UpgradeAsync<'f>
    where
        'g: 'f,
    {
        Upgrade::upgrade_async(self)
    }

    #[inline]
    fn into_guard(
        self,
    ) -> <Self::ParentSess as TrAsyncRwLockAcqSess<'a, T>>::UpgradableGuard<'g>
    {
        Upgrade::into_guard(self)
    }
}

/// 手写的可升级读获取流程。
pub(super) struct UpgradableReadAcquireInner<'a, 'g, T: ?Sized + 'a, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    sess_: Option<&'g mut CooperativeAcqSession<'a, T, D, B, O>>,
    pending_: Option<(Arc<WaitNode>, usize)>,
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> UpgradableReadAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        UpgradableReadAcquireInner {
            sess_: Option::Some(sess),
            pending_: Option::None,
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Future
    for UpgradableReadAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Output =
        Result<UpgradableReaderGuard<'a, 'g, T, D, B, O>, CoopRwLockError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(sess) = this.sess_.take() else {
            panic!("[UpgradableReadAcquireInner::poll] polled after completion");
        };

        if this.pending_.is_none() && sess.core().try_upgradable_read_fast() {
            return Poll::Ready(Result::Ok(UpgradableReaderGuard::new(sess)));
        }

        let pending = this.pending_.clone();
        let claimed = match pending {
            Option::Some((node, slot)) => {
                sess.core().poll_acquire(&node, slot, cx.waker())
            }
            Option::None => {
                match sess.core().acquire(WaitKind::UpgradableRead, cx.waker()) {
                    AcqProgress::Acquired => true,
                    AcqProgress::Waiting { node_, slot_ } => {
                        this.pending_ = Option::Some((node_, slot_));
                        false
                    }
                }
            }
        };

        if claimed {
            this.pending_ = Option::None;
            Poll::Ready(Result::Ok(UpgradableReaderGuard::new(sess)))
        } else {
            this.sess_ = Option::Some(sess);
            Poll::Pending
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Drop
    for UpgradableReadAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn drop(&mut self) {
        let (Option::Some(sess), Option::Some((node, slot))) =
            (self.sess_.as_ref(), self.pending_.as_ref())
        else {
            return;
        };
        sess.core().cancel_wait(node, *slot);
    }
}

/// 可取消的可升级读获取。
#[gen_may_cancel_future(UpgradableReadAcquire, pub)]
async fn upgradable_read_acquire_async_<'a, 'g, T, D, B, O, C>(
    sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>,
    cancel: C,
) -> Result<UpgradableReaderGuard<'a, 'g, T, D, B, O>, CoopRwLockError>
where
    'a: 'g,
    T: ?Sized + 'a,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
    C: TrCancellationToken,
{
    let acq = UpgradableReadAcquireInner::new(sess);
    match cancel.cancellation().ok_or(acq).await {
        Result::Ok(_) => Result::Err(CoopRwLockError::Cancelled),
        Result::Err(r) => r,
    }
}

/// 手写的"升级为写者"流程。
///
/// 升级不进 FIFO 队列，而是挂起状态字里的升级栅栏并登记到 `RwCore` 的
/// 专用等待槽；栅栏保证新读者无法进入，因此既有读者排空后升级必定成功。
pub(super) struct UpgradeAcquireInner<'a, 'g, 'f, T: ?Sized + 'a, D, B, O>
where
    'g: 'f,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    upg_: Option<&'f mut Upgrade<'a, 'g, T, D, B, O>>,
    waiting_: bool,
}

impl<'a, 'g, 'f, T: ?Sized + 'a, D, B, O> UpgradeAcquireInner<'a, 'g, 'f, T, D, B, O>
where
    'g: 'f,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn new(upg: &'f mut Upgrade<'a, 'g, T, D, B, O>) -> Self {
        UpgradeAcquireInner {
            upg_: Option::Some(upg),
            waiting_: false,
        }
    }
}

impl<'a, 'g, 'f, T: ?Sized + 'a, D, B, O> Future
    for UpgradeAcquireInner<'a, 'g, 'f, T, D, B, O>
where
    'g: 'f,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Output = Result<WriterGuard<'a, 'f, T, D, B, O>, CoopRwLockError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(upg) = this.upg_.take() else {
            panic!("[UpgradeAcquireInner::poll] polled after completion");
        };

        if upg.sess_ref().core().upgrade_or_wait(cx.waker()) {
            this.waiting_ = false;
            let sess = upg.share_guard_sess_mut();
            Poll::Ready(Result::Ok(WriterGuard::new(sess)))
        } else {
            this.waiting_ = true;
            this.upg_ = Option::Some(upg);
            Poll::Pending
        }
    }
}

impl<'a, 'g, 'f, T: ?Sized + 'a, D, B, O> Drop
    for UpgradeAcquireInner<'a, 'g, 'f, T, D, B, O>
where
    'g: 'f,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn drop(&mut self) {
        if !self.waiting_ {
            return;
        }
        if let Option::Some(upg) = self.upg_.as_mut() {
            upg.sess_ref().core().cancel_upgrade_wait();
            self.waiting_ = false;
        }
    }
}

/// 可取消的升级获取。
#[gen_may_cancel_future(UpgradeAcquire, pub)]
async fn upgrade_acquire_async_<'a, 'g, 'f, T, D, B, O, C>(
    upg: &'f mut Upgrade<'a, 'g, T, D, B, O>,
    cancel: C,
) -> Result<WriterGuard<'a, 'f, T, D, B, O>, CoopRwLockError>
where
    'a: 'g,
    'g: 'f,
    T: ?Sized + 'a,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
    C: TrCancellationToken,
{
    let acq = UpgradeAcquireInner::new(upg);
    match cancel.cancellation().ok_or(acq).await {
        Result::Ok(_) => Result::Err(CoopRwLockError::Cancelled),
        Result::Err(r) => r,
    }
}
