use core::{
    borrow::BorrowMut,
    ops::{Deref, Try},
};

use funty::Unsigned;
use atomex::{
    x_deps::funty,
    Bitwise, TrAtomicData, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::{NonCancellableToken, TrCancellationToken},
    may_break::TrMayBreak,
    sync_lock::*,
};

use crate::rwlock::TrShareMut;
use super::{
    rwlock_::{Acquire, may_break_with_impl_},
    reader_::ReaderGuard,
    writer_::WriterGuard,
};

#[derive(Debug)]
pub struct UpgradableReaderGuard<'a, 'g, T, D, B, O>(&'g mut Acquire<'a, T, D, B, O>)
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
    pub(super) fn new(acquire: &'g mut Acquire<'a, T, D, B, O>) -> Self {
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
        self.0.drop_upgradable_read_guard()
    }
}

impl<'a, 'g, T, D, B, O> TrShareMut<'g, Acquire<'a, T, D, B, O>> for UpgradableReaderGuard<'a, 'g, T, D, B, O>
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

impl<'a, 'g, T, D, B, O> TrAcqRefGuard<'a, 'g, T> for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    'a: 'g,
    Self: 'g,
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

impl<'a, 'g, T, D, B, O> TrSyncReaderGuard<'a, 'g, T> for UpgradableReaderGuard<'a, 'g, T, D, B, O>
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

impl<'a, 'g, T, D, B, O> TrSyncUpgradableReaderGuard<'a, 'g, T> for UpgradableReaderGuard<'a, 'g, T, D, B, O>
where
    T: 'a + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    #[inline]
    fn downgrade(self) -> <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::ReaderGuard<'g> {
        UpgradableReaderGuard::downgrade(self)
    }

    #[inline]
    fn try_upgrade(
        self,
    ) -> Result<<Self::Acquire as TrSyncRwLockAcquire<'a, T>>::WriterGuard<'g> , Self> {
        UpgradableReaderGuard::try_upgrade(self)
    }

    #[inline]
    fn upgrade(
        self,
    ) -> impl TrSyncUpgrade<'a, 'g, T, Acquire = Self::Acquire> {
        UpgradableReaderGuard::upgrade(self)
    }
}

pub struct MayBreakUpgradableRead<'a, 'g, T, D, B, O>(&'g mut Acquire<'a, T, D, B, O>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, O> MayBreakUpgradableRead<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(acquire: &'g mut Acquire<'a, T, D, B, O>) -> Self {
        MayBreakUpgradableRead(acquire)
    }

    pub fn may_break_with<C>(
        self,
        cancel: &mut C,
    ) -> Option<UpgradableReaderGuard<'a, 'g, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        may_break_with_impl_(
            self,
            |t| t.0,
            Acquire::try_upgradable_read,
            cancel,
        )
    }

    #[inline]
    pub fn wait(self) -> Option<UpgradableReaderGuard<'a, 'g, T, D, B, O>> {
        TrMayBreak::wait(self)
    }

    #[inline]
    pub fn wait_or<F>(self, f: F) -> UpgradableReaderGuard<'a, 'g, T, D, B, O>
    where
        F: FnOnce() -> UpgradableReaderGuard<'a, 'g, T, D, B, O>,
    {
        TrMayBreak::wait_or(self, f)
    }
}

impl<'a, 'g, T, D, B, O> TrMayBreak for MayBreakUpgradableRead<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayBreakOutput = Option<UpgradableReaderGuard<'a, 'g, T, D, B, O>>;

    #[inline]
    fn may_break_with<C>(self, cancel: &mut C) -> Self::MayBreakOutput
    where
        C: TrCancellationToken,
    {
        MayBreakUpgradableRead::may_break_with(self, cancel)
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
        &'u mut self,
    ) -> Option<WriterGuard<'a, 'u, T, D, B, O>> {
        Acquire::try_upgrade_mut_to_writer(self.guard_mut())
    }

    pub fn upgrade<'u>(
        &'u mut self,
    ) -> MayBreakUpgrade<'a, 'g, 'u, T, D, B, O>
    where
        'g: 'u,
    {
        MayBreakUpgrade::new(self)
    }

    pub fn into_guard(self) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        self.0
    }

    pub fn upgrade_with_cancel<'u, C>(
        &'u mut self,
        cancel: &mut C,
    ) -> Option<WriterGuard<'a, 'u, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        let guard_ptr = self.guard_mut() as *mut _;
        loop {
            let guard_mut = unsafe { &mut *guard_ptr };
            let opt = Acquire::try_upgrade_mut_to_writer(guard_mut);
            if opt.is_some()  {
                break opt;
            };
            if cancel.is_cancelled() {
                break Option::None;
            }
        }
    }

    fn guard_mut(&mut self) -> &mut UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        // Safe to get an `Pin<&mut UpgradableReaderGuard>` without moving it
        &mut self.0
    }
}

impl<'a, 'g, T, D, B, O> TrSyncUpgrade<'a, 'g, T> for Upgrade<'a, 'g, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Acquire = Acquire<'a, T, D, B, O>;

    #[inline]
    fn try_upgrade<'u>(
        &'u mut self,
    ) -> impl Try<Output = <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::WriterGuard<'u>>
    where
        'g: 'u,
    {
        Upgrade::try_upgrade(self)
    }

    #[inline]
    fn upgrade<'u>(
        &'u mut self,
    ) -> impl TrMayBreak<MayBreakOutput: Try<Output = 
            <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::WriterGuard<'u>>>
    where
        'g: 'u,
    {
        Upgrade::upgrade(self)
    }

    #[inline]
    fn into_guard(
        self,
    ) -> <Self::Acquire as TrSyncRwLockAcquire<'a, T>>::UpgradableGuard<'g> {
        Upgrade::into_guard(self)
    }
}

pub struct MayBreakUpgrade<'a, 'g, 'u, T, D, B, O>(&'u mut Upgrade<'a, 'g, T, D, B, O>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, 'g, 'u, T, D, B, O> MayBreakUpgrade<'a, 'g, 'u, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        upgrade: &'u mut Upgrade<'a, 'g, T, D, B, O>,
    ) -> Self {
        MayBreakUpgrade(upgrade)
    }

    pub fn may_break_with<C>(
        self,
        cancel: &mut C,
    ) -> Option<WriterGuard<'a, 'u, T, D, B, O>>
    where
        C: TrCancellationToken,
    {
        self.0.upgrade_with_cancel(cancel)
    }

    #[inline]
    pub fn wait(self) -> Option<WriterGuard<'a, 'u, T, D, B, O>> {
        self.may_break_with(NonCancellableToken::shared_mut())
    }

    #[inline]
    pub fn wait_or<F>(self, f: F) -> WriterGuard<'a, 'u, T, D, B, O>
    where
        F: FnOnce() -> WriterGuard<'a, 'u, T, D, B, O>,
    {
        TrMayBreak::wait_or(self, f)
    }
}

impl<'a, 'u, T, D, B, O> TrMayBreak for MayBreakUpgrade<'a, '_, 'u, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type MayBreakOutput = Option<WriterGuard<'a, 'u, T, D, B, O>>;

    #[inline]
    fn may_break_with<C>(self, cancel: &mut C) -> Self::MayBreakOutput
    where
        C: TrCancellationToken,
    {
        MayBreakUpgrade::may_break_with(self, cancel)
    }
}
