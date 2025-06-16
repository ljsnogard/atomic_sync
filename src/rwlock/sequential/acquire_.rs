use core::{
    borrow::BorrowMut,
    fmt,
    marker::{PhantomData, PhantomPinned},
    ops::Try,
    pin::Pin,
    ptr::{self, NonNull},
    sync::atomic::*,
};

use funty::{Integral, Unsigned};

use atomex::{
    x_deps::funty,
    AtomexPtr, Bitwise, CmpxchResult, StrictOrderings,
    TrAtomicCell, TrAtomicData, TrAtomicFlags, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_lock::{self, TrSyncRwLock},
    sync_tasks::TrSyncTask,
};

use super::{
    reader_::{ReaderGuard, ReadTask},
    rwlock_::SpinningRwLock,
    upgrade_::{UpgradableReaderGuard, UpgradableReadTask},
    writer_::{WriterGuard, WriteTask},
};

pub(super) type AtomAcqLinkPtr<O> =
    AtomexPtr<AcqLink<O>, AtomicPtr<AcqLink<O>>, O>;

#[derive(Debug)]
#[repr(C)]
pub struct Acquire<'a, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    lock_: &'a SpinningRwLock<T, B, D, O>,
    link_: AcqLink<O>,
}

impl<'a, T, B, D, O> Acquire<'a, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    #[inline]
    pub const fn new(rwlock: &'a SpinningRwLock<T, B, D, O>) -> Self {
        Self {
            lock_: rwlock,
            link_: AcqLink::new_unlinked(),
        }
    }

    pub fn try_read(
        self: Pin<&mut Self>,
    ) -> Option<ReaderGuard<'a, '_, T, B, D, O>> {
        todo!()
    }

    pub fn try_write(
        self: Pin<&mut Self>,
    ) -> Option<WriterGuard<'a, '_, T, B, D, O>> {
        todo!()
    }

    pub fn try_upgradable_read(
        self: Pin<&mut Self>,
    ) -> Option<UpgradableReaderGuard<'a, '_, T, B, D, O>> {
        todo!()
    }

    #[inline]
    pub fn read(self: Pin<&mut Self>) -> ReadTask<'a, '_, T, B, D, O> {
        ReadTask::new(self)
    }

    #[inline]
    pub fn write(self: Pin<&mut Self>) -> WriteTask<'a, '_, T, B, D, O> {
        WriteTask::new(self)
    }

    #[inline]
    pub fn upgradable_read(
        self: Pin<&mut Self>,
    ) -> UpgradableReadTask<'a, '_, T, B, D, O> {
        UpgradableReadTask::new(self)
    }
}

impl<T, B, D, O> Acquire<'_, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn deref_impl(&self) -> &T {
        unsafe { &*self.lock_.data_cell_().get() }
    }

    pub(super) fn deref_mut_impl(self: Pin<&mut Self>) -> &mut T {
        unsafe { &mut *self.lock_.data_cell_().get() }
    }
}

impl<'a, T, B, D, O> sync_lock::TrAcquire<'a, T> for Acquire<'a, T, B, D, O>
where
    Self: 'a,
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type ReaderGuard<'g> = ReaderGuard<'a, 'g, T, B, D, O> where 'a: 'g;

    type WriterGuard<'g> = WriterGuard<'a, 'g, T, B, D, O> where 'a: 'g;

    type UpgradableGuard<'g> =
        UpgradableReaderGuard<'a, 'g, T, B, D, O> where 'a: 'g;

    #[inline]
    fn try_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::ReaderGuard<'g>>
    where
        'a: 'g,
    {
        Acquire::try_read(self)
    }

    #[inline]
    fn try_write<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::WriterGuard<'g>>
    where
        'a: 'g
    {
        Acquire::try_write(self)
    }

    #[inline]
    fn try_upgradable_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::UpgradableGuard<'g>>
    where
        'a: 'g
    {
        Acquire::try_upgradable_read(self)
    }

    #[inline]
    fn read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<MayCancelOutput = Self::ReaderGuard<'g>>
    where
        'a: 'g
    {
        Acquire::read(self)
    }

    #[inline]
    fn write<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<MayCancelOutput = Self::WriterGuard<'g>>
    where
        'a: 'g
    {
        Acquire::write(self)
    }

    #[inline]
    fn upgradable_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<MayCancelOutput = Self::UpgradableGuard<'g>>
    where
        'a: 'g
    {
        Acquire::upgradable_read(self)
    }
}

#[derive(Debug)]
#[repr(C)]
pub(super) struct AcqLink<O>
where
    O: TrCmpxchOrderings,
{
    /// Also plays the role as a mutex of current node.
    prev_: AtomAcqLinkPtr<O>,
    next_: AtomAcqLinkPtr<O>,
    _pin_: PhantomPinned,
}

