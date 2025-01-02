use core::{
    borrow::BorrowMut,
    cell::UnsafeCell,
    fmt,
    marker::{PhantomData, PhantomPinned},
    mem::ManuallyDrop,
    ops::Try,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::*,
};

use funty::{Integral, Unsigned};

use atomex::{
    x_deps::funty,
    Bitwise, StrictOrderings, TrAtomicCell, TrAtomicData, TrAtomicFlags,
    TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_lock::{self, TrSyncRwLock},
    sync_tasks::TrSyncTask,
};

use crate::rwlock::BorrowPinMut;
use super::{
    reader_::{ReadTask, ReaderGuard},
    writer_::{WriteTask, WriterGuard},
    upgrade_::{UpgradableReadTask, UpgradableReaderGuard},
};

pub type SpinningRwLockBorrowed<'a, T, C = AtomicUsize, O = StrictOrderings> =
    SpinningRwLock<T, <C as TrAtomicCell>::Value, &'a mut C, O>;

pub type SpinningRwLockOwned<T, C = AtomicUsize, O = StrictOrderings> =
    SpinningRwLock<T, <C as TrAtomicCell>::Value, C, O>;

impl<T, C, O> SpinningRwLockOwned<T, C, O>
where
    C: TrAtomicCell + Bitwise,
    <C as TrAtomicCell>::Value: TrAtomicData<AtomicCell = C> + Unsigned,
    O: TrCmpxchOrderings,
{
    pub fn new_owned(data: T) -> Self {
        let val = <<C as TrAtomicCell>::Value as Integral>::ZERO;
        let cell = <C as TrAtomicCell>::new(val);
        SpinningRwLockOwned::<T, C, O>::new(data, cell)
    }
}

#[derive(Debug)]
pub struct SpinningRwLock<T, D = usize, B = AtomicUsize, O = StrictOrderings>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    _pin_: PhantomPinned,
    stat_: RwLockState<D, B, O>,
    data_: UnsafeCell<T>,
}

impl<T, D, B, O> SpinningRwLock<T, D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    /// Creates a new spinlock wrapping the supplied data.
    pub const fn new(data: T, cell: B) -> Self {
        SpinningRwLock {
            _pin_: PhantomPinned,
            stat_: RwLockState::new(cell),
            data_: UnsafeCell::new(data),
        }
    }

    /// Consumes this `RwLock`, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        // We know statically that there are no outstanding references to
        // `self` so there's no need to lock.
        self.data_.into_inner()
    }
}

impl<T, D, B, O> SpinningRwLock<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    /// Return the number of readers that currently hold the lock, including
    /// upgradable readers and upgraded writer.
    ///
    /// ## Safety
    ///
    /// This function provides no synchronization guarantees and so its result
    /// should be considered 'out of date' the instant it is called. Do not use
    /// it for synchronization purposes. However, it may be useful as a
    /// heuristic.
    /// 
    /// ## Example
    /// 
    /// ```
    /// use pin_utils::pin_mut;
    /// use atomic_sync::{
    ///     rwlock::preemptive::SpinningRwLockOwned,
    ///     x_deps::pin_utils,
    /// };
    ///
    /// let lock = SpinningRwLockOwned::<()>::new_owned(());
    /// assert_eq!(lock.reader_count(), 0);
    ///
    /// let acq0 = lock.acquire();
    /// pin_mut!(acq0);
    /// let r0 = acq0.upgradable_read().wait();
    /// assert_eq!(lock.reader_count(), 1);
    /// 
    /// let acq1 = lock.acquire();
    /// pin_mut!(acq1);
    /// let r1 = acq1.read().wait();
    /// assert_eq!(lock.reader_count(), 2);
    /// 
    /// let upg = r0.upgrade();
    /// pin_mut!(upg);
    /// assert_eq!(lock.reader_count(), 2);
    ///
    /// drop(r1);
    /// assert_eq!(lock.reader_count(), 1);
    ///
    /// let writer_guard = upg.upgrade().wait();
    /// assert_eq!(lock.reader_count(), 1);
    /// ```
    pub fn reader_count(&self) -> usize {
        let Result::Ok(c) = self.stat_.reader_count().try_into() else {
            unreachable!("[SpinningRwLock::reader_count]")
        };
        c
    }

    pub const fn acquire(&self) -> Acquire<'_, T, D, B, O> {
        Acquire::new(self)
    }

    /// Returns a mutable pointer to the underlying data.
    ///
    /// This is mostly meant to be used for applications which require manual
    /// unlocking, but where storing both the lock and the pointer to the inner
    /// data gets inefficient.
    ///
    /// While this is safe, writing to the data is undefined behavior unless the
    /// current thread has acquired a writer guard, and reading requires either
    /// a reader or writer guard.
    ///
    /// # Example
    /// ```
    /// use core::mem::ManuallyDrop;
    /// use pin_utils::pin_mut;
    /// use atomic_sync::{
    ///     rwlock::preemptive::SpinningRwLockOwned,
    ///     x_deps::pin_utils,
    /// };
    ///
    /// let lock = SpinningRwLockOwned::<usize>::new_owned(42);
    /// let acq = lock.acquire();
    /// pin_mut!(acq);
    /// unsafe {
    ///     let mut m = ManuallyDrop::new(acq.as_mut().write().wait());
    ///
    ///     assert_eq!(lock.as_mut_ptr().read(), 42);
    ///     lock.as_mut_ptr().write(58);
    ///
    ///     ManuallyDrop::drop(&mut m);
    /// }
    ///
    /// assert_eq!(*(acq.as_mut().read().wait()), 58);
    /// ```
    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.data_.get()
    }

    fn state_(&self) -> &RwLockState<D, B, O> {
        &self.stat_
    }
}

