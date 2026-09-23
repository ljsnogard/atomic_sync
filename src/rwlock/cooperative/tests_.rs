//! `rwlock::cooperative` 的行为测试。

use core::{future::Future, pin::Pin};
use std::{
    boxed::Box,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
};

use abs_sync::x_deps::abs_cancel::{CancelledToken, TrMayCancel};
use futures_lite::future::block_on;

use super::{CoopRwLockError, CooperativeRwLockOwned};

/// 记录被唤醒次数的 waker。
struct CountingWake(AtomicUsize);

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn new_waker() -> (Arc<CountingWake>, Waker) {
    let w = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = Waker::from(w.clone());
    (w, waker)
}

fn poll_once<F: Future>(fut: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    let mut cx = Context::from_waker(waker);
    fut.poll(&mut cx)
}

fn expect_ready<T>(polled: Poll<T>, what: &str) -> T {
    match polled {
        Poll::Ready(x) => x,
        Poll::Pending => panic!("{what} 应当已经就绪"),
    }
}

/// 测试非阻塞获取在冲突时返回 `WouldBlock`，并在持有者释放后成功。
/// - 手段：用会话 A 取得写守卫，再用会话 B 依次尝试读、写、可升级读；
///   释放写守卫后再用会话 B 获取读守卫。
/// - 判断：三种尝试都必须返回 `Err(WouldBlock)`；释放后读守卫成功且能看到写入的值。
#[test]
fn try_acquire_should_would_block_on_conflict() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_a = lock.acquire_session();
    let mut sess_b = lock.acquire_session();

    let mut w = sess_a.try_write().unwrap();
    *w = 7;

    assert!(matches!(
        sess_b.try_read(),
        Result::Err(CoopRwLockError::WouldBlock)
    ));
    assert!(matches!(
        sess_b.try_write(),
        Result::Err(CoopRwLockError::WouldBlock)
    ));
    assert!(matches!(
        sess_b.try_upgradable_read(),
        Result::Err(CoopRwLockError::WouldBlock)
    ));

    drop(w);
    assert_eq!(*sess_b.try_read().unwrap(), 7);
}

/// 测试无竞争时异步获取走快速路径，不与任何等待节点打交道。
/// - 手段：直接在空闲锁上用 `block_on` 获取写守卫与读守卫。
/// - 判断：两次获取都成功，写入可被后续读者观察到。
#[test]
fn async_acquire_should_work_when_uncontended() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(1);
    let mut sess = lock.acquire_session();

    {
        let mut w = block_on(sess.write_async().into_future()).unwrap();
        *w += 41;
    }
    let r = block_on(sess.read_async().into_future()).unwrap();
    assert_eq!(*r, 42);
}

/// 测试写者必须等所有既有读者退出后才能进入。
/// - 手段：先取两个读守卫，再轮询写 future；每释放一个读者观察一次唤醒与轮询结果。
/// - 判断：任一读者仍在时写 future 保持 `Pending` 且未登记唤醒；最后一个读者释放后
///   写者被唤醒并能立即完成。
#[test]
fn writer_should_wait_for_all_readers() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_r0 = lock.acquire_session();
    let mut sess_r1 = lock.acquire_session();
    let mut sess_w = lock.acquire_session();

    let r0 = block_on(sess_r0.read_async().into_future()).unwrap();
    let r1 = block_on(sess_r1.read_async().into_future()).unwrap();

    let mut wfut = Box::pin(sess_w.write_async().into_future());
    let (cnt_w, wk_w) = new_waker();
    assert!(poll_once(wfut.as_mut(), &wk_w).is_pending());

    drop(r0);
    assert!(poll_once(wfut.as_mut(), &wk_w).is_pending());
    assert_eq!(cnt_w.0.load(Ordering::SeqCst), 0);

    drop(r1);
    assert!(
        cnt_w.0.load(Ordering::SeqCst) >= 1,
        "最后一个读者释放后必须唤醒写者"
    );
    let mut w = match poll_once(wfut.as_mut(), &wk_w) {
        Poll::Ready(r) => r.unwrap(),
        Poll::Pending => panic!("写者已被唤醒却还不能进入"),
    };
    *w = 9;
    drop(w);
    assert_eq!(lock.reader_count(), 0);
}

