﻿use core::{
    borrow::BorrowMut,
    ops::{Deref, Try},
    pin::Pin,
};

use funty::Unsigned;
use atomex::{
    x_deps::funty,
    Bitwise, TrAtomicData, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_lock,
    sync_tasks::TrSyncTask,
};

use super::{
    acquire_::Acquire,
    rwlock_::SpinningRwLock,
};

#[derive(Debug)]
pub struct ReaderGuard<'a, 'g, T, B, D, O>(Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> ReaderGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>) -> Self {
        ReaderGuard(acquire)
    }
}

impl<'a, T, B, D, O> Deref for ReaderGuard<'a, '_, T, B, D, O>
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
for ReaderGuard<'a, 'g, T, B, D, O>
where
    T: 'a + ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, B, D, O>;
}

impl<'a, T, B, D, O> Drop for ReaderGuard<'a, '_, T, B, D, O>
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

pub struct ReadTask<'a, 'g, T, B, D, O>(Pin<&'g mut Acquire<'a, T, B, D, O>>)
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, B, D, O> ReadTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: Pin<&'g mut Acquire<'a, T, B, D, O>>) -> Self {
        ReadTask(acquire)
    }

    #[inline(always)]
    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<ReaderGuard<'a, 'g, T, B, D, O>>
    where
        C: TrCancellationToken,
    {
        todo!()
    }

    #[inline(always)]
    pub fn wait(self) -> <Self as TrSyncTask>::MayCancelOutput {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'g, T, B, D, O> TrSyncTask for ReadTask<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayCancelOutput = ReaderGuard<'a, 'g, T, B, D, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::MayCancelOutput>
    where
        C: TrCancellationToken,
    {
        ReadTask::may_cancel_with(self, cancel)
    }
}

impl<'a, 'g, T, B, D, O> From<ReadTask<'a, 'g, T, B, D, O>>
for ReaderGuard<'a, 'g, T, B, D, O>
where
    T: ?Sized,
    D: Copy + Unsigned + TrAtomicData,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn from(task: ReadTask<'a, 'g, T, B, D, O>) -> Self {
        task.wait()
    }
}
