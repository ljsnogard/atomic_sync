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
    rwlock_::{Acquire, may_cancel_with_impl_},
    reader_::ReaderGuard,
    writer_::WriterGuard,
};

#[derive(Debug)]
pub struct UpgradableReaderGuard<'a, 'g, T, D, B, O>(
    Pin<&'g mut Acquire<'a, T, D, B, O>>)
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, D, B, O>>) -> Self {
        UpgradableReaderGuard(acquire)
    }

    pub fn downgrade(self) -> ReaderGuard<'a, 'g, T, D, B, O> {
        Acquire::downgrade_upgradable_to_reader(self)
    }

    pub fn try_upgrade(self) -> Result<WriterGuard<'a, 'g, T, D, B, O>, Self> {
        Acquire::try_upgrade_to_writer(self)
    }

    pub fn upgrade(self) -> Upgrade<'a, 'g, T, D, B, O> {
        Upgrade::new(self)
    }
}

impl<'a, T, D, B, O> Drop for UpgradableReaderGuard<'a, '_, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        self.0.as_mut().drop_upgradable_read_guard()
    }
}

impl<'a, 'g, T, D, B, O> BorrowPinMut<'g, Acquire<'a, T, D, B, O>>
for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn borrow_pin_mut(&mut self) -> &mut Pin<&'g mut Acquire<'a, T, D, B, O>> {
        &mut self.0
    }
}

impl<'a, T, D, B, O> Deref for UpgradableReaderGuard<'a, '_, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref_impl()
    }
}

impl<'a, 'g, T, D, B, O> sync_lock::TrReaderGuard<'a, 'g, T>
for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    Self: 'g,
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, D, B, O>;
}

impl<'a, 'g, T, D, B, O> sync_lock::TrUpgradableReaderGuard<'a, 'g, T>
for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
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

pub struct UpgradableReadTask<'a, 'g, T, D, B, O>(
    Pin<&'g mut Acquire<'a, T, D, B, O>>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> UpgradableReadTask<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, D, B, O>>) -> Self {
        UpgradableReadTask(acquire)
    }

    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<UpgradableReaderGuard<'a, 'g, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        may_cancel_with_impl_(
            self,
            |t| t.0.as_mut(),
            Acquire::try_upgradable_read,
            cancel,
        )
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::MayCancelOutput {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'g, T, D, B, O> TrSyncTask for UpgradableReadTask<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayCancelOutput = UpgradableReaderGuard<'a, 'g, T, D, B, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::MayCancelOutput>
    where
        C: TrCancellationToken,
    {
        UpgradableReadTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, T, D, B, O> From<UpgradableReadTask<'a, 'g, T, D, B, O>>
for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn from(task: UpgradableReadTask<'a, 'g, T, D, B, O>) -> Self {
        task.wait()
    }
}


pub struct Upgrade<'a, 'g, T, D, B, O>(UpgradableReaderGuard<'a, 'g, T, D, B, O>)
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> Upgrade<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub fn new(guard: UpgradableReaderGuard<'a, 'g, T, D, B, O>) -> Self {
        Upgrade(guard)
    }

    pub fn try_upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> Option<WriterGuard<'a, 'u, T, D, B, O>> {
        Acquire::try_upgrade_pinned_to_writer(self.guard_pinned())
    }

    pub fn upgrade<'u>(
        self: Pin<&'u mut Self>,
    ) -> UpgradeTask<'a, 'g, 'u, T, D, B, O>
    where
        'g: 'u,
    {
        UpgradeTask::new(self.guard_pinned())
    }

    pub fn into_guard(self) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        self.0
    }

    fn guard_pinned(
        self: Pin<&mut Self>,
    ) -> Pin<&mut UpgradableReaderGuard<'a, 'g, T, D, B, O>> {
        // Safe to get an `Pin<&mut UpgradableReaderGuard>` without moving it
        unsafe {
            let this = self.get_unchecked_mut();
            Pin::new_unchecked(&mut this.0)
        }
    }
}

impl<'a, 'g, T, D, B, O> sync_lock::TrUpgrade<'a, 'g, T>
for Upgrade<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, D, B, O>;

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
    ) -> impl TrSyncTask<MayCancelOutput =
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

pub struct UpgradeTask<'a, 'g, 'u, T, D, B, O>(
    Pin<&'u mut UpgradableReaderGuard<'a, 'g, T, D, B, O>>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, 'u, T, D, B, O> UpgradeTask<'a, 'g, 'u, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        guard: Pin<&'u mut UpgradableReaderGuard<'a, 'g, T, D, B, O>>,
    ) -> Self {
        UpgradeTask(guard)
    }

    pub fn may_cancel_with<C>(
        mut self,
        cancel: Pin<&mut C>,
    ) -> Option<WriterGuard<'a, 'g, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        let guard_pin = &mut self.0;
        loop {
            let pin = unsafe {
                let ptr = guard_pin.as_mut().get_unchecked_mut();
                let mut p = NonNull::new_unchecked(ptr);
                Pin::new_unchecked(p.as_mut())
            };
            let opt = Acquire::try_upgrade_pinned_to_writer(pin);
            if opt.is_some()  {
                break opt;
            };
            if cancel.is_cancelled() {
                break Option::None;
            }
        }
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::MayCancelOutput {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'u, T, D, B, O> TrSyncTask for UpgradeTask<'a, '_, 'u, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayCancelOutput = WriterGuard<'a, 'u, T, D, B, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::MayCancelOutput>
    where
        C: TrCancellationToken,
    {
        UpgradeTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, 'u, T, D, B, O> From<UpgradeTask<'a, 'g, 'u, T, D, B, O>>
for WriterGuard<'a, 'u, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn from(task: UpgradeTask<'a, 'g, 'u, T, D, B, O>) -> Self {
        task.wait()
    }
}