/// 测试严格 FIFO：排在写者后面的读者不能插到写者前面。
/// - 手段：构造队列 `[读 A][写 W][读 B]`；先释放持有写锁的初始写者，再释放读 A。
/// - 判断：初始写者释放后只有读 A 被唤醒；读 A 释放后只有写 W 被唤醒，读 B 始终未被唤醒。
#[test]
fn fifo_should_prevent_reader_barging_behind_writer() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_hold = lock.acquire_session();
    let mut sess_a = lock.acquire_session();
    let mut sess_w = lock.acquire_session();
    let mut sess_b = lock.acquire_session();

    let hold = sess_hold.try_write().unwrap();

    let mut afut = Box::pin(sess_a.read_async().into_future());
    let (cnt_a, wk_a) = new_waker();
    assert!(poll_once(afut.as_mut(), &wk_a).is_pending());

    let mut wfut = Box::pin(sess_w.write_async().into_future());
    let (cnt_w, wk_w) = new_waker();
    assert!(poll_once(wfut.as_mut(), &wk_w).is_pending());

    let mut bfut = Box::pin(sess_b.read_async().into_future());
    let (cnt_b, wk_b) = new_waker();
    assert!(poll_once(bfut.as_mut(), &wk_b).is_pending());

    drop(hold);
    assert!(cnt_a.0.load(Ordering::SeqCst) >= 1, "队首读者应被唤醒");
    assert_eq!(cnt_w.0.load(Ordering::SeqCst), 0, "写者不该越过读者");
    assert_eq!(cnt_b.0.load(Ordering::SeqCst), 0, "读 B 不该越过写者");

    let ga = match poll_once(afut.as_mut(), &wk_a) {
        Poll::Ready(r) => r.unwrap(),
        Poll::Pending => panic!("读 A 应能进入"),
    };
    drop(ga);

    assert!(cnt_w.0.load(Ordering::SeqCst) >= 1, "读 A 释放后应唤醒写者");
    assert_eq!(cnt_b.0.load(Ordering::SeqCst), 0, "读 B 仍应排在写者之后");

    let _w = match poll_once(wfut.as_mut(), &wk_w) {
        Poll::Ready(r) => r.unwrap(),
        Poll::Pending => panic!("写者应能进入"),
    };
    assert_eq!(cnt_b.0.load(Ordering::SeqCst), 0);
}

/// 测试"队首等待者被唤醒后立刻取消"不会让后续等待者永久饥饿（活性回归）。
/// - 手段：构造 `[读 A][写 W]`，释放初始写者唤醒读 A，随即丢弃读 A 的 future。
/// - 判断：写 W 必须被唤醒，并且能够立即完成获取——若取消时不做补救，写 W 将无人唤醒。
#[test]
fn cancelled_head_waiter_should_not_starve_followers() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_hold = lock.acquire_session();
    let mut sess_a = lock.acquire_session();
    let mut sess_w = lock.acquire_session();

    let hold = sess_hold.try_write().unwrap();

    let mut afut = Box::pin(sess_a.read_async().into_future());
    let (cnt_a, wk_a) = new_waker();
    assert!(poll_once(afut.as_mut(), &wk_a).is_pending());

    let mut wfut = Box::pin(sess_w.write_async().into_future());
    let (cnt_w, wk_w) = new_waker();
    assert!(poll_once(wfut.as_mut(), &wk_w).is_pending());

    drop(hold);
    assert!(cnt_a.0.load(Ordering::SeqCst) >= 1);

    drop(afut);
    assert!(
        cnt_w.0.load(Ordering::SeqCst) >= 1,
        "队首读者取消后必须有人继续唤醒写者"
    );
    let mut w = match poll_once(wfut.as_mut(), &wk_w) {
        Poll::Ready(r) => r.unwrap(),
        Poll::Pending => panic!("写者应能被放行"),
    };
    *w = 3;
    drop(w);
    assert_eq!(lock.reader_count(), 0);
}

