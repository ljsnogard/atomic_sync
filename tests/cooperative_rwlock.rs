//! `rwlock::cooperative` 的集成测试。
//!
//! 这里刻意**只使用 crate 的公开 API**，并以 `abs_sync` 的 trait 作为唯一入口
//! 驱动整个锁：只要 trait 的某个关联类型或方法没有正确实现，本文件就无法编译
//! 或无法运行。

use atomic_sync::{
    rwlock::cooperative::{CoopRwLockError, CooperativeRwLockOwned},
    x_deps::abs_sync::{
        async_rwlock::{
            TrAsyncRwLock, TrAsyncRwLockAcqSess, TrAsyncRwLockUpgradeSession,
            TrUpgradableReaderGuard, TrWriterGuard,
        },
        x_deps::abs_cancel::{CancelledToken, NonCancellableToken, TrMayCancel},
    },
};
use futures_lite::future::block_on;

/// 只用 trait 接口完成一轮：写 → 读 → 可升级读 → 升级为写 → 降级为读。
///
/// 这个函数对具体锁类型一无所知，因此它编译通过本身就说明
/// `CooperativeRwLock` 对 `TrAsyncRwLock` 的实现是完整的。
fn generic_round_trip<L>(lock: &L) -> usize
where
    L: TrAsyncRwLock<Target = usize>,
{
    let mut sess = lock.acq_session();

    {
        let mut w = block_on(sess.write_async().into_future()).unwrap();
        *w += 1;
    }

    let observed = {
        let r = block_on(sess.read_async().into_future()).unwrap();
        *r
    };

    {
        let upg = block_on(sess.upgradable_read_async().into_future()).unwrap();
        let mut upg_sess = upg.upgrade_session();
        let mut w = block_on(upg_sess.upgrade_async().into_future()).unwrap();
        *w += 1;
        let r = w.downgrade();
        assert_eq!(*r, observed + 1);
    }

    {
        // 降级得到的读者守卫只能读；要再写必须重新获取写许可。
        let mut w = block_on(sess.write_async().into_future()).unwrap();
        *w += 1;
    }

    {
        let r = block_on(sess.read_async().into_future()).unwrap();
        *r
    }
}

/// 测试完全经由 `abs_sync` trait 驱动的一轮读、写、升级与降级。
/// - 手段：构造 `CooperativeRwLockOwned<usize>`，交给只依赖 trait 的泛型函数驱动。
/// - 判断：返回值等于按步骤累加后的结果，说明各关联类型与方法都可用且语义正确。
#[test]
fn generic_trait_driven_round_trip() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    let v = generic_round_trip(&lock);
    assert_eq!(v, 3);
    assert_eq!(lock.into_inner(), 3);
}

/// 测试通过 `TrMayCancel` 传入永不取消的令牌时行为等同普通等待。
/// - 手段：用 `NonCancellableToken` 包裹读获取并 `block_on`。
/// - 判断：获取成功，读到的值与写入值一致。
#[test]
fn non_cancellable_token_should_behave_like_plain_wait() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(11);
    let mut sess = lock.acq_session();

    let r = block_on(
        sess.read_async()
            .may_cancel_with(NonCancellableToken::new()),
    )
    .unwrap();
    assert_eq!(*r, 11);
}

/// 测试通过 `TrMayCancel` 传入已取消的令牌时获取被中止。
/// - 手段：用 `CancelledToken` 包裹处于竞争状态的写获取。
/// - 判断：返回 `Err(Cancelled)`，且持锁者释放后锁仍可正常使用。
#[test]
fn cancelled_token_should_abort_contended_write() {
    let lock = CooperativeRwLockOwned::<usize>::new_owned(1);
    let mut sess_hold = lock.acquire_session();
    let mut sess = lock.acquire_session();

    let hold = sess_hold.try_write().unwrap();
    let r = block_on(sess.write_async().may_cancel_with(CancelledToken::new()));
    assert!(matches!(r, Result::Err(CoopRwLockError::Cancelled)));
    drop(r);

    drop(hold);
    let r = block_on(sess.read_async().into_future()).unwrap();
    assert_eq!(*r, 1);
}

/// 测试跨线程共享时锁可被安全地借用（`Send`/`Sync` 断言与并发读写）。
/// - 手段：把锁借用给 4 个作用域线程，每个线程通过 trait 接口累加多次。
/// - 判断：最终值等于全部线程的累加总量，且读取过程中未出现越界值。
#[test]
fn shared_lock_across_threads_should_keep_updates() {
    const THREADS: usize = 4;
    const ITERS: usize = 100;

    let lock = CooperativeRwLockOwned::<usize>::new_owned(0);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                for _ in 0..ITERS {
                    let mut sess = lock.acquire_session();
                    let mut w = block_on(sess.write_async().into_future()).unwrap();
                    *w += 1;
                    drop(w);
                    let r = block_on(sess.read_async().into_future()).unwrap();
                    assert!(*r <= THREADS * ITERS);
                }
            });
        }
    });
    assert_eq!(lock.into_inner(), THREADS * ITERS);
}
