# cooperative 公平性策略调整：实施方案（待执行）

- 日期：2026-09-24 01:42
- 前置阅读：`rwlock-20260923-2204.md` §4.5（升级插队）、§4.9（一次性放行）、§6.3（8 线程崩塌的定位过程）
- 状态：**方案 B 已实现、测试全绿并保留**；可靠测量范围内无退化，但 N=8 的
  墙钟收益未能在当前共享主机上判定（见 §11）。§11.6 记录了复测口径。

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
- `CoopRwStateSnapshot` 增加 `writer_queued: bool` 字段（**仅用于测试与 `debug_assert`
  观测**，`pass_` 的放行判定不依赖它——`pass_` 只看写者/可升级/读者计数）；
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
| `state_.rs` | 位域重排；`K_WAITER_QUEUED` / `waiter_queued` 一族改名 `WAITERS_PRESENT`；新增 `WRITER_QUEUED` 谓词与 set/clear；`snapshot` 加字段（观测用）；`expect_can_read_fast_` 换条件（**`expect_can_write_fast_` / `expect_can_upgradable_read_fast_` 保持 `WAITERS_PRESENT` 不变**）；同步 `Debug` 输出与 `try_read_queued` 文档注释；补位域单测 |
| `wait_.rs` | 不改（槽位状态机与 `Admitted` 语义保持） |
| `core_.rs` | `RwCore` 加 `writer_waiters_: usize`；`enqueue_` 里 +1/置位；`acquire`/`poll_acquire`/`cancel_wait` 里在 `mark_*` 之后 -1/清位；`run_pass_` 的快出口改用 `WAITERS_PRESENT`（`pass_` 的放行判定不含读快路径条件，无需改动） |
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

---

## 10. 本轮设计复核确认的边界（2026-09-24）

对源码（`cooperative/state_.rs`、`core_.rs`、`wait_.rs`、三个获取 future）与
`preemptive/rwlock_.rs` 逐条复核后，确认以下判定；实施时以此为准。前三条是
本轮明确拍板的取舍，后七条是复核中补出的实施细节。

1. **只放松普通读**。只有 `expect_can_read_fast_` 改用 `expect_writer_queued_`；
   `try_write_fast` 与 `try_upgradable_read_fast` **继续要求队列为空**
   （`WAITERS_PRESENT == false`）。依据是 §1 的对照实验：只删掉读快路径里那一个检查
   就得到 `快路/op = 0.754`，与负载中 75% 的普通读吻合；可升级读若一并插队，会把
   "升级需等既有读者排空"的不确定性放大，且没有实测数据支撑。
2. **`snapshot().writer_queued` 只作观测**。它供单元测试与 `pass_` 内的
   `debug_assert`（`writer_waiters_ > 0 ⟺ 置位`）使用，不参与放行判定。
   原文 §3、§6 中"`pass_` 里读快路径判定改用 `snapshot().writer_queued`"是措辞错误，已修正。
3. **窄 `D` 的读者上限下降接受**。`K_MAX_READER_COUNT` 由 `D::MAX >> 3` 变 `>> 4`：
   默认 `usize` 下为 `2^60 - 1`（无实际影响），`D = u8` 时 31 → 15。
   实施时在类型文档与 CHANGELOG 点名；按 AGENTS.md 第 1 条，这属于面向使用者的
   语义变化，须随实现一起记录，但不是新增/修改公开签名。
4. **计数递减的单次性**。三个 `*AcquireInner` 在领取成功时先清 `pending_` 再返回
   `Ready`（见 `reader_.rs` 的 `poll`），因此 `Drop` 的 `cancel_wait` 不可能对同一槽位
   二次执行。即便如此，递减处仍加 `debug_assert!(writer_waiters_ > 0)`：
   `usize` 下溢是静默 wrap，属于必须靠断言暴露的一类（同 §4.8 的教训）。
5. **独占槽位的判定用 `!node.is_readish()`**，这样 `cancel_wait` 不必再取节点锁，
   `wait_.rs` 无需新增接口，与 §6 的"不改"一致。
6. **活性判据（比 §8 的表述更完整）**：任何让"排队读者"领取失败的原因——写者持有、
   队首是独占节点、读者计数饱和——**同时也让读快路径失效**（前两者分别命中
   `WRITER_ACTIVE` 与 `WRITER_QUEUED`，第三者命中 `expect_reader_lt_max_`）。
   所以插队读者不可能成为排队读者持续失败的原因。§8 只覆盖了前两者。
7. **时序论证**：`atomex::TrAtomicFlags::try_spin_compare_exchange_weak` 每次重试都用
   最新负载值重新求值谓词，因此"读者观察到 `WRITER_QUEUED == 0` 且 CAS 成功"的线性化点
   必定早于写者置位；不存在读者在写者入队之后仍挤进快路径的窗口。
