use core::{
    borrow::Borrow,
    cell::UnsafeCell,
    fmt::{self, Debug},
    ops::{Deref, DerefMut, Try},
    pin::Pin,
    sync::atomic::*,
};
use funty::{Integral, Unsigned};
use atomex::{
    x_deps::funty,
    Bitwise, StrictOrderings,
    TrAtomicCell, TrAtomicData, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::{CancelledToken, TrCancellationToken},
    sync_lock::{TrMutexGuard, TrSyncMutex},
    sync_tasks::TrSyncTask,
};
use super::state_::{MutexState, TicketCtx};

pub type SpinningMutexBorrowed<'a, T, C = AtomicUsize, O = StrictOrderings> =
    SpinningMutex<T, <C as TrAtomicCell>::Value, &'a mut C, O>;

pub type SpinningMutexOwned<T, C = AtomicUsize, O = StrictOrderings> =
    SpinningMutex<T, <C as TrAtomicCell>::Value, C, O>;

impl<T, C, O> SpinningMutexOwned<T, C, O>
where
    C: TrAtomicCell + Bitwise,
    <C as TrAtomicCell>::Value: TrAtomicData<AtomicCell = C> + Unsigned,
    O: TrCmpxchOrderings,
{
    pub fn new_owned(data: T) -> Self {
        let val = <<C as TrAtomicCell>::Value as Integral>::ZERO;
        let cell = <C as TrAtomicCell>::new(val);
        SpinningMutexOwned::<T, C, O>::new(data, cell)
    }
}

/// A spinlock for fairness. Lockers are queued FIFO based on ticket number.
pub struct SpinningMutex<
    T,
    D = usize,
    B = AtomicUsize,
    O = StrictOrderings>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    state_: MutexState<D, B, O>,
    value_: UnsafeCell<T>,
}

impl<T, D, B, O> SpinningMutex<T, D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    /// # Safety
    /// The atomic cell must be storing zero.
    pub const fn new(data: T, cell: B) -> Self {
        SpinningMutex {
            state_: MutexState::new(cell),
            value_: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.value_.into_inner()
    }
}

impl<T, D, B, O> SpinningMutex<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    /// Tests if the mutex is acquired.
    ///
    /// # Example
    /// ```
    /// use core::sync::atomic::AtomicUsize;
    /// use atomic_sync::mutex::ticketing::SpinningMutex;
    ///
    /// let mut atom = AtomicUsize::new(0);
    /// let mut data = 42usize;
    /// let mutex = SpinningMutex::<&mut usize, usize, &mut AtomicUsize>
    ///     ::new(&mut data, &mut atom);
    /// let guard = mutex.acquire().wait();
    /// assert!(mutex.is_acquired());
    /// assert!(mutex.try_acquire().is_none());
    /// ```
    #[inline(always)]
    pub fn is_acquired(&self) -> bool {
        self.state_.is_acquired()
    }

    /// Attempt to acquire a mutex guard without blocking current thread.
    ///
    /// If a mutex guard could not be acquired at this time, then [`None`] is
    /// returned. Otherwise, a guard is returned that releases the mutex when
    /// dropped.
    ///
    /// This is almost equivalent to call 
    /// `acquire_with_cancel(CancelledToken::shared_ref())`
    ///
    /// # Examples
    ///
    /// ```
    /// use atomic_sync::mutex::ticketing::SpinningMutexOwned;
    ///
    /// let mutex = SpinningMutexOwned::<usize>::new_owned(1usize);
    /// let mut guard = mutex.try_acquire().unwrap();
    /// assert!(mutex.is_acquired());
    /// assert_eq!(*guard, 1);
    /// assert!(mutex.try_acquire().is_none());
    /// ```
    pub fn try_acquire(&self) -> Option<MutexGuard<'_, T, D, B, O>> {
        if self.state_.is_acquired() {
            Option::None
        } else {
            self.spin_acquire_with_cancel(CancelledToken::pinned()).ok()
        }
    }

    /// Acquire the mutex guard with a task that will spin wait, blocking the
    /// current thread until cancelled.
    ///
    /// # Example
    /// ```
    /// use atomic_sync::mutex::ticketing::SpinningMutexOwned;
    ///
    /// let mutex = SpinningMutexOwned::<usize>::new_owned(42);
    /// let guard = mutex.acquire().wait();
    /// assert!(mutex.is_acquired());
    /// assert!(mutex.try_acquire().is_none());
    /// ```
    pub fn acquire(&self) -> AcquireTask<'_, T, D, B, O> {
        AcquireTask::new(self)
    }

    pub(super) fn spin_acquire_with_cancel<'a, 'c, C>(
        &'a self,
        mut cancel: Pin<&'c mut C>,
    ) -> Result<MutexGuard<'a, T, D, B, O>, Pin<&'c mut C>>
    where
        C: TrCancellationToken,
    {
        let Result::Ok(ticket) = self
            .state_
            .spin_get_ticket(cancel.as_mut())
        else {
            return Result::Err(cancel);
        };
        let x = TicketCtx::<C, D, B, O>::new(
            &self.state_,
            cancel.as_mut(),
            ticket,
        );
        let Result::Ok(ticket) = MutexState::spin_wait(&x) else {
            return Result::Err(cancel);
        };
        Result::Ok(MutexGuard::new(self, ticket))
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
    /// use atomic_sync::mutex::ticketing::SpinningMutexOwned;
    ///
    /// let mutex = SpinningMutexOwned::<usize>::new_owned(42usize);
    /// unsafe {
    ///     let mut m = ManuallyDrop::new(mutex.acquire().wait());
    ///     assert_eq!(mutex.as_mut_ptr().read(), 42);
    ///     mutex.as_mut_ptr().write(58);
    ///     ManuallyDrop::drop(&mut m);
    /// }
    /// assert_eq!(*mutex.acquire().wait(), 58);
    /// ```
    pub fn as_mut_ptr(&self) -> *mut T {
        self.value_.get()
    }
}

