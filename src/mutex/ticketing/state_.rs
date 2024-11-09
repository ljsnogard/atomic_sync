use core::{
    borrow::Borrow,
    fmt::{self, Debug},
    marker::PhantomData,
    pin::Pin,
};

use funty::Unsigned;

use atomex::{
    x_deps::funty,
    Bitwise, CmpxchResult,
    TrAtomicCell, TrAtomicData, TrAtomicFlags, TrCmpxchOrderings,
};
use abs_sync::cancellation::TrCancellationToken;

pub(super) struct MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    state_: B,
    _use_d: PhantomData<D>,
    _use_o: PhantomData<O>,
}

impl<D, B, O> MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    const K_NOT_ACQUIRED: D = D::ZERO;
    const K_FIRST_TICKET: D = D::ONE;

    const K_FLAG_BITS: u32 = 2;
    const K_HALF_BITS: u32 = (D::BITS - Self::K_FLAG_BITS) >> 1;

    /// A flag that indicates ONE AND ONLY ONE awaiter is exiting the queue
    /// and announce its ticket numbered with `n` in the ticket field. The
    /// following awaiter who hold the ticket numbered greater than `n` should
    /// contend for the ticker number
    ///
    /// The last affected awaiter in the queue should find the ticket number
    /// stops increase in a few round of checking and is responsible to turn off
    /// this flag.
    ///
    /// 用于标记有且只有一个锁的等待者退出了等待队列并放弃 ticket, 将其持有的编号`n`写在了
    /// Ticket 变量上。因此持有比`n`大的 ticket 的所有者应该与其他符合调教的持有者竞争获
    /// 取 `n` 的位置。
    ///
    /// 最后一个受影响的等待者（可以在若干个循环检查中发现）将负责清除这个标记。
    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_REPORT_FLAG() -> D { D::ONE << (D::BITS - 1) }

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_POISON_FLAG() -> D { D::ONE << (D::BITS - 2) }

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_HALF_MAX() -> D { (D::ONE << Self::K_HALF_BITS) - D::ONE }

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_TICKET_MASK() -> D { Self::K_HALF_MAX() }

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_SERVICE_MASK() -> D { Self::K_HALF_MAX() << Self::K_HALF_BITS }

    #[allow(non_snake_case)]
    #[inline(always)]
    fn K_NON_FLAG_MASK() -> D { Self::K_POISON_FLAG() - D::ONE }

    #[inline(always)]
    fn split_service_and_ticket(v: D) -> (D, D) {
        let ticket = v & Self::K_TICKET_MASK();
        let service = (v & Self::K_SERVICE_MASK()) >> Self::K_HALF_BITS;
        (service, ticket)
    }
    #[inline(always)]
    fn merge_service_and_ticket(
        current: D,
        service: D,
        ticket: D,
    ) -> D {
        debug_assert!(service <= Self::K_HALF_MAX());
        debug_assert!(ticket <= Self::K_HALF_MAX());
        let s = service << Self::K_HALF_BITS;
        let t = ticket;
        let vl = (s | t) & Self::K_NON_FLAG_MASK();
        let vh = current & (!Self::K_NON_FLAG_MASK());
        vl | vh
    }

    fn expect_can_get_ticket(v: D) -> bool {
        if Self::expect_report_flag_off(v) {
            let (s, t) = Self::split_service_and_ticket(v);
            s <= t && t < Self::K_HALF_MAX()
        } else {
            false
        }
    }
    fn desire_ticket_inc(v: D) -> D {
        let (s, t) = Self::split_service_and_ticket(v);
        Self::merge_service_and_ticket(v, s, t + D::ONE)
    }

    fn expect_report_flag_off(v: D) -> bool {
        !Self::expect_report_flag_on(v)
    }
    fn expect_report_flag_on(v: D) -> bool {
        let f = Self::K_REPORT_FLAG();
        v & f == f
    }
    fn desire_report_flag_off(v: D) -> D {
        let f = Self::K_REPORT_FLAG();
        let m = !f;
        v & m
    }
    fn desire_report_flag_on(v: D) -> D {
        let f = Self::K_REPORT_FLAG();
        v | f
    }

    fn expect_service_0(v: D) -> bool {
        let (s, t) = Self::split_service_and_ticket(v);
        s == Self::K_NOT_ACQUIRED && s < t
    }
    fn desire_service_1(v: D) -> D {
        let (_, t) = Self::split_service_and_ticket(v);
        Self::merge_service_and_ticket(v, Self::K_FIRST_TICKET, t)
    }
}

