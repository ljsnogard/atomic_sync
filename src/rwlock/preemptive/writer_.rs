use core::{
    borrow::BorrowMut,
    ops::{Deref, DerefMut},
};

use funty::Unsigned;
use atomex::{
    x_deps::funty,
    Bitwise, TrAtomicData, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    may_break::TrMayBreak,
    sync_lock::*,
};

use crate::rwlock::TrShareMut;
use super::{
    rwlock_::{Acquire, may_break_with_impl_},
    reader_::ReaderGuard,
    upgrade_::UpgradableReaderGuard,
};

#[derive(Debug)]
pub struct WriterGuard<'a, 'g, T, D, B, O>(&'g mut Acquire<'a, T, D, B, O>)
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
        acquire: &'g mut Acquire<'a, T, D, B, O>,
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
        self.0.drop_writer_guard()
    }
}

impl<'a, 'g, T, D, B, O> TrShareMut<'g, Acquire<'a, T, D, B, O>>
for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn share_mut(&mut self) -> &'g mut Acquire<'a, T, D, B, O> {
        let p = self.0 as *mut _;
        unsafe { &mut *p }
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
        self.0.deref_mut_impl()
    }
}

impl<'a, 'g, T, D, B, O> TrAcqRefGuard<'a, 'g, T> for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

impl<'a, 'g, T, D, B, O> TrAcqMutGuard<'a, 'g, T> for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

impl<'a, 'g, T, D, B, O> TrSyncReaderGuard<'a, 'g, T> for WriterGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, D, B, O>;
}

impl<'a, 'g, T, D, B, O> TrSyncWriterGuard<'a, 'g, T>
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
    ) -> <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::ReaderGuard<'g> {
        WriterGuard::downgrade_to_reader(self)
    }

    #[inline(always)]
    fn downgrade_to_upgradable(
        self,
    ) -> <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::UpgradableGuard<'g> {
        WriterGuard::downgrade_to_upgradable(self)
    }
}

pub struct MayBreakWrite<'a, 'g, T, D, B, O>(&'g mut Acquire<'a, T, D, B, O>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> MayBreakWrite<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: &'g mut Acquire<'a, T, D, B, O>) -> Self {
        MayBreakWrite(acquire)
    }

    #[inline]
    pub fn may_break_with<C>(
        self,
        cancel: &mut C,
    ) -> Option<WriterGuard<'a, 'g, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        may_break_with_impl_(
            self,
            |t| t.0,
            Acquire::try_write,
            cancel,
        )
    }

    #[inline]
    pub fn wait(self) -> Option<WriterGuard<'a, 'g, T, D, B, O>> {
        TrMayBreak::wait(self)
    }

    #[inline]
    pub fn wait_or<F>(self, f: F) -> WriterGuard<'a, 'g, T, D, B, O>
    where
        F: FnOnce() -> WriterGuard<'a, 'g, T, D, B, O>
    {
        TrMayBreak::wait_or(self, f)
    }
}

impl<'a, 'g, T, D, B, O> TrMayBreak for MayBreakWrite<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayBreakOutput = Option<WriterGuard<'a, 'g, T, D, B, O>>;

    #[inline]
    fn may_break_with<C>(self, cancel: &mut C) -> Self::MayBreakOutput
    where
        C: TrCancellationToken,
    {
        MayBreakWrite::may_break_with(self, cancel)
    }
}
