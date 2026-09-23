//! 写许可的守卫与异步获取。

use alloc::sync::Arc;
use core::{
    borrow::BorrowMut,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
};

use funty::Unsigned;

use atomex::{x_deps::funty, TrAtomicData, TrCmpxchOrderings};
use abs_cancel::TrCancellationToken;
use abs_sync::{
    async_rwlock::*,
    ok_or::XtOkOr,
    sync_guard::{TrAcqMutGuard, TrAcqRefGuard},
    x_deps::abs_cancel,
};
use gen_mcf2::gen_may_cancel_future;

use crate::rwlock::TrShareMut;

use super::{
    core_::AcqProgress,
    error_::CoopRwLockError,
    reader_::ReaderGuard,
    rwlock_::CooperativeAcqSession,
    upgrade_::UpgradableReaderGuard,
    wait_::{WaitKind, WaitNode},
};

/// 写许可守卫。
pub struct WriterGuard<'a, 'g, T: ?Sized + 'a, D, B, O>(
    &'g mut CooperativeAcqSession<'a, T, D, B, O>,
)
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings;

impl<'a, 'g, T: ?Sized + 'a, D, B, O> WriterGuard<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    pub(super) fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        WriterGuard(sess)
    }

    /// 降级为普通读守卫。
    pub fn downgrade_to_reader(self) -> ReaderGuard<'a, 'g, T, D, B, O> {
        CooperativeAcqSession::downgrade_writer_to_reader(self)
    }

    /// 降级为可升级读守卫。
    pub fn downgrade_to_upgradable(
        self,
    ) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        CooperativeAcqSession::downgrade_writer_to_upgradable(self)
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Drop for WriterGuard<'a, '_, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn drop(&mut self) {
        self.0.core().release_writer()
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrShareMut<'g, CooperativeAcqSession<'a, T, D, B, O>>
    for WriterGuard<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn share_mut(&mut self) -> &'g mut CooperativeAcqSession<'a, T, D, B, O> {
        let p = self.0 as *mut _;
        // SAFETY: 守卫独占地持有会话借用（`&'g mut`），把它按原生命周期
        // 交还给调用方不会产生别名；调用方（`destruct_guard_`）会先用
        // `ManuallyDrop` 阻止本守卫析构。
        unsafe { &mut *p }
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Deref for WriterGuard<'a, '_, T, D, B, O>
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

impl<'a, T: ?Sized + 'a, D, B, O> DerefMut for WriterGuard<'a, '_, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut_impl()
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrAcqRefGuard<'a, 'g, T>
    for WriterGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrAcqMutGuard<'a, 'g, T>
    for WriterGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrReaderGuard<'a, 'g, T>
    for WriterGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Acquire = CooperativeAcqSession<'a, T, D, B, O>;
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrWriterGuard<'a, 'g, T>
    for WriterGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    #[inline]
    fn downgrade(
        self,
    ) -> <Self::Acquire as TrAsyncRwLockAcqSess<'a, T>>::ReaderGuard<'g> {
        WriterGuard::downgrade_to_reader(self)
    }

    #[inline]
    fn downgrade_to_upgradable(
        self,
    ) -> <Self::Acquire as TrAsyncRwLockAcqSess<'a, T>>::UpgradableGuard<'g>
    {
        WriterGuard::downgrade_to_upgradable(self)
    }
}

/// 手写的写者获取流程；结构同 [`super::reader_::ReadAcquireInner`]。
pub(super) struct WriteAcquireInner<'a, 'g, T: ?Sized + 'a, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    sess_: Option<&'g mut CooperativeAcqSession<'a, T, D, B, O>>,
    pending_: Option<(Arc<WaitNode>, usize)>,
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> WriteAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        WriteAcquireInner {
            sess_: Option::Some(sess),
            pending_: Option::None,
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Future for WriteAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Output = Result<WriterGuard<'a, 'g, T, D, B, O>, CoopRwLockError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(sess) = this.sess_.take() else {
            panic!("[WriteAcquireInner::poll] polled after completion");
        };

        if this.pending_.is_none() && sess.core().try_write_fast() {
            return Poll::Ready(Result::Ok(WriterGuard::new(sess)));
        }

        let pending = this.pending_.clone();
        let claimed = match pending {
            Option::Some((node, slot)) => {
                sess.core().poll_acquire(&node, slot, cx.waker())
            }
            Option::None => {
                match sess.core().acquire(WaitKind::Write, cx.waker()) {
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
            Poll::Ready(Result::Ok(WriterGuard::new(sess)))
        } else {
            this.sess_ = Option::Some(sess);
            Poll::Pending
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Drop for WriteAcquireInner<'a, 'g, T, D, B, O>
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

/// 可取消的写获取。
#[gen_may_cancel_future(WriteAcquire, pub)]
async fn write_acquire_async_<'a, 'g, T, D, B, O, C>(
    sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>,
    cancel: C,
) -> Result<WriterGuard<'a, 'g, T, D, B, O>, CoopRwLockError>
where
    'a: 'g,
    T: ?Sized + 'a,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
    C: TrCancellationToken,
{
    let acq = WriteAcquireInner::new(sess);
    match cancel.cancellation().ok_or(acq).await {
        Result::Ok(_) => Result::Err(CoopRwLockError::Cancelled),
        Result::Err(r) => r,
    }
}
