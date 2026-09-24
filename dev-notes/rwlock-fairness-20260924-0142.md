# cooperative 公平性策略调整：实施方案（待执行）

- 日期：2026-09-24 01:42
- 前置阅读：`rwlock-20260923-2204.md` §4.5（升级插队）、§4.9（一次性放行）、§6.3（8 线程崩塌的定位过程）
- 状态：**方案，尚未实现**。本文只描述"要改什么、为什么、怎么验证"，代码留到下一次会话。

---

## 1. 要解决的问题

`rwlock::cooperative` 现在采用**严格 FIFO**：只要等待队列非空，新读者在快速路径上
就被 `WAITER_QUEUED` 拒绝。8 线程压测显示这条规则把"存在一个等待者"放大成
"所有读者都必须排队"：

| 指标（读多写少，tokio 多线程，N=8） | 数值 |
|---|---|
| `快路/op`（快速路径成功率） | **0.008** |
| `入队/op` | 0.942 |
| `pass/op`（抢队列锁次数） | 2.664 |
| ns/op | 610（tokio 对照 497） |

对照实验（临时删掉读快路径里的 `WAITER_QUEUED` 检查，即"永远允许读者插队"）：
`快路/op` 0.008 → **0.754**、`入队/op` 0.94 → 0.20、ns/op 1007 → **334**。

也就是说：**8 线程下大约一半的开销来自"读者被迫排队"**，而其中大部分排队
是无谓的——队列里往往只有读者，此时新读者完全可以走无锁快速路径。

## 2. 目标语义

把"队列非空"拆成两个不同的判断：

| 判断 | 含义 | 用途 |
|---|---|---|
| `WAITERS_PRESENT` | 队列非空 | `run_pass_` 的快出口（没人排队就不必抢队列锁） |
| `WRITER_QUEUED` | 队列里存在 **Write 或 Upgrade** 等待者 | 读快速路径的准入条件 |

读快速路径从

```text
!WRITER_ACTIVE && !WAITERS_ANY && READER_COUNT < MAX
```

改为

```text
!WRITER_ACTIVE && !WRITER_QUEUED && READER_COUNT < MAX
```

即：**只有"写者或升级在排队"才挡住新读者；队列里只有读者时，读者可以插队。**

这与 `rwlock::preemptive` 的既有策略同源（它的 `expect_can_read` 只看
`writer_not_queued`），但补上了 preemptive 缺失的两块（见 §5）。

三种策略的对比：

| 策略 | 读快路径条件 | 写者饥饿 | 排队读者饥饿 | N=8 读多写少 |
|---|---|---|---|---|
| A. 严格 FIFO（现状） | `!W && !队列非空` | 不会 | — | 610 ns/op（实测） |
| **B. 写者屏障（本方案）** | `!W && !写者/升级排队` | 不会 | 不会（§4） | 预期 350~500 |
| C. 完全放开（仅作上界参考） | `!W` | **会** | — | 334 ns/op（实测） |

## 3. 状态字改动

现在的位布局（`state_.rs`）：

```text
bit N-1        WRITER_ACTIVE
bit N-2        WAITER_QUEUED     ← 拆成两个
bit N-3        UPGRADE_ACTIVE
bit [0, N-4]   READER_COUNT
```

改为：

```text
bit N-1        WRITER_ACTIVE
bit N-2        WAITERS_PRESENT
bit N-3        WRITER_QUEUED
bit N-4        UPGRADE_ACTIVE
bit [0, N-5]   READER_COUNT
```

`K_MAX_READER_COUNT` 相应从 `D::MAX >> 3` 变成 `D::MAX >> 4`。

> **注意（对使用者的影响）**：`D` 是公开的默认类型参数（默认 `usize`）。
> 64 位下读者上限仍是 2^60，无实际影响；但若有人显式用 `D = u8`，
> 上限会从 31 降到 15。这项变化需要在 CHANGELOG / 文档里点名。

具体到代码：

- `K_WAITER_QUEUED` 改名为 `K_WAITERS_PRESENT`，语义不变（队列非空）；
- 新增 `K_WRITER_QUEUED` 与一组 `expect_writer_queued_ / desire_*` 谓词；
- 新增 `pub(super) fn mark_writer_queued()` / `clear_writer_queued()`（与现有的
  `mark_waiter_queued` 同型，走 `try_spin_update_(|_| true, ..)`）；