8. **§7 单元测试 1/2 的拆分**：`fast_path_should_reject_when_queue_not_empty` 必须拆成两条——
   只置 `WAITERS_PRESENT` 时 `try_read_fast` 应当**成功**（而 `try_write_fast`、
   `try_upgradable_read_fast` 仍失败）；置 `WRITER_QUEUED` 时 `try_read_fast` 才失败。
9. **§7 第 5 条的确定性写法**：单线程 compio 下
   "写者持锁 → 读者 A 异步入队 → 释放写者 → **不 await** 立刻 `sess_b.try_read()`"。
   此时 A 已 `Admitted` 但尚未被 `poll`，队列非空而 `writer_waiters_ == 0`：
   旧实现返回 `WouldBlock`，新实现返回 `Ok`，两种语义可判定地分开。
   若该执行器会内联 `poll` 破坏这个窗口，退路是给 `RwCore` 加 `#[cfg(test)]` 观测探针。
10. **性能复测口径**：bench 当前已无 `快路/op`、`入队/op`、`pass/op`、`wake/op`
    计数器（当时是临时探针，未保留）。验收以 §7 的 ns/op 硬指标为准；若要验证
    "机制生效"而不只是"数字变好"，临时加回计数器，测完撤回。

---

## 11. 实施与实测结果（2026-09-24，方案 B）

本节记录实际实施与测量。**结论：方案 B 的机制正确生效，但 ns/op 收益远低于
§2 预测（预期 350~500），在 N=8 读多写少上没有可判定的提升，未达 §7 的
≤450 验收线。** 根因见 §11.4。

### 11.1 落地内容

| 文件 | 改动 |
|---|---|
| `state_.rs` | 5 域位布局；`WAITERS_PRESENT` / `WRITER_QUEUED` 位与谓词；`expect_can_read_fast_` 换条件；`Debug`、快照、文档注释；单测重组为 4 条位域语义测试 |
| `core_.rs` | `QueuePayload { nodes_, writer_waiters_ }`；`enqueue_` / `acquire` / `poll_acquire` / `cancel_wait` 维护计数与位；`pass_` 一致性 `debug_assert` 与队列空时的防御性归零 |
| `mod.rs` / `rwlock_.rs` / `upgrade_.rs` | 公平性策略与读者上限的文档更新 |
| `tests_.rs` / `tests/cooperative_rwlock.rs` | 新增 4 条运行时测试 + 1 条集成回归（读者插队、写者不饥饿、升级屏障、取消收回标记） |

**与原文的实现偏差（1 处）**：§4 原写"在 `RwCore` 增加一个普通字段
`writer_waiters_: usize`"。但 `RwCore` 共享在 `Arc` 里，所有入口只拿得到
`&self`，普通字段无法可变访问。实际做法是把它放进队列锁的载荷
`QueuePayload`（`queue_: SpinLock<QueuePayload>`），与队列同锁保护，
既满足"只在队列锁内读写"，也不需要 `UnsafeCell`/`unsafe`。`wait_.rs` 确实未改。

验证：`cargo test`（51 lib + 7 集成 + 8 preemptive + 3 doc）、
`cargo test --release`、`cargo clippy --all-targets -- -D warnings` 全部通过。

### 11.2 机制确实生效（临时探针，读多写少，N=8）

探针只用于取**比率**（它本身有原子开销，ns/op 不可信）：

| 指标（N=8 读多写少） | 严格 FIFO（§6.3 记录） | 方案 B 实测 | 条件 C 实测 |
|---|---|---|---|
| 读快路径命中率 | 0.008 | **0.328** | 0.999 |
| 入队/op（读） | 0.942（合计） | 0.505 | 0.001 |
| 入队/op（写 / 升级读 / 升级） | — | 0.149 / 0.049 / 0.000 | 0.286 / 0.094 / 0.000 |
| 抢队列锁/op | 2.664 | **1.042** | 1.994 |
| wake/op | 0.68 | 0.686 | 0.381 |
| ns/op | 610（当时） | 609.9（干净复测） | **355.2** |

即：方案 B 把读快路径命中率提高了约 40 倍、入队率降了约 1/3、抢锁次数降到
约 40%，**但 ns/op 没有随之下降**；而条件 C 仍能复现文档记录的量级
（355 vs 334）。说明剩余的墙钟时间主要不在"读者抢队列锁"，而在
**读者排队后等待跨线程唤醒**：条件 C 几乎不让读者入队，wake/op 从 0.686
降到 0.381，这才是 610 → 355 的来源。

### 11.3 墙钟 A/B（同机、紧邻、同配置，stash 切换基线）

本机是 4 核共享容器，测量期间主机 load average ≈ 2.75，**N=4/8 的跨线程方差
远大于效应**，因此下表只作记录、不作结论：

