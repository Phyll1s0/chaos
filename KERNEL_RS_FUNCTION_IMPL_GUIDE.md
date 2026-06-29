# kernel.rs 函数实现细读文档

这份文档专门补上一份“函数实现级”的说明，配合 `KERNEL_RS_CR_GUIDE.md` 使用。

读法建议：

- 先看行号和函数名，知道它在源码哪里；
- 再看“实现怎么做”，也就是它实际读写了哪些字段、用了什么锁、什么条件返回；
- 最后看“系统意义”，也就是它模拟真实 OS 的哪个动作。

注意：`kernel/src/kernel.rs` 是教学和测试用 kernel 模型，里面有不少 `_xxx` 临时变量用于模拟成本、统计、占位或防止逻辑过空。CR 时可以坦白说：这些不是完整真实 OS 实现，而是为了把系统概念放进可测试代码里。

## 0. 快速定位

```text
209-331     KernLock / GKL
443-762     SyncQueue / Sema / Futex
763-1431    地址转换、VmRegion、VmMap、FramePool、SharedPage、KStk、用户访问检查
1432-1806   CircBuf、Slab、ELF、辅助算法
1807-2466   FHandle、Pipe、FLike、epoll、Channel
2467-3245   PageCache、对象注册表、BlockCache、MountTable、IoQueue、Disk
3246-4029   IPC、共享内存、能力、信号、timer、Context、TrapCtl、时钟
4032-4704   调度器、Task、TaskTable
4704-5894   Kernel 和 dispatch_syscall
5895-6529   AddrSpace、ProcessGroup、WaitQueue、ResourceLimits、BuddyAllocator
```

## 1. KernLock / GKL

### 字段

```rust
flag: AtomicBool
holder: AtomicUsize
depth: AtomicUsize
```

`flag` 是真正的全局占用位；`holder` 记录持锁者传入的 id；`depth` 是全局可见的重入层数。当前线程自己的重入深度不在结构体里，而是放在 `GKL_LOCAL_DEPTH` 这个 thread-local 变量里。

### `KernLock::new`，215 行

实现：

- 把 `flag` 初始化成 `false`，表示锁空闲；
- 把 `holder` 初始化成 `0`；
- 把 `depth` 初始化成 `0`。

系统意义：创建全局大内核锁 `GKL` 时用。它是 `const fn`，所以可以用于 `pub static GKL: KernLock = KernLock::new()`。

### `local_depth`，223 行

实现：

- 通过 `GKL_LOCAL_DEPTH.with(...)` 进入当前线程的 thread-local storage；
- 用 `Cell::get()` 读出当前线程自己的持锁层数。

系统意义：区分“全局有人持锁”和“当前线程持锁”。这对防止 foreign leave 很关键。

### `set_local_depth`，227 行

实现：

- 同样进入 thread-local；
- 用 `Cell::set(depth)` 写入当前线程的重入层数。

系统意义：`enter`、`try_enter`、`leave` 都靠它维护本线程视角的锁深度。

### `enter_reentrant`，231 行

实现：

- 传入旧的 `local_depth`；
- 本线程 depth 设为 `local_depth + 1`；
- 全局 `depth.fetch_add(1, Relaxed)`。

系统意义：当前线程已经持有 GKL 时，不再抢 `flag`，只增加重入层数。这样同一线程内核路径可以重复进入。

### `spin_until_acquired`，236 行

实现：

- 循环执行 `flag.compare_exchange(false, true, Acquire, Relaxed)`；
- 如果 CAS 成功，说明从未加锁变成已加锁；
- 如果失败，就增加 `spins`；
- 每自旋到 `spins & 0x3ff == 0` 时 `thread::yield_now()`，其他时候 `core::hint::spin_loop()`。

系统意义：这是阻塞式抢 GKL 的核心。`Acquire` 用在成功加锁，保证进入临界区后能看到释放锁前的写入。

### `record_owner`，252 行

实现：

- `holder.store(id, Relaxed)` 记录持有者；
- `depth.store(1, Relaxed)` 设置全局锁深度为 1；
- `set_local_depth(1)` 设置当前线程自己的锁深度为 1。

系统意义：抢到锁之后，把全局状态和本线程状态同步起来。

### `release_global`，258 行

实现：

- `holder` 清 0；
- `depth` 清 0；
- `flag.store(false, Release)` 释放锁。

系统意义：真正打开大内核锁。`Release` 保证临界区内写入在解锁前完成。

### `enter`，264 行

实现：

- 先读 `local_depth`；
- 如果大于 0，说明当前线程已持锁，调用 `enter_reentrant` 后返回；
- 否则调用 `spin_until_acquired` 抢全局 `flag`；
- 抢到后调用 `record_owner(id)`。

系统意义：阻塞式进入内核临界区。它允许同一线程重入，但不同线程会自旋等待。

### `leave`，275 行

实现：

- 先读当前线程 `local_depth`；
- 如果是 0，直接返回，说明当前线程没有持锁，不能释放别人锁；
- 如果大于 1，只把本线程 depth 和全局 depth 各减一，然后返回；
- 如果等于 1，先把本线程 depth 清 0；
- 再看全局 `depth`：为 0 就直接返回，大于 1 就只减全局 depth；
- 全局 depth 也是最后一层时调用 `release_global`。

系统意义：这是防止死锁调试里 foreign leave 的关键。没有拿锁的线程调用 `leave()` 不应该释放别人的 GKL。

### `held`，300 行

实现：读 `flag`。

系统意义：只表示“全局锁现在是否被某个线程持有”，不代表当前线程持有。

### `held_by_current`，302 行

实现：检查 `local_depth() > 0`。

系统意义：判断当前线程是否已经持有 GKL。`FramePool::get` 这类函数靠它避免自锁。

### `owner` / `level`，306-308 行

实现：

- `owner` 读 `holder`；
- `level` 读全局 `depth`。

系统意义：主要用于测试和调试，帮助解释锁当前归属和重入层数。

### `try_enter`，310 行

实现：

- 如果当前线程已经持锁，走 `enter_reentrant` 并返回 `true`；
- 否则尝试 `compare_exchange(false, true, Acquire, Relaxed)`；
- 成功就 `record_owner(id)` 并返回 `true`；
- 失败返回 `false`，不阻塞。

系统意义：给 tick、cache sync 这种“不能卡死”的路径用。拿不到锁就跳过工作。

## 2. Spin、EvBus、SyncQueue

### `Spin::new`，353 行

实现：把内部 `AtomicBool v` 初始化成 `false`。

系统意义：最小自旋锁，不记录 owner，也不支持重入。

### `Spin::acquire`，354 行

实现：

- 循环 CAS `false -> true`；
- 成功用 `Acquire`；
- 失败时 `spin_loop()`。

系统意义：保护短临界区，比如 cache chain。它不能跨睡眠持有。

### `Spin::try_acquire`，359 行

实现：只尝试一次 CAS，成功返回 true，失败 false。

系统意义：用于避免死等，比如 `BlockCache::sync_all` 遇到忙 chain 就跳过。

### `Spin::release`，362 行

实现：`v.store(false, Release)`。

系统意义：释放自旋锁。

### `Spin::is_held`，363 行

实现：读 `v`。

系统意义：测试用，查看锁是否被占用。

### `EvFlag` 常量，373 行附近

实现：只是事件 bit 常量集合，比如 `READABLE`、`WRITABLE`、`PROC_QUIT`、`RECV_SIG`。

系统意义：用 bit mask 表示事件集合。

### `EvBus::make`，393 行

实现：返回 `Arc<Mutex<EvBus::default()>>`。

系统意义：事件总线通常被多个对象共享，所以直接包装成 `Arc<Mutex<_>>`。

### `EvBus::set` / `clear` / `change`，394-396 行

实现：

- `set(s)` 调 `change(0, s)`；
- `clear(s)` 调 `change(s, 0)`；
- `change(rst, s)` 先保存旧事件 `o`，再执行 `(ev & !rst) | s`；
- 如果事件变化，就用当前事件值调用回调，并用 `retain(|f| !f(ev))` 删除返回 true 的回调。

系统意义：这是简化版事件通知。事件状态变化后，订阅者回调有机会被触发并移除。

### `EvBus::sub` / `cb_len`，401-402 行

实现：

- `sub` 把回调压入 `cbs`；
- `cb_len` 返回回调数量。

系统意义：给 semaphore、pipe、process event 等等待逻辑使用。

### 全局 `wait_ev`，405 行

实现：

- 循环加锁检查 `ev & mask`；
- 如果命中就返回对应事件；
- 否则注册一个回调，回调判断 `e & mask != 0`；
- 注册后 `yield_now()`，继续轮询。

系统意义：简化版事件等待。它不是严格阻塞队列，而是 callback 加 yield 的模型。

### `SyncQueue::new`，449 行

实现：创建三个字段：

- `waiters`：等待线程队列；
- `epoll_waiters`：epoll 订阅记录；
- `pending_signals`：没有等待者时积攒的 signal 数。

系统意义：条件等待队列。

### `enqueue_current_thread`，457 行

实现：

- 锁住 `waiters`；
- `push_back(thread::current())`；
- 返回队列长度。

系统意义：把当前宿主线程登记为等待者。

### `pop_waiter`，463 行

实现：锁住 `waiters`，从队头 `pop_front`。

系统意义：FIFO 唤醒一个等待线程。

### `wake_one_waiter`，467 行

实现：取出一个等待者，有就调用 `unpark()`。

系统意义：真正执行唤醒。

### `wake_all_waiters`，473 行

实现：

- 把 `waiters` 整体 `drain(..).collect()` 到临时 Vec；
- 遍历临时 Vec 调 `unpark()`。

系统意义：先清队列再唤醒，避免唤醒过程中长期持锁。

### `condition_is_ready`，478 行

实现：

- 锁住传入的 `Mutex<T>`；
- 对数据执行谓词 `pred(&data)`。

系统意义：统一“在数据锁保护下检查条件”。

### `park_on`，483 行

实现：

- 先检查条件，成立立刻返回 true；
- 不成立就锁 `waiters`；
- 如果 `pending_signals > 0`，消费一个 signal，释放 waiters 锁，再重新检查条件；
- 如果没有 pending signal，就把当前线程 push 进队列；
- drop waiters 锁后 `thread::park()`；
- 醒来后重新检查条件并返回结果。

系统意义：这是条件变量风格：睡前检查，入队睡眠，醒后重查。它能处理虚假唤醒和 signal 早到。

### `signal`，500 行

实现：

- 如果队列里有 waiter，取一个并 `unpark()`；
- 如果没有 waiter，就 `pending_signals += 1`。

系统意义：避免“signal 先到，waiter 后睡”的丢信号问题。

### `broadcast`，509 行

实现：调用 `wake_all_waiters()`。

系统意义：唤醒所有等待者，但每个等待者醒来仍应重新检查条件。

### `signal_n`，513 行

实现：

- 锁 waiters；
- `to_wake = min(n, waiters.len())`；
- 循环 pop 并 unpark；
- 返回实际唤醒数。

系统意义：批量唤醒固定数量线程。

### `pending`，529 行

实现：返回 `waiters.len()`。

系统意义：名字像 pending signal，但当前实现返回等待者数量。CR 可说这里命名不够准确。

### `wait_ev`，533 行

实现：

- 循环锁外部 guard；
- `cond(&data)` 如果返回 `Some(result)`，立刻返回；
- 否则把当前线程加入队列并 `park()`。

系统意义：用 `Option<bool>` 表示“条件是否已经决定返回”。

### `wait_events`，544 行

实现：

- 和 `wait_ev` 类似；
- 条件没满足时，把当前线程分别加入多个 `SyncQueue`；
- 然后 park。

系统意义：等待多个队列中的任意事件。

### `wait_guard` / `wait_timeout`，557-563 行

实现：

- 先入队；
- 锁一下外部 guard 后立刻 drop；
- 分别调用 `park()` 或 `park_timeout(timeout)`。

系统意义：简化等待接口。它们没有像 `park_on` 那样传 predicate，所以语义较弱。

### `reg_epoll` / `unreg_epoll`，570-574 行

实现：

- `reg_epoll` 把 `(task_id, epfd, fd)` 放进 `epoll_waiters`；
- `unreg_epoll` 线性扫描，找到完全匹配项就 remove。

系统意义：把普通等待队列和 epoll 监听关系联系起来。

## 3. Sema / Futex

### `Sema::new`，596 行

实现：创建 `SemaInner`，`cnt = c`、`rm = false`、`pid = 0`、`bus = EvBus::default()`，再包进 `Arc<Mutex<_>>`。

系统意义：计数信号量，`cnt` 是资源数量。

### `Sema::remove`，599 行

实现：锁住内部状态，把 `rm` 设为 true，然后在事件总线上 set `SEM_RM`。

系统意义：标记 semaphore 被删除，等待者之后应该感知 removed。

### `Sema::release`，604 行

实现：`cnt += 1`，如果 `cnt >= 1` 就 set `SEM_ACQ`。

系统意义：释放一个资源，并通知可能等待获取的人。

### `Sema::try_acquire`，609 行

实现：

- 如果 `rm` 为 true，返回 `Err("removed")`；
- 如果 `cnt >= 1`，减一；
- 减完如果 `cnt < 1`，清掉 `SEM_ACQ`；
- 成功返回 `Ok(true)`；
- 没资源返回 `Ok(false)`。

系统意义：非阻塞获取资源。

### `Sema::acquire_spin`，620 行

实现：循环调用 `try_acquire()`，失败时 `yield_now()`。

系统意义：忙等式获取 semaphore。

### `Sema::access`，628 行

实现：先 `acquire_spin()`，成功后返回 `SemaGuard { s: self }`。

系统意义：RAII 风格持有 semaphore。guard drop 时会自动 release。

### `get_val` / `get_ncnt` / `get_pid` / `set_pid` / `set_val`，632-636 行

实现：

- `get_val` 读 `cnt`；
- `get_ncnt` 返回事件总线回调数量；
- `get_pid` / `set_pid` 读写 pid；
- `set_val` 改 `cnt`，如果新值可获取就 set `SEM_ACQ`。

系统意义：模拟 SysV semaphore 的状态查询和设置。

### `SemaGuard::drop` / `Deref`，643-646 行

实现：