- `CoopRwStateSnapshot` 增加 `writer_queued: bool` 字段；
- `expect_can_read_fast_` 用 `expect_writer_queued_` 取代 `expect_no_waiter_queued_`；
- `expect_can_read_queued_` **不动**（排队路径本来就不看这两个标记）。

## 4. `WRITER_QUEUED` 的维护与不变式

因为它是"队列里是否还有活的写/升级等待者"，维护点必须精确到**槽位状态变化**，
而不是"节点是否还在队列里"（被取消的写节点可能长期留在队中，见 §5.2）。

在 `RwCore` 增加一个计数器（只在队列锁内读写，普通字段即可，不必原子）：

```text
writer_waiters_: usize     // 队列中仍处于 Pending(Waiting|Admitted) 的 Write/Upgrade 槽位数
```

规则：

| 时机 | 位置 | 动作 |
|---|---|---|
| 入队 Write/Upgrade 槽位 | `enqueue_` | `+1`；若由 0 变 1 则 `mark_writer_queued()` |
| 该槽位领取成功 | `acquire` / `poll_acquire` 里调用 `mark_acquired` 之后 | `-1`；若归 0 则 `clear_writer_queued()` |
| 该槽位被取消 | `cancel_wait` 里 `mark_cancelled` 之后 | 同上 |
| 队列清空 | `pass_` 尾部 | 一并 `clear_writer_queued()`（防御性） |

不变式：

1. `writer_waiters_ > 0` ⟺ `WRITER_QUEUED` 置位（由上述两个点维护，且在队列锁内）；
2. **领取/取消后就可以递减**：写者一旦拿到许可，`WRITER_ACTIVE` 本身就会挡住新读者，
   不需要继续靠标记挡；升级成功同理（`WRITER_ACTIVE` 置位）。取消则是因为等待者已消失。
3. 递减时如果该节点恰好成为队首且 `pending_count() == 0`，随后的 `pass_` 会把它剪掉——
   顺序无关紧要，因为计数已经归零。

## 5. 必须一起处理的两个坑（否则会退化或死锁）

### 5.1 升级必须算作 barrier

`preemptive` 的 `WRITER_QUEUED` 只由写者置位，升级不置位，结果是"持续有新读者时
升级可能被无限推迟"（已在 `rwlock-20260923-2204.md` §2.2 记录，
且 `preemptive::tests_::pending_upgrade_should_not_block_new_readers` 固化了这个现象）。

cooperative 版本**不能**照抄这一点：`Upgrade` 节点此刻是"插到队首"的请求
（§4.5），如果它不算 barrier，新读者会不断插进来，升级永远等不到
`READER_COUNT == 1`，等于把 §4.5 的成果丢掉。所以入队计数必须包含 `WaitKind::Upgrade`。

### 5.2 取消必须收回标记

`preemptive` 的 `may_break_with_impl_` 收到取消信号时直接返回，
`WRITER_QUEUED` 留在状态字里，之后所有读者都被挡住，直到某个写者成功并析构
（没有写者就是活锁）。`preemptive::tests_::failed_try_write_should_block_later_readers`
固化了这个行为。

cooperative 版必须由 `Drop`（`cancel_wait` → `mark_cancelled` → 递减计数）把标记收回，
这正是 §4 表格里"取消"那一行的意义。**回归测试必须覆盖**：一个写者等待被取消后，
新读者应当能立刻走快速路径。

## 6. 代码改动清单

| 文件 | 改动 |
|---|---|
| `state_.rs` | 位域重排；`K_WAITERS_PRESENT` 改名；新增 `WRITER_QUEUED` 谓词与 set/clear；`snapshot` 加字段；`expect_can_read_fast_` 换条件；补位域单测 |
| `wait_.rs` | 不改（槽位状态机与 `Admitted` 语义保持） |
| `core_.rs` | `RwCore` 加 `writer_waiters_: usize`；`enqueue_` 里 +1/置位；`acquire`/`poll_acquire`/`cancel_wait` 里在 `mark_*` 之后 -1/清位；`pass_` 里读快路径判定改用 `snapshot().writer_queued`；`run_pass_` 的快出口改用 `WAITERS_PRESENT` |
| `rwlock_.rs` / `reader_.rs` / `writer_.rs` / `upgrade_.rs` | 不改（接口与 future 形状不变） |
| `tests_.rs` | 新增"只有读者排队时新读者走快速路径"等测试（见 §7） |
| `tests/cooperative_rwlock.rs` | 新增取消写者等待后读者恢复的回归测试；升级 barrier 仍生效 |
| `benches/rwlock_cooperative.rs` | 不改结构，直接复测 |

