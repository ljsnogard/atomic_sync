use core::{
    borrow::BorrowMut,
    ops::{Deref, Try},
    pin::Pin,
    ptr::NonNull,
};

use funty::Unsigned;
use atomex::{
    x_deps::funty,
    Bitwise, TrAtomicData, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_lock::{self, TrAcquire},
    sync_tasks::TrSyncTask,
};

use crate::rwlock::BorrowPinMut;
use super::{
    acquire_::Acquire,
    reader_::ReaderGuard,
    rwlock_::SpinningRwLock,
    writer_::WriterGuard,
};

#[derive(Debug)]
pub struct UpgradableReaderGuard<'a, 'g, T, B, D, O>(
    Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> UpgradableReaderGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>) -> Self {
        UpgradableReaderGuard(acquire)
    }

    pub fn downgrade(self) -> ReaderGuard<'a, 'g, T, B, D, O> {
        todo!()
    }

    pub fn try_upgrade(self) -> Result<WriterGuard<'a, 'g, T, B, D, O>, Self> {
        todo!()
    }

    pub fn upgrade(self) -> Upgrade<'a, 'g, T, B, D, O> {
        todo!()
    }
}

impl<'a, T, B, D, O> Drop for UpgradableReaderGuard<'a, '_, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        todo!()
    }
}

impl<'a, 'g, T, B, D, O> BorrowPinMut<'g, Acquire<'a, T, B, D, O>>
for UpgradableReaderGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn borrow_pin_mut(&mut self) -> &mut Pin<&'g mut Acquire<'a, T, B, D, O>> {
        &mut self.0
    }
}

impl<'a, T, B, D, O> Deref for UpgradableReaderGuard<'a, '_, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref_impl()
    }
}

impl<'a, 'g, T, B, D, O> sync_lock::TrReaderGuard<'a, 'g, T>
for UpgradableReaderGuard<'a, 'g, T, B, D, O>
where
    'a: 'g,
    Self: 'g,
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, B, D, O>;
}

impl<'a, 'g, T, B, D, O> sync_lock::TrUpgradableReaderGuard<'a, 'g, T>
for UpgradableReaderGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    #[inline(always)]
    fn downgrade(self) -> <Self::Acquire as TrAcquire<'a, T>>::ReaderGuard<'g> {
        UpgradableReaderGuard::downgrade(self)
    }

    #[inline(always)]
    fn try_upgrade(
        self,
    ) -> Result<<Self::Acquire as TrAcquire<'a, T>>::WriterGuard<'g> , Self> {
        UpgradableReaderGuard::try_upgrade(self)
    }

    #[inline(always)]
    fn upgrade(
        self,
    ) -> impl sync_lock::TrUpgrade<'a, 'g, T, Acquire = Self::Acquire> {
        UpgradableReaderGuard::upgrade(self)
    }
}

pub struct UpgradableReadTask<'a, 'g, T, B, D, O>(
    Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> UpgradableReadTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>) -> Self {
        UpgradableReadTask(acquire)
    }

    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<UpgradableReaderGuard<'a, 'g, T, B, D, O>>
    where
        C: TrCancellationToken,
    {
        todo!()
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::Output {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'g, T, B, D, O> TrSyncTask for UpgradableReadTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Output = UpgradableReaderGuard<'a, 'g, T, B, D, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::Output>
    where
        C: TrCancellationToken,
    {
        UpgradableReadTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, T, B, D, O> From<UpgradableReadTask<'a, 'g, T, B, D, O>>
for UpgradableReaderGuard<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn from(task: UpgradableReadTask<'a, 'g, T, B, D, O>) -> Self {
        task.wait()
    }
}


pub struct Upgrade<'a, 'g, T, B, D, O>(UpgradableReaderGuard<'a, 'g, T, B, D, O>)
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> Upgrade<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub fn new(guard: UpgradableReaderGuard<'a, 'g, T, B, D, O>) -> Self {
        Upgrade(guard)
    }

    pub fn try_upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> Option<WriterGuard<'a, 'u, T, B, D, O>> {
        todo!()
    }

    pub fn upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> UpgradeTask<'a, 'g, 'u, T, B, D, O>
    where
        'g: 'u,
    {
        UpgradeTask::new(self.guard_pinned())
    }

    pub fn into_guard(self) -> UpgradableReaderGuard<'a, 'g, T, B, D, O> {
        self.0
    }

    fn guard_pinned(
        self: Pin<&mut Self>,
    ) -> Pin<&mut UpgradableReaderGuard<'a, 'g, T, B, D, O>> {
        // Safe to get an `Pin<&mut UpgradableReaderGuard>` without moving it
        unsafe {
            let this = self.get_unchecked_mut();
            Pin::new_unchecked(&mut this.0)
        }
    }
}

impl<'a, 'g, T, B, D, O> sync_lock::TrUpgrade<'a, 'g, T>
for Upgrade<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, B, D, O>;

    #[inline(always)]
    fn try_upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> impl Try<Output = <Self::Acquire as TrAcquire<'a, T>>::WriterGuard<'u>>
    where
        'g: 'u,
    {
        Upgrade::try_upgrade(self)
    }

    #[inline(always)]
    fn upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> impl TrSyncTask<Output =
            <Self::Acquire as TrAcquire<'a, T>>::WriterGuard<'u>>
    where
        'g: 'u,
    {
        Upgrade::upgrade(self)
    }

    #[inline(always)]
    fn into_guard(
        self,
    ) -> <Self::Acquire as TrAcquire<'a, T>>::UpgradableGuard<'g> {
        Upgrade::into_guard(self)
    }
}

pub struct UpgradeTask<'a, 'g, 'u, T, B, D, O>(
    Pin<&'u mut UpgradableReaderGuard<'a, 'g, T, B, D, O>>)
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, 'u, T, B, D, O> UpgradeTask<'a, 'g, 'u, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        guard: Pin<&'u mut UpgradableReaderGuard<'a, 'g, T, B, D, O>>,
    ) -> Self {
        UpgradeTask(guard)
    }

    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<WriterGuard<'a, 'g, T, B, D, O>>
    where
        C: TrCancellationToken,
    {
        let _ = cancel;
        todo!()
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::Output {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'u, T, B, D, O> TrSyncTask for UpgradeTask<'a, '_, 'u, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Output = WriterGuard<'a, 'u, T, B, D, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::Output>
    where
        C: TrCancellationToken,
    {
        UpgradeTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, 'u, T, B, D, O> From<UpgradeTask<'a, 'g, 'u, T, B, D, O>>
for WriterGuard<'a, 'u, T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn from(task: UpgradeTask<'a, 'g, 'u, T, B, D, O>) -> Self {
        task.wait()
    }
}