- `drop` 调 `self.s.release()`；
- `Deref` 返回内部 `Sema` 引用。

系统意义：进入作用域获取，离开作用域自动释放。

### `FutexBucket::new`，653 行

实现：创建 `VecDeque<(addr, Thread, flag)>`。

系统意义：一个 bucket 管一组 futex 等待者。

### `FutexBucket::wait`，654 行

实现：

- 创建 `Arc<AtomicBool>` 作为是否被正常唤醒的标记；
- 先检查用户原子值 `val` 是否等于 expected，不等就 `Err("changed")`；
- 把 `(addr, current thread, flag)` 放进队列；
- 根据 timeout 调 `park_timeout` 或 `park`；
- 醒来后，如果 flag 被 wake 设置成 true，就 Ok，否则 Err("timeout")。

系统意义：futex wait 的核心是“检查值仍等于 expected 后才睡”。

### `FutexBucket::wake`，662 行

实现：

- 锁等待队列；
- `retain` 遍历；
- 对地址匹配且未超过 count 的项设置 flag=true、unpark、并从队列删除；
- 返回唤醒数量。

系统意义：按地址唤醒一定数量等待者。

### `FutexBucket::requeue`，675 行

实现：

- 扫描等待队列；
- src 地址先唤醒 `wake_n` 个；
- 后续最多 `move_n` 个改地址到 dst；
- 最后删除已经被唤醒的项；
- 返回唤醒数。

系统意义：模拟 futex requeue，把等待者从一个 futex 地址迁到另一个地址。

### `FutexBucket::pending_at`，693 行

实现：统计队列里地址等于 `addr` 的等待者数量。

系统意义：调试/测试某 futex 地址是否有人睡眠。

### `FutexTable::new`，703 行

实现：创建全局简单 futex 等待队列。

系统意义：另一个更简化的 futex table。

### `FutexTable::ftx_wait`，705 行

实现：

- 检查 `val == expected`；
- 把 `(addr, current thread)` push 进 table；
- drop 锁后 `park()`；
- 醒来返回 true。

系统意义：简化 futex wait，不区分超时和正常唤醒。

### `FutexTable::ftx_wake`，714 行

实现：

- 扫描队列；
- 地址匹配时增加 `wk`；
- 在 `wk < limit` 时 remove 并 unpark；
- 返回 `wk`。

系统意义：按地址唤醒。注意当前实现的 `wk <= limit` 和 `wk < limit` 组合比较绕，CR 时可说它是教学模型。

### `FutexTable::ftx_requeue`，737 行

实现：

- 遍历 VecDeque；
- 对 src 地址先唤醒 `wake_n` 个；
- 然后把最多 `move_n` 个地址改成 dst；
- 返回唤醒数。

系统意义：简化版 requeue。

## 4. 地址转换、页引用、VmRegion、VmMap

### `p2v`，763 行

实现：`pa + PHYS_OFF`。

系统意义：模拟直接映射区中物理地址到内核虚拟地址。

### `v2p`，769 行

实现：

- 如果 `va >= PHYS_OFF`，返回 `va - PHYS_OFF`；
- 否则返回 0。

系统意义：模拟内核虚拟地址反推物理地址。

### `k_off`，774 行

实现：

- 如果 `va >= KERN_BASE`，返回 `va - KERN_BASE`；
- 否则返回 0。

系统意义：计算内核高地址映射偏移。

### `PgFrame::new` / `with_rc`，782-783 行

实现：创建 `AtomicUsize` 引用计数，分别从 0 或指定值开始。

系统意义：给共享页和 COW 记录页帧引用数。

### `PgFrame::up` / `down` / `count` / `set`，784-799 行

实现：

- `up` 用 `fetch_add(1)`，返回加之前的值；
- `down` 用 `fetch_sub(1)`，返回减之前的值；
- `count` load；
- `set` store。

系统意义：维护页帧生命周期。

### `PgFrame::cas`，802 行

实现：`compare_exchange(expected, desired, AcqRel, Relaxed).is_ok()`。

系统意义：原子条件更新引用计数。

### `PgFrame::inc_if_nonzero`，805 行

实现：

- 循环 load 当前值；
- 如果是 0 返回 false；
- 否则 CAS 成 `cur + 1`；
- CAS 失败则重试。

系统意义：只在对象仍活着时增加引用，避免 resurrection。

### `VmRegion::new` / `with_offset`，817-821 行

实现：

- 设置 `base`、`len`、`flags`；
- `new` 的 `offset = 0`；
- `with_offset` 使用传入 offset；
- `tag = 0`，`ref_count = 1`。

系统意义：创建一段连续、同属性的虚拟内存区域。

### `VmRegion::end`，825 行

实现：返回 `base + len`。

系统意义：半开区间 `[base, end)` 的右边界。

### `VmRegion::contains`，827 行

实现：`addr >= base && addr < base + len`。

系统意义：判断地址是否属于此 region。

### `VmRegion::overlaps`，831 行

实现：

- 算出两个区间末尾；
- 如果 `a_end <= other.base` 或 `b_end < self.base` 就不重叠；
- 否则重叠。

系统意义：插入映射前检查地址范围冲突。注意第二个条件用 `<`，边界表达略不完全对称，CR 可指出要谨慎。

### `VmRegion::split_at`，838 行

实现：

- 如果切分点不在 region 内部，返回 None；
- 左长度 `ll = addr - base`，右长度 `rl = len - ll`；
- 右侧 offset = 原 offset + 左长度；
- 如果原 flags 有 `VM_GROWSDOWN`，左半清掉该 flag；
- 创建左右两个新 `VmRegion`，复制 tag 和 ref_count 当前值。

系统意义：`munmap` / `mprotect` 处理中间区域时需要把一个 region 切成两段。

### `VmRegion::merge_with`，853 行

实现：

- 只有 `self.end == other.base` 才能合并；
- flags 和 tag 必须一致；
- 合并后 base 用 self.base，len 相加，offset 用 self.offset；
- ref_count 取两个当前值的 max。

系统意义：减少 region 碎片。连续且属性相同的映射可以合并。

### `VmRegion::ref_up` / `ref_down` / `ref_get`，869-871 行

实现：对 `ref_count` 做 `fetch_add`、`fetch_sub`、`load`。

系统意义：记录 region 层面的共享引用。

### `VmMap::new`，881 行

实现：

- `regions = Vec::new()`；
- `brk = 0x0040_0000`；
- `mmap_base = 0x7000_0000`。

系统意义：初始化进程虚拟地址空间的映射表。

### `VmMap::insert`，885 行

实现：

- 计算新区间 `[rb, re)`；
- 线性扫描已有 region；
- 如果发现 `rb < ee && eb < re`，返回 `Err("overlap")`；
- 找到第一个 base 大于新区 base 的位置；
- 目前只计算 `_coalesce_prev`，没有真正合并；
- 在排序位置插入 region。

系统意义：插入新的 mmap/brk/stack 区域，同时保持按地址排序。

### `VmMap::find`，905 行

实现：

- 如果没有 region 返回 None；
- 对排序 regions 做二分；
- addr 小于 mid.base 向左找；
- addr 大于等于 mid.end 向右找；
- 否则返回该 region。

系统意义：缺页异常或权限检查时按地址找 region。

### `VmMap::remove_range`，920 行

实现：

- 算删除区间 `[base, end)`；
- 扫描 regions；
- 完全包含的 region 删除；
- 只要有部分重叠的 region 也直接删除；
- 返回删除数量。

系统意义：简化版 unmap。真实 OS 会保留未重叠的左右部分，这里没有精细 split。

### `VmMap::find_free`，938 行

实现：

- len 为 0 返回 mmap_base；
- align 小于等于 1 时按 PAGE_SZ 对齐；
- 从 mmap_base 向上找候选地址；
- 对每个候选，遍历 regions 看是否冲突；
- 冲突则跳到冲突 region 的 end 后重新对齐；
- 如果越过 KERN_BASE 或溢出返回 None。

系统意义：实现 mmap 不指定地址时找空洞。

### `VmMap::total_mapped`，966 行

实现：遍历 regions，把 len 累加。

系统意义：统计映射总字节数。

### `VmMap::clone_regions`，974 行

实现：逐个复制 region 的 base/len/flags/offset/tag/ref_count 当前值到新 Vec。

系统意义：fork 时复制虚拟地址布局描述。

### `VmMap::gap_after`，990 行

实现：

- idx 越界返回 0；
- 计算当前 region end；
- 如果有下一个 region，返回下一个 base 与当前 end 的差；
- 否则返回到 KERN_BASE 的差。

系统意义：分析虚拟地址空间空洞。

## 5. 网络和 FramePool

### `tcp_checksum`，1001 行

实现：

- 把 src/dst IP、协议号 6、payload 长度加入 sum；
- 每两个 payload 字节合成一个 16 位字加入；
- 奇数字节补到高位；
- 把进位折叠到 16 位；
- 返回反码。

系统意义：TCP 伪首部校验和。

### `parse_ipv4_header`，1023 行

实现：

- 检查长度至少 20；
- 检查 version 是 4；
- 读取 IHL 并检查 header 长度；
- 读 total_len、protocol、src_ip、dst_ip；
- 计算 header checksum 但没有验证它；
- 返回四元组。

系统意义：解析 IPv4 header 的基本字段。

### `build_pseudo_header`，1048 行

实现：按网络序把 src/dst/proto/length 推入 12 字节 Vec。

系统意义：给 TCP/UDP checksum 生成伪首部。

### `compute_inet_checksum`，1065 行

实现：标准 Internet checksum：16 位求和、奇数字节补高位、折叠进位、取反。

系统意义：网络协议校验。

### `FramePool::new`，1086 行

实现：

- `slots = vec![true; frame_count]`；
- `cap = frame_count`。

系统意义：用 bool 表示每个物理页帧是否空闲，true 表示空闲。

### `take_first_free`，1093 行

实现：遍历 slots，找到第一个 true，设成 false 并返回下标。

系统意义：最简单 first-fit 页分配。

### `FramePool::get`，1103 行

实现：

- 如果当前线程已经持有 GKL，直接调用 `get_inner()`；
- 否则 `GKL.enter(id)`；
- 调 `get_inner()`；
- `GKL.leave()`；
- 返回 frame index。

系统意义：带大内核锁语义的页分配入口，避免已持锁路径重复阻塞拿锁。

### `FramePool::get_inner`，1116 行

实现：锁 `slots`，调用 `take_first_free`。

系统意义：纯分配逻辑，不处理 GKL。

### `FramePool::get_contig`，1121 行

实现：

- 根据 `align_log2` 得到 alignment；
- 用 `step_by(alignment)` 扫描起点；
- 检查 `[start, start + frame_count)` 全部空闲；
- 全部设 false 后返回 start。

系统意义：分配连续物理页。

### `FramePool::put`，1134 行

实现：如果 frame_index 在范围内，把 slots[index] 设 true。

系统意义：释放页帧。

### `FramePool::avail` / `free_count`，1139-1143 行

实现：

- `avail` 检查 index 范围和 slots[index]；
- `free_count` 统计 true 的数量。

系统意义：查询页池状态。

### `get_zone_aware` / `put_zone_aware`，1147-1162 行

实现：

- 分配前先调用 `zone.zone_can_alloc()`；
- 只扫描 zone 的 `[base_pfn, base_pfn + page_count)`；
- 分配成功同时 `zone.free_count -= 1`；
- 释放时设置 slots true 并 `zone.free_count += 1`。

系统意义：模拟 DMA/NORMAL/HIGHMEM 这样的内存 zone。

### `FramePool::batch_alloc`，1170 行

实现：一次锁住 slots，顺序找空闲页，直到达到 count 或没有空闲。

系统意义：批量页分配，减少多次锁开销。

### `ZoneInfo::new`，1185 行

实现：设置 zone id、起始 PFN、页数、水位线、free_count 和 managed。

系统意义：描述一段物理内存区域。

### `zone_can_alloc` / `zone_pressure` / `reclaim_target` / `contains_pfn`，1197-1216 行

实现：

- `zone_can_alloc` 判断 free_count 是否高于 low watermark；
- `zone_pressure` 根据 free 在 high/low 之间的位置算百分比压力；
- `reclaim_target` 返回离 high watermark 还差多少页；
- `contains_pfn` 判断 pfn 是否在 zone 范围。

系统意义：模拟内存压力和回收目标。

### `frame_alloc`，1221 行

实现：

- 从 `CLK % len` 作为扫描起点；
- 找到空闲 frame 后设 false；
- 返回物理地址 `id * PAGE_SZ + MEM_OFF`。

系统意义：全局辅助分配函数，返回的是物理地址，不是 frame index。

### `frame_dealloc`，1245 行

实现：

- 小于 MEM_OFF 直接返回；
- 计算 frame index 和页内 remainder；
- remainder 不为 0 说明地址不是页对齐，返回；
- index 合法则 slots[index] = true。

系统意义：按物理地址释放页。

### `frame_alloc_contig`，1257 行

实现：

- sz 为 0 返回 None；
- alignment = `1 << align`；
- 按对齐起点扫描；
- 遇到占用页就把 start 跳到占用页后；
- 找到连续 sz 页后设 false；
- 返回起始物理地址。

系统意义：全局连续页分配。

## 6. SharedPage、KStk、用户访问、堆

### `SharedPage::new`，1286 行

实现：设置当前 frame，`w=false`，`pending=true`。

系统意义：表示一个还没解析完 COW 的共享页。

### `SharedPage::fault`，1289 行

实现：

- 如果 `pending` 已经 false，直接返回当前 frame；
- 否则从 pool 里找新 frame；
- 从 old_frame 附近开始循环扫描；
- 找到空闲页后设 false；
- `self.frame` 改成新 frame；
- 源 `PgFrame` 引用计数减一；
- `w=true`，`pending=false`；
- 返回新 frame。

系统意义：COW fault 时分配私有页。

### `is_cow_resolved` / `frame_id`，1313-1316 行

实现：

- `is_cow_resolved` 检查 pending false 且 w true；
- `frame_id` 读取当前 frame。

系统意义：查询 COW 状态。

### `KStk::new`，1323 行

实现：

- 分配 `vec![0u8; KSTK_SZ]`；
- 转成 boxed slice；
- `Box::into_raw` 拿裸指针保存。