impl<T, D, B, O> TrSyncRwLock for SpinningRwLock<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Target = T;

    #[inline]
    fn acquire(&self) -> impl sync_lock::TrAcquire<'_, Self::Target> {
        SpinningRwLock::acquire(self)
    }
}

// Same unsafe impls as `std::sync::RwLock`
unsafe impl<T, D, B, O> Send for SpinningRwLock<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

unsafe impl<T, D, B, O> Sync for SpinningRwLock<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

#[derive(Debug)]
pub struct Acquire<'a, T, D, B, O>(&'a SpinningRwLock<T, D, B, O>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, T, D, B, O> Acquire<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    #[inline]
    pub const fn new(lock: &'a SpinningRwLock<T, D, B, O>) -> Self {
        Acquire(lock)
    }

    pub fn try_read(
        self: Pin<&mut Self>,
    ) -> Option<ReaderGuard<'a, '_, T, D, B, O>> {
        if self.0.state_().try_read() {
            Option::Some(ReaderGuard::new(self))
        } else {
            Option::None
        }
    }

    pub fn try_write(
        self: Pin<&mut Self>,
    ) -> Option<WriterGuard<'a, '_, T, D, B, O>> {
        if self.0.state_().try_write() {
            Option::Some(WriterGuard::new(self))
        } else {
            Option::None
        }
    }

    pub fn try_upgradable_read(
        self: Pin<&mut Self>,
    ) -> Option<UpgradableReaderGuard<'a, '_, T, D, B, O>> {
        if self.0.state_().try_upgradable_read() {
            Option::Some(UpgradableReaderGuard::new(self))
        } else {
            Option::None
        }
    }

    #[inline]
    pub fn read(self: Pin<&mut Self>) -> ReadTask<'a, '_, T, D, B, O> {
        ReadTask::new(self)
    }

    #[inline]
    pub fn write(self: Pin<&mut Self>) -> WriteTask<'a, '_, T, D, B, O> {
        WriteTask::new(self)
    }

    #[inline]
    pub fn upgradable_read(
        self: Pin<&mut Self>,
    ) -> UpgradableReadTask<'a, '_, T, D, B, O> {
        UpgradableReadTask::new(self)
    }
}

