use core::{
    borrow::BorrowMut,
    cell::UnsafeCell,
    marker::PhantomData,
    ops::{Deref, DerefMut, Try},
    pin::Pin,
    sync::atomic::*,
};

use funty::Unsigned;

use atomex::{
    x_deps::funty,
    CmpxchResult, StrictOrderings,
    TrAtomicCell, TrAtomicData, TrAtomicFlags, TrCmpxchOrderings,
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_mutex::{self, TrMutexGuard, TrSyncMutex},
    sync_tasks::TrSyncTask
};

/// An helper trait to define spinlock behaviour
pub trait TrMutexSignal<V: Copy> {
    fn is_acquired(val: V) -> bool;

    fn is_released(val: V) -> bool {
        !Self::is_acquired(val)
    }

    fn make_acquired(val: V) -> V;

    fn make_released(val: V) -> V;
}

#[derive(Debug)]
pub struct MsbAsMutexSignal<V: Unsigned>(PhantomData<V>);

impl<V: Unsigned> MsbAsMutexSignal<V> {
    #[allow(non_snake_case)]
    #[inline(always)]
    pub fn K_MSB_FLAG() -> V {
        V::ONE << (V::BITS - 1)
    }
}

impl<V: Unsigned> TrMutexSignal<V> for MsbAsMutexSignal<V> {
    fn is_acquired(val: V) -> bool {
        val & Self::K_MSB_FLAG() == Self::K_MSB_FLAG()
    }

    fn make_acquired(val: V) -> V {
        val | Self::K_MSB_FLAG()
    }

    fn make_released(val: V) -> V {
        val & (!Self::K_MSB_FLAG())
    }
}

#[derive(Debug)]
pub struct PtrAsMutexSignal<T: Sized>(PhantomData<*mut T>);

impl<T: Sized> PtrAsMutexSignal<T> {
    const K_MOD: usize = 2;
    const K_RES: usize = 1;
}

impl<T: Sized> TrMutexSignal<*mut T> for PtrAsMutexSignal<T> {
    fn is_acquired(val: *mut T) -> bool {
        (val as usize) % Self::K_MOD == Self::K_RES
    }

    fn make_acquired(val: *mut T) -> *mut T {
        ((val as usize) + Self::K_RES) as *mut T
    }

    fn make_released(val: *mut T) -> *mut T {
        ((val as usize) - Self::K_RES) as *mut T
    }
}

pub type SpinningMutexBorrowed<
        'a,
        T,
        C = AtomicUsize,
        S = MsbAsMutexSignal<<C as TrAtomicCell>::Value>,
        O = StrictOrderings,
    > = SpinningMutex<T, <C as TrAtomicCell>::Value, &'a mut C, S, O>;

pub type SpinningMutexOwned<
        T,
        C = AtomicUsize,
        S = MsbAsMutexSignal<<C as TrAtomicCell>::Value>,
        O = StrictOrderings,
    > = SpinningMutex<T, <C as TrAtomicCell>::Value, C, S, O>;

impl<T, C, S, O> SpinningMutexOwned<T, C, S, O>
where
    C: TrAtomicCell<Value: TrAtomicData<AtomicCell = C> + Copy + Default>,
    S: TrMutexSignal<<C as TrAtomicCell>::Value>,
    O: TrCmpxchOrderings,
{
    pub fn new_owned(data: T) -> Self {
        let val = <<C as TrAtomicCell>::Value as Default>::default();
        let cell = <C as TrAtomicCell>::new(val);
        SpinningMutexOwned::<T, C, S, O>::new(data, cell)
    }
}

/// A configurable spinlock implementation, usually for further encapsulation.
#[derive(Debug)]
pub struct SpinningMutex<
    T,
    D = usize,
    B = AtomicUsize,
    S = MsbAsMutexSignal<D>,
    O = StrictOrderings>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    _use_s: PhantomData<S>,
    _use_d: PhantomData<D>,
    _use_o: PhantomData<O>,
    state_: B,
    value_: UnsafeCell<T>,
}

impl<T, D, B, S, O> SpinningMutex<T, D, B, S, O>
where
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    pub const fn new(data: T, cell: B) -> Self {
        SpinningMutex {
            _use_s: PhantomData,
            _use_d: PhantomData,
            _use_o: PhantomData,
            state_: cell,
            value_: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.value_.into_inner()
    }
}