impl<O> AcqLink<O>
where
    O: TrCmpxchOrderings,
{
    pub const fn new(
        prev: *mut AcqLink<O>,
        next: *mut AcqLink<O>,
    ) -> Self {
        Self {
            prev_: AtomAcqLinkPtr::new(AtomicPtr::new(prev)),
            next_: AtomAcqLinkPtr::new(AtomicPtr::new(next)),
            _pin_: PhantomPinned,
        }
    }

    #[inline]
    pub const fn new_unlinked() -> Self {
        Self::new(ptr::null_mut(), ptr::null_mut())
    }

    pub const unsafe fn acq_ptr<'a, T, D, B>(
        self: Pin<&Self>,
    ) -> NonNull<Acquire<'a, T, B, D, O>>
    where
        T: ?Sized,
        B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
        D: TrAtomicData + Unsigned,
        <D as TrAtomicData>::AtomicCell: Bitwise,
        O: TrCmpxchOrderings,
    {
        let count= core::mem::offset_of!(Acquire<'a, T, B, D, O>, link_);
        let this = self.get_ref() as *const _ as *mut Self;
        let ptr = unsafe { (this as *mut u8).sub(count) };
        let ptr = ptr as *mut Acquire<'a, T, B, D, O>;
        unsafe { NonNull::new_unchecked(ptr) }
    }

    /// Try to insert a candidate node as the next node to the current node.
    /// 
    /// Returns Ok(candidate), or Err(Some(self.next_)) if self.next not null,
    /// or Err(None) if cancelled.
    /// 
    /// ## Safety
    /// * Call this fn only when self is pinned along with `Acquire`
    fn try_insert_next_<'a, 'f, C: TrCancellationToken>(
        self: Pin<&'a Self>,
        candidate: Pin<&'a Self>,
        mut cancel: Pin<&'f mut C>,
    ) -> Result<Pin<&'a Self>, Option<Pin<&'a Self>>>
    where
        'a: 'f,
    {
        // try_to lock candidate node
        let res = AcqLinkGuard::try_acquire(candidate, cancel.as_mut());
        let Result::Ok(mut candidate_guard) = res else {
            return Result::Err(Option::None)
        };
        // to lock the previous node
        let res = AcqLinkGuard::try_acquire(self, cancel);
        let Result::Ok(this_guard) = res else {
            return Result::Err(Option::None)
        };
        // followed.next_ is not null,
        if let Option::Some(prev_next) = self.next_.load() {
            let x = unsafe { Pin::new_unchecked(prev_next.as_ref()) };
            Result::Err(Option::Some(x))
        } else {
            candidate_guard.update_prev(self.get_ref() as *const _ as *mut _);
            drop(this_guard);
            Result::Ok(candidate)
        }
    }

    pub fn try_detach_(self: Pin<&Self>) -> bool {
        todo!()
    }
}

struct AcqLinkGuard<'a, O>
where
    O: TrCmpxchOrderings,
{
    node_: Pin<&'a AcqLink<O>>,
    prev_: *mut AcqLink<O>,
}

impl<'a, O> AcqLinkGuard<'a, O>
where
    O: TrCmpxchOrderings,
{
    pub const fn guarded() -> *mut AcqLink<O> {
        usize::MAX as *mut AcqLink<O>
    }

    pub fn try_acquire<'f, C: TrCancellationToken>(
        node: Pin<&'a AcqLink<O>>,
        cancel: Pin<&'f mut C>,
    ) -> Result<Self, *mut AcqLink<O>>
    where
        'a: 'f,
    {
        let desired = Self::guarded();
        let mut current = ptr::null_mut();
        loop {
            match node.prev_.compare_exchange_weak(current, desired) {
                Result::Err(x) => {
                    if cancel.is_cancelled() {
                        break Result::Err(x)
                    }
                    if x != Self::guarded() {
                        current = x
                    }
                },
                Result::Ok(p) => {
                    break Result::Ok(AcqLinkGuard { node_: node, prev_: p })
                },
            }
        }
    }

    pub fn update_prev(&mut self, prev: *mut AcqLink<O>) {
        self.prev_ = prev
    }
}

impl<O> Drop for AcqLinkGuard<'_, O>
where
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        let guard = Self::guarded();
        let expect = |p: *mut AcqLink<O>| ptr::eq(p, guard);
        let desire = |_| self.prev_;
        let x = self
            .node_
            .prev_
            .try_spin_compare_exchange_weak(expect, desire);
        assert!(x.is_succ())
    }
}