impl<'a, T, D, B, O> Acquire<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub(super) fn downgrade_writer_to_reader<'g>(
        guard: WriterGuard<'a, 'g, T, D, B, O>,
    ) -> ReaderGuard<'a, 'g, T, D, B, O> {
        let acquire = Self::destruct_guard_(guard);
        let lock = acquire.0;
        if lock.state_().try_downgrade_write_to_read() {
            ReaderGuard::new(acquire)
        } else {
            unreachable!("[Acquire::downgrade_writer_to_reader]")
        }
    }

    pub(super) fn downgrade_writer_to_upgradable<'g>(
        guard: WriterGuard<'a, 'g, T, D, B, O>,
    ) -> UpgradableReaderGuard<'a, 'g, T, D, B, O> {
        let acquire = Self::destruct_guard_(guard);
        let lock = acquire.0;
        if lock.state_().try_downgrade_write_to_upgradable() {
            UpgradableReaderGuard::new(acquire)
        } else {
            unreachable!("[Acquire::downgrade_writer_to_upgradable]")
        }
    }

    pub(super) fn downgrade_upgradable_to_reader<'g>(
        guard: UpgradableReaderGuard<'a, 'g, T, D, B, O>,
    ) -> ReaderGuard<'a, 'g, T, D, B, O> {
        let acquire = Self::destruct_guard_(guard);
        let lock = acquire.0;
        if lock.state_().try_downgrade_upgradable_to_read() {
            ReaderGuard::new(acquire)
        } else {
            unreachable!("[Acquire::downgrade_upgradable_to_reader]")
        }
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn try_upgrade_to_writer<'g>(
        mut guard: UpgradableReaderGuard<'a, 'g, T, D, B, O>,
    ) -> Result<
            WriterGuard<'a, 'g, T, D, B, O>,
            UpgradableReaderGuard<'a, 'g, T, D, B, O>,
        >
    {
        let guard_pin = unsafe {
            let mut p = NonNull::new_unchecked(&mut guard);
            Pin::new_unchecked(p.as_mut())
        };
        if let Option::Some(g) = Self::try_upgrade_pinned_to_writer(guard_pin) {
            Result::Ok(g)
        } else {
            Result::Err(guard)
        }
    }

    pub(super) fn try_upgrade_pinned_to_writer<'g, 'u>(
        mut guard: Pin<&'u mut UpgradableReaderGuard<'a, 'g, T, D, B, O>>,
    ) -> Option<WriterGuard<'a, 'u, T, D, B, O>> {
        let acq_pin = guard.borrow_pin_mut();
        let lock = acq_pin.0;
        if lock.state_().try_upgrade_upgradable_to_write() {
            let acquire = unsafe {
                let ptr = acq_pin.as_mut().get_unchecked_mut();
                let mut p = NonNull::new_unchecked(ptr);
                Pin::new_unchecked(p.as_mut())
            };
            Option::Some(WriterGuard::new(acquire))
        } else {
            Option::None
        }
    }

    fn destruct_guard_<'g, G>(guard: G) -> Pin<&'g mut Self>
    where
        Self: 'g,
        G: BorrowPinMut<'g, Self>,
    {
        let mut m = ManuallyDrop::new(guard);
        let pin = (*m).borrow_pin_mut().as_mut();
        unsafe {
            let ptr = pin.get_unchecked_mut();
            let mut p = NonNull::new_unchecked(ptr);
            Pin::new_unchecked(p.as_mut())
        }
    }

    pub(super) fn deref_impl(&self) -> &T {
        unsafe { &*self.0.data_.get() }
    }

    pub(super) fn deref_mut_impl(self: Pin<&mut Self>) -> &mut T {
        unsafe { &mut *self.0.data_.get() }
    }

    pub(super) fn drop_reader_guard(self: Pin<&mut Self>) {
        let x = self.0.stat_.decrease_reader_count();
        debug_assert!(x.is_ok())
    }

    pub(super) fn drop_writer_guard(self: Pin<&mut Self>) {
        self.0.stat_.on_writer_guard_drop()
    }

    pub(super) fn drop_upgradable_read_guard(self: Pin<&mut Self>) {
        let x = self.0.stat_.on_upgradable_read_drop();
        debug_assert!(x);
    }
}

impl<'a, T, D, B, O> sync_lock::TrAcquire<'a, T> for Acquire<'a, T, D, B, O>
where
    Self: 'a,
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type ReaderGuard<'g> = ReaderGuard<'a, 'g, T, D, B, O> where 'a: 'g;

    type WriterGuard<'g> = WriterGuard<'a, 'g, T, D, B, O> where 'a: 'g;

    type UpgradableGuard<'g> =
            UpgradableReaderGuard<'a, 'g, T, D, B, O> where 'a: 'g;

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

struct RwLockState<D, B, O>(B, PhantomData<D>, PhantomData<O>)
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<D, B, O> AsRef<D::AtomicCell> for RwLockState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn as_ref(&self) -> &D::AtomicCell {
        self.0.borrow()
    }
}

impl<D, B, O> TrAtomicFlags<D, O> for RwLockState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