系统意义：模拟内核栈分配。

### `KStk::top`，1328 行

实现：返回 `base + KSTK_SZ`。

系统意义：栈向下增长，所以 top 是初始栈指针。

### `Drop for KStk`，1330 行

实现：用保存的裸指针重新构造 boxed slice，让 Box drop 释放内存。

系统意义：RAII 释放内核栈。

### `check_access`，1338 行

实现：检查 `addr + len` 不越过 KERN_BASE 且没有回绕。

系统意义：用户指针基本合法性检查。

### `check_access_rw`，1342 行

实现：

- len 为 0 返回 true；
- 计算边界并检查是否越过内核空间或溢出；
- 计算覆盖页数量；
- writable 时计算对齐检查但不作为返回条件；
- 最终返回 `boundary < KERN_BASE`。

系统意义：写访问的扩展检查，但仍是模拟。

### `cfu`，1357 行

实现：

- len 为 0 时用 `size_of::<T>()`；
- 调 `check_access`；
- 成功返回 `T::default()`。

系统意义：模拟 copy-from-user，但不真实读取用户内存。

### `ctu`，1364 行

实现：计算有效长度，调用 `check_access_rw(..., true)`。

系统意义：模拟 copy-to-user，只检查地址合法性。

### `rdu_fixup`，1369 行

实现：读当前 tick 和 mask，但总是返回 1。

系统意义：占位式异常修复函数。

### `heap_init`，1375 行

实现：

- base 向上页对齐；
- size 向下页对齐；
- 返回 `aligned_base + aligned_sz`。

系统意义：初始化堆边界。

### `heap_grow`，1383 行

实现：

- 最多尝试 `n * 2` 次；
- 从 pool 里找空闲页；
- 转成内核虚拟地址 `PHYS_OFF + pg * PAGE_SZ`；
- 如果和上一个分配段相邻就合并，否则 push 新段；
- 返回 `(va, len)` 列表。

系统意义：堆增长时分配若干页，并把连续页合并成区间。

## 7. CircBuf、Slab、ELF 和辅助算法

### `CircBuf::new`，1433 行

实现：创建定长 Vec，`rd=0`、`wr=0`、`cap=capacity`、`n=0`。

系统意义：环形缓冲区初始化。

### `CircBuf::with_pos`，1443 行

实现：

- 根据 read_pos/write_pos 计算当前 len；
- 初始化 rd/wr/cap/n。

系统意义：测试用，可以直接构造某种读写指针状态。

### `next_write_index`，1458 行

实现：

- 如果 `n >= cap`，返回 None；
- `wr = wr.wrapping_add(1)`；
- index = `wr % cap`；
- 如果 index 越过 data.len，回退 wr 并返回 None；
- 否则返回 index。

系统意义：计算写入位置。这里采用“先加再取模”的指针模型。

### `next_read_index`，1472 行

实现：

- 如果 `n == 0` 返回 None；
- `rd = rd.wrapping_add(1)`；
- index = `rd % cap`；
- 越界则回退 rd；
- 否则返回 index。

系统意义：计算读取位置。

### `push` / `pop`，1486-1496 行

实现：

- `push` 调 `next_write_index`，成功后写 data[index] 并 `n += 1`；
- `pop` 调 `next_read_index`，成功后 `n -= 1` 并返回 data[index]。

系统意义：环形队列读写。`n` 用于区分空和满。

### `len` / `empty` / `full` / `peek` / `drain_to` / `fill_from` / `remaining`，1502-1530 行

实现：

- 前三个直接看 `n`；
- `peek` 看 `rd + 1` 的位置但不移动指针；
- `drain_to` 最多 pop max 个，push 到目标 Vec；
- `fill_from` 逐字节 push，满了停止；
- `remaining` 用 `cap.saturating_sub(n)`。

系统意义：给 channel、tty 等缓冲提供批量操作。

### `SlabEntry::new`，1534 行

实现：

- obj_size 按 `SLAB_ALIGN` 对齐；
- data 大小为 `aligned * capacity`；
- free_list 放入每个对象起始 offset。

系统意义：固定大小对象分配器。

### `slab_alloc`，1551 行

实现：

- 从 free_list 取一个 slot；
- 计算对象结束位置；
- `needs_init = zeroed | false`；
- 当前代码在 `!needs_init` 时把区域清 0，这和名字略反直觉；
- `allocated += 1`；
- 返回 offset。

系统意义：分配一个 slab 对象。CR 可以指出 zeroed 逻辑命名可优化。

### `slab_free`，1571 行

实现：

- 检查 offset 在 data 内且按 obj_size 对齐；
- 计算 `_dup` 但没有阻止重复 free；
- 把 offset push 回 free_list；
- allocated 大于 0 时减一。

系统意义：释放对象。重复释放检测不完善，是可重构点。

### `slab_used` / `slab_avail` / `shrink` / `obj_at` / `obj_at_mut`，1581-1601 行

实现：

- `slab_used` 返回 allocated；
- `slab_avail` 返回 free_list.len；
- `shrink` 在 allocated 为 0 时清空 data 和 free_list；
- `obj_at` / `obj_at_mut` 检查 offset + obj_size 不越界后返回切片。

系统意义：查询和访问 slab 内对象。

### `validate_elf_header`，1610 行

实现：

- 检查长度至少 64；
- 检查 ELF magic；
- 检查 64-bit、小端、version；
- 检查 e_type 是 exec 或 dyn；
- 读取 entry、program header offset、phentsize、phnum；
- 检查 program header 不越界；
- 遍历 phdr，统计 LOAD 段和 INTERP；
- 没有 LOAD 返回错误，否则返回 entry。

系统意义：exec/new_user_task 中验证 ELF 的简化模型。

### `compute_load_balance`，1662 行

实现：

- 对每个 CPU 计算 score；
- 任务越多分数越低，优先级越高分数越高，I/O blocked 扣分；
- 有任务加 cache_bonus；
- 前半 CPU 加一点 numa_factor；
- 排序后取接近最高分的 candidates[0]。

系统意义：模拟负载均衡目标选择。

### `audit_fd_table`，1691 行

实现：

- 按 fd 顺序扫描；
- 发现 fd 缺口就把 gap 加到 leaks；
- pipe 如果 poll 出 error，也加入；
- file 如果 path 为空，也加入。

系统意义：检查 fd table 异常。

### `rehash_mount_cache`，1717 行

实现：对每个 mount entry 的 prefix 做 FNV 类 hash，再混入 target 长度，把 hash 映射到 entry index。

系统意义：模拟 mount cache 重建。

### `defragment_frame_pool`，1733 行

实现：

- 统计 free_count；
- 计算空闲 run 数和最大连续空闲 run 的 order；
- 返回 free_count。

系统意义：模拟碎片分析，当前没有真实搬迁页。

### `verify_page_alignment`，1772 行

实现：

- align = `PAGE_SZ << order`；
- 检查 addr 与 mask 对齐；
- 检查地址低于 KERN_BASE；
- order < 12；
- block_end > block_start 防溢出。

系统意义：验证大页/伙伴块对齐。

### `compute_rss_watermark`，1786 行

实现：

- 对每个 region 计算页数；
- EXEC 权重 3，WRITE 权重 2，其他权重 1；
- shared factor 为 1，private 为 2；
- 总权重乘 100 除 pool_cap；
- clamp 到 pool_cap/2。

系统意义：估算 RSS 水位或内存压力。

## 8. FHandle、PipeNode、FLike、epoll、Channel

### `FdOpt::default`，1814 行

实现：默认可读、不可写、不 append、不 nonblock。

系统意义：默认打开选项。

### `FdState::create`，1819 行

实现：创建 `Arc<RwLock<FdState>>`，offset 为 0，保存打开选项，`flk=0`。

系统意义：多个 dup 出来的 fd 可以共享同一个 `FdState`，因此共享 offset。

### `FHandle::new`，1837 行

实现：

- path 转 String；
- data 初始化为空 `Arc<Mutex<Vec<u8>>>`；
- desc 用 `FdState::create(opt)`；
- 保存 pipe 和 cloexec。

系统意义：普通文件 handle。当前实现把 inode 内容和 file offset 都放在 handle 周围，结构比较混。

### `FHandle::with_data`，1846 行

实现：和 `new` 类似，但 data 用传入 Vec 初始化，pipe=false，cloexec=false。

系统意义：创建带初始内容的模拟文件。

### `FHandle::dup`，1855 行

实现：

- path clone；
- data clone，也就是共享同一个 `Arc<Mutex<Vec<u8>>>`；
- desc clone，也就是共享 offset/flags；
- pipe 原样复制；
- cloexec 使用传入值。

系统意义：模拟 dup 后共享文件对象和 offset。

### `set_opt` / `get_opt`，1864-1868 行

实现：

- `set_opt` 只根据 `O_NONBLOCK` 改 `nb`；
- `get_opt` 读 desc 里的 opt。

系统意义：fcntl 修改/读取打开选项。

### `FHandle::read`，1870 行

实现：

- 读当前 offset；
- 调 `read_at(off, buf)`；
- 成功后把 offset 加上读到的长度；
- 返回长度。

系统意义：按当前 fd offset 顺序读。

### `FHandle::read_at`，1876 行

实现：

- 如果 opt.rd 为 false，返回 `ebadf`；
- nonblock 分支和普通分支都锁 data；
- off 超过文件长度返回 0；
- n = min(buf.len, data.len - off)；
- copy 到 buf；
- 返回 n。

系统意义：从指定 offset 读，不改变 offset。

### `FHandle::write`，1891 行

实现：

- 先读 desc；
- 如果 append，则 offset 用当前 data.len；
- 否则用 desc.off；
- 调 `write_at`；
- 写完把 desc.off 加写入长度。

系统意义：按当前 offset 或 append 位置写。

### `FHandle::write_at`，1900 行

实现：

- 检查 opt.wr，否则 `ebadf`；
- 锁 data；
- 如果写入尾部超过当前长度，resize 到新长度，空洞补 0；
- copy buf 到目标范围；
- 返回 buf.len。

系统意义：指定 offset 写，不一定改变 offset。

### `FHandle::seek`，1907 行

实现：

- 锁 desc 写；
- `Start(o)` 设为 o；
- `End(o)` 设为 `data.len + o`；
- `Cur(o)` 设为当前 off + o；
- 返回新 offset。

系统意义：移动文件偏移。当前没有检查负数越界导致的转换问题。

### `FHandle::transfer`，1917 行

实现：

- 先算 `_path_hash`，但只作占位；
- 如果 `dir & 1 != 0`，按 read 路径分发；
- 有 offset 调 `read_at`，无 offset 调 `read`；
- 否则走 write 路径；
- 参数组合不合法返回 `einval`。

系统意义：统一 read/write/read_at/write_at 的包装。

### `set_len` / `sync_all` / `sync_data` / `metadata_sz`，1938-1945 行

实现：

- `set_len` 要求 writable，resize data；
- `sync_all` / `sync_data` 当前直接 Ok；
- `metadata_sz` 返回 data.len。

系统意义：模拟 truncate、fsync、metadata。

### `lookup` / `read_entry`，1946-1947 行

实现：

- `lookup` 当前总是 Ok；
- `read_entry` 检查可读，然后取当前 off，off += 1，返回 `"entry_{off}"`。

系统意义：目录查找和读取目录项的占位模型。

### `poll_status` / `io_ctl` / `mmap` / `inode_ref`，1954-1957 行

实现：

- `poll_status` 固定返回可读可写无错误；
- `io_ctl` 固定 Ok(0)；
- `mmap` 固定 Ok；
- `inode_ref` 返回 data 的 Arc clone。

系统意义：文件操作接口占位。

### `advise_readahead`，1959 行

实现：

- 锁 data；
- 计算 `actual_end = min(offset + len, d.len())`；
- 算出涉及页数 `_readahead_pages`；
- 返回 Ok。

系统意义：模拟 readahead 计算，不真正预读。

### `fallocate`，1966 行

实现：

- 检查 writable；
- 需要长度为 `offset + len`；
- 如果超过当前 data.len，就 resize 补 0。

系统意义：预分配文件空间。

### `splice_to`，1976 行

实现：

- 读源 offset；
- 锁源 data；
- 如果源 offset 已经到尾，返回 0；
- 取最多 count 字节到临时 Vec；
- drop 源 data 锁；
- 推进源 offset；
- 调目标 `dst.write(&chunk)`。

系统意义：模拟文件到文件的数据搬运，避免同时持有源/目标锁太久。

### `Debug for FHandle`，1989 行

实现：打印 `off` 和 `path`。

系统意义：调试 fd 状态。

### `Drop for PipeNode`，2011 行

实现：

- 锁共享 PipeBuf；
- `ends -= 1`；
- set `CLOSED` 事件。

系统意义：管道一端被 drop 时通知另一端。

### `PipeNode::pair`，2020 行

实现：

- 创建共享 `PipeBuf { buf, bus, ends: 2 }`；
- 返回读端 `PipeDir::Rd` 和写端 `PipeDir::Wr`，共享同一个 Arc。

系统意义：pipe 创建一对 fd。

### `can_read` / `can_write`，2028-2033 行

实现：

- 读端只有在方向是 Rd 时可读；
- buffer 有数据或 ends < 2 时可读；
- 写端只有方向是 Wr 且 ends == 2 时可写。

系统意义：poll/select 判断 pipe 状态。

### `PipeNode::read_at`，2037 行

实现：

- 空 buf 返回 0；
- 非读端返回 0；
- buffer 空且两端都开着返回 `again`；
- 否则 pop_front 到用户 buf；
- 如果读完为空，清 READABLE；
- 返回读取数量。

系统意义：pipe 读。空但写端还在时表示暂时无数据。

### `PipeNode::write_at`，2047 行

实现：

- 非写端返回 0；
- 把输入字节 push_back 到 buffer；
- set READABLE；
- 返回写入长度。

系统意义：pipe 写。

### `PipeNode::poll`，2054 行

实现：返回 `(can_read, can_write, false)`。

系统意义：pipe poll 状态。

### `FLike::dup`，2067 行

实现：

- File：clone path/data/desc，设置 cloexec；
- Pipe：clone 共享 PipeBuf 和方向；
- Ep：clone events、ready、new_ctl。

系统意义：fd table 存的是统一 enum，dup 时按具体类型复制。

