//! 读许可的守卫与异步获取。

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

use super::{
    core_::AcqProgress,
    error_::CoopRwLockError,
    rwlock_::CooperativeAcqSession,
    wait_::{WaitKind, WaitNode},
};

/// 读许可守卫。
///
/// 它借用会话，因此同一会话在守卫存活期间不能再发起别的获取。
pub struct ReaderGuard<'a, 'g, T: ?Sized + 'a, D, B, O>(
    &'g mut CooperativeAcqSession<'a, T, D, B, O>,
)
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings;

impl<'a, 'g, T: ?Sized + 'a, D, B, O> ReaderGuard<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    pub(super) fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        ReaderGuard(sess)
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Drop for ReaderGuard<'a, '_, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn drop(&mut self) {
        self.0.core().release_reader()
    }
}

impl<'a, T: ?Sized + 'a, D, B, O> Deref for ReaderGuard<'a, '_, T, D, B, O>
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
    for ReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> TrReaderGuard<'a, 'g, T>
    for ReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Acquire = CooperativeAcqSession<'a, T, D, B, O>;
}

/// 手写的获取流程：入队、领取、注册 waker、以及取消时的出队。
///
/// 取消的竞速由 [`read_acquire_async_`] 里的 `OkOr` 完成，本 future 只关心
/// "怎么拿到许可"与"拿不到时怎么排队"。它自身实现取消安全：被丢弃时若仍在
/// 队列里，会主动撤销并重跑放行流程。
pub(super) struct ReadAcquireInner<'a, 'g, T: ?Sized + 'a, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    sess_: Option<&'g mut CooperativeAcqSession<'a, T, D, B, O>>,
    pending_: Option<(Arc<WaitNode>, usize)>,
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> ReadAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    fn new(sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>) -> Self {
        ReadAcquireInner {
            sess_: Option::Some(sess),
            pending_: Option::None,
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Future for ReadAcquireInner<'a, 'g, T, D, B, O>
where
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
{
    type Output = Result<ReaderGuard<'a, 'g, T, D, B, O>, CoopRwLockError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 本类型所有字段都是 `Unpin`，取 `&mut self` 是安全的。
        let this = self.get_mut();
        let Some(sess) = this.sess_.take() else {
            panic!("[ReadAcquireInner::poll] polled after completion");
        };

        // 快速路径：无竞争时不分配节点、不加锁。
        if this.pending_.is_none() && sess.core().try_read_fast() {
            return Poll::Ready(Result::Ok(ReaderGuard::new(sess)));
        }

        let pending = this.pending_.clone();
        let claimed = match pending {
            Option::Some((node, slot)) => {
                sess.core().poll_acquire(&node, slot, cx.waker())
            }
            Option::None => match sess.core().acquire(WaitKind::Read, cx.waker())
            {
                AcqProgress::Acquired => true,
                AcqProgress::Waiting { node_, slot_ } => {
                    this.pending_ = Option::Some((node_, slot_));
                    false
                }
            },
        };

        if claimed {
            this.pending_ = Option::None;
            Poll::Ready(Result::Ok(ReaderGuard::new(sess)))
        } else {
            this.sess_ = Option::Some(sess);
            Poll::Pending
        }
    }
}

impl<'a, 'g, T: ?Sized + 'a, D, B, O> Drop for ReadAcquireInner<'a, 'g, T, D, B, O>
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

/// 可取消的读获取。
///
/// 返回的 future 既可 `.await`（不可取消），也可先
/// `.may_cancel_with(token)` 再 `.await`。
#[gen_may_cancel_future(ReadAcquire, pub)]
async fn read_acquire_async_<'a, 'g, T, D, B, O, C>(
    sess: &'g mut CooperativeAcqSession<'a, T, D, B, O>,
    cancel: C,
) -> Result<ReaderGuard<'a, 'g, T, D, B, O>, CoopRwLockError>
where
    'a: 'g,
    T: ?Sized + 'a,
    D: TrAtomicData + Unsigned + 'a,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell> + 'a,
    O: TrCmpxchOrderings + 'a,
    C: TrCancellationToken,
{
    let acq = ReadAcquireInner::new(sess);
    match cancel.cancellation().ok_or(acq).await {
        Result::Ok(_) => Result::Err(CoopRwLockError::Cancelled),
        Result::Err(r) => r,
    }
}
