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
    let mut acq_r0 = rwlock.acquire();
    let r0 = acq_r0.read().wait().unwrap();
    assert_eq!(*r0, ANSWER);
    assert_eq!(rwlock.reader_count(), 1);

    let mut acq_r1 = rwlock.acquire();
    let r1 = acq_r1.read().wait().unwrap();
    assert_eq!(*r1, *r0);
    assert_eq!(rwlock.reader_count(), 2);

    let mut acq_w = rwlock.acquire();
    let opt_w = acq_w.try_write();
    assert!(opt_w.is_none());

    drop(opt_w);
    drop(r0);
    assert_eq!(rwlock.reader_count(), 1);

    let opt_w = acq_w.try_write();
    assert!(opt_w.is_none());

    drop(opt_w);
    drop(r1);
    assert_eq!(rwlock.reader_count(), 0);

    let opt_w = acq_w.try_write();
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
    let mut acq_r0 = rwlock.acquire();

    let r0 = acq_r0.read().wait().unwrap();
    assert_eq!(*r0, ANSWER);
    assert_eq!(rwlock.reader_count(), 1);

    let mut acq_r1 = rwlock.acquire();
    let r1 = acq_r1.upgradable_read().wait().unwrap();
    assert_eq!(*r1, *r0);
    assert_eq!(rwlock.reader_count(), 2);

    let mut upg = r1.upgrade();
    // creating `Upgrade` should not decrease reader count
    assert_eq!(rwlock.reader_count(), 2);
    let opt_u = upg.try_upgrade();
    assert!(opt_u.is_none());

    drop(opt_u);
    drop(r0);
    assert_eq!(rwlock.reader_count(), 1);

    let mut w = upg.upgrade().wait().unwrap();
    // upgraded from an upgradable reader guard will not decrease reader count
    assert_eq!(rwlock.reader_count(), 1);
    assert_eq!(*w, ANSWER);
    *w = MYSTERY;

    drop(w);
    let x = unsafe { *rwlock.as_mut_ptr() };
    assert_eq!(x, MYSTERY);
}