### `FLike::read`，2095 行

实现：

- 空 buf 返回 0；
- File：检查可读，按 desc.off 从 data copy，推进 off；
- Pipe：检查方向，空且两端开着返回 `again`，否则 pop 数据；读空后清 READABLE 并触发回调 retain；
- Ep：返回 `enosys`。

系统意义：统一 fd read 分发。

### `FLike::write`，2135 行

实现：

- 空 buf 返回 0；
- File：检查 writable，append 时写到 data.len，否则写 desc.off；必要时 extend；写完设置新 off；
- Pipe：检查方向，把字节 push 到 pipe buffer，设置 READABLE 并触发回调；
- Ep：返回 enosys。

系统意义：统一 fd write 分发。

### `FLike::io_ctl`，2179 行

实现：

- File：小命令 0..=0xFF 直接 Ok(0)，其他转给 `FHandle::io_ctl`；
- Pipe：只接受 `0x5421`，其他 `enotty`；
- Ep：enosys。

系统意义：不同 fd 类型 ioctl 能力不同。

### `FLike::mmap_fl`，2197 行

实现：

- start >= end 返回 einval；
- File：计算文件页数后调用 `FHandle::mmap`；
- 其他 fd 返回 enosys。

系统意义：只有普通文件支持 mmap。

### `FLike::poll`，2210 行

实现：

- File：readable/writable 来自 desc.opt，error 是 path 空且 data 空；
- Pipe：根据 buffer 是否有数据、ends 是否关闭、方向判断；
- Ep：ready set 非空则可读。

系统意义：poll/epoll 的底层状态查询。

### `PseudoNode::new` / `read_at` / `write_at` / `metadata_sz`，2251-2259 行

实现：

- new 把字符串转 Vec；
- read_at 从 content copy；
- write_at 固定 nosup；
- metadata_sz 返回 content.len。

系统意义：模拟 procfs/sysfs 这种只读伪文件。

### `read_as_vec`，2262 行

实现：`data.to_vec()`。

系统意义：辅助转换。

### `EpEvent::has`，2285 行

实现：`events & ev != 0`。

系统意义：检查 epoll 事件位。

### `EpInst::new`，2302 行

实现：events 空 map，ready 和 new_ctl 用 `Arc<Mutex<BTreeSet>>`。

系统意义：epoll 实例状态。

### `EpInst::control`，2309 行

实现：

- op 1 ADD：插入 fd -> event，并记录 new_ctl；
- op 3 MOD：存在才修改，不存在 eperm；
- op 2 DEL：remove 成功 Ok，否则 eperm；
- 其他 eperm。

系统意义：epoll_ctl 维护监听集合。

### `TrmIO::default`，2346 行

实现：填入一组类 Linux 终端默认 flag 和控制字符。

系统意义：ioctl TCGETS/TCSETS 的 termios 数据模型。

### `Channel::new`，2371 行

实现：

- cap 为 0 时改成 1；
- cap 超过 `1 << 20` 时截断；
- 创建 `CircBuf`、`Spin`、`SyncQueue`、`shut=false`。

系统意义：有界字节 channel。

### `Channel::recv`，2381 行

实现：

- 循环；
- 先 acquire guard；
- 锁 buf 并 pop；
- 立刻 release guard；
- 如果读到字节，返回；
- 如果 shut 为 true，返回 None；
- 否则调用 `wq.park_on(&buf, |b| !b.empty() || shut)`；
- 醒来继续循环。

系统意义：正确点是睡眠前已经释放 guard，不会持自旋锁睡觉。

### `Channel::send`，2406 行

实现：

- 锁 buf，push byte；
- 成功则 `wake_one_waiter()`；
- 返回 push 是否成功。

系统意义：生产者写入并唤醒一个消费者。

### `Channel::close`，2417 行

实现：`shut.store(true, Release)`，然后唤醒所有等待者。

系统意义：关闭 channel，recv 之后会返回 None。

### `Channel::try_recv`，2422 行

实现：

- 直接 CAS guard，失败返回 None；
- 成功后锁 buf pop；
- 手动 release guard；
- 返回结果。

系统意义：非阻塞接收。

### `send_batch` / `depth` / `drain_all` / `is_closed` / `remaining_capacity`，2434-2461 行

实现：

- `send_batch` 锁 buf 批量 fill，写入大于 0 就唤醒一个等待者；
- `depth` 返回 ring.len；
- `drain_all` 按当前 len 全部 drain 到 Vec；
- `is_closed` 读 shut；
- `remaining_capacity` 返回 ring.remaining。

系统意义：批量操作和状态查询。

## 9. PageCache、对象注册表、BlockCache、MountTable、IoQueue、Disk

### `PageCache::new`，2485 行

实现：初始化 entries、capacity、hits/misses/evictions、lru_order。

系统意义：页缓存容器。

### `PageCache::lookup`，2496 行

实现：

- 命中则 hits++，把 page_id 从 lru_order 删除后 push_back；
- 更新 access_tick；
- 返回 data slice；
- 未命中 misses++ 返回 None。

系统意义：缓存查找并维护 LRU 顺序。

### `PageCache::insert`，2511 行

实现：

- 如果 entries.len >= capacity，先 evict_lru；
- 创建 entry，dirty=false，access_tick=CLK，pin_count=0；
- 插入 map，并把 page_id push_back 到 LRU。

系统意义：插入缓存页。

### `evict_lru`，2526 行

实现：

- 按 lru_order 找第一个 pin_count == 0 的 page；
- 从 entries 和 lru_order 删除；
- evictions++；
- 没有可淘汰页返回 false。

系统意义：不能淘汰被 pin 的页。

### `mark_dirty` / `writeback_all` / `stats`，2546-2563 行

实现：

- `mark_dirty` 设置 entry.dirty；
- `writeback_all` 遍历 dirty 页，清 dirty 并计数；
- `stats` 返回 hits/misses/evictions。

系统意义：模拟脏页写回。

### `pin` / `unpin` / `invalidate` / `flush_range`，2571-2598 行

实现：

- pin/unpin 修改 pin_count；
- invalidate 删除 entry 并从 lru_order 删除；
- flush_range 收集范围内 page id，清 dirty 并计数。

系统意义：页缓存生命周期管理。

### `KObjRegistry::new`，2632 行

实现：objects 空 map，seq 从 1 开始，type_index 空 map。

系统意义：内核对象注册表。

### `register` / `register_child`，2640-2656 行

实现：

- seq.fetch_add 生成 id；
- 创建 KObjEntry；
- 插入 objects；
- 在 type_index[type_tag] 里 push id；
- child 版本多记录 parent_id。

系统意义：记录对象和父子关系。

### `unregister`，2672 行

实现：

- 从 objects remove；
- 如果存在，根据 entry.type_tag 到 type_index 里删除 id；
- 返回是否成功。

系统意义：删除对象并维护索引一致性。

### `find_by_type` / `dump_graph` / `gc_sweep`，2685-2700 行

实现：

- find_by_type 从 type_index 克隆 id 列表；
- dump_graph 遍历 objects，把 parent -> child 边收集出来；
- gc_sweep 找 ref_count 为 0 的对象，删除并更新 type_index。

系统意义：按类型查询、导出依赖图、清理无引用对象。

### `ref_up` / `ref_down` / `count` / `owner_objects`，2718-2742 行

实现：

- ref_up/ref_down 修改对象 ref_count，down 用 saturating_sub；
- count 返回 objects.len；
- owner_objects 过滤 owner_pid。

系统意义：对象引用计数和 owner 查询。

### `CacheChain::new` / `acquire` / `try_acquire` / `release`，2753-2763 行

实现：每条 chain 有一个 `Spin` 和一个 `Mutex<Vec<CacheSlot>>`；函数只是包装锁操作。

系统意义：BlockCache 分链锁，降低全局锁竞争。

### `BlockCache::new`，2770 行

实现：创建 width 条 CacheChain。

系统意义：多链块缓存。

### `idx` / `fetch_chain_index`，2776-2778 行

实现：

- `idx` 用 `block_id % width`；
- `fetch_chain_index` 用 `block_id ^ (block_id >> 7)` 混合后取模。

系统意义：把 block 分散到不同 chain。

### `cached_payload`，2783 行

实现：锁 chain.items，find id 等于 block_id 的 slot，clone payload 返回。

系统意义：读缓存命中内容。

### `synthetic_block`，2791 行

实现：

- seed = `block_id * 常数 ^ tick`；
- 生成 512 字节，每字节是 seed + offset 的低 8 位。

系统意义：没有真实磁盘时生成确定性数据。

### `BlockCache::fetch`，2800 行

实现：

- 计算 chain；
- acquire chain lock；
- 先查 cached_payload，命中则 release 后返回；
- miss 时记录 tick，按 latency sleep；
- 生成 synthetic block；
- push 一个 CacheSlot；
- release；
- 返回数据 clone。

系统意义：缓存读路径：查缓存、模拟 I/O、填缓存。

### `BlockCache::sync_all`，2829 行

实现：

- 先 `GKL.try_enter(id)`，失败直接返回；
- 遍历每条 chain；
- `try_acquire` 失败就跳过；
- `items.try_lock()` 成功才遍历 dirty slot，把 modified=false；
- release chain；
- 最后 GKL.leave。

系统意义：全局同步路径不能阻塞等待 GKL 或 chain，否则容易和 scheduler/FramePool 形成死锁链。

### `invalidate` / `total_entries` / `dirty_count` / `evict_cold`，2850-2885 行

实现：

- invalidate 锁对应 chain，retain 删除 block；
- total_entries 遍历 chain 累加 items.len；
- dirty_count 遍历 modified；
- evict_cold 按伪 age 删除冷且非 modified 的 slot。

系统意义：块缓存管理。

### `MountTable::new` / `bind`，2910-2911 行

实现：

- new 创建空 Vec；
- bind 加写锁，避免重复 prefix/target；
- 插入后按 prefix 长度降序排序。

系统意义：最长前缀匹配路径挂载。

### `MountTable::resolve`，2924 行

实现：

- 读挂载表；
- 找最长 prefix 匹配；
- 命中后取 rest，drop 表锁，递归 resolve(rest)，再拼成 `target:sub`；
- 没命中则折叠重复 `/`，返回 canonical。

系统意义：路径解析。注意递归没有显式深度限制，CR 可说 `MNT_DEPTH` 没被充分利用。

### `unmount` / `list_mounts` / `find_mount` / `mount_count` / `has_prefix`，2975-3024 行

实现：

- unmount 删除 prefix 匹配项；
- list_mounts 克隆所有 pair；
- find_mount 找最长前缀并 clone；
- mount_count 返回 entries.len；
- has_prefix 精确比较 bytes。

系统意义：挂载表查询和维护。

### `IoQueue::new`，3047 行

实现：pending 空队列，head_pos=0，direction_up=true，统计项为 0。

系统意义：模拟磁盘 I/O 调度队列。

### `submit` / `submit_batch`，3057-3068 行

实现：

- submit 创建 IoRequest 并 push_back；
- batch 循环 push，多于 IOQUEUE_DEPTH 时调用 merge_adjacent。

系统意义：提交 I/O 请求。

### `dispatch`，3088 行

实现：

- 如果队列空返回 None；
- 根据 head_pos 和 direction_up 计算每个请求距离；
- 选择距离最小的请求 remove；
- 更新 head_pos；
- 如果该方向上没有更远请求，翻转方向；
- dispatched++；
- 返回 `(block, write)`。

系统意义：电梯算法风格的磁盘调度。

### `merge_adjacent` / `depth`，3121-3137 行

实现：

- 扫描相邻请求；
- 如果 block 连续且读写方向相同，删除后一个并计数；
- depth 返回 pending.len。

系统意义：合并相邻 I/O。

### `Disk::new` / `failing`，3149-3152 行

实现：创建 label、ops=0、journal=None，`failing` 额外把 errs 初始化为 n。

系统意义：正常磁盘和会失败 n 次的磁盘。

### `attach_journal` / `set_errs`，3155-3156 行

实现：设置 journal Arc 或错误次数。

系统意义：模拟日志盘和错误注入。

### `begin_op` / `remaining_errors` / `consume_transient_error`，3158-3166 行

实现：

- begin_op 让 ops++；
- remaining_errors 读 errs；
- consume_transient_error 在 errs 不是 usize::MAX 时递减。

系统意义：统计操作和消耗瞬时错误。`usize::MAX` 表示持久错误。

### `fill_block` / `fill_limited_read`，3172-3179 行

实现：

- `fill_block` 用 `((sector as u8) * 0x9D) | 0xAA` 填满 out；
- `fill_limited_read` 用 `0xAA ^ offset` 填 out。

系统意义：测试用可预测磁盘内容。

### `retry_journal`，3185 行

实现：如果有 journal，就用 8 字节 scratch 调 journal.read_block_n。

系统意义：主盘失败时尝试 journal 路径。

### `read_block`，3192 行

实现：

- loop；
- begin_op；
- 如果 remaining_errors 为 0，fill_block 后 Ok；
- 否则消费错误并 retry_journal；
- 没有重试上限。

系统意义：会一直重试直到错误耗尽；持久错误会无限循环，需注意。

### `read_block_n`，3206 行

实现：

- attempt++；
- begin_op；
- 错误为 0 时 fill_limited_read，返回 attempt；
- 否则消费错误、retry_journal；
- 如果 limit > 0 且 attempt >= limit，返回 `Err("limit")`。

系统意义：带最大尝试次数的读。

### `total_ops` / `reset_ops` / `write_block` / `flush`，3222-3235 行

实现：

- total_ops/load，reset/store 0；
- write_block begin_op，仍有错误就消费并返回 io_error，否则 Ok；
- flush begin_op，如果有 journal 就 journal.ops++。

系统意义：磁盘写和 flush 的简化模型。

## 10. SysV IPC、共享内存、进程初始化、capability、signal

### `SemArr::index`，3275 行

实现：实现 `Index<usize>`，直接返回 `&self.sems[i]`。

系统意义：让 `arr[num]` 可以直接访问某个 semaphore。

### `SemArr::remove`，3278 行

实现：遍历 `sems`，对每个 `Sema` 调 `remove()`。

系统意义：删除整个 semaphore array 时，所有子 semaphore 都标记 removed。