impl<D, B, O> MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub const fn new(cell: B) -> Self {
        MutexState {
            state_: cell, 
            _use_d: PhantomData,
            _use_o: PhantomData,
        }
    }

    pub fn is_acquired(&self) -> bool {
        self.value() != Self::K_NOT_ACQUIRED
    }

    pub fn try_release(&self, ticket: D) -> Result<D, D> {
        if ticket < Self::K_HALF_MAX() {
            let expect = |v| {
                let (s, t) = Self::split_service_and_ticket(v);
                s == ticket && s <= t
            };
            let desire = |v| {
                let (mut s, t) = Self::split_service_and_ticket(v);
                if Self::expect_report_flag_off(v) {
                    if s == t {
                        // It indicates no other contenders are queued for the
                        // guard so can we reset the ticket.
                        let z = Self::K_NOT_ACQUIRED;
                        Self::merge_service_and_ticket(v, z, z)
                    } else {
                        s += D::ONE;
                        Self::merge_service_and_ticket(v, s, t)
                    }
                } else {
                    s += D::ONE;
                    Self::merge_service_and_ticket(v, s, t)
                }
            };
            self.try_spin_compare_exchange_weak(expect, desire)
                .into()
        } else {
            let expect = |v| {
                let (s, t) = Self::split_service_and_ticket(v);
                s == ticket && s == t
            };
            let desire = |_| Self::K_NOT_ACQUIRED;
            self.try_spin_compare_exchange_weak(expect, desire)
                .into()
        }
        
    }

    pub fn spin_get_ticket<'a, 'c, C>(
        &'a self,
        cancel: Pin<&'c mut C>,
    ) -> Result<D, Pin<&'c mut C>>
    where
        C: 'c + TrCancellationToken,
    {
        let mut v = Self::K_NOT_ACQUIRED;
        loop {
            match self.try_once_compare_exchange_weak(
                v,
                Self::expect_can_get_ticket,
                Self::desire_ticket_inc,
            ) {
                CmpxchResult::Succ(x) => {
                    let (_s, t) = Self::split_service_and_ticket(x);
                    // log::trace!("[MutexState::spin_get_ticket] s({s}) t({t})");
                    break Result::Ok(t + D::ONE);
                }
                CmpxchResult::Fail(x) => v = x,
                CmpxchResult::Unexpected(x) => v = x,
            }
            if cancel.borrow().is_cancelled() {
                break Result::Err(cancel);
            }
        }
    }
}

