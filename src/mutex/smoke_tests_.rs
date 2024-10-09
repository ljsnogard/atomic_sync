﻿use std::{
    ops::{ControlFlow, DerefMut, Try},
    sync::Arc,
    thread,
    vec::Vec,
};

use abs_sync::{
    cancellation as mod_can,
    sync_lock::TrSyncMutex,
    sync_tasks::TrSyncTask,
};

fn init_env_logger_() {
    let _ = env_logger::builder().is_test(true).try_init();
}

pub(crate) fn usize_smoke_test<M>(
    new_mutex: impl FnOnce(usize) -> M,
    as_mut_ptr: impl Fn(&M) -> *mut usize)
where
    M: TrSyncMutex<Target = usize>,
{
    use core::mem::ManuallyDrop;
    const ANSWER: usize = 42;
    const SECRET: usize = 58;

    let mutex = new_mutex(ANSWER);
    unsafe {
        let mut m = ManuallyDrop::new(mutex.acquire().wait());
        assert_eq!(as_mut_ptr(&mutex).read(), ANSWER);
        as_mut_ptr(&mutex).write(SECRET);
        ManuallyDrop::drop(&mut m);
    }
    assert_eq!(*mutex.acquire().wait(), SECRET);
}

pub(crate) fn try_acquired_smoke<M>(new_mutex: impl FnOnce(usize) -> M)
where
    M: TrSyncMutex<Target = usize>,
{
    let mutex = new_mutex(1);
    let guard = mutex.try_acquire().unwrap();
    assert!(mutex.is_acquired());
    assert_eq!(*guard, 1);
    assert!(mutex.try_acquire().is_none());
    drop(guard)
}

pub(crate) fn multithreaded_usize_smoke_<M>(
    mutex: &Arc<M>,
    as_mut_ptr: impl Fn(&M) -> *mut usize)
where
    M: 'static + TrSyncMutex<Target = usize> + Send + Sync,
{
    init_env_logger_();

    const INIT_VAL: usize = 0;
    const TEST_MAX: usize = 16;
    // const SLEEP_DUR: Duration = Duration::from_micros(100);

    assert!(unsafe { as_mut_ptr(mutex).read() } == INIT_VAL);

    let thread1 = {
        let mutex_clone = mutex.clone();
        move || testing_thread_work_(
            mutex_clone,
            TEST_MAX,
            is_odd,
            |u| u + 1,
        )
    };
    let thread2 = {
        let mutex_clone = mutex.clone();
        move || testing_thread_work_(
            mutex_clone,
            TEST_MAX,
            is_even,
            |u| u + 1,
        )
    };
    let t1 = thread::spawn(thread1);
    let t2 = thread::spawn(thread2);

    let r1 = t1.join();
    let r2 = t2.join();
    assert!(r1.is_ok());
    assert!(r2.is_ok());
    let v1 = r1.unwrap();
    let v2 = r2.unwrap();
    assert_eq!(v1.len(), TEST_MAX / 2);
    assert_eq!(v2.len(), TEST_MAX / 2);
    assert!(v1.into_iter().all(is_even));
    assert!(v2.into_iter().all(is_odd));

    fn is_even(u: usize) -> bool {
        u % 2 == 0
    }
    fn is_odd(u: usize) -> bool {
        u % 2 == 1
    }

    fn testing_thread_work_<FnExpect, FnDesire, TMutex>(
        mutex: Arc<TMutex>,
        max: usize,
        expect: FnExpect,
        desire: FnDesire,
    ) -> Vec<usize>
    where
        FnExpect: Fn(usize) -> bool,
        FnDesire: Fn(usize) -> usize,
        TMutex: TrSyncMutex<Target = usize>,
    {
        let mut c = 0usize;
        let id = &mutex;
        let mut vec = Vec::with_capacity(1);
        // let cancel = mod_can::NonCancellableToken::shared_ref();
        let mut cancel = mod_can::CancelledToken::pinned();
        loop {
            c += 1usize;
            let acq = mutex
                .acquire()
                .may_cancel_with(cancel.as_mut());
            let ControlFlow::Continue(mut guard) = acq.branch() else {
                // log::trace!("{id:p} #{c} vec.len({}) no guard acquired", vec.len());
                continue;
            };
            let v = guard.deref_mut();
            if *v >= max {
                log::info!("{id:p} #{c} vec.len({}) exit with v({})", vec.len(), *v);
                break;
            }
            log::trace!("{id:p} #{c} max({max}) v({}) {}", *v, expect(*v));
            if expect(*v) {
                *v = desire(*v);
                vec.push(*v);
            }
            // thread::sleep(SLEEP_DUR);
        }
        vec
    }
}