### `SemArr::otime_now` / `ctime_now`，3279-3280 行

实现：锁 `ds`，把 `otime` 或 `ctime` 设置为 0。

系统意义：模拟 SysV semaphore 的操作时间/修改时间。当前用 0 作为占位。

### `SemArr::set_ds`，3281 行

实现：

- 锁 `ds`；
- 只复制 `uid/gid/mode`；
- mode 只保留低 9 位权限位。

系统意义：模拟 `semctl SETVAL/IPC_SET` 这类修改描述符权限的操作。

### `SemArr::get_or_create`，3287 行

实现：

- 锁全局 `store: RwLock<BTreeMap<u32, Weak<SemArr>>>` 的写锁；
- 如果 key 为 0，就找一个未使用 key；
- 如果 key 已存在并且 Weak 能 upgrade，说明对象还活着；
- 同时设置 create/excl 时返回 `eexist`；
- 否则返回已有对象；
- 如果需要新建，就创建 nsems 个初值为 0 的 `Sema`；
- 构造 `SemDs`，mode 来自 flags 低 9 位；
- 用 `Arc::downgrade` 存入全局 store；
- 返回 Arc。

系统意义：SysV IPC 典型模式：全局 key 找对象，对象用弱引用缓存，进程拿到强引用。

### `SemCtx::add`，3330 行

实现：

- 从 0 开始找第一个不在 `arrays` 里的 id；
- 插入 `Arc<SemArr>`；
- 返回 id。

系统意义：进程私有的 semaphore id 表。

### `SemCtx::remove` / `free_id` / `get`，3335-3337 行

实现：

- remove 删除 id；
- free_id 找空 id；
- get clone Arc。

系统意义：维护当前进程可见的 semaphore array。

### `SemCtx::add_undo`，3338 行

实现：

- 读取 `(id, num)` 当前 undo 值，没有则 0；
- 插入 `old - op`。

系统意义：模拟 `SEM_UNDO`，进程退出时反向恢复 semaphore 操作。

### `Clone for SemCtx`，3343 行

实现：fork 时复制 `arrays`，但 `undos` 清空。

系统意义：子进程继承 semaphore 引用，但不继承父进程 undo 记录。

### `Drop for SemCtx`，3348 行

实现：

- 遍历 undo 表；
- 找到对应 SemArr；
- 当前只对 op == 1 的情况调用 release。

系统意义：进程结束时执行 semaphore undo。

### `ShmTag::set_addr`，3369 行

实现：修改 attach 地址。

系统意义：记录共享内存段映射到进程虚拟地址的位置。

### `shm_get_or_create`，3372 行

实现：

- 锁全局 shm store；
- 如果 key 已存在且 Weak 能 upgrade，返回已有共享页数组；
- 否则创建 `Arc<Mutex<Vec<usize>>>`，长度为 npages，初值 0；
- 把 Weak 存进 store；
- 返回 Arc。

系统意义：按 key 管理共享内存对象。

### `ShmCtx::add`，3389 行

实现：

- 找当前进程第一个空 shmid；
- 插入 `ShmTag { addr: 0, pages: g }`；
- 返回 id。

系统意义：进程 attach 一个共享内存对象后，放进自己的 shm 表。

### `ShmCtx::get` / `set` / `get_id_by_addr` / `pop`，3394-3399 行

实现：

- get clone tag；
- set 覆盖 id 对应 tag；
- get_id_by_addr 线性查 addr；
- pop 删除 id。

系统意义：支持 shmat/shmdt 风格查找。

### `Clone for ShmCtx`，3401 行

实现：clone 整个 ids map。

系统意义：fork 时子进程继承共享内存 attachment。

### `ProcInit::push_at`，3411 行

实现：

- 从用户栈顶 `top` 往低地址布局；
- 先为第一个 argv 字符串预留空间；
- 再为 env 字符串预留空间；
- 再为 argv 字符串预留空间；
- 计算 auxv、env 指针数组、argv 指针数组所需字节；
- 再减一个 word 放 argc 或对齐占位；
- 最后按 16 字节对齐。

系统意义：模拟 exec 时构造用户栈，放 argv/envp/auxv。

### `ProcInit::total_size`，3443 行

实现：

- 累加 args/envs 字符串长度加终止符；
- 再加 auxv、argv 指针、env 指针、argc 等 word 数。

系统意义：估计初始化栈需要多少空间。

### `CapSet::new` / `full`，3453-3455 行

实现：

- new 所有 capability 为空；
- full 把 bits/effective 设为全 1，ambient 仍 0。

系统意义：进程 capability 集合。

### `CapSet::check`，3459 行

实现：

- cap >= 64 返回 false；
- 检查 effective 里对应 bit。

系统意义：权限检查看 effective set。

### `grant` / `drop_cap`，3464-3471 行

实现：

- grant 设置 bits 和 effective 对应位；
- drop_cap 清 bits 和 effective 对应位。

系统意义：授予或删除能力。

### `CapSet::inherit`，3478 行

实现：

- mask = `INHERITABLE_MASK`；
- 计算 `filtered_b = parent.bits & !mask`；
- 计算 `filtered_e = parent.effective & !mask`；
- ambient 直接继承；
- `_cap_count` 只是统计占位。

系统意义：模拟 exec/fork capability 继承。这里用 `!mask` 过滤，语义上可以讨论是否符合真实 Linux。

### `has_any` / `clear_ambient` / `raise_ambient`，3493-3501 行

实现：

- `has_any` 检查 effective 与 mask 是否有交集；
- `clear_ambient` 清空 ambient；
- `raise_ambient` 只有 cap 在 bits 中存在时才设置 ambient 位。

系统意义：ambient capability 管理。

### `SigSet::new`，3514 行

实现：

- 创建长度 `NSIG + 1` 的 actions；
- 每个 action 默认 `SIG_DFL`；
- pending 和 blocked 都为 0。

系统意义：初始化进程信号状态。

### `sig_pending` / `sig_raise` / `sig_clear`，3522-3543 行

实现：

- pending 是 u64 位图；
- sig_pending 检查某位；
- sig_raise 设置某位；
- sig_clear 清某位；
- signo 必须小于 NSIG。

系统意义：普通 signal 用位图合并，同一个信号多次 pending 仍是一位。

### `coalesce_pending`，3532 行

实现：

- active = pending & !blocked；
- 遍历 1..NSIG，把 active 的信号位放入 result；
- 返回 result。

系统意义：计算当前可递送的 pending 信号集合。

### `sig_block` / `sig_unblock` / `sig_setmask`，3549-3558 行

实现：

- sig_block 把 mask OR 到 blocked，再强制清 SIGKILL/SIGSTOP；
- sig_unblock 从 blocked 里清 mask；
- sig_setmask 直接设置，但也不能阻塞 SIGKILL/SIGSTOP。

系统意义：不可屏蔽信号永远不能被 mask。

### `deliverable`，3562 行

实现：

- actionable = pending & !blocked；
- 没有则 None；
- 从小信号号开始找第一位并返回。

系统意义：选择下一个可递送信号。

### `set_action` / `get_action` / `is_ignored` / `clear_non_caught`，3573-3595 行

实现：

- set_action 不允许修改 SIGKILL/SIGSTOP；
- get_action 越界时返回 actions[0]；
- is_ignored 判断 handler 是否 SIG_IGN；
- clear_non_caught 把既不是默认也不是忽略的 handler 重置成默认。

系统意义：维护 signal handler，exec 时清理用户自定义 handler。

## 11. Timer、Context、TrapCtl、时钟函数

### `TimerEntry::new`，3605 行

实现：设置 deadline、interval、callback_id，active=true，repeat 取决于 interval 是否大于 0。

系统意义：一个 timer 项。

### `expired` / `reset` / `remaining` / `cancel`，3609-3626 行

实现：

- expired 判断 `CLK > deadline`；
- reset 对 repeat timer 设置新 deadline，否则 active=false；
- remaining 返回 deadline-now 或 0；
- cancel 设置 active=false。

系统意义：timer 生命周期。

### `TimerWheel::new`，3635 行

实现：创建 `TIMER_WHEEL_SIZE` 个 Vec slot，current_slot=0。

系统意义：时间轮。

### `add_timer`，3643 行

实现：slot = deadline % wheel size，把 entry push 进对应 slot。

系统意义：按 deadline 放入时间轮槽。

### `advance`，3648 行

实现：

- current_slot 前进一格；
- 取出当前 slot 所有 entry；
- active 且 expired 的放入 fired；
- active 但没过期的放回 remaining；
- 对 fired 中 repeat timer 调 reset，并 clone 一个新 TimerEntry 放入新 slot；
- 返回 fired。

系统意义：每 tick 推进 timer wheel，找出到期 timer。

### `cancel` / `active_count`，3672-3684 行

实现：

- cancel 遍历所有 slot，找到 callback_id 匹配且 active 的 timer，设 inactive；
- active_count 统计 active timer。

系统意义：取消 timer 和统计。

### `Context::new`，3696 行

实现：寄存器数组全 0，ip=0，flags=0。

系统意义：空 CPU 上下文。

### `Context::capture`，3697 行

实现：从传入 `[u64; N_REGS]` 复制寄存器，ip/flags 设 0。

系统意义：从寄存器快照构造 Context。

### `Context::apply`，3708 行

实现：

- 创建输出寄存器数组；
- 复制前两个寄存器，再复制剩余寄存器；
- 计算 `_checksum` 但不返回；
- 返回数组。

系统意义：把 Context 应用回寄存器数组。

### `set_ip` / `set_sp` / `set_ret` / `set_tls`，3729-3741 行

实现：

- set_ip 改 ip；
- set_sp 改最后一个寄存器；
- set_ret 改 r[0]；
- set_tls 改倒数第二个寄存器。

系统意义：exec/fork/clone 时设置入口、栈、返回值、TLS。

### `transform`，3746 行

实现：

- 复制当前 Context；
- 根据 `op & 0x0F` 选择修改 r0/ip/sp/tls/flags/某个寄存器；
- 未知 op 做 no-op 占位；
- 返回新 Context。

系统意义：统一上下文变换辅助。

### `syscall_args`，3774 行

实现：返回 r[0] 到 r[5]，不足则补 0。

系统意义：从寄存器约定提取 syscall 参数。

### `clone_with_ret`，3784 行

实现：复制整个 Context，只把 r[0] 改成 ret。

系统意义：fork/clone 子任务返回值通常要改成 0。

### `diff` / `hash` / `reg_class`，3799-3827 行

实现：

- diff 返回所有不同寄存器、ip、flags；
- hash 用 FNV 风格混合寄存器/ip/flags；
- reg_class 根据高 4 位分类处理。

系统意义：调试上下文差异和分类。

### `TrapCtl::new`，3850 行

实现：active=false，mask=0，nest=0，frame=None，stack 空，irq_on=true，suppressed=false。

系统意义：trap 控制器初始状态。

### `configure`，3862 行

实现：

- 计算 combined 和 parity 占位；
- `hw_mask.store(b)`；
- `sw_mask.store(a)`。

系统意义：设置硬件/软件 trap mask。这里 a 是软件 mask，b 是硬件 mask。

### `hw` / `sw`，3873-3878 行

实现：分别 load hw_mask 和 sw_mask。

系统意义：查询 trap mask。

### `in_handler`，3883 行

实现：读取 active 和 nest，只要 active 或 nest > 0 就 true。

系统意义：判断当前是否处在 trap handler 中。

### `dispatch`，3888 行

实现：

- 锁 frame，替换成当前 ctx 的 clone；
- nest 加一再减一；
- 返回 ctx 的 clone。

系统意义：模拟 trap 分发保存现场，但不真正修改上下文。

### `current`，3916 行

实现：读取 frame 中保存的 Context，手动 clone 返回。

系统意义：获取当前 trap frame。

### `handle_irq`，3934 行

实现：

- active 设 true；
- irq_on 设 true；
- 保存 ctx 到 frame；
- nest 加一再减一；
- 读 suppressed；
- 最后 active=false；
- 返回 ctx clone。

系统意义：中断处理路径的模拟。

### `on_pgfault`，3959 行

实现：

- 如果 fault 地址在内核空间 `>= KERN_BASE`，返回 Err("fault")；
- 否则计算页地址和页内 offset；
- 返回 Ok。

系统意义：缺页异常合法性检查。当前不要求 `in_handler()`，而是按地址判断。

### `dispatch_vector`，3968 行

实现：

- 读 hw/sw mask；
- vector 0/1/2..=7 根据 hw bit 决定是否 dispatch；
- vector 8..=15 根据 sw bit；
- 但 `14` 放在后面，实际上被 `8..=15` 分支覆盖，后面的 14 分支不可达。

系统意义：按中断向量分发。CR 可指出 match 顺序里 14 分支位置有问题。

### `push_frame` / `pop_frame` / `nest_depth` / `suppress` / `unsuppress`，3997-4013 行

实现：

- push/pop 操作 stack；
- nest_depth 读取 nest；
- suppress/unsuppress 设置 suppressed。

系统意义：嵌套 trap 栈和抑制开关。

### `wclk` / `cclk` / `dtk` / `up_ms` / `tmr` / `ser`，4021-4029 行

实现：

- wclk 读全局 CLK；
- cclk 读 CLK_ALL；
- dtk 在 cpu_id==0 时 CLK++，所有 CPU 都 CLK_ALL++；
- up_ms 用 tick 转毫秒；
- tmr 调 dtk；
- ser 把 `\r` 转 `\n`。

系统意义：全局时钟和串口字符处理。

## 12. 调度器、Task、TaskTable

### `SchedulePolicy::new`，4041 行

实现：普通调度策略，prio=0，nice=0，time_slice=10，vruntime=0。

系统意义：默认调度参数。

### `SchedulePolicy::with_prio`，4045 行

实现：用传入 prio 设置 prio/nice，time_slice = `20 - prio as usize`。

系统意义：按优先级创建策略。注意负 prio 转 usize 会有风险，CR 可指出。

### `SchedulePolicy::weight`，4049 行

实现：根据 nice 分段返回权重，nice 越小权重越大。

系统意义：模拟 CFS 的 nice-to-weight。

### `RunQueue::new`，4068 行

实现：queue 空，current=None，preempt_count=0。

系统意义：运行队列初始状态。