impl<T, D, B, S, O> SpinningMutex<T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    pub fn is_acquired(&self) -> bool {
        let state = TrAtomicFlags::value(self);
        S::is_acquired(state)
    }

    pub fn acquire(&self) -> Acquire<'_, T, D, B, S, O> {
        Acquire(self)
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.value_.get()
    }
}

impl<T, D, B, S, O> AsRef<D::AtomicCell> for SpinningMutex<T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    fn as_ref(&self) -> &D::AtomicCell {
        self.state_.borrow()
    }
}

impl<T, D, B, S, O> TrAtomicFlags<D, O> for SpinningMutex<T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{}

impl<T, D, B, S, O> TrSyncMutex for SpinningMutex<T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    type Target = T;

    #[inline]
    fn acquire(&self) -> impl sync_mutex::TrAcquire<'_, Self::Target> {
        SpinningMutex::acquire(self)
    }
}

unsafe impl<T, D, B, S, O> Send for SpinningMutex<T, D, B, S, O>
where
    T: Sync + ?Sized,
    D: Unsigned + TrAtomicData,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{}

unsafe impl<T, D, B, S, O> Sync for SpinningMutex<T, D, B, S, O>
where
    T: Sync + ?Sized,
    D: Unsigned + TrAtomicData,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{}

pub struct Acquire<'a, T, D, B, S, O>(&'a SpinningMutex<T, D, B, S, O>)
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings;

impl<'a, T, D, B, S, O> Acquire<'a, T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    pub fn lock(self: Pin<&mut Self>) -> LockAcqTask<'a, '_, T, D, B, S, O> {
        LockAcqTask(self)
    }

    pub fn try_lock<'g>(
        self: Pin<&'g mut Self>,
    ) -> Option<MutexGuard<'a, 'g, T, D, B, S, O>> {
        self.0
            .try_once_compare_exchange_weak(
                self.0.value(),
                S::is_released,
                S::make_acquired)
            .succ()
            .map(|_| MutexGuard::new(self))
    }

    fn mutex_(&self) -> &'a SpinningMutex<T, D, B, S, O> {
        self.0
    }

    fn try_spin_acquire_<'g, 'c, C>(
        self: Pin<&'g mut Self>,
        cancel: Pin<&'c mut C>,
    ) -> Option<MutexGuard<'a, 'g, T, D, B, S, O>>
    where
        'g: 'c,
        C: TrCancellationToken,
    {
        let mut current = self.0.value();
        loop {
            match self.mutex_().try_once_compare_exchange_weak(
                current,
                S::is_released,
                S::make_acquired,
            ) {
                CmpxchResult::Succ(_) =>
                    break Option::Some(MutexGuard::new(self)),
                CmpxchResult::Fail(x) =>
                    current = x,
                CmpxchResult::Unexpected(_) => (),
            }
            if cancel.is_cancelled() {
                break Option::None
            }
        }
    }

    fn try_spin_release_(&self) -> bool {
        self.0
            .try_spin_compare_exchange_weak(
                S::is_acquired,
                S::make_released)
            .is_succ()
    }
}

impl<'a, T, D, B, S, O> sync_mutex::TrAcquire<'a, T>
for Acquire<'a, T, D, B, S, O>
where
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    type MutexGuard<'g> = MutexGuard<'a, 'g, T, D, B, S, O> where 'a: 'g;

    #[inline]
    fn try_lock<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl Try<Output = Self::MutexGuard<'g>>
    where
        'a: 'g
    {
        Acquire::try_lock(self)
    }

    #[inline]
    fn lock<'g>(
        self: Pin<&'g mut Self>,
    ) -> impl TrSyncTask<Output = Self::MutexGuard<'g>>
    where
        'a: 'g
    {
        Acquire::lock(self)
    }
}

pub struct LockAcqTask<'a, 'g, T, D, B, S, O>(Pin<&'g mut Acquire<'a, T, D, B, S, O>>)
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, S, O> LockAcqTask<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    pub fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> Option<MutexGuard<'a, 'g, T, D, B, S, O>>
    where
        C: TrCancellationToken,
    {
        self.0.try_spin_acquire_(cancel)
    }

    #[inline]
    pub fn wait(self) -> MutexGuard<'a, 'g, T, D, B, S, O> {
        TrSyncTask::wait(self)
    }
}

