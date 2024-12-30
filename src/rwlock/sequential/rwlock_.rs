use core::{
    borrow::BorrowMut,
    cell::UnsafeCell,
    fmt,
    marker::{PhantomData, PhantomPinned},
    mem::ManuallyDrop,
    ops::Try,
    pin::Pin,
    ptr::{self, NonNull},
    sync::atomic::*,
};

use funty::{Integral, Unsigned};

use atomex::{
    x_deps::funty, AtomexPtr, Bitwise, CmpxchResult, StrictOrderings, TrAtomicCell, TrAtomicData, TrAtomicFlags, TrCmpxchOrderings
};
use abs_sync::{
    cancellation::TrCancellationToken,
    sync_lock::{self, TrSyncRwLock},
    sync_tasks::TrSyncTask,
};

use super::{
    acquire_::{Acquire, AtomAcqLinkPtr},
};

#[derive(Debug)]
pub struct SpinningRwLock<T, B = AtomicUsize, D = usize, O = StrictOrderings>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    _pin_: PhantomPinned,
    stat_: RwLockState<B, D, O>,
    data_: UnsafeCell<T>,
}

impl<T, B, D, O> SpinningRwLock<T, B, D, O>
where
    T: ?Sized,
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    pub const fn acquire(&self) -> Acquire<'_, T, B, D, O> {
        Acquire::new(self)
    }

    #[inline]
    pub(super) fn data_cell_(&self) -> &UnsafeCell<T> {
        &self.data_
    }
}

#[derive(Debug)]
pub(super) struct RwLockState<B, D, O>
where
    B: BorrowMut<<D as TrAtomicData>::AtomicCell>,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    O: TrCmpxchOrderings,
{
    flag_: B,
    head_: AtomAcqLinkPtr<O>,
    ___o_: PhantomData<D>,
}