改动量估计：`core_.rs` 约 +40 行、`state_.rs` 约 +30 行，其余为测试。

## 7. 测试计划

**单元（`state_.rs`）**
1. `writer_queued` 置/清后，`try_read_fast` 的成败随之变化；
2. `WAITERS_PRESENT` 与 `WRITER_QUEUED` 相互独立（只置前者不影响读快路径）；
3. 位域不再重叠（读者计数上界、各标记位互不干扰）。

**单元（`tests_.rs`，运行时）**
4. 写者持锁 → 读者 A 排队 → 释放写者 → A 被放行（现有测试，须继续通过）；
5. **只有读者排队时新读者不排队**：写者持锁 → 读者 A 排队 → 释放写者（A 已被放行但尚未领取）
   → 此刻 `WRITER_QUEUED == 0`，新读者 B 应当**不经过队列**直接成功；
   —— 这条最能体现本次改动（可用内部计数器或行为断言）。
6. 升级请求排队时，新读者必须 `WouldBlock`（现有
   `upgrade_should_jump_ahead_of_queued_writer` 的姊妹用例，覆盖 barrier 语义）；
7. **取消写者等待后标记被收回**：写者排队 → 取消它的 future → 新读者立刻成功；
8. 写者不饥饿：写者排队后持续有新读者尝试，写者在既有读者排空后仍能进入；
9. 现有的活性/升级/取消测试全部保持通过（尤其
   `cancelled_head_waiter_should_not_starve_followers`）。

**集成（`tests/cooperative_rwlock.rs`）**
10. 新读者在"只有读者排队"时不被拖进队列（行为级：用 `reader_count` 与超时观察）；
11. 多线程下写者仍不被饿死（有界循环 + 超时断言）。

**性能（`benches/rwlock_cooperative.rs`）**
12. 复测读多写少的 N=1/2/4/8，对照 `tokio::sync::RwLock`；
13. 复测写多读少的同组数字（确认没有把写路径拖慢）。

验收标准：

- N=8 读多写少 **≤ 450 ns/op**（现状 610，理论上界 334）；
- N=1 不退化（≤ 70 ns/op）；
- 写多读少不退化超过 10%；
- 上述测试 9/10/11 全绿（是否饥饿是**硬性**要求，比数字重要）。

## 8. 风险与回退

| 风险 | 影响 | 应对 |
|---|---|---|
| 排队读者被插队者饿死 | 活性 | §4 论证：能挡住排队读者的原因（写者持锁 / 队首是写或升级）同时也挡住快路径；同节点里的可升级槽位不因新读者插队而恶化。用测试 8 覆盖 |
| 写者饥饿 | 活性 | `WRITER_QUEUED` 在写者入队后立刻置位，新读者停止插队 → 既有读者排空即可进入 |
| 升级饥饿 | 活性 | §5.1：`Upgrade` 计入 barrier |
| 取消后标记泄漏 | 活性 | §5.2 + 测试 7 |
| `D = u8` 等小位宽读者上限下降 | API 语义 | §3 注记，必要时在文档与 CHANGELOG 提示 |
| 数字未达预期 | 性能 | 回退点只有一个：把 `expect_can_read_fast_` 的条件换回 `WAITERS_PRESENT`；`writer_waiters_` 与 `WRITER_QUEUED` 可保留（无用但无害）或一并删除 |

## 9. 备选方案（本方案不采用，仅记录）

1. **"入队即领取"**：不改状态位，改为在 `acquire` 里发现合并进队首读类节点后立刻尝试领取。
   它省不掉分配节点与抢队列锁，因此预计收益远小于改用快路径（对照实验中快路径成功率
   从 0.008 回升到 0.754 才是主要收益来源）。
2. **按 `D` 拆分策略**：给锁加一个类型参数选择公平性策略。增加公开 API 复杂度，
   收益与方案 B 相同，暂不采用。
3. **批量放行连续读类节点**：属于另一个正交优化（减少交接次数），
   与本文的准入条件无关，见 `rwlock-20260923-2204.md` §7。