/// 测试取消令牌触发时异步获取返回 `Cancelled`，且不破坏锁状态。
/// - 手段：持有一个写守卫，用已取消的 `CancelledToken` 去获取读；随后释放写守卫。
/// - 判断：获取返回 `Err(Cancelled)`；释放后普通读获取成功，说明没有泄漏等待者或许可。
#[test]
fn cancelled_token_should_abort_acquire_cleanly() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(5);
    let mut sess_hold = lock.acquire_session();
    let mut sess = lock.acquire_session();

    let hold = sess_hold.try_write().unwrap();

    let r = block_on(sess.read_async().may_cancel_with(CancelledToken::new()));
    assert!(matches!(r, Result::Err(CoopRwLockError::Cancelled)));
    // `Result` 里含有带 Drop 的守卫，会一直占着会话的可变借用，需显式释放。
    drop(r);

    drop(hold);
    let r = block_on(sess.read_async().into_future()).unwrap();
    assert_eq!(*r, 5);
}

/// 测试可升级读者在还有其他读者时升级失败，独占后才成功。
/// - 手段：一个普通读者加一个可升级读者，先用 `try_upgrade` 失败，释放普通读者后重试。
/// - 判断：第一次返回被拒绝（原样归还守卫），第二次成功得到写守卫并能写数据。
#[test]
fn try_upgrade_should_require_exclusive_reader() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(1);
    let mut sess_r = lock.acquire_session();
    let mut sess_u = lock.acquire_session();

    let r = block_on(sess_r.read_async().into_future()).unwrap();
    let upg = block_on(sess_u.upgradable_read_async().into_future()).unwrap();

    let upg = match upg.try_upgrade() {
        Result::Ok(_) => panic!("还有其他读者时不该升级成功"),
        Result::Err(upg) => upg,
    };

    drop(r);
    let mut w = match upg.try_upgrade() {
        Result::Ok(w) => w,
        Result::Err(_) => panic!("独占后应能升级"),
    };
    *w = 2;
    drop(w);
}

/// 测试异步升级的栅栏语义与唤醒时机。
/// - 手段：可升级读者挂起 `upgrade_async`，同时另有一个普通读者；
///   此时尝试新读获取；随后释放普通读者。
/// - 判断：栅栏生效期间新读者返回 `WouldBlock`；普通读者释放后升级被唤醒并成功。
#[test]
fn async_upgrade_should_set_barrier_and_wake_when_alone() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(10);
    let mut sess_r = lock.acquire_session();
    let mut sess_u = lock.acquire_session();
    let mut sess_new = lock.acquire_session();

    let upg = block_on(sess_u.upgradable_read_async().into_future()).unwrap();
    let mut upg_sess = upg.upgrade_session();
    let r = block_on(sess_r.read_async().into_future()).unwrap();

    let mut ufut = Box::pin(upg_sess.upgrade_async().into_future());
    let (cnt_u, wk_u) = new_waker();
    assert!(poll_once(ufut.as_mut(), &wk_u).is_pending());

    assert!(
        matches!(
            sess_new.try_read(),
            Result::Err(CoopRwLockError::WouldBlock)
        ),
        "升级请求挂起时必须挡住新读者"
    );

    drop(r);
    assert!(cnt_u.0.load(Ordering::SeqCst) >= 1, "独占后应唤醒升级者");
    let mut w = match poll_once(ufut.as_mut(), &wk_u) {
        Poll::Ready(x) => x.unwrap(),
        Poll::Pending => panic!("升级者应能完成"),
    };
    *w = 11;
    drop(w);
    drop(ufut);

    drop(upg_sess);
    assert_eq!(lock.reader_count(), 0);
    let r = block_on(sess_new.read_async().into_future()).unwrap();
    assert_eq!(*r, 11);
}

/// 测试写者的两条降级路径。
/// - 手段：写守卫分别降级为读守卫与可升级读守卫，各自再降级/释放。
/// - 判断：计数与可升级标志按预期迁移，最终回到空闲。
#[test]
fn downgrade_paths_should_restore_idle_state() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess = lock.acquire_session();

    let w = sess.try_write().unwrap();
    let r = w.downgrade_to_reader();
    assert_eq!(lock.reader_count(), 1);
    drop(r);
    assert_eq!(lock.reader_count(), 0);

    let w = sess.try_write().unwrap();
    let upg = w.downgrade_to_upgradable();
    assert_eq!(lock.reader_count(), 1);
    let r = upg.downgrade();
    assert_eq!(lock.reader_count(), 1);
    drop(r);
    assert_eq!(lock.reader_count(), 0);
}