### `RunQueue::enqueue`，4076 行

实现：

- 把 `(task_id, policy)` push 到 queue；
- 用冒泡排序按 score 排序；
- score 综合 prio、nice、vruntime、weight；
- `_dup` 计算是否重复但未阻止重复。

系统意义：加入可运行任务并排序。重复任务防护是可改进点。

### `dequeue`，4105 行

实现：

- 队列空返回 None；
- 扫描找 score 最小的任务；
- remove 并返回。

系统意义：取出下一个运行任务。

### `pick_next`，4117 行

实现：不删除任务，只找 score 最小的 id。

系统意义：预览下一个任务。

### `cmp_priority`，4132 行

实现：按 weight、prio、nice、vruntime 算 score 后比较。

系统意义：调度策略比较函数。当前没被核心排序直接复用。

### `rebalance`，4140 行

实现：

- 读 tick；
- 对每个 task 的 vruntime 加权增加；
- 然后按 vruntime 排序。

系统意义：重新平衡运行队列。

### `set_current` / `clear_current` / `len` / `remove`，4157-4169 行

实现：

- current 写 Some 或 None；
- len 返回 queue.len；
- remove 扫描删除所有 task_id 匹配项。

系统意义：记录当前运行任务和维护队列。

### `update_vruntime`，4179 行

实现：

- 找到 task；
- delta 按 weight 缩放；
- 加到 vruntime。

系统意义：任务运行后更新虚拟运行时间。

### `preempt_disable` / `preempt_enable` / `preemptible`，4191-4202 行

实现：

- disable 让 preempt_count++；
- enable 让它--，如果从 1 到 0，计算 `_need_resched`；
- preemptible 判断 count 是否 0。

系统意义：临界区内禁止抢占。

### `boost_priority` / `yield_current`，4206-4216 行

实现：

- boost 找到 task，把 prio 减 amount，最低 -20；
- yield_current 把 current 取出，重新用默认策略 push 回队列。

系统意义：优先级提升和主动让出 CPU。

### `Pid::new` / `get` / `is_init` / Display，4237-4242 行

实现：小包装，默认 0，读取内部值，判断是否为 1，Display 打印数字。

系统意义：让 pid 类型更明确。

### `ThdCtx::default`，4259 行

实现：默认 Context、clear_tid=0、smask=0。

系统意义：线程上下文初始值。

### `Task::make`，4288 行

实现：

- 创建 Arc<Task>；
- 初始化 info、parent、subtasks、files、cwd、exec_path；
- futexes/sem_ctx/shm_ctx/pid/pgid/threads/event bus/exit_code/signal/epoll/kernel stack/thread context/vm_token 全部初始化。

系统意义：创建进程/线程对象。Task 是整个 kernel 中“进程控制块 PCB”的核心。

### `id` / `tag`，4313-4314 行

实现：锁 info，返回 id 或 clone tag。

系统意义：任务身份。

### `link_parent` / `link_child`，4315-4316 行

实现：设置 parent 或把 child push 到 subtasks。

系统意义：维护进程树。

### `done` / `n_children`，4317-4318 行

实现：done 看 status 是否 Some，n_children 返回 subtasks.len。

系统意义：判断进程退出和子进程数量。

### `get_free_fd` / `get_free_fd_from`，4319-4323 行

实现：锁 files，从 0 或 arg 开始找第一个没有被占用的 fd。

系统意义：fd 分配器。

### `add_file`，4327 行

实现：先找 free fd，再 insert `FLike`，返回 fd。

系统意义：open/pipe/dup 后把文件对象放入 fd table。

### `get_file`，4332 行

实现：锁 files，按 fd get 后 cloned。

系统意义：系统调用层通过 fd 找 File 对象。

### `get_futex`，4335 行

实现：

- 锁 futexes；
- 如果 uaddr 不存在，插入新的 FutexBucket；
- 返回 Arc clone。

系统意义：每个进程按用户地址维护 futex bucket。

### `exit_proc`，4342 行

实现：

- 收集所有 fd；
- 逐个从 files 删除；
- 做一次 fd table gap 审计占位；
- 设置自身 event bus 的 `PROC_QUIT`；
- 如果有 parent，设置 parent event bus 的 `CHILD_QUIT`；
- 保存 exit_code；
- 清空 threads；
- info.status = Some(code & 0xFF)。

系统意义：进程退出，关闭 fd、通知父进程、记录状态。

### `exited`，4388 行

实现：线程列表为空或 status 为 Some 就认为退出。

系统意义：wait/reap 判断 zombie。

### `get_ep_mut` / `get_ep_ref` / `set_ep`，4392-4403 行

实现：

- get_ep_mut 从 ep_inst map 找 epfd，手动 clone EpInst；
- get_ep_ref 直接调用 get_ep_mut；
- set_ep 插入或覆盖 epfd。

系统意义：维护任务自己的 epoll 实例表。

### `begin_run` / `end_run`，4407-4421 行

实现：

- begin_run 从 `thd_ctx: Mutex<Option<ThdCtx>>` take 出上下文；
- 如果没有则返回默认上下文；
- end_run 把上下文放回 Some。

系统意义：模拟线程被调度运行时取出/保存上下文。

### `has_sig`，4425 行

实现：

- 如果 signal queue 空，false；
- 读取 sig_mask 和当前 tid；
- 遍历队列；
- sender 不是 -1 且不是当前 tid 的信号跳过；
- 信号位不在 mask 中则 found=true。

系统意义：判断当前任务是否有可处理信号。

### `send_sig`，4441 行

实现：

- 把 `(signo, sender_tid)` push 进 sig_queue；
- 设置 event bus 的 `RECV_SIG`；
- 事件变化时触发回调。

系统意义：给任务发送信号。

### `close_fd`，4453 行

实现：

- 从 files remove fd；
- 如果存在，poll 一次作为占位，然后 Ok；
- 不存在返回 ebadf。

系统意义：关闭 fd。

### `dup_fd`，4465 行

实现：

- 取 old_fd 对应 FLike；
- 调 `fl.dup(cloexec)`；
- 找最小空 fd；
- 插入新 fd；
- 返回新 fd。

系统意义：实现 dup。

### `dup2_fd`，4481 行

实现：

- old==new 直接返回；
- 找 old fd，不存在 ebadf；
- dup 一份；
- 删除 new_fd 原对象；
- 插入 new_fd。

系统意义：实现 dup2。

### `fd_count` / `set_cloexec`，4494-4501 行

实现：

- fd_count 返回 files.len；
- set_cloexec 只检查 fd 是否存在，没有真正修改 cloexec。

系统意义：fd 数量和 close-on-exec 设置。`set_cloexec` 是重构可完善点。

### `TaskTable::new`，4525 行

实现：map 空，seq=1，root=None。

系统意义：全局任务表。

### `spawn` / `spawn_root`，4528-4534 行

实现：

- spawn 用 seq.fetch_add 生成 id，Task::make，插入 map；
- spawn_root 创建 tag 为 init 的任务，并保存 root。

系统意义：创建任务和 init 任务。

### `find` / `find_by_tag` / `process_of_tid` / `pgid_group`，4539-4550 行

实现：

- find 按 id 读 map；
- find_by_tag 过滤 tag；
- process_of_tid 查哪个 task.threads 包含 tid；
- pgid_group 过滤 pgid。

系统意义：任务检索。

### `register`，4555 行

实现：设置 task.pid，并以 pid.get() 为 key 插入 map。

系统意义：把已有 Task 注册成某个 pid。

### `reap`，4559 行

实现：

- 找 task；
- 设置 status=Some(0)；
- drain 子进程；
- 把子进程重新挂到 root；
- 从 map 删除 id。

系统意义：回收 zombie，并把孤儿进程交给 init。

### `fork_task`，4575 行

实现：

- 分配新 id；
- 用父 tag 创建新 Task；
- 复制 cwd、exec_path；
- 复制 fd table，每个 fd 调 `fl.dup(false)`；
- 复制 pgid、sem_ctx、shm_ctx、sig_mask；
- 设置 parent；
- 把 child push 到 src.subtasks；
- register child；
- child.threads push nid；
- 又 push 一次 child 到 src.subtasks。

系统意义：fork 复制进程资源。注意当前实现对子进程列表 push 了两次，这是 CR 可指出的代码质量问题。

### `clone_thread`，4619 行

实现：

- 新建 Task；
- 构造 ThdCtx，ret=0，sp=stack_top，tls=tls，clear_tid=clear_tid；
- 继承 signal mask；
- vm_token 复制；
- 插入 task table；
- 把新 thread id 加进 src.threads。

系统意义：创建同进程资源下的新线程模型。

### `new_user_task`，4634 行

实现：

- spawn 一个 path tag 的任务；
- 设置 exec_path；
- 用内置 ELF 字节调用 validate_elf_header；
- ProcInit 计算用户栈 sp；
- 设置线程上下文 sp；
- 创建 fd0/fd1/fd2 指向 `/dev/tty`；
- 注册 pid；
- threads push id。

系统意义：创建用户态初始任务。

### `terminate_and_collect`，4667 行

实现：find task，调用 exit_proc，再 reap。

系统意义：终止并回收指定任务。

### `active_tasks` / `zombie_tasks`，4678-4685 行

实现：过滤 task.done() false 或 true，返回 id 列表。

系统意义：任务状态查询。

### `send_signal_group`，4692 行

实现：查 pgid_group，遍历发送信号，返回发送数量。

系统意义：kill 负 pid 或进程组广播。

## 13. Kernel 普通方法

### `Kernel::new`，4716 行

实现：

- 创建 TaskTable；
- 创建 BlockCache，链数是 `N_CHAINS`；
- 创建 FramePool，页数来自参数 nf；
- 初始化 MAX_CPU 个 CPU 当前任务槽为 None；
- 创建 MountTable；
- 创建 semaphore/shared memory 全局弱引用 store；
- 创建 tty buffer；
- 创建 label 为 `"kernel"` 的 Disk。

系统意义：把内核所有子系统组合成一个总对象。

### `Kernel::tick`，4729 行

实现：

- 用 `GKL.try_enter(id)` 尝试拿大内核锁，拿不到就跳过后面的全局维护；
- 用 `cpus.try_lock()` 统计 CPU 占用率 `_ir`；
- 如果拿到 GKL，就遍历 block cache chain；
- 每条 chain 用 `try_acquire()`，忙就跳过；
- `items.try_lock()` 成功时把所有 slot.modified 清 false；
- 最后释放 chain lock 和 GKL。

系统意义：tick 路径不能阻塞等待锁，否则可能和调度/缓存/内存形成死锁链。所以这里都用 try。

### `cur_task`，4759 行

实现：

- 锁 cpus；
- cpu 越界返回 None；
- 有任务就 clone Arc 返回。

系统意义：获取某 CPU 当前运行任务。

### `set_cur`，4771 行

实现：

- 锁 cpus；
- cpu 合法就 take 掉旧任务，写入新 Option。

系统意义：调度时切换当前任务。

### `handle_pgfault`，4778 行

实现：

- 计算页地址和 offset；
- 获取 CPU0 当前任务；
- 有任务就读 vm_token 占位并返回 true；
- 无任务返回 false。

系统意义：缺页处理入口的简化模型。

### `handle_pgfault_ext`，4790 行

实现：

- 算页号和 offset；
- 如果 access 有写位，调用 handle_pgfault；
- 否则也调用 handle_pgfault。

系统意义：带访问类型的缺页入口，目前读写路径没有区分。

### `proc_init`，4796 行

实现：

- spawn_root 创建 init；
- 把 root id 放入 threads；
- 分配 KStk；
- 保存到 root.kstk。

系统意义：初始化第一个任务。

### `tty_push` / `tty_pop`，4803-4808 行

实现：

- push 时把 `\r` 转 `\n`；
- tty_buf 长度小于 4096 才 push；
- pop 从队头取一个字节。

系统意义：终端输入缓冲。

### `get_sem` / `get_shm`，4812-4815 行

实现：分别调用 `SemArr::get_or_create` 和 `shm_get_or_create`。

系统意义：Kernel 提供 IPC 全局对象入口。

### `spawn_thread`，4818 行

实现：

- clone task 进入宿主线程；
- 循环 begin_run -> end_run；
- 如果 task.done() 就退出；
- 否则 yield_now。

系统意义：用宿主线程模拟内核调度运行任务。

## 14. `dispatch_syscall` 总入口

### 总体结构，4830 行

实现：

- 参数是 syscall number `nr` 和六个参数 `a0..a5`；
- 先计算 `_audit` 和 `_ts_enter` 作为审计/时间占位；
- 通过 `self.cpus` 找当前任务的 vm_token 作为 `_caller_token`；
- 对 `nr` 做 match；
- 每个分支返回 `Result<usize, &'static str>`。

系统意义：这是系统调用层。真实 OS 会从 trap frame 中取参数，这里直接传入参数模拟。

### `SYS_READ`，4840 行

实现：

- 参数：fd=a0，buf_addr=a1，count=a2；
- buf_addr 为 0 且 count>0 返回 efault；
- count 为 0 返回 0；
- `check_access(buf_addr, count)` 失败返回 efault；
- 算 page_start/page_end/page_span；
- 用 `fd % cache.width` 找 cache chain；
- 上 chain lock，检查是否有 slot.id == fd；
- 命中缓存时，transfer 最多是页跨度容量，超过 PAGE_SZ 会扣掉 readahead；
- 未命中时，最多返回 `PAGE_SZ * 16`。

系统意义：模拟 read 的地址检查和缓存影响，没有真实 copy 数据。

### `SYS_WRITE`，4871 行

实现：

- 检查用户 buf 地址和 count；
- 根据页内偏移计算 `actual_len`；
- 找 fd 对应 cache chain；
- 如果有 slot.id == fd，就标记 modified=true；
- fd <= 2 时增加 disk.ops；
- 返回 actual_len。

系统意义：模拟 write 标脏 cache 和标准输出/错误的 I/O 统计。

### `SYS_OPEN`，4902 行

实现：