| 配置 | 基线（严格 FIFO） | 方案 B |
|---|---|---|
| 读多写少 N=1（ops=20k, 9 轮） | 65.3 ns | 64.9 ns |
| 读多写少 N=2（同上） | 211.4 ns | **199.2 ns（−5.8%）** |
| 写多读少 N=1（同上） | 71.2 ns | 72.6 ns |
| 写多读少 N=2（同上） | 307.9 ns | 313.1 ns |
| 读多写少 N=4（ops=8k, 5 轮） | 85.4 ns | 68.5 ns |
| 读多写少 N=8（ops=8k, 5 轮） | 345.1 ns | 605.9 ns（**与另一轮 750.8 → 609.9 矛盾**） |
| 读多写少 N=8 compio 单线程 | 56.2 ns | 56.8 ns（无退化） |

同一份代码在 N=8 的两轮之间可以从 345 变到 750（基线）、从 610 变到 503
（方案 B）。**结论：本机无法可靠测量 N≥4 的 ns/op 差异；唯一稳定的收益信号是
N=2 读多写少的约 6%。**

### 11.4 根因：`WRITER_QUEUED` 在 N=8 下大部分时间都是置位的

方案 B 只在"队列里没有写者/升级"时放行读者。探针显示 N=8 读多写少时读快路径
只有 0.328——也就是约 2/3 的读请求到达时，已经有一个写者/升级在排队。
读多写少负载里写 + 升级占 20%，但这些排他操作每次都要等既有读者排空，
等待期很长；于是"写者排队"几乎覆盖了大部分墙钟时间，读者照样入队。

§1 的对照实验（删掉整个检查）测的是**条件 C**，而 §2 表格把它的收益外推成
"方案 B 预期 350~500"，这一步外推是错的：条件 C 的收益里有很大一部分来自
"读者可以越过已排队的写者"，那正是方案 B 为了写者公平而主动放弃的部分。

### 11.5 待决策

1. **保留方案 B**：公平性/活性不变（写者、升级不饥饿），N=2 约 −6%，
   N=8 无可判定收益。改动本身自洽、测试完备，代价是多了两次状态字 RMW 与
   一个计数器。
2. **改用条件 C**（读快路径只看 `!WRITER_ACTIVE`）：可复现 ~355 ns/op
   （约 1.7x），但**写者可能被持续到达的读者饿死**，§5.1/§7 的"不饥饿"硬性
   要求与 `writer_should_not_starve_while_readers_keep_arriving` 等测试都要改，
   属于主动放弃写者公平性。
3. **混合（有界插队）**：在条件 C 上引入"每个排队写者只放行有限次读者插队"的
   预算或老化机制。需要额外状态位/计数器，且新状态位会再次压缩读者计数位宽；
   收益与复杂度需要先小规模验证。

**本轮决策：保留方案 B。** 理由：机制正确、公平性与活性承诺不变、测试完备，
在可靠测量范围内（N≤2、compio 单线程）没有退化；当前主机负载太高，
不足以支撑"方案 B 收益小"或"方案 B 有害"的结论，因此不据此改动策略。
条件 C 与混合方案留作备选，等安静机器上的 N=8 数据出来再评估。

### 11.6 N=8 复测口径（留给下一轮）

前提：机器空闲（`uptime` 的 load average 应显著小于核数），否则 N≥4 的跨线程
数据不可用。基线用 `git stash` 切换，A/B 必须紧邻、同配置、同轮数。

```bash
cd atomic_sync
CFG="RWLOCK_BENCH_OPS=20000 RWLOCK_BENCH_ROUNDS=7 RWLOCK_BENCH_COMPETITORS=4,8"

# 方案 B
env $CFG RWLOCK_BENCH_WEIGHTS=readheavy  cargo bench --bench rwlock_cooperative
env $CFG RWLOCK_BENCH_WEIGHTS=writeheavy cargo bench --bench rwlock_cooperative

# 严格 FIFO 基线
git stash push -m baseline
env $CFG RWLOCK_BENCH_WEIGHTS=readheavy  cargo bench --bench rwlock_cooperative
env $CFG RWLOCK_BENCH_WEIGHTS=writeheavy cargo bench --bench rwlock_cooperative
git stash pop
```

判读要点：

- 只信**同一次运行内**的方案 B 与基线对比；跨次运行的绝对值在本机曾相差 2 倍；
- 同时看 `整轮总耗时` 列（§6.4 的口径问题）；
- 若要再次确认"机制生效"，临时加回 §11.2 的探针（读快路径命中率、入队/op、
  抢锁/op、wake/op），测完删除——探针的原子操作会污染 ns/op；
- 验收线仍是 §7 的 N=8 读多写少 ≤450 ns/op。若安静机器上仍 ≈600，
  说明方案 B 的收益上限确实被"写者排队期读者必须等唤醒"锁死，届时再评估
  条件 C（~355，牺牲写者不饥饿）或 §11.5 的混合方案。