impl<D, B, O> RwLockState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub const fn new(cell: B) -> Self {
        RwLockState(cell, PhantomData, PhantomData)
    }

    // const K_NOT_LOCKED: D = D::ZERO;

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_WRITER_ACTIVE() -> D {
        D::ONE << (D::BITS - 1)
    }
    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_WRITER_QUEUED() -> D {
        D::ONE << (D::BITS - 2)
    }
    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_UPGRADE_ACTIVE() -> D {
        D::ONE << (D::BITS - 3)
    }
    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_MAX_READER_COUNT() -> D {
        D::MAX >> 3
    }

    #[inline]
    fn try_spin_update_(
        &self,
        expect: impl FnMut(D) -> bool,
        desire: impl FnMut(D) -> D,
    ) -> Result<D, D> {
        self.try_spin_compare_exchange_weak(expect, desire)
            .into()
    }

    #[inline]
    fn load_state(&self) -> D {
        TrAtomicFlags::value(self)
    }

    // fn expect_lock_released(state: D) -> bool {
    //     state == Self::K_NOT_LOCKED
    // }

    fn expect_writer_active(s: D) -> bool {
        s & Self::K_WRITER_ACTIVE() == Self::K_WRITER_ACTIVE()
    }
    fn expect_writer_inactive(s: D) -> bool {
        !Self::expect_writer_active(s)
    }
    fn desire_writer_active(s: D) -> D {
        s | Self::K_WRITER_ACTIVE()
    }
    fn desire_writer_inactive(s: D) -> D {
        s & (!Self::K_WRITER_ACTIVE())
    }

    fn expect_writer_queued(s: D) -> bool {
        s & Self::K_WRITER_QUEUED() == Self::K_WRITER_QUEUED()
    }
    fn expect_writer_not_queued(s: D) -> bool {
        !Self::expect_writer_queued(s)
    }
    fn desire_writer_queued(s: D) -> D {
        s | Self::K_WRITER_QUEUED()
    }
    fn desire_writer_not_queued(s: D) -> D {
        s & (!Self::K_WRITER_QUEUED())
    }

    fn expect_upgrade_active(s: D) -> bool {
        s & Self::K_UPGRADE_ACTIVE() == Self::K_UPGRADE_ACTIVE()
    }
    fn expect_upgrade_inactive(s: D) -> bool {
        !Self::expect_upgrade_active(s)
    }
    fn desire_upgrade_inactive(s: D) -> D {
        s & (!Self::K_UPGRADE_ACTIVE())
    }
    fn desire_upgrade_active(s: D) -> D {
        s | Self::K_UPGRADE_ACTIVE()
    }

    fn get_reader_count(s: D) -> D {
        s & Self::K_MAX_READER_COUNT()
    }
    fn expect_reader_lt_max(s: D) -> bool {
        Self::get_reader_count(s) < Self::K_MAX_READER_COUNT()
    }
    fn expect_reader_gt_min(s: D) -> bool {
        Self::get_reader_count(s) > D::ZERO
    }
    fn desire_inc_reader(s: D) -> D {
        s + D::ONE
    }
    fn desire_dec_reader(s: D) -> D {
        s - D::ONE
    }

    #[inline]
    pub fn on_writer_guard_drop(&self) {
        let r = self.try_spin_update_(
            Self::expect_writer_active,
            Self::desire_writer_not_queued_inactive_,
        );
        debug_assert!(r.is_ok())
    }
    fn desire_writer_not_queued_inactive_(s: D) -> D {
        let s = Self::desire_writer_inactive(s);
        Self::desire_writer_not_queued(s)
    }

    #[inline]
    pub fn reader_count(&self) -> D {
        Self::get_reader_count(self.load_state())
    }
 
    pub fn decrease_reader_count(&self) -> Result<D, D> {
        self.try_spin_update_(
                Self::expect_reader_gt_min,
                Self::desire_dec_reader)
            .map(Self::get_reader_count)
            .map_err(Self::get_reader_count)
    }

    /// When returning true, will increase reader count in RwLock::state_.
    pub fn try_read(&self) -> bool {
        self.try_spin_update_(
                Self::expect_can_read,
                Self::desire_inc_reader)
            .is_ok()
    }
    fn expect_can_read(state: D) -> bool {
        Self::expect_reader_lt_max(state)
            && Self::expect_writer_not_queued(state)
            && Self::expect_writer_inactive(state)
    }

    pub fn try_write(&self) -> bool {
        let r = self.try_spin_update_(
            Self::expect_can_write,
            Self::desire_writer_active,
        );
        if r.is_ok() {
            return true;
        }
        let _ = self.try_spin_update_(
            Self::expect_writer_not_queued,
            Self::desire_writer_queued,
        );
        false
    }
    fn expect_can_write(s: D) -> bool {
        Self::get_reader_count(s) == D::ZERO
            && Self::expect_writer_inactive(s)
    }

    pub fn try_upgradable_read(&self) -> bool {
        self.try_spin_update_(
                Self::expect_write_not_queued_and_upgrade_inactive,
                Self::desire_upgrade_active_and_reader_inc)
            .is_ok()
    }
    fn expect_write_not_queued_and_upgrade_inactive(s: D) -> bool {
        Self::expect_writer_not_queued(s)
            && Self::expect_writer_inactive(s)
            && Self::expect_upgrade_inactive(s)
    }
    fn desire_upgrade_active_and_reader_inc(s: D) -> D {
        let s = Self::desire_upgrade_active(s);
        Self::desire_inc_reader(s)
    }

    pub fn on_upgradable_read_drop(&self) -> bool {
        self.try_spin_update_(
                Self::expect_upgrade_active,
                Self::desire_upgrade_inactive_and_reader_dec)
            .is_ok()
    }
    fn desire_upgrade_inactive_and_reader_dec(s: D) -> D {
        let s = Self::desire_upgrade_inactive(s);
        Self::desire_dec_reader(s)
    }

    pub fn try_downgrade_write_to_read(&self) -> bool {
        self.try_spin_update_(
                Self::expect_writer_active,
                Self::desire_write_to_read_)
            .is_ok()
    }
    fn desire_write_to_read_(s: D) -> D {
        let s = Self::desire_writer_inactive(s);
        Self::desire_inc_reader(s)
    }

    pub fn try_downgrade_write_to_upgradable(&self) -> bool {
        self.try_spin_update_(
                Self::expect_writer_active,
                Self::desire_write_to_upgradable_)
            .is_ok()
    }
    fn desire_write_to_upgradable_(s: D) -> D {
        let s = Self::desire_writer_inactive(s);
        let s = Self::desire_upgrade_active(s);
        Self::desire_inc_reader(s)
    }

    pub fn try_downgrade_upgradable_to_read(&self) -> bool {
        self.try_spin_update_(
                Self::expect_can_downgrade_to_read_,
                Self::desire_upgrade_inactive)
            .is_ok()
    }
    fn expect_can_downgrade_to_read_(s: D) -> bool {
        Self::expect_upgrade_active(s)
            && Self::expect_writer_inactive(s)
            && Self::get_reader_count(s) > D::ZERO
    }

    pub fn try_upgrade_upgradable_to_write(&self) -> bool {
        self.try_spin_update_(
                Self::expect_can_upgrade_,
                Self::desire_upgradable_to_write_)
            .is_ok()
    }
    fn expect_can_upgrade_(s: D) -> bool {
        Self::expect_upgrade_active(s)
            && Self::get_reader_count(s) == D::ONE
    }
    fn desire_upgradable_to_write_(s: D) -> D {
        Self::desire_writer_active(s)
    }
}

