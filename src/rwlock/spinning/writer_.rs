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
    x_deps::atomex
};

use super::{
    impl_::{Acquire, BorrowPinMut, may_cancel_with_impl_},
    reader_::ReaderGuard,
    upgrade_::UpgradableReaderGuard,
};

#[derive(Debug)]
pub struct WriterGuard<'a, 'g, T, B, D, O>(Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> WriterGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>,
    ) -> Self {
        WriterGuard(acquire)
    }

    pub fn downgrade_to_reader(self) -> ReaderGuard<'a, 'g, T, B, D, O> {
        Acquire::downgrade_writer_to_reader(self)
    }

    pub fn downgrade_to_upgradable(
        self,
    ) -> UpgradableReaderGuard<'a, 'g, T, B, D, O> {
        Acquire::downgrade_writer_to_upgradable(self)
    }
}

impl<'a, 'g, T, B, D, O> Drop for WriterGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        self.0.as_mut().drop_writer_guard()
    }
}

impl<'a, 'g, T, B, D, O> BorrowPinMut<'g, Acquire<'a, T, B, D, O>>
for WriterGuard<'a, 'g, T, B, D, O>
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

impl<'a, 'g, T, B, D, O> Deref for WriterGuard<'a, 'g, T, B, D, O>
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

impl<'a, 'g, T, B, D, O> DerefMut for WriterGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().deref_mut_impl()
    }
}

impl<'a, 'g, T, B, D, O> sync_lock::TrReaderGuard<'a, 'g, T>
for WriterGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, B, D, O>;
}

impl<'a, 'g, T, B, D, O> sync_lock::TrWriterGuard<'a, 'g, T>
for WriterGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
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

pub struct WriteTask<'a, 'g, T, B, D, O>(Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> WriteTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>) -> Self {
        WriteTask(acquire)
    }

    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<WriterGuard<'a, 'g, T, B, D, O>>
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

impl<'a, 'g, T, B, D, O> TrSyncTask for WriteTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Output = WriterGuard<'a, 'g, T, B, D, O>;

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

impl<'a, 'g, T, B, D, O> From<WriteTask<'a, 'g, T, B, D, O>>
for WriterGuard<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn from(task: WriteTask<'a, 'g, T, B, D, O>) -> Self {
        task.wait()
    }
}