impl<D, B, O> MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    /// The max number of turns to take for reporter to check if itself becomes
    /// the last one to report.
    const REPORT_MAX_CHK: usize = 16;

    #[cfg(test)]
    fn _trace<C>(
        p: &'static str,
        x: &TicketCtx<'_, '_, C, D, B, O>,
        n: AcqSt,
        m: D)
    where
        C: TrCancellationToken,
    {
        let v = x.s.value();
        let (s, t) = Self::split_service_and_ticket(v);
        let b = Self::expect_can_get_ticket(v);
        log::trace!("[{p}] {x:?}: go {n:?} with ticket({m})@(s: {s}, t{t}, can_get_ticket({b})")
    }

    #[cfg(not(test))]
    fn _trace<C>(
        _: &'static str,
        _: &TicketCtx<'_, '_, C, D, B, O>,
        _: AcqSt,
        _: D)
    where
        C: TrCancellationToken,
    {}

    pub fn spin_wait<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
    ) -> Result<D, ()>
    where
        C: TrCancellationToken,
    {
        let mut m = x.t;
        let mut a = AcqSt::TicketStandby;
        let mut _i = 0usize;
        loop {
            (a, m) = match a {
                AcqSt::TicketStandby  => Self::ticket_standby_ (x, m),
                AcqSt::ProactPrepare  => Self::proact_prepare_ (x, m),
                AcqSt::ProactSubmit   => Self::proact_submit_  (x, m),
                AcqSt::ReportStandby  => Self::report_standby_ (x, m),
                AcqSt::PassivePrepare => Self::passive_prepare_(x, m),
                AcqSt::PassiveSubmit  => Self::passive_submit_ (x, m),
                AcqSt::ReportFlagRst  => Self::report_flagrst_ (x, m),
                AcqSt::AcqCompleted => break Result::Ok(m),
                AcqSt::AcqCancelled => break Result::Err(()),
            };
            // log::trace!("[MutexState::spin_wait_] {x:?}: #{_i} {a:?}");
            _i += 1;
        }
    }
    
    fn ticket_standby_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        loop {
            let v = x.s.value();
            let (srv, tkt) = Self::split_service_and_ticket(v);
            if srv == m {
                let n = AcqSt::AcqCompleted;
                Self::_trace("MutexState::ticket_standby_", x, n, m);
                break (n, m);
            }
            if srv == Self::K_NOT_ACQUIRED && m == Self::K_FIRST_TICKET {
                let _ = x.s.try_spin_compare_exchange_weak(
                    Self::expect_service_0,
                    Self::desire_service_1,
                );
                // log::trace!("[MutexState::ticket_standby_] {x:?}: first_ticket");
                continue;
            }
            // DO NOT PUT AFTER report flag check
            if x.c.borrow().is_cancelled() {
                let n = AcqSt::ProactPrepare;
                Self::_trace("MutexState::ticket_standby_", x, n, m);
                break (n, m)
            }
            if Self::expect_report_flag_on(v) {
                let n = if tkt > m {
                    AcqSt::ReportStandby
                } else {
                    debug_assert!(m > tkt);
                    AcqSt::PassivePrepare
                };
                Self::_trace("MutexState::ticket_standby_", x, n, m);
                break (n, m);
            }
        }
    }

    fn proact_prepare_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        let expect = |v| {
            let (srv, _) = Self::split_service_and_ticket(v);
            Self::expect_report_flag_off(v) && srv < m
        };
        let desire = |v| {
            let u = Self::desire_report_flag_on(v);
            let (srv, _) = Self::split_service_and_ticket(u);
            Self::merge_service_and_ticket(u, srv, m)
        };
        let r = x.s.try_spin_compare_exchange_weak(expect, desire);
        // the try_spin_update_ could fail if more than one queued tasks
        // are trying to cancel. So the failed task should delay its report
        // until the other one completed.
        let n = if r.is_succ() {
            AcqSt::ProactSubmit
        } else {
            AcqSt::TicketStandby
        };
        Self::_trace("MutexState::proactive_prepare_", x, n, m);
        (n, m)
    }

    fn proact_submit_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        let mut i = 0usize;
        loop {
            let v = x.s.value();
            let (_, tkt) = Self::split_service_and_ticket(v);
            if tkt > m {
                let n = AcqSt::AcqCancelled;
                Self::_trace("MutexState::proact_submit_", x, n, m);
                break (n, m);
            }
            if i >= Self::REPORT_MAX_CHK {
                let n = AcqSt::ReportFlagRst;
                Self::_trace("MutexState::proact_submit_", x, n, m);
                break (n, m);
            }
            if tkt == m {
                i += 1usize;
            }
        }
    }

    fn report_standby_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        // let mut i = 0usize;
        loop {
            let v = x.s.value();
            let (s, _) = Self::split_service_and_ticket(v);
            if s > m {
                unreachable!("[MutexState::report_standby_]")
            }
            if s == m {
                let n = AcqSt::AcqCompleted;
                Self::_trace("MutexState::report_standby_", x, n, m);
                break (n, m);
            }
            if Self::expect_report_flag_off(v) {
                let n = AcqSt::TicketStandby;
                Self::_trace("MutexState::report_standby_", x, n, m);
                break (n, m);
            }
        }
    }

    fn passive_prepare_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        let expect = |v| {
            let (_, t) = Self::split_service_and_ticket(v);
            t < m && Self::expect_report_flag_on(v)
        };
        let desire = |v| {
            let (s, _) = Self::split_service_and_ticket(v);
            Self::merge_service_and_ticket(v, s, m)
        };
        let mut v = x.s.value();
        loop {
            let r = x.s.try_once_compare_exchange_weak(v, expect, desire);
            match r {
                CmpxchResult::Succ(v) => {
                    let (_, t) = Self::split_service_and_ticket(v);
                    let n = AcqSt::PassiveSubmit;
                    Self::_trace("MutexState::passive_submit_", x, n, m);
                    break (n, t);
                }
                CmpxchResult::Unexpected(v) => {
                    let (_, t) = Self::split_service_and_ticket(v);
                    debug_assert!(Self::expect_report_flag_on(v));
                    debug_assert!(t > m);
                    let n = AcqSt::ReportStandby;
                    Self::_trace("MutexState::passive_submit_", x, n, m);
                    break (n, t);
                }
                CmpxchResult::Fail(f) => {
                    v = f;
                    continue;
                }
            }
        }
    }
    
    fn passive_submit_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        let mut i = 0usize;
        loop {
            let v = x.s.value();
            if i >= Self::REPORT_MAX_CHK {
                let n = AcqSt::ReportFlagRst;
                Self::_trace("MutexState::passive_submit_", x, n, m);
                break (n, m);
            }
            let (_, tkt) = Self::split_service_and_ticket(v);
            if tkt > m {
                let n = AcqSt::ReportStandby;
                Self::_trace("MutexState::passive_submit_", x, n, m);
                break (n, m);
            }
            if tkt == m {
                i += 1usize;
            }
        }
    }
    
    fn report_flagrst_<C>(
        x: &TicketCtx<'_, '_, C, D, B, O>,
        m: D,
    ) -> (AcqSt, D)
    where
        C: TrCancellationToken,
    {
        let n = if x.c.borrow().is_cancelled() {
            let desire = |v| {
                let (mut s, t) = Self::split_service_and_ticket(v);
                s += if s == m { D::ONE } else { D::ZERO };
                let u = Self::merge_service_and_ticket(v, s, t);
                Self::desire_report_flag_off(u)
            };
            let _ = x.s.try_spin_compare_exchange_weak(
                Self::expect_report_flag_on,
                desire,
            );
            AcqSt::AcqCancelled
        } else {
            // The job may be done by other task so we can just move on
            let _ = x.s.try_spin_compare_exchange_weak(
                Self::expect_report_flag_on,
                Self::desire_report_flag_off,
            );
            AcqSt::TicketStandby
        };
        Self::_trace("MutexState::report_flagrst_", x, n, m);
        (n, m)
    }
}

