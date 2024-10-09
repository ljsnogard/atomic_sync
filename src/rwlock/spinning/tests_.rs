use pin_utils::pin_mut;

use crate::x_deps::pin_utils;
use super::*;

#[test]
fn rwlock_default_test() {
    const ANSWER: usize = 42;
    let rwlock = SpinningRwLockOwned::<usize>::new_owned(ANSWER);
    assert_eq!(rwlock.reader_count(), 0);
    assert_eq!(rwlock.into_inner(), ANSWER);
}

#[test]
fn acquire_reader_guard_should_block_acq_writer_guard() {
    const ANSWER: usize = 42;
    const MYSTERY: usize = ANSWER * ANSWER;

    let rwlock = SpinningRwLockOwned::<usize>::new_owned(ANSWER);
    let acq_r0 = rwlock.acquire();
    pin_mut!(acq_r0);
    let r0 = acq_r0.read().wait();
    assert_eq!(*r0, ANSWER);
    assert_eq!(rwlock.reader_count(), 1);

    let acq_r1 = rwlock.acquire();
    pin_mut!(acq_r1);
    let r1 = acq_r1.read().wait();
    assert_eq!(*r1, *r0);
    assert_eq!(rwlock.reader_count(), 2);

    let acq_w = rwlock.acquire();
    pin_mut!(acq_w);
    let opt_w = acq_w.as_mut().try_write();
    assert!(opt_w.is_none());

    drop(opt_w);
    drop(r0);
    assert_eq!(rwlock.reader_count(), 1);

    let opt_w = acq_w.as_mut().try_write();
    assert!(opt_w.is_none());

    drop(opt_w);
    drop(r1);
    assert_eq!(rwlock.reader_count(), 0);

    let opt_w = acq_w.as_mut().try_write();
    let mut w = opt_w.unwrap();
    assert_eq!(*w, ANSWER);
    *w = MYSTERY; 

    drop(w);
    assert_eq!(rwlock.into_inner(), MYSTERY);
}

#[test]
fn acquire_reader_guard_should_block_upgrade() {
    const ANSWER: usize = 42;
    const MYSTERY: usize = ANSWER * ANSWER;

    let rwlock = SpinningRwLockOwned::<usize>::new_owned(ANSWER);
    let acq_r0 = rwlock.acquire();
    pin_mut!(acq_r0);
    let r0 = acq_r0.read().wait();
    assert_eq!(*r0, ANSWER);
    assert_eq!(rwlock.reader_count(), 1);

    let acq_r1 = rwlock.acquire();
    pin_mut!(acq_r1);
    let r1 = acq_r1.upgradable_read().wait();
    assert_eq!(*r1, *r0);
    assert_eq!(rwlock.reader_count(), 2);

    let upg = r1.upgrade();
    pin_mut!(upg);
    // creating `Upgrade` should not decrease reader count
    assert_eq!(rwlock.reader_count(), 2);
    let opt_u = upg.as_mut().try_upgrade();
    assert!(opt_u.is_none());

    drop(opt_u);
    drop(r0);
    assert_eq!(rwlock.reader_count(), 1);

    let mut w = upg.as_mut().upgrade().wait();
    // upgraded from an upgradable reader guard will not decrease reader count
    assert_eq!(rwlock.reader_count(), 1);
    assert_eq!(*w, ANSWER);
    *w = MYSTERY;

    drop(w);
    let x = unsafe { *rwlock.as_mut_ptr() };
    assert_eq!(x, MYSTERY);
}