impl<'a, 'g, T, D, B, S, O> TrSyncTask for LockAcqTask<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    type Output = MutexGuard<'a, 'g, T, D, B, S, O>;

    #[inline]
    fn may_cancel_with<C>(
        self,
        cancel: Pin<&mut C>,
    ) -> impl Try<Output = Self::Output>
    where
        C: TrCancellationToken,
    {
        LockAcqTask::may_cancel_with(self, cancel)
    }
}

pub struct MutexGuard<'a, 'g, T, D, B, S, O>(Pin<&'g mut Acquire<'a, T, D, B, S, O>>)
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings;

impl<'a, 'g, T, D, B, S, O> MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    pub(super) fn new(
        acquire: Pin<&'g mut Acquire<'a, T, D, B, S, O>>,
    ) -> Self {
        MutexGuard(acquire)
    }
}

impl<'a, 'g, T, D, B, S, O> Drop for MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    fn drop(&mut self) {
        let _ = self.0.try_spin_release_();
    }
}

impl<'a, 'g, T, D, B, S, O> Deref for MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let opt = unsafe { self.0.mutex_().value_.get().as_ref() };
        let Option::Some(t) = opt else {
            unreachable!("[embedded::MutexGuard::deref]")
        };
        t
    }
}

impl<'a, 'g, T, D, B, S, O> DerefMut for MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        let opt = unsafe { self.0.mutex_().value_.get().as_mut() };
        let Option::Some(t) = opt else {
            unreachable!("[embedded::MutexGuard::deref_mut]")
        };
        t
    }
}

impl<'a, 'g, T, D, B, S, O> TrMutexGuard<'a, 'g, T>
for MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{}

unsafe impl<'a, 'g, T, D, B, S, O> Sync for MutexGuard<'a, 'g, T, D, B, S, O>
where
    'a: 'g,
    T: ?Sized,
    D: TrAtomicData + Copy,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    S: TrMutexSignal<D>,
    O: TrCmpxchOrderings,
{}


#[cfg(test)]
mod tests_ {
    use std::{
        boxed::Box,
        mem,
        ptr,
        sync::Arc,
        sync::atomic::{AtomicUsize, AtomicPtr, Ordering},
    };
    use pin_utils::pin_mut;
    use atomex::{LocksOrderings, StrictOrderings};
    use crate::{mutex::smoke_tests_, x_deps::{atomex, pin_utils}};
    use super::{
        MsbAsMutexSignal, PtrAsMutexSignal,
        SpinningMutexBorrowed, SpinningMutexOwned,
    };

    #[test]
    fn usize_smoke_test() {
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
        smoke_tests_::multithreaded_usize_smoke_(&Arc::new(
            SpinningMutexOwned::<
                    usize,
                    AtomicUsize,
                    MsbAsMutexSignal<usize>,
                    StrictOrderings,
                >::new_owned(0)),
            SpinningMutexOwned::as_mut_ptr,
        )
    }

    #[test]
    fn multithreaded_smoke_locks_orderings() {
        smoke_tests_::multithreaded_usize_smoke_(&Arc::new(
            SpinningMutexOwned::<
                    usize,
                    AtomicUsize,
                    MsbAsMutexSignal<usize>,
                    LocksOrderings,
                >::new_owned(0)),
            SpinningMutexOwned::as_mut_ptr,
        )
    }

    #[test]
    fn ptr_smoke_test() {
        const ANSWER: usize = 42;
        const PTR_SIZE: usize = mem::size_of::<*mut usize>();

        let mut cell = Box::new(AtomicPtr::new(PTR_SIZE as *mut usize));
        let ptr = cell.load(Ordering::Relaxed);
        let mut data = Box::new(ANSWER);
        let lock = SpinningMutexBorrowed
            ::<&mut usize, AtomicPtr<usize>, PtrAsMutexSignal<usize>, StrictOrderings>
            ::new(&mut data, &mut cell);

        let acq = lock.acquire();
        pin_mut!(acq);
        let g = acq.as_mut().lock().wait();
        assert_eq!(**g, ANSWER);
        drop(g);
        assert!(ptr::eq(ptr, cell.load(Ordering::Relaxed)))
    }
}