impl<D, B, O> AsRef<<D as TrAtomicData>::AtomicCell> for MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn as_ref(&self) -> &<D as TrAtomicData>::AtomicCell {
        self.state_.borrow()
    }
}

impl<D, B, O> TrAtomicFlags<D, O> for MutexState<D, B, O>
where
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{}

#[derive(Clone, Copy, Debug)]
enum AcqSt {
    /// Wait for the ticket to acquire the guard.
    TicketStandby,

    /// The task is to turn on the report flag.
    ProactPrepare,

    /// The task has announced its cancelling ticket number.
    ProactSubmit,

    /// Report flag is found on but no need to report.
    ReportStandby,

    /// Report flag is on and to contend for the ticket.
    PassivePrepare,

    /// The ticket has acquired and started waiting for turning off report flag.
    PassiveSubmit,

    /// The task is the last affected enqueued one, who is to turn off the
    /// report flag.
    ReportFlagRst,

    /// The acquire task is completed.
    AcqCompleted,

    /// The acquire task is cancelled.
    AcqCancelled,
}

pub(super) struct TicketCtx<'a, 'c, C, D, B, O>
where
    C: 'c + TrCancellationToken,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub s: &'a MutexState<D, B, O>,
    pub c: Pin<&'c mut C>,
    pub t: D,
}