- path_addr 为 0 返回 efault；
- 检查 path 地址；
- 解析 flags：访问模式、create、excl、truncate、nonblock、append、cloexec、follow；
- 扫描 mount table 找最长 prefix 长度作为解析占位；
- 如果 create+excl，就用 path_addr 对应 cache chain 判断是否已存在；
- 如果有当前任务：
  - 根据 flags 构造 FdOpt；
  - 创建 path 为 `"anon"` 的 FHandle；
  - `t.add_file(FLike::File(fh))` 分配 fd；
  - 如果 truncate 且 writable，调用 set_len(0)；
- 没有当前任务就返回一个模拟 fd；
- 最后计算 mode 权限占位，返回 fd。

系统意义：系统调用层通过当前 Task 的 fd table 插入 File 对象。

### `SYS_CLOSE`，4969 行

实现：

- fd 大于限制返回 ebadf；
- 找 fd 对应 cache chain；
- 删除 slot.id == fd 的缓存项；
- 如果删除了缓存，disk.ops++；
- fd < 3 也直接 Ok；
- 返回 0。

系统意义：模拟 close 清理 cache。注意这里没有真正从当前 task.files 删除 fd，和 `Task::close_fd` 分离。

### `SYS_STAT | SYS_FSTAT`，4990 行

实现：

- stat_buf 为 0 或不可访问返回 efault；
- STAT 额外检查 path_addr；
- FSTAT 用 fd/4 计算 dev 占位；
- 返回 0。

系统意义：模拟 stat/fstat 的用户地址检查。

### `SYS_MMAP`，5006 行

实现：

- len 为 0 返回 einval；
- len 和 offset 页对齐；
- 解析 MAP_ANON/MAP_FIXED/MAP_PRIVATE/MAP_SHARED；
- 根据 prot 生成 vm_flags；
- fixed 且 addr 非 0 时使用 addr；
- 否则用 base + tick/fd 生成一个页对齐地址；
- 检查 frame pool free_count 是否足够；
- 非匿名映射时检查 aligned_off；
- 返回映射地址。

系统意义：模拟 mmap 地址选择和内存容量检查，但没有真正插入 VmMap。

### `SYS_MUNMAP`，5040 行

实现：

- addr 必须页对齐；
- len 向上页对齐；
- 循环计算每页 va 但不真实删除；
- 返回 0。

系统意义：munmap 占位。

### `SYS_BRK`，5051 行

实现：

- a0 为 0 返回默认 brk；
- new_brk >= KERN_BASE 返回 enomem；
- new_brk 页对齐；
- 如果有当前任务：
  - 读 old_brk = vm_token；
  - 缩小时计算释放页占位；
  - 增长时检查 free_count，不够 enomem；
  - 增长时调用 frame_alloc 分配页；
  - vm_token.store(aligned)；
- 返回 aligned。

系统意义：用 Task.vm_token 模拟进程 brk。

### `SYS_IOCTL`，5078 行

实现：

- 根据 cmd 分支；
- TCGETS/TCSETS 检查 TrmIO 地址；
- TIOCGPGRP/TIOCSPGRP 检查 4 字节地址；
- TIOCGWINSZ 检查 WinSz 地址；
- FIONCLEX/FIOCLEX 直接 Ok；
- FIONBIO 检查 4 字节地址；
- 其他 enotty。

系统意义：终端 ioctl 的简化实现。

### `SYS_PIPE`，5112 行

实现：

- 检查 fds_addr；
- 检查用户空间能写两个 i32；
- 有当前任务时检查 fd_count + 2；
- `PipeNode::pair()` 创建读写端；
- add_file 插入两个 fd；
- 返回 `rd_fd | (wr_fd << 32)`；
- 无当前任务返回 esrch。

系统意义：创建 pipe 并把两个端点放进 fd table。

### `SYS_DUP`，5131 行

实现：

- old_fd 越界 ebadf；
- 有当前任务时，只找一个未占用 fd candidate；
- 没有真正插入 dup 的 FLike；
- 无当前任务返回 old_fd+1。

系统意义：当前分支更像 fd 号分配模拟。真正 dup 逻辑在 `Task::dup_fd`。

### `SYS_DUP2`，5145 行

实现：

- 检查 old_fd/new_fd；
- old==new 返回 new_fd；
- 有当前任务时锁 files；
- remove new_fd；
- 找 old_fd，存在就 `fl.dup(false)` 插到 new_fd，不存在 ebadf；
- 返回 new_fd。

系统意义：把 old fd 复制到指定 fd。

### `SYS_FORK`，5164 行

实现：

- 读取 caller token 占位；
- 根据 free_count 和 task count 估计 child copy cost；
- 用 `tasks.seq.fetch_add` 生成新 pid；
- 计算内存使用比例，超过 90% 返回 enomem；
- 如果空闲页不足估计成本，也 enomem；
- 返回 new_pid。

系统意义：这个 syscall 分支只模拟 fork 成本和 pid，不调用 `do_fork`，是可重构点。

### `SYS_EXEC`，5187 行

实现：

- 检查 path/argv/envp 指针；
- 用内置 ELF 字节调用 validate_elf_header；
- 返回 0。

系统意义：exec 地址合法性和 ELF 验证占位。真实执行逻辑在 `do_exec` 更完整。

### `SYS_EXIT`，5208 行

实现：

- status=a0；
- 找当前任务；
- 调 `t.exit_proc(status)`；
- 给父进程发 SIGCHLD；
- 把子进程重挂到 init；
- 返回 0。

系统意义：进程退出、通知父进程、处理孤儿进程。

### `SYS_WAIT4`，5230 行

实现：

- 检查 status_addr 和 rusage_addr；
- 解析 WNOHANG/WUNTRACED/WCONTINUED/WALL；
- pid == -1：找任意 zombie；
- pid == 0：找当前进程组 zombie；
- pid > 0：找指定 pid；
- pid < -1：找指定进程组；
- 没有 zombie 时，WNOHANG 返回 0，否则 echild。

系统意义：等待子进程退出。当前实现基于全局 task table，亲子关系检查较弱。

### `SYS_KILL`，5318 行

实现：

- 检查 sig <= NSIG；
- SIGKILL/SIGSTOP 不允许发给 pid <= 1；
- pid == 0：给当前进程组发；
- pid == -1：给所有 active task 发，跳过 pid <= 1；
- pid > 0：找指定任务，done 且 sig!=0 时 esrch，否则 send_sig；
- pid < -1：给对应进程组发。

系统意义：信号发送。

### `SYS_FCNTL`，5366 行

实现：

- 检查 fd 范围；
- F_DUPFD：返回 base + tick 低位；
- F_DUPFD_CLOEXEC：返回 base+1；
- F_GETFD：从 cache chain 里看 slot.modified，当作 cloexec；
- F_SETFD：解析 cloexec 但不保存；
- F_GETFL：fd<=2 返回 NONBLOCK|APPEND，否则 NONBLOCK；
- F_SETFL：只允许 NONBLOCK|APPEND；
- F_GETLK/F_SETLK/F_SETLKW 检查用户 flock 地址；
- 其他 einval。

系统意义：fcntl 的模拟。真实 fd flag 应该在 `FHandle/FdState` 中维护。

### `SYS_GETPID` / `SYS_GETPPID`，5423-5430 行

实现：

- GETPID 有当前任务返回 t.id，否则 1；
- GETPPID 有 parent 返回 parent.id，否则 0。

系统意义：进程 id 查询。

### `SYS_SETPGID`，5443 行

实现：

- pid 为 0 表示当前进程；
- pgid 为 0 表示 target_pid；
- 如果 target 不是 caller，要求 target 是 caller 子进程，否则 esrch；
- 找到 target 后写 t.pgid；
- 返回 0。

系统意义：设置进程组。

### `SYS_GETPGID`，5467 行

实现：

- pid 为 0 表示当前进程；
- target 为 0 返回 esrch；
- 找 task，返回 pgid，否则 esrch。

系统意义：查询进程组。

### `SYS_SETSID`，5481 行

实现：

- 当前任务不存在 esrch；
- 如果当前已经是进程组 leader，eperm；
- 否则 pgid 设置为 tid；
- 返回 tid。

系统意义：创建新 session 的简化模型。

### `SYS_EPOLL_CREATE`，5495 行

实现：

- size 为 0 einval；
- epfd = 3 + size % 61；
- 检查 size * EpEvent 是否溢出；
- 返回 epfd。

系统意义：epoll fd 分配模拟，没有插入 fd table。

### `SYS_EPOLL_CTL`，5503 行

实现：

- 检查 event 地址；
- ADD/MOD 要求 ev_addr 非 0；
- DEL 不需要 event；
- op 非法 einval。

系统意义：epoll_ctl 参数检查占位。

### `SYS_EPOLL_WAIT`，5518 行

实现：

- events_addr 不能为 0，max_events 不能为 0；
- 计算总 buffer 大小并检查乘法溢出；
- 检查用户 buffer；
- timeout==0 返回 0；
- timeout>0 时计算 deadline，但不真实等待；
- 返回 0。

系统意义：epoll_wait 参数检查和超时计算占位。

### `SYS_CLOCK_GETTIME`，5537 行

实现：

- 检查 timespec 指针；
- 读取 CLK；
- clk_id 0：按 TIMER_TICK_HZ 算 secs/nsecs；
- clk_id 1：加 BOOT_EPOCH；
- clk_id 4：raw tick；
- 不支持的 clk_id 返回 einval。

系统意义：时间查询占位，当前不真实写回 timespec。

### `SYS_SIGACTION`，5563 行

实现：

- 检查 signo 范围；
- 当前代码对 `signo != SIGKILL && signo != SIGSTOP` 返回 einval，这意味着只允许 KILL/STOP，和真实语义相反；
- 检查 act/oldact 地址；
- 解析 flags/mask 占位；
- 返回 0。

系统意义：signal action 参数检查。CR 可以指出这里逻辑明显需要重构。

### `SYS_SIGPROCMASK`，5575 行

实现：

- 检查 set/oldset 地址；
- unmaskable 是 SIGKILL/SIGSTOP；
- 找当前任务；
- oldset 非 0 时保存 old_mask 占位；
- set 非 0 时，把 `set_addr as u64` 当成 new_set；
- how 0 block，1 unblock，2 setmask；
- 永远清掉 unmaskable。

系统意义：修改当前任务 signal mask。当前没有真实从用户地址读取 mask。

### `SYS_FUTEX`，5601 行

实现：

- 检查 uaddr；
- op & 0x80 是 private 标志；
- futex_op = op & 0xF；
- 0 WAIT：检查 timeout 地址；
- 1 WAKE：返回 min(wake_count, task_count)；
- 3 REQUEUE：检查 uaddr2，返回 wake+requeue 的上限；
- 5 WAIT_BITSET：要求 timeout 非 0 并检查；
- 9 CMP_REQUEUE_PI 类似移动和唤醒计数；
- 其他 enosys。

系统意义：futex syscall 参数模型，没有真正挂入 FutexBucket。

## 15. Kernel 后续辅助方法

### `schedule_tick`，5645 行

实现：

- 调 `dtk(cpu)` 推进时钟；
- 如果当前 CPU 有任务，读取 tid 和 children_count；
- 根据 children_count 估算剩余时间片；
- 时间片为 0 时标记需要 resched，并找其他 runnable task；
- 计算 `_time_in_kernel` 占位。

系统意义：调度 tick 的简化模型。

### `balance_load`，5672 行

实现：

- 锁 cpus；
- 为每个 CPU 填 counts/prios/blocked；
- counts 来自当前任务子任务数 + 1；
- prios 用 pgid 占位；
- blocked 用 done；
- 计算平均负载和 imbalance；
- 调 `compute_load_balance` 返回目标 CPU。

系统意义：跨 CPU 负载均衡。

### `reclaim_zombies`，5696 行

实现：

- 获取 zombie task id 列表；
- 统计数量；
- 对每个 zombie 估算可回收页数为 fd_count；
- 调 `tasks.reap(id)` 删除；
- 返回数量。

系统意义：回收僵尸进程。

### `lookup_path`，5712 行

实现：

- path 空返回 enoent；
- 先计算规范化路径 `_canonical`：处理空组件、`.`、`..`；
- 但最终调用 `self.mnt.resolve(path)`，不是 `_canonical`；
- 重建 mount cache 占位；
- 返回 resolved。

系统意义：路径解析入口。CR 可指出 canonical 计算未用于 resolve。

### `alloc_pages`，5732 行

实现：

- 如果 free_count < count，调用 defragment_frame_pool；
- 循环 count 次；
- 每次锁 slots 找第一个 true，设 false；
- 转成物理地址 `idx * PAGE_SZ + MEM_OFF`；
- 找不到就提前停止；
- 返回页地址列表。

系统意义：Kernel 层多页分配。

### `free_pages`，5761 行

实现：

- 对每个物理地址计算 idx；
- 锁 slots；
- idx 合法则设 true。

系统意义：释放多页。

### `memory_pressure`，5772 行

实现：

- total = pool.cap，free = pool.free_count；
- pressure = used * 100 / total；
- 额外统计空闲 run 数作为碎片占位；
- 返回 pressure。

系统意义：内存压力百分比。

### `cache_stats`，5791 行

实现：返回 `(cache.total_entries(), cache.dirty_count())`。

系统意义：块缓存统计。

### `do_fork`，5795 行

实现：

- find parent；
- 调 `tasks.fork_task(&parent)` 创建 child；
- 复制 parent.vm_token 到 child；
- 估算父进程 fd 对应数据页数 `_est_pages`；
- 返回 child id。

系统意义：比 syscall 分支更真实的 fork helper。

### `do_exec`，5817 行

实现：

- find task；
- 更新 exec_path；
- 用内置 ELF 数据 validate；
- 遍历 fd table，收集 cloexec 的 fd 并删除；
- ProcInit 计算新用户栈；
- 新建 ThdCtx，设置 sp 和 ip=0x0040_0000；
- 写回 task.thd_ctx。

系统意义：exec 替换进程镜像的简化实现。

### `do_pipe`，5855 行

实现：

- find task；
- `PipeNode::pair()`；
- 两端 add_file；
- 返回两个 fd。

系统意义：pipe helper。

### `do_wait`，5863 行

实现：

- find parent；
- 根据 target_pid 判断匹配规则；
- 遍历 parent.subtasks；
- 找到 done 的 child 就记录 id/code；
- 找到后 `tasks.reap(id)` 并返回；
- 没找到且 WNOHANG 返回 `(0,0)`，否则 echild。

