﻿use core::{
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
    acquire_::Acquire,
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
        todo!()
    }

    pub fn downgrade_to_upgradable(
        self,
    ) -> UpgradableReaderGuard<'a, 'g, T, B, D, O> {
        todo!()
    }
}

impl<'a, T, B, D, O> Drop for WriterGuard<'a, '_, T, B, D, O>
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

impl<'a, T, B, D, O> Deref for WriterGuard<'a, '_, T, B, D, O>
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

impl<'a, T, B, D, O> DerefMut for WriterGuard<'a, '_, T, B, D, O>
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
        let _ = cancel;
        todo!()
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::MayCancelOutput {
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
    type MayCancelOutput = WriterGuard<'a, 'g, T, B, D, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::MayCancelOutput>
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
