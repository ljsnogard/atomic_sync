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

use crate::mutex::embedded::{MsbAsMutexSignal, SpinningMutexBorrowed};
use super::{
    reader_::{ReaderGuard, ReadTask},
    rwlock_::SpinningRwLock,
    upgrade_::{UpgradableReaderGuard, UpgradableReadTask},
    writer_::{WriterGuard, WriteTask},
};

type AcqLinkMutex<'a, O> =
    SpinningMutexBorrowed<'a, (), AtomicUsize, MsbAsMutexSignal<usize>, O>;

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


    #[inline(always)]
    fn try_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::ReaderGuard<'g>>
    where
        'a: 'g,
    {
        Acquire::try_read(self)
    }

    #[inline(always)]
    fn try_write<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::WriterGuard<'g>>
    where
        'a: 'g
    {
        Acquire::try_write(self)
    }

    #[inline(always)]
    fn try_upgradable_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::UpgradableGuard<'g>>
    where
        'a: 'g
    {
        Acquire::try_upgradable_read(self)
    }

    #[inline(always)]
    fn read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<Output = Self::ReaderGuard<'g>>
    where
        'a: 'g
    {
        Acquire::read(self)
    }

    #[inline(always)]
    fn write<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<Output = Self::WriterGuard<'g>>
    where
        'a: 'g
    {
        Acquire::write(self)
    }

    #[inline(always)]
    fn upgradable_read<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<Output = Self::UpgradableGuard<'g>>
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

    const fn guard_() -> *mut AcqLink<O> {
        usize::MAX as *mut AcqLink<O>
    }

    /// Try to become the next of the followed node. 
    /// 
    /// Returns Ok(followed.prev_), or Err(followed.next_) if followed.next is 
    /// not null.
    /// 
    /// ## Safety
    /// * Call this fn only when self is pinned along with `Acquire`
    pub fn try_follow(
        self: Pin<&Self>,
        followed: Pin<&Self>,
    ) -> Result<*mut Self, Pin<&Self>> {
        let mut prev_prev = ptr::null_mut();
        // to lock the previous node
        let guard = Self::guard_();
        loop {
            let r = followed
                .prev_
                .compare_exchange_weak(prev_prev, guard);
            if let Result::Err(x) = r {
                if x != guard {
                    prev_prev = x
                }
            } else {
                break
            }
        };
        if let Option::Some(prev_next) = followed.next_.load() {
            followed.prev_.store(prev_prev);
            let x = unsafe { Pin::new_unchecked(prev_next.as_ref()) };
            Result::Err(x)
        } else {
            followed.prev_.store(prev_prev);
            followed.next_.store(self.get_ref() as *const _ as *mut Self);
            self.prev_.store(followed.get_ref() as *const _ as *mut Self);
            Result::Ok(prev_prev)
        }
    }

    pub fn try_detach(self: Pin<&Self>) -> bool {
        todo!()
    }
}