/// 测试可升级读者在升级栅栏被取消后不会卡住其他读者。
/// - 手段：可升级读者挂起升级、取消该 future（丢弃），随后获取普通读。
/// - 判断：取消后读获取不再被栅栏阻挡，且能成功。
#[test]
fn cancelled_upgrade_should_lift_barrier() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_u = lock.acquire_session();
    let mut sess_r = lock.acquire_session();
    let mut sess_new = lock.acquire_session();

    let upg = block_on(sess_u.upgradable_read_async().into_future()).unwrap();
    let mut upg_sess = upg.upgrade_session();
    // 再放一个普通读者，使升级无法立刻完成，从而真正挂起栅栏。
    let r = block_on(sess_r.read_async().into_future()).unwrap();

    let mut ufut = Box::pin(upg_sess.upgrade_async().into_future());
    let (_, wk) = new_waker();
    assert!(poll_once(ufut.as_mut(), &wk).is_pending());
    assert!(matches!(
        sess_new.try_read(),
        Result::Err(CoopRwLockError::WouldBlock)
    ));

    drop(ufut);
    // 栅栏随取消一并撤除，新读者立刻可以进入。
    let r2 = block_on(sess_new.read_async().into_future()).unwrap();
    assert_eq!(*r2, 0);
    drop(r2);
    drop(r);
    drop(upg_sess);
    assert_eq!(lock.reader_count(), 0);
}

/// 测试同质合并的读者会被整组放行。
/// - 手段：写者持有时让 3 个读者依次排队（前一个入队后队尾是读节点，后两个会合并进去），
///   然后释放写者。
/// - 判断：3 个读者在同一次释放里全部被唤醒，并且能同时持有读许可（读者计数为 3）。
#[test]
fn coalesced_readers_should_be_admitted_together() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let mut sess_hold = lock.acquire_session();
    let mut s0 = lock.acquire_session();
    let mut s1 = lock.acquire_session();
    let mut s2 = lock.acquire_session();

    let hold = sess_hold.try_write().unwrap();

    let mut f0 = Box::pin(s0.read_async().into_future());
    let mut f1 = Box::pin(s1.read_async().into_future());
    let mut f2 = Box::pin(s2.read_async().into_future());
    let (c0, w0) = new_waker();
    let (c1, w1) = new_waker();
    let (c2, w2) = new_waker();

    assert!(poll_once(f0.as_mut(), &w0).is_pending());
    assert!(poll_once(f1.as_mut(), &w1).is_pending());
    assert!(poll_once(f2.as_mut(), &w2).is_pending());
    assert_eq!(lock.reader_count(), 0);

    drop(hold);
    assert!(
        c0.0.load(Ordering::SeqCst) >= 1
            && c1.0.load(Ordering::SeqCst) >= 1
            && c2.0.load(Ordering::SeqCst) >= 1,
        "整组读者应在同一次放行中被唤醒"
    );

    let g0 = expect_ready(poll_once(f0.as_mut(), &w0), "读 0").unwrap();
    let g1 = expect_ready(poll_once(f1.as_mut(), &w1), "读 1").unwrap();
    let g2 = expect_ready(poll_once(f2.as_mut(), &w2), "读 2").unwrap();
    assert_eq!(lock.reader_count(), 3);
    assert_eq!((*g0, *g1, *g2), (0, 0, 0));

    drop(g0);
    drop(g1);
    drop(g2);
    assert_eq!(lock.reader_count(), 0);
}

/// 测试多线程竞争下读者与写者不会丢失更新。
/// - 手段：4 个线程各自反复"写一次、读一次"，全部通过 `block_on` 驱动。
/// - 判断：最终值等于总写入次数；期间读到的值不超过上界。
#[test]
fn multithreaded_writers_should_not_lose_updates() {
    const THREADS: usize = 4;
    const ITERS: usize = 200;

    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                for _ in 0..ITERS {
                    let mut sess_w = lock.acquire_session();
                    let mut w =
                        block_on(sess_w.write_async().into_future()).unwrap();
                    *w += 1;
                    drop(w);

                    let mut sess_r = lock.acquire_session();
                    let r = block_on(sess_r.read_async().into_future()).unwrap();
                    assert!(*r <= THREADS * ITERS);
                }
            });
        }
    });
    assert_eq!(lock.into_inner(), THREADS * ITERS);
}
