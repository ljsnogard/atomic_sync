use core::{
    borrow::BorrowMut,
    ops::{Deref, DerefMut, Try},
    pin::Pin,
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
    upgrade_::UpgradableReaderGuard,
};

#[derive(Debug)]
pub struct WriterGuard<'a, 'g, T, D, B, O>(Pin<&'g mut Acquire<'a, T, D, B, O>>)
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        acquire: Pin<&'g mut Acquire<'a, T, D, B, O>>,
    ) -> Self {
        WriterGuard(acquire)
    }

    pub fn downgrade_to_reader(self) -> ReaderGuard<'a, 'g, T, D, B, O> {
        Acquire::downgrade_writer_to_reader(self)
    }

    pub fn downgrade_to_upgradable(
        self,
    ) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        Acquire::downgrade_writer_to_upgradable(self)
    }
}

impl<'a, T, D, B, O> Drop for WriterGuard<'a, '_, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        self.0.as_mut().drop_writer_guard()
    }
}

impl<'a, 'g, T, D, B, O> BorrowPinMut<'g, Acquire<'a, T, D, B, O>>
for WriterGuard<'a, 'g, T, D, B, O>
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

impl<'a, T, D, B, O> Deref for WriterGuard<'a, '_, T, D, B, O>
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

impl<'a, T, D, B, O> DerefMut for WriterGuard<'a, '_, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().deref_mut_impl()
    }
}

impl<'a, 'g, T, D, B, O> sync_lock::TrReaderGuard<'a, 'g, T>
for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, D, B, O>;
}

impl<'a, 'g, T, D, B, O> sync_lock::TrWriterGuard<'a, 'g, T>
for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: 'a + TrCmpxchOrderings,
{
    #[inline(always)]
    fn downgrade_to_reader(
        self,
    ) -> <Self::Acquire as TrAcquire<'a, T>>::ReaderGuard<'g> {
        WriterGuard::downgrade_to_reader(self)
    }

    #[inline(always)]
    fn downgrade_to_upgradable(
        self,
    ) -> <Self::Acquire as TrAcquire<'a, T>>::UpgradableGuard<'g> {
        WriterGuard::downgrade_to_upgradable(self)
    }
}

pub struct WriteTask<'a, 'g, T, D, B, O>(Pin<&'g mut Acquire<'a, T, D, B, O>>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> WriteTask<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, D, B, O>>) -> Self {
        WriteTask(acquire)
    }

    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<WriterGuard<'a, 'g, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        may_cancel_with_impl_(
            self,
            |t| t.0.as_mut(),
            Acquire::try_write,
            cancel,
        )
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::Output {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'g, T, D, B, O> TrSyncTask for WriteTask<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Output = WriterGuard<'a, 'g, T, D, B, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::Output>
    where
        C: TrCancellationToken,
    {
        WriteTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, T, D, B, O> From<WriteTask<'a, 'g, T, D, B, O>>
for WriterGuard<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn from(task: WriteTask<'a, 'g, T, D, B, O>) -> Self {
        task.wait()
    }
}