impl<D, B, O> fmt::Debug for RwLockState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.load_state();
        write!(
            f,
            "[RwLockState: W({}), Q({}), U({}), R({})]",
            Self::expect_writer_active(s),
            Self::expect_writer_queued(s),
            Self::expect_upgrade_active(s),
            Self::get_reader_count(s),
        )
    }
}

type FpTryAcquire<'a, 'g, T, B, D, O, X> =
    fn(Pin<&'g mut Acquire<'a, T, D, B, O>>) -> Option<X>;

pub(super) fn may_cancel_with_impl_<'a, 'g, TTask, T, B, D, O, C, X>(
    mut task: TTask,
    mut get_pin: impl FnMut(&mut TTask) -> Pin<&mut Acquire<'a, T, D, B, O>>,
    try_acquire: FpTryAcquire<'a, 'g, T, B, D, O, X>,
    cancel: Pin<&mut C>,
) -> Option<X>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
    C: TrCancellationToken,
{
    loop {
        let pin = unsafe {
            let ptr = get_pin(&mut task).get_unchecked_mut();
            let mut p = NonNull::new_unchecked(ptr);
            Pin::new_unchecked(p.as_mut())
        };
        if let Option::Some(g) = try_acquire(pin) {
            break Option::Some(g);
        };
        if cancel.is_cancelled() {
            break Option::None;
        }
    }
}