impl<T, D, B, O> TrSyncMutex for SpinningMutex<T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Target = T;
    type MutexGuard<'a> = MutexGuard<'a, T, D, B, O> where Self: 'a;

    #[inline(always)]
    fn is_acquired(&self) -> bool {
        SpinningMutex::is_acquired(self)
    }

    #[inline(always)]
    fn try_acquire(&self) -> Option<Self::MutexGuard<'_>> {
        SpinningMutex::try_acquire(self)
    }

    #[inline(always)]
    fn acquire(&self) -> impl TrSyncTask<Output=Self::MutexGuard<'_>> {
        SpinningMutex::acquire(self)
    }
}

impl<T, D, B, O> Debug for SpinningMutex<T, D, B, O>
where
    T: Debug + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = unsafe { self.value_.get().as_ref() };
        write!(f, "SpinningMutex@{:p}({:?})", self, d)
    }
}

unsafe impl<T, D, B, O> Send for SpinningMutex<T, D, B, O>
where
    T: Send + ?Sized,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{}

unsafe impl<T, D, B, O> Sync for SpinningMutex<T, D, B, O>
where
    T: Send + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

pub struct MutexGuard<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    mutex_: &'a SpinningMutex<T, D, B, O>,
    ticket_: D,
}

impl<'a, T, D, B, O> MutexGuard<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn new(
        mutex: &'a SpinningMutex<T, D, B, O>,
        ticket: D,
    ) -> Self {
        MutexGuard {
            mutex_: mutex,
            ticket_: ticket,
        }
    }
}

impl<T, D, B, O> Drop for MutexGuard<'_, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        let r = self.mutex_.state_.try_release(self.ticket_);
        assert!(
            r.is_ok(),
            "[MutexGuard::drop] failed release ticket({})", self.ticket_,
        )
    }
}

impl<T, D, B, O> Deref for MutexGuard<'_, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let x = unsafe { self.mutex_.value_.get().as_ref() };
        let Option::Some(t) = x else {
            unreachable!("[MutexGuard::deref]")
        };
        t
    }
}

impl<T, D, B, O> DerefMut for MutexGuard<'_, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        let x = unsafe { self.mutex_.value_.get().as_mut() };
        let Option::Some(t) = x else {
            unreachable!("[MutexGuard::deref_mut]")
        };
        t
    }
}

impl<'a, T, D, B, O> TrMutexGuard<'a, T> for MutexGuard<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Mutex = SpinningMutex<T, D, B, O>;
}

unsafe impl<T, D, B, O> Send for MutexGuard<'_, T, D, B, O>
where
    T: Send + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

unsafe impl<T, D, B, O> Sync for MutexGuard<'_, T, D, B, O>
where
    T: Sync + ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

pub struct AcquireTask<'a, T, D, B, O>(&'a SpinningMutex<T, D, B, O>)
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings;

impl<'a, T, D, B, O> AcquireTask<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub const fn new(mutex: &'a SpinningMutex<T, D, B, O>) -> Self {
        AcquireTask(mutex)
    }

    #[inline(always)]
    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Result<MutexGuard<'a, T, D, B, O>, Pin<&mut C>>
    where
        C: TrCancellationToken,
    {
        self.0.spin_acquire_with_cancel(cancel)
    }

    #[inline(always)]
    pub fn wait(self) -> MutexGuard<'a, T, D, B, O> {
        TrSyncTask::wait(self)
    }
}

impl<'a, T, D, B, O> TrSyncTask for AcquireTask<'a, T, D, B, O>
where
    T: ?Sized,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    type Output = MutexGuard<'a, T, D, B, O>;

    #[inline(always)]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output=Self::Output>
    where
        C: TrCancellationToken,
    {
        AcquireTask::may_cancel_with(self, cancel)
    }
}

#[cfg(test)]
mod tests_ {
    use std::{
        sync::Arc,
        sync::atomic::AtomicUsize,
    };
    use atomex::{LocksOrderings, StrictOrderings};

    use crate::{mutex::smoke_tests_, x_deps::atomex};

    use super::SpinningMutexOwned;

    #[test]
    fn smoke_test() {
        smoke_tests_::usize_smoke_test(
            SpinningMutexOwned::<usize>::new_owned,
            SpinningMutexOwned::<usize>::as_mut_ptr,
        )
    }

    #[test]
    fn try_acquired_smoke() {
        smoke_tests_::try_acquired_smoke(SpinningMutexOwned::<usize>::new_owned)
    }

    #[test]
    fn multithreaded_smoke_strict_orderings() {
        smoke_tests_::multithreaded_usize_smoke_(
            &Arc::new(SpinningMutexOwned::<usize, AtomicUsize, StrictOrderings>::new_owned(0)),
            SpinningMutexOwned::as_mut_ptr,
        )
    }

    #[test]
    fn multithreaded_smoke_locks_orderings() {
        smoke_tests_::multithreaded_usize_smoke_(
            &Arc::new(SpinningMutexOwned::<usize, AtomicUsize, LocksOrderings>::new_owned(0)),
            SpinningMutexOwned::as_mut_ptr,
        )
    }
}