impl<'a, 'c, C, D, B, O> TicketCtx<'a, 'c, C, D, B, O>
where
    C: 'c + TrCancellationToken,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    pub fn new(
        state: &'a MutexState<D, B, O>,
        cancel: Pin<&'c mut C>,
        ticket: D,
    ) -> Self {
        TicketCtx {
            s: state,
            c: cancel,
            t: ticket,
        }
    }
}

impl<'b, C, D, B, O> Debug for TicketCtx<'_, 'b, C, D, B, O>
where
    C: 'b + TrCancellationToken,
    D: TrAtomicData + Unsigned,
    <D as TrAtomicData>::AtomicCell: Bitwise + TrAtomicCell,
    B: Borrow<<D as TrAtomicData>::AtomicCell>,
    O: TrCmpxchOrderings,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Task({:p})Ticket({})", self, self.t)
    }
}

#[cfg(test)]
mod tests_ {
    use core::sync::atomic::*;
    use abs_sync::cancellation::CancelledToken;
    use atomex::StrictOrderings;

    use super::*;

    #[test]
    fn mutex_state_mask_test() {
        type St = MutexState<u32, AtomicU32, StrictOrderings>;

        let s = St::new(AtomicU32::new(0));
        let v = s.value();
        assert_eq!(v, 0);
        assert!(St::expect_can_get_ticket(v));
        let v = St::desire_report_flag_on(v);
        assert!(!St::expect_can_get_ticket(v));

        assert_eq!(St::K_REPORT_FLAG(), (1 << 31));
        assert_eq!(St::K_HALF_MAX(), (1 << 15) - 1);
        assert_eq!(St::K_SERVICE_MASK(), ((1 << 15) - 1) << 15);
        assert_eq!(St::K_SERVICE_MASK(), St::K_TICKET_MASK() << 15);
        {
            let (s, t) = St::split_service_and_ticket(v);
            assert!(s == 0 && t == 0);
            let v1 = St::merge_service_and_ticket(v, s, 1);
            assert!(St::expect_report_flag_on(v1));
            let (s1, t1) = St::split_service_and_ticket(v1);
            assert!(s1 == s && t1 == 1);
        }

        let r = s.try_spin_compare_exchange_weak(
            St::expect_report_flag_off,
            St::desire_report_flag_on
        );
        assert!(r.is_succ());

        let r = s.spin_get_ticket(CancelledToken::pinned());
        assert!(r.is_err())
    }

    #[test]
    fn mutex_state_update_test() {
        type St = MutexState<u8, AtomicU8, StrictOrderings>;

        let m = 1u8;
        let expect = |v| {
            let (srv, _) = St::split_service_and_ticket(v);
            St::expect_report_flag_off(v) && srv < m
        };
        let desire = |v| {
            let u = St::desire_report_flag_on(v);
            let (s, _) = St::split_service_and_ticket(u);
            St::merge_service_and_ticket(u, s, m)
        };
        let s = St::new(AtomicU8::new(0));
        let r = s.try_spin_compare_exchange_weak(expect, desire);
        assert!(matches!(r, CmpxchResult::Succ(_)));
        let v = s.value();
        assert!(St::expect_report_flag_on(v));
        assert!(!St::expect_report_flag_off(v));
        assert!(s.spin_get_ticket(CancelledToken::pinned()).is_err());
    }
}