系统意义：wait helper，比 syscall 分支更接近亲子关系。

## 16. 后半部分工具函数、AddrSpace、进程组、等待队列、资源限制、BuddyAllocator

### `validate_access`，5895 行

实现：

- len 为 0 直接 Ok；
- `end = addr + len`，如果回绕返回 eoverflow；
- end 进入内核空间返回 efault；
- mode 0：只调用 `check_access`；
- mode 1：调用 `check_access`，额外计算覆盖页数；
- mode 2：计算页对齐 span，span 超过 KHEAP_SZ 返回 efault，再 check_access；
- 其他 mode 返回 einval。

系统意义：比 `check_access` 更细一点的用户地址权限检查。

### `mem_scan_pattern`，5924 行

实现：

- pattern 空或 data 比 pattern 短时返回空 Vec；
- 先构造 KMP failure table；
- 再扫描 data；
- 匹配一次就 push 起始位置；
- 达到 max_matches 后停止。

系统意义：内存模式扫描工具，使用 KMP 避免朴素回退。

### `compute_crc32`，5948 行

实现：

- 初始 crc 为全 1；
- 每个 byte xor 到 crc；
- 循环 8 次按多项式 `0xEDB88320` 更新；
- 最后取反。

系统意义：CRC32 校验。

### `encode_varint`，5963 行

实现：

- 每次取低 7 位；
- value 右移 7；
- 如果还有剩余，把当前 byte 最高位置 1；
- push 到 out；
- 返回写入字节数。

系统意义：变长整数编码。

### `decode_varint`，5976 行

实现：

- 遍历输入字节；
- 低 7 位左移 shift 后 OR 到 result；
- 最高位 0 表示结束，返回 `(value, bytes_used)`；
- shift 过大或超过 10 字节返回 None；
- 数据耗尽仍未结束返回 None。

系统意义：变长整数解码并检测溢出/截断。

### `AddrSpace::new`，6000 行

实现：

- 创建 VmMap；
- page_table_root=0；
- asid 使用参数；
- ref_count=1；
- cow_pages 空 map。

系统意义：进程地址空间对象。

### `AddrSpace::fork_from`，6010 行

实现：

- 创建 child；
- 复制 brk 和 mmap_base；
- 遍历 parent.vm_map.regions：
  - 创建新 VmRegion；
  - 如果原 region writable，调用 region.ref_up；
  - 插入 child.vm_map；
- 复制 parent.cow_pages：
  - 对每个 frame.up；
  - child 插入同地址 PgFrame::with_rc(frame.count())；
- 再次遍历 writable region 并 ref_up。

系统意义：fork 地址空间，写页走 COW。注意 writable region ref_up 做了两轮，是 CR 可疑点。

### `AddrSpace::handle_cow_fault`，6038 行

实现：

- page_addr 按页对齐；
- 用 vm_map.find 找 region，找不到 segfault；
- region 不可写也 segfault；
- 锁 cow_pages；
- 如果 page 已有 frame：
  - rc <= 1 说明不需要复制，返回 page_addr；
  - rc > 1，从 pool.get_inner 分配新 frame；
  - 原 frame.down；
  - cow map 插入 rc=1 的新 PgFrame；
  - 返回新物理地址；
- 如果 page 不在 cow_pages：
  - 分配新 frame；
  - 插入 rc=1；
  - 返回物理地址。

系统意义：COW 写时复制。

### `unmap_range`，6060 行

实现：

- 调 vm_map.remove_range；
- 锁 cow_pages；
- 收集在 `[start, end)` 内的 cow 页；
- remove 它们并 frame.down；
- 返回 region 删除数 + cow 页删除数。

系统意义：取消映射并减少 COW 引用。

### `protect`，6076 行

实现：

- 找所有和 `[start, end)` 重叠的 region index；
- 逆序遍历 affected，把 region.flags 改成 new_flags；
- 返回 Ok。

系统意义：mprotect 修改权限。当前不会 split region，可能影响超出范围的整段 region。

### `rss_pages` / `cow_sharers`，6092-6096 行

实现：

- rss_pages 返回 cow_pages.len；
- cow_sharers 统计 count > 1 的 PgFrame。

系统意义：内存统计。

### `split_region`，6101 行

实现：

- 用 vm_map.find(addr) 找 region；
- offset 必须在 region 内部；
- 创建第二段 `VmRegion::new(addr, region.len - offset, region.flags)`；
- 直接 push 到 regions；
- 没有修改原 region，也没有保持排序。

系统意义：region 切分的半成品。CR 可说应同时缩短原 region、复制 offset/tag/ref_count、并保持排序。

### `ProcessGroup::new`，6120 行

实现：设置 pgid、leader、members 初始包含 leader、session_id、foreground=false。

系统意义：进程组对象。

### `add_member` / `remove_member`，6130-6137 行

实现：

- add_member 只有不存在才 push；
- remove_member retain 删除 pid，返回是否删除。

系统意义：维护进程组成员。

### `is_empty` / `member_count` / `is_leader`，6144-6152 行

实现：分别读 members 是否空、长度、leader 是否等于 pid。

系统意义：进程组状态查询。

### `set_foreground` / `is_foreground`，6156-6160 行

实现：foreground AtomicBool store/load。

系统意义：前台进程组状态。

### `broadcast_signal`，6164 行

实现：

- clone members；
- 遍历 pid；
- tasks.find(pid)；
- 找到就 `send_sig(signo, leader as isize)`。

系统意义：向进程组广播信号。

### `WaitQueue::new`，6184 行

实现：inner 空 VecDeque，wake_count=0。

系统意义：按 key 睡眠的通用等待队列。

### `WaitQueue::sleep`，6191 行

实现：

- push `(key, current thread, flags)`；
- drop 锁；
- `thread::park()`。

系统意义：当前线程按 key 睡眠。

### `sleep_timeout`，6198 行

实现：

- push 等待项；
- park_timeout；
- 醒后锁队列；
- retain 删除所有 key 相同项；
- 返回是否删除过。

系统意义：带超时睡眠。注意它删除同 key 所有等待者，不区分当前线程。

### `wake_one`，6209 行

实现：

- 找第一个 key 匹配项；
- remove；
- unpark；
- wake_count++；
- 返回 true；
- 找不到返回 false。

系统意义：唤醒一个等待者。

### `wake_all`，6221 行

实现：

- drain 全队列；
- key 匹配的 unpark 并计数；
- 不匹配的放入 remaining；
- 队列替换为 remaining；
- wake_count 加 count；
- 返回 count。

系统意义：唤醒某 key 的所有等待者。

### `wake_filtered`，6238 行

实现：

- drain 全队列；
- 对每个 entry 调 pred(key, flags)；
- pred true 则 unpark；
- false 放回 remaining；
- wake_count 加唤醒数。

系统意义：按自定义条件唤醒。

### `pending_count` / `total_wakes` / `has_waiters_for`，6255-6263 行

实现：

- pending_count 返回 inner.len；
- total_wakes 读 wake_count；
- has_waiters_for 检查是否有 key。

系统意义：等待队列统计。

### `reorder_by_priority`，6267 行

实现：`VecDeque::make_contiguous().sort_by(|a,b| a.2.cmp(&b.2))`，按 flags 排序。

系统意义：把 flags 当优先级重排等待者。

### `ResourceLimits::default_limits`，6284 行

实现：设置默认 fd、线程、栈、数据段、文件大小、映射数量、CPU 时间限制。

系统意义：进程资源限制。

### `check_fd` / `check_threads` / `check_stack` / `check_data` / `check_filesize` / `check_mappings`，6296-6301 行

实现：逐项和对应上限比较。

系统意义：资源使用前检查是否超限。

### `inherit`，6303 行

实现：复制所有资源限制字段。

系统意义：fork 时子进程继承限制。

### `set_limit`，6315 行

实现：

- resource 0 设置 cpu_time_limit；
- 1 设置 max_file_size；
- 2 设置 max_data_size；
- 3 设置 max_stack_size；
- 7 设置 max_fds；
- 其他 einval。

系统意义：setrlimit 的简化模型。

### `get_limit`，6326 行

实现：按 resource 返回对应限制，不支持则 einval。

系统意义：getrlimit 的简化模型。

### `exceeds_any`，6337 行

实现：检查 fds、threads、stack 三项是否超过上限，任一超过返回 true。

系统意义：快速综合检查。

### `bitwise_merge`，6347 行

实现：`(a & !mask) | (b & mask)`。

系统意义：按 mask 从 b 取位合并到 a。

### `rotate_bits`，6351 行

实现：

- width 为 0 或 >64 返回原值；
- amount 对 width 取模；
- 计算 width 位 mask；
- 在 mask 范围内循环左移。

系统意义：固定位宽 bit rotate。

### `popcount64`，6360 行

实现：用并行位计数技巧统计 1 的个数。

系统意义：高效 bit count。

### `clz64`，6367 行

实现：通过分段左移统计前导 0 个数。

系统意义：计算最高位位置。

### `ffs64`，6380 行

实现：

- v 为 0 返回 None；
- `v & -v` 取最低 set bit；
- 用 `63 - clz64(...)` 得到 bit index。

系统意义：find first set。

### `align_up` / `align_down`，6385-6390 行

实现：

- align 必须是 2 的幂，否则返回原地址；
- align_up 使用 `(addr + align - 1) & !(align - 1)`；
- align_down 使用 `addr & !(align - 1)`。

系统意义：地址对齐。

### `is_power_of_two` / `log2_floor`，6395-6399 行

实现：

- power of two 检查 `v != 0 && (v & (v-1)) == 0`；
- log2_floor 用 leading_zeros。

系统意义：buddy allocator 和对齐常用工具。

### `hash_combine` / `murmurhash3_finalize`，6404-6408 行

实现：

- hash_combine 用 boost 风格公式混合 seed/value；
- murmurhash3_finalize 做 xor-shift 和乘法 avalanche。

系统意义：hash 辅助。

### `BuddyAllocator::new`，6427 行

实现：

- 创建 `max_order + 1` 个 free list；
- `order = log2_floor(total_pages)`，再截到 max_order；
- 先用最大 usable_order 的块覆盖尽量多页面；
- 剩余页面按更小 order 从大到小塞进 free_lists；
- allocated=0。

系统意义：初始化 buddy 分配器，把一段连续内存拆成 2^order 块。

### `alloc_order`，6459 行

实现：

- order 大于 max_order 返回 None；
- 从 requested order 往上找第一个非空 free list；
- pop 一个大块；
- 如果块比请求大，不断拆分：
  - current_order--；
  - buddy 地址 = addr + 2^current_order * PAGE_SZ；
  - buddy 放回对应 free list；
- allocated 加 `1 << order`；
- 返回 addr。

系统意义：buddy 分配的经典 split 过程。

### `free_order`，6477 行

实现：

- order 越界返回；
- 从当前 addr/order 开始；
- 计算 buddy_addr = current_addr ^ block_size；
- 如果 buddy 在同 order free list 里，移除 buddy，并把当前块合并成更高 order；
- 找不到 buddy 就停止；
- 把最终块放入 free list；
- allocated 减 `1 << order`。

系统意义：buddy 释放的经典 coalesce 过程。

### `free_pages_count`，6496 行

实现：遍历每个 order 的 free list，累加 `list.len() * 2^order`。

系统意义：统计空闲页数。

### `largest_free_order`，6504 行

实现：从 max_order 逆序找第一个非空 free list，返回 order。

系统意义：查询最大连续空闲块大小。

### `fragmentation_score`，6511 行

实现：

- total_free 为 0 返回 0；
- largest_block = 2^largest_order；
- 如果 total_free <= largest_block，碎片分数 0；
- 否则 `(total_free - largest_block) * 100 / total_free`。

系统意义：估算碎片程度。

### `snapshot`，6520 行

实现：clone free_lists，复制 max_order/base_addr/total_pages，allocated 用当前值重新建 AtomicUsize。

系统意义：复制 allocator 状态用于调试或测试。

## 17. CR 时可以主动指出的实现细节

这些不是让你否定自己的代码，而是展示你真的读懂了：

1. `VmRegion` 是必要抽象，但 `VmMap::remove_range` 和 `AddrSpace::split_region` 目前没有完整保留左右半段，真实 OS 需要更精细 split/merge。
2. `GKL::leave` 当前已经用 thread-local depth 防止没持锁的线程释放全局锁，这是 advanced 调试里的关键修复点。
3. `FramePool::get` 和 `get_inner` 分层是为了避免已持有 GKL 时再次阻塞拿锁。
4. `Channel::recv` 的正确点是睡眠前释放 `guard`，否则 sender 可能无法推进。
5. `BlockCache::sync_all` 和 `Kernel::tick` 用 try lock，是为了避免全局维护路径造成死锁链。
6. `dispatch_syscall` 里有些分支只是参数检查或模拟返回，比如 `SYS_DUP`、`SYS_FORK`、`SYS_EXEC`，更完整逻辑在 `Task`/`Kernel::do_*` helper。
7. `SYS_SIGACTION` 当前对 SIGKILL/SIGSTOP 的判断和真实语义相反，是很适合作为重构点讲的例子。
8. `FHandle` 把内容和 offset 都放在一起，而 MemFS 重构把 inode 内容和 FileHandle offset 拆开，更符合 rCore/VFS 思想。

## 18. 这份文档和总览文档怎么配合用

`KERNEL_RS_CR_GUIDE.md` 适合先读，它回答“这个 kernel.rs 整体在做什么、每个模块在系统中是什么角色”。

`KERNEL_RS_FUNCTION_IMPL_GUIDE.md` 适合被问到具体函数时查，它回答“这个函数代码里具体怎么实现、锁怎么拿、字段怎么改、什么条件返回错误”。

CR 前建议按这个顺序准备：

1. 先用总览文档讲 5 分钟整体结构：`Kernel -> Task -> fd table -> VmMap -> FramePool -> BlockCache -> syscall`。
2. 再重点背本文件里的 8 个问题：GKL、VmRegion、FramePool、Channel、BlockCache、dispatch_syscall、FHandle、AddrSpace。
3. 如果助教问具体函数，就按函数名在本文件搜索，例如搜 `SYS_OPEN`、`FramePool::get`、`VmMap::remove_range`。
4. 如果助教问“哪里可以重构”，优先说第 17 节列出来的点。
