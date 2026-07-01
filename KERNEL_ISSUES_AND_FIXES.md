# kernel.rs 明显问题与修改方案整理

这份文档用于 Code Review 复习，重点不是说 `kernel.rs` 完全不能运行，而是指出它作为一个 rCore / OS 风格内核模拟器时，哪些地方设计不清楚、容易出错、或者明显只是为了测试而写。每一条都按照“问题是什么、为什么不合理、最小修法、理想设计”来整理。

## 一、同步与锁相关

### 1. Spin 没有 RAII guard

位置：`Spin::acquire/release`

问题：`Spin` 现在是手动调用：

```rust
spin.acquire();
...
spin.release();
```

如果中间出现 `return`、`panic`，或者后续改代码时忘记释放锁，锁就会永远保持 held 状态，其他线程会一直 spin。

最小修法：增加一个 guard 类型：

```rust
pub struct SpinGuard<'a> {
    lock: &'a Spin,
}

impl Spin {
    pub fn lock(&self) -> SpinGuard<'_> {
        self.acquire();
        SpinGuard { lock: self }
    }
}

impl Drop for SpinGuard<'_> {
    fn drop(&mut self) {
        self.lock.release();
    }
}
```

然后调用方写成：

```rust
let _guard = spin.lock();
```

这样出了作用域自动释放。

理想设计：内核里所有手动 acquire/release 的小锁都应该逐渐改成 RAII guard，减少死锁和漏解锁风险。

### 2. SyncQueue::wait_timeout 需要只删除当前线程

位置：`SyncQueue::wait_timeout`

问题：等待队列如果 timeout 后按 key 批量删除 waiter，会误删其他还在等待同一个 key 的线程。

最小修法：记录当前线程 id，timeout 醒来后只删除 `(key, current_thread)` 这一项。当前代码已经朝这个方向改了：

```rust
if let Some(pos) = q.iter().position(|(k, t, _)| {
    *k == key && t.id() == current_id
}) {
    q.remove(pos);
    false
} else {
    true
}
```

含义是：如果自己还在队列里，说明大概率是 timeout 自己醒来，需要删掉自己并返回 false；如果自己已经不在队列里，说明被 wake 方移除了，返回 true。

理想设计：等待接口最好带 predicate，醒来后重新检查条件，避免 spurious wakeup 导致错误返回。

### 3. WaitQueue keyed wait 的 wake/sleep 语义要统一

位置：`WaitQueue::sleep/sleep_timeout/wake_one/wake_all`

问题：`sleep` 直接 park，没有 predicate；如果 signal 发生在 push 前后，很容易出现 lost wakeup 或 spurious wakeup 语义不清楚。`sleep_timeout` 的返回值现在只能表达“是否被别人从队列移除”，不能表达“等待条件是否真的满足”。

最小修法：把 `sleep_timeout` 的语义写清楚，返回值只表示 wake 来源；调用方醒来后必须重新检查条件。

理想设计：改成类似：

```rust
wait_event_timeout(key, timeout, || condition_is_ready())
```

进入睡眠前和醒来后都检查条件。

## 二、内存管理相关

### 4. FramePool 的 frame index 和 physical address 容易混用

位置：`FramePool::get_inner`、`frame_alloc`、`frame_dealloc`

问题：`get_inner()` 返回的是 frame index，比如 `3` 表示第 3 个页帧；`frame_alloc()` 返回的是 physical address，比如 `MEM_OFF + 3 * PAGE_SZ`。两者都是 `usize`，编译器无法区分。

错误例子：

```rust
let idx = pool.get_inner().unwrap();
frame_dealloc(&pool, idx);
```

这里 `idx` 是编号，但 `frame_dealloc` 需要地址，释放会失败。

最小修法：新增集中转换函数：

```rust
fn frame_index_to_phys_addr(frame_index: usize) -> Option<usize>
fn phys_addr_to_frame_index(addr: usize) -> Option<usize>
```

现在所有转换尽量走 helper，不再到处手写 `MEM_OFF + idx * PAGE_SZ`。

理想设计：引入 newtype：

```rust
struct FrameIndex(usize);
struct PhysAddr(usize);
```

让编译器强制区分页帧编号和物理地址。

### 5. FramePool 中 true 表示空闲，容易反直觉

位置：`FramePool::slots`

问题：`slots[i] = true` 表示空闲，`false` 表示已占用。这个语义可以工作，但阅读时容易误解为 true 表示 used。

最小修法：至少把变量名写成 `is_free`，并在注释里明确：

```rust
slots: Mutex<Vec<bool>>, // true means free
```

理想设计：用枚举或 bitmap wrapper：

```rust
enum FrameState { Free, Used }
```

### 6. SlabEntry::slab_free 计算了 _dup 但没有使用

位置：`SlabEntry::slab_free`

问题：`free_list` 存放空闲 slot 的 offset。如果同一个 offset 被 free 两次，就会在 free_list 里出现两遍，后续可能把同一个 slot 分配给两个对象。

当前逻辑：

```rust
let _dup = self.free_list.iter().any(|&s| s == offset);
self.free_list.push_back(offset);
```

`_dup` 只是算了，但没有阻止 double free。

最小修法：

```rust
let dup = self.free_list.iter().any(|&s| s == offset);
if dup {
    return;
}
self.free_list.push_back(offset);
```

理想设计：维护 allocated bitmap，free 时必须检查这个 slot 当前确实处于 allocated 状态。

### 7. KStk 手动管理 Box 原始指针

位置：`KStk::new`、`Drop for KStk`

问题：`KStk` 用：

```rust
let ptr = Box::into_raw(v) as *mut u8 as usize;
```

然后在 Drop 里：

```rust
Box::from_raw(std::slice::from_raw_parts_mut(...))
```

这属于手动内存管理。如果 `KStk(usize)` 里的地址被错误复制、破坏，或者未来手写 Clone，就可能 double free 或释放非法地址。

最小修法：让 `KStk` 直接保存 `Box<[u8]>`，top 地址临时计算：

```rust
pub struct KStk {
    data: Box<[u8]>,
}
```

理想设计：如果必须暴露地址，使用 wrapper 保存所有权，同时禁止 Clone，避免裸 `usize` 代表拥有的内存。

### 8. AddrSpace::fork_from writable region ref_up 两次

位置：`AddrSpace::fork_from`

问题：函数里遍历了两次 parent regions，对 writable region 都调用了 `region.ref_up()`。这会导致引用计数比实际共享者更多。

最小修法：保留一次 ref_up，删除第二个循环。

理想设计：fork 时明确区分 region 元数据引用计数、物理页引用计数、COW 页引用计数，不能混在一起。

### 9. AddrSpace::split_region 只 push 第二段，没有缩短原 region

位置：`AddrSpace::split_region`

问题：假设原 region 是 `[1000, 2000)`，在 `1500` split。当前代码只新增 `[1500, 2000)`，但原来的 `[1000, 2000)` 还在，于是两个 region 重叠。

最小修法：找到 region 的下标，使用 `VmRegion::split_at` 得到左右两段，用左右两段替换原 region。

伪代码：

```rust
let idx = find_index(addr)?;
let (left, right) = self.vm_map.regions[idx].split_at(addr).ok_or("einval")?;
self.vm_map.regions.remove(idx);
self.vm_map.regions.insert(idx, right);
self.vm_map.regions.insert(idx, left);
```

理想设计：所有 split/remove/protect 操作都由 `VmMap` 统一维护排序和不重叠约束。

### 10. AddrSpace 里 page_table_root 基本没有真正参与翻译

位置：`AddrSpace`

问题：`AddrSpace` 有 `page_table_root`，但大部分地址翻译还是用 `p2v/v2p` 的固定 offset 模型。也就是说它没有真正实现页表 walk、PTE 权限检查、TLB 刷新。

最小修法：注释里明确这是模拟字段，不是真页表。

理想设计：引入 PageTable 结构，`VmMap` 管区域，PageTable 管具体 VA -> PA 映射。

### 11. validate_access 和 check_access 边界不完全一致

位置：`check_access`、`check_access_rw`、`validate_access`

问题：`check_access` 允许 `end <= KERN_BASE`，但 `validate_access` 使用 `end >= KERN_BASE` 返回错误。这会导致边界地址语义不一致。

最小修法：统一半开区间语义 `[addr, end)`，用户空间最后一个合法范围应该允许 `end == KERN_BASE`。

理想设计：所有用户地址检查都调用一个统一函数，不要三处各写一套。

## 三、文件、fd 和管道相关

### 12. FHandle::read/write 的 offset 和 data 分开加锁

位置：`FHandle::read`、`FHandle::write`

问题：当前读写流程大概是：

```rust
读 offset
锁 data 读写
再写 offset
```

如果两个线程共享同一个 fd，可能同时读到同一个 offset，然后都从同一位置读写，最后 offset 更新互相覆盖。

最小修法：让 read/write 持有 `desc.write()` 覆盖整个 offset 读写过程。注意要固定锁顺序，避免和其他函数反向锁 `data -> desc`。

理想设计：把 `offset + options + data` 的并发语义重新设计清楚。真实 OS 中 dup 后共享 open file description，所以共享 offset；pread/pwrite 才不移动 offset。

### 13. FHandle::seek 负数 cast 成 u64

位置：`FHandle::seek`

问题：

```rust
FSeek::Cur(o) => (d.off as i64 + o) as u64
```

如果结果是负数，cast 到 `u64` 后会变成巨大正数。

最小修法：

```rust
let new_off = ...;
if new_off < 0 {
    return Err("einval");
}
d.off = new_off as u64;
```

理想设计：同时检查超过文件大小、超过 `usize::MAX`、以及不同 whence 的合法性。

### 14. PipeNode 没有容量限制，也没有真正阻塞等待

位置：`PipeNode::read_at/write_at`

问题：写端无限 push 到 `VecDeque`，读端没数据时返回 `"again"`。这更像非阻塞 pipe 的简化模型，不像真实阻塞 pipe。

最小修法：增加容量，比如 `PIPE_CAP`；写满时返回 `again` 或 park；读空时根据 nonblock 决定返回 `again` 还是等待。

理想设计：PipeBuf 内部有 read_wait 和 write_wait 两个 WaitQueue，读写端通过条件变量/等待队列互相唤醒。

### 15. Task::add_file 不是原子的

位置：`Task::add_file`

问题：它先调用 `get_free_fd()`，释放锁后再重新 lock 插入。两个线程并发 open 时，可能都拿到同一个 fd。

当前结构：

```rust
let fd = self.get_free_fd();
self.files.lock().unwrap().insert(fd, fl);
```

最小修法：一次 lock 内完成寻找和插入：

```rust
let mut files = self.files.lock().unwrap();
let fd = (0..).find(|i| !files.contains_key(i)).unwrap();
files.insert(fd, fl);
fd
```

理想设计：封装 `FdTable`，所有 fd 分配、dup、close 都在同一个结构里完成。

### 16. Task::dup_fd 也有类似非原子 fd 分配问题

位置：`Task::dup_fd`

问题：它先锁一次拿旧 fd，再锁一次找新 fd，最后第三次锁插入。并发场景下同样可能抢同一个新 fd。

最小修法：用一次 `files` lock 完成旧 fd 获取、新 fd 查找、插入。

理想设计：同第 15 条，抽出 FdTable。

## 四、挂载、缓存、磁盘和 IO 队列

### 17. MountTable::resolve 没有路径组件匹配和递归深度限制

位置：`MountTable::resolve`

问题一：前缀匹配是字节前缀，`/dev` 可能错误匹配 `/device`。

问题二：`resolve` 递归调用 `self.resolve(rest)`，没有深度限制，也没有 mount cycle 检查。错误配置可能无限递归。

最小修法：匹配时要求：

```rust
path == prefix || path.starts_with(prefix + "/")
```

并增加最大递归深度，比如 32。

理想设计：mount table 用规范化路径作为 key，resolve 时按路径组件查找 longest prefix，记录 visited 防环。

### 18. Kernel::lookup_path 计算了 canonical 但没有使用

位置：`Kernel::lookup_path`

问题：函数里构造了 `_canonical`，但最终传给 `self.mnt.resolve(path)` 的还是原始 path。

最小修法：

```rust
let canonical = ...;
let resolved = self.mnt.resolve(&canonical)?;
```

理想设计：路径解析应该统一处理 cwd、`.`、`..`、重复 `/`、mount、symlink，而不是各处临时处理。

### 19. IoQueue::submit_batch 有自锁死锁风险

位置：`IoQueue::submit_batch`

问题：函数持有 `self.pending.lock()` 时调用 `self.merge_adjacent()`；而 `merge_adjacent()` 又会 lock 同一个 `pending`。标准 `Mutex` 不可重入，这会死锁。

最小修法：在调用 `merge_adjacent` 前释放 q：

```rust
let need_merge = q.len() > IOQUEUE_DEPTH;
drop(q);
if need_merge {
    self.merge_adjacent();
}
```

理想设计：把 merge 逻辑拆成接收 `&mut VecDeque<IoRequest>` 的内部 helper，避免重复加锁：

```rust
fn merge_adjacent_locked(q: &mut VecDeque<IoRequest>) -> usize
```

### 20. BlockCache::fetch 需要避免睡眠时持有 chain lock

位置：`BlockCache::fetch`

问题：缓存 miss 后如果持有 chain lock 去模拟磁盘 latency，会阻塞同一个 chain 的所有操作，甚至和 GKL/其他锁形成锁链死锁。

当前较合理的修法：先查 cache，miss 后释放 chain lock，sleep / load block，回来后再次锁 chain，二次检查是否已有其他线程插入。

关键代码：

```rust
if let Some(existing) = items.iter().find(|slot| slot.id == block_id) {
    existing.payload.clone()
} else {
    items.push(CacheSlot { ... });
    block_data
}
```

理想设计：cache slot 有 loading 状态，多个线程 miss 同一 block 时只让一个线程真的读磁盘，其他线程等待 loading 完成。

### 21. Disk::read_block 在 persistent error 下可能无限 retry

位置：`Disk::read_block`

问题：如果 `errs == usize::MAX`，`consume_transient_error` 不会减少错误数，`read_block` 会无限循环。

最小修法：给 `read_block` 增加最大重试次数，或者遇到 persistent error 直接返回 Err。

理想设计：区分 transient error 和 permanent error，读接口返回具体错误，调用者决定是否 retry。

### 22. Disk::write_block 不保存数据

位置：`Disk::write_block`

问题：`write_block` 成功时只返回 Ok，没有把 `_data` 保存到任何地方；`read_block` 也只是生成 deterministic pattern。因此这个 Disk 是测试模拟器，不是真磁盘。

最小修法：给 Disk 增加：

```rust
blocks: Mutex<BTreeMap<usize, Vec<u8>>>
```

write 保存，read 优先读保存的数据。

理想设计：分层成 BlockDevice trait，MemDisk / ErrorDisk / JournalDisk 分别实现。

## 五、调度与任务管理

### 23. SchedulePolicy::with_prio 负数 prio 可能溢出

位置：`SchedulePolicy::with_prio`

问题：

```rust
time_slice: 20 - prio as usize
```

如果 `prio = -1`，`prio as usize` 会变成一个巨大数，导致下溢。

最小修法：

```rust
let bounded = prio.clamp(-20, 19);
let time_slice = (20i32 - bounded).max(1) as usize;
```

理想设计：nice/prio 到 time_slice/weight 的转换用独立函数，带边界测试。

### 24. RunQueue::enqueue 计算 _dup 但不用

位置：`RunQueue::enqueue`

问题：同一个 task 可以重复入队，之后调度器可能多次调度同一个任务。

最小修法：

```rust
if q.iter().any(|(id, _)| *id == task_id) {
    return;
}
```

理想设计：runqueue 用 `BTreeMap<TaskId, SchedEntity>` 或者同时维护 membership set。

### 25. RunQueue::rebalance 不是真正 CFS

位置：`RunQueue::rebalance`

问题：它用全局 tick 给所有任务增加 vruntime，而真实 CFS 是只给实际运行过的任务按运行时间记账。

最小修法：重命名为 `age_all_tasks` 或注释说明是模拟。

理想设计：每个 task 记录 `last_start_time` 和 `delta_exec`，只有当前运行任务更新 vruntime。

### 26. TaskTable::fork_task 把 child 加入 parent.subtasks 两次

位置：`TaskTable::fork_task`

问题：

```rust
src.subtasks.lock().unwrap().push(tgt.clone());
...
src.subtasks.lock().unwrap().push(tgt.clone());
```

同一个 child 会出现两遍，wait/reap 逻辑可能重复处理。

最小修法：删除其中一次。

理想设计：`link_parent/link_child` 封装父子关系维护，并防重复。

### 27. Task::send_sig 计算 dup 但不使用

位置：`Task::send_sig`

问题：代码计算了：

```rust
let dup = sq.iter().any(...)
```

但仍然 push。对于普通 pending signal，真实 OS 通常会合并相同非实时信号。

最小修法：如果该模拟希望合并普通信号，则：

```rust
if dup {
    return;
}
```

理想设计：区分普通 signal 和 realtime signal，普通信号 coalesce，实时信号排队。

### 28. Kernel::spawn_thread 没有真正运行任务上下文

位置：`Kernel::spawn_thread`

问题：它只是反复 begin_run/end_run/yield，不会执行用户代码，也不会根据 Context 做真实上下文切换。

最小修法：注释说明这是 host thread 上的模拟循环。

理想设计：如果要在 QEMU 上跑，需要 trapframe、switch.S、页表切换、中断返回，而不是 Rust host thread。

## 六、系统调用分发

### 29. dispatch_syscall 过大，职责混杂

位置：`Kernel::dispatch_syscall`

问题：一个 match 里塞进 read/write/open/mmap/fork/signal/futex 等大量逻辑，很多只是模拟返回值，不利于测试和维护。

最小修法：按模块拆 helper：

```rust
sys_read(...)
sys_write(...)
sys_open(...)
sys_mmap(...)
sys_fcntl(...)
```

理想设计：系统调用层只做参数解析和用户内存拷贝，具体逻辑交给 fs/vm/task/ipc 子系统。

### 30. SYS_READ / SYS_WRITE 没有真正操作 fd table

位置：`SYS_READ`、`SYS_WRITE`

问题：它们主要检查地址，然后用 block cache 估算返回长度；没有通过当前 task 的 fd table 找到 `FLike`，也没有真正把数据 copy 到用户 buffer。

最小修法：取当前 task，`get_file(fd)`，调用 `FLike::read/write`。

理想设计：先 `copy_from_user/copy_to_user`，再调用 File trait。

### 31. SYS_OPEN 忽略真实路径

位置：`SYS_OPEN`

问题：它检查 `path_addr`，但没有从用户空间读取字符串，最后创建的是：

```rust
FHandle::new("anon", ...)
```

最小修法：在模拟环境里至少从测试传入的路径表或 mock user memory 解析 path，并传给 `lookup_path`。

理想设计：VFS lookup -> inode -> open file handle -> fd table。

### 32. SYS_CLOSE 没有关闭当前 task 的 fd

位置：`SYS_CLOSE`

问题：它清理了 block cache 里的 slot，但没有调用 `Task::close_fd(fd)`，所以 fd table 里的文件还在。

最小修法：

```rust
if let Some(t) = self.cur_task(0) {
    t.close_fd(fd)?;
}
```

理想设计：close 应处理引用计数、pipe 端关闭、epoll 注册清理、cloexec 等关联状态。

### 33. SYS_DUP 返回新 fd 但没有插入 fd table

位置：`SYS_DUP`

问题：它只计算了 candidate，然后 `Ok(new_fd)`，没有实际复制文件对象。

最小修法：

```rust
let new_fd = t.dup_fd(old_fd, false)?;
Ok(new_fd)
```

理想设计：dup/dup2/dup3 共用 fd table helper，保证原子性。

### 34. SYS_FCNTL 的 F_SETFD / F_GETFD 没有真实操作 cloexec

位置：`SYS_FCNTL`

问题：`F_SETFD` 只是计算 `_cloexec` 后返回 Ok；`F_GETFD` 查的是 cache dirty bit，不是 fd 的 close-on-exec 状态。

最小修法：

```rust
F_SETFD => t.set_cloexec(fd, (arg & FD_CLOEXEC) != 0)
F_GETFD => 从 fd table 取 file.cloexec
```

理想设计：cloexec 应属于 fd entry，而不是只存在 `FHandle`，因为同一个 file object 可以被不同 fd 以不同 cloexec 打开。

### 35. SYS_MMAP / SYS_MUNMAP 没有更新 VmMap

位置：`SYS_MMAP`、`SYS_MUNMAP`

问题：`SYS_MMAP` 计算返回地址，但没有向当前 task 的地址空间插入 VmRegion；`SYS_MUNMAP` 也只是循环计算 `_va`，没有 unmap。

最小修法：当前 task 需要有 `AddrSpace` 或 `VmMap`，mmap 插入 region，munmap 调 `remove_range`。

理想设计：mmap 要同时处理 VMA、页表懒分配、文件映射、匿名映射、权限位。

### 36. SYS_BRK 把 brk 存进 vm_token，语义混乱

位置：`SYS_BRK`

问题：`vm_token` 名字像地址空间 token，但这里被用来保存 brk 地址。一个字段承担两个语义。

最小修法：给 Task 增加明确字段：

```rust
brk: AtomicUsize
```

理想设计：brk 属于 `AddrSpace.vm_map.brk`，不应该在 task 上另存一份。

### 37. SYS_EPOLL_WAIT 没有真的等待

位置：`SYS_EPOLL_WAIT`

问题：它计算 timeout 和 deadline，但最终直接 `Ok(0)`，没有检查 epoll ready list，也没有 park。

最小修法：从 task 的 epoll 实例读取 ready events；如果没有 ready 且 timeout > 0，再用等待队列 sleep。

理想设计：File/Pipe/Socket 的 readiness 变化通过 EvBus/WaitQueue 通知 epoll。

### 38. SYS_CLOCK_GETTIME 没有写回用户 timespec

位置：`SYS_CLOCK_GETTIME`

问题：计算了 `secs` 和 `nsecs`，但没有通过 `ctu` 或 user memory 写到 `tp_addr`。

最小修法：构造 timespec，调用 copy_to_user 模拟函数。

理想设计：真实 user pointer copy，错误处理 EFAULT。

### 39. SYS_SIGACTION 逻辑反了

位置：`SYS_SIGACTION`

问题：当前逻辑：

```rust
if signo != SIGKILL && signo != SIGSTOP {
    return Err("einval");
}
```

这表示只允许给 SIGKILL/SIGSTOP 设置 handler。真实 OS 正好相反：SIGKILL 和 SIGSTOP 不能被捕获、忽略或改 handler，普通信号才可以。

最小修法：

```rust
if signo == SIGKILL || signo == SIGSTOP {
    return Err("einval");
}
```

理想设计：Task 保存 signal action table，`sigaction` 读写 action，并返回 old action。

### 40. SYS_FUTEX 没有使用 FutexBucket/FutexTable

位置：`SYS_FUTEX`

问题：WAIT/WAKE/REQUEUE 分支基本只做地址检查和返回估算值，没有真正把线程加入 futex wait queue。

最小修法：WAIT 调 `get_futex(uaddr).wait(...)`，WAKE 调 `wake(...)`。

理想设计：实现 futex word 检查、timeout、private/shared key、requeue、PI 等语义的子集。

## 七、信号、能力和资源

### 41. SigSet 高位信号可能处理不完整

位置：`SigSet`、`coalesce_pending`

问题：`NSIG = 64`，但有些地方用 `u32` 保存 pending 结果，会丢失 32 以上的信号位。

最小修法：统一用 `u64` 表示 signal set。

理想设计：封装 `SigSet(u64)`，所有位操作都通过方法。

### 42. CapSet::inherit 语义可能反了

位置：`CapSet::inherit`

问题：如果实现成 `parent.bits & !INHERITABLE_MASK`，语义像是“把可继承位清掉”，而不是“只继承可继承位”。

最小修法：确认常量语义。如果 `INHERITABLE_MASK` 表示可继承集合，应该是：

```rust
parent.bits & INHERITABLE_MASK
```

理想设计：明确 effective/permitted/inheritable 三套 capability 集合。

### 43. ResourceLimits::exceeds_any 使用 > 而不是 >= 需要确认语义

位置：`ResourceLimits::exceeds_any`

问题：`check_fd` 用 `current < max_fds`，但 `exceeds_any` 用 `fds > max_fds`。如果已经等于 max，前者不允许再开，后者不认为超限。

最小修法：统一成 `>=` 或 `>`，并注明 limit 是最大允许数量还是下一个申请前的门槛。

理想设计：资源检查统一通过 `can_allocate_*` 接口。

## 八、代码结构层面的重构建议

### 44. kernel.rs 文件过大

问题：一个文件超过 7000 行，包含锁、VM、FS、Disk、Task、Syscall、Signal、Timer、算法工具函数等多个子系统。读代码时很难建立模块边界。

最小修法：先不改行为，只按模块拆文件：

```text
sync.rs
vm.rs
frame.rs
fs.rs
pipe.rs
block.rs
task.rs
syscall.rs
signal.rs
timer.rs
```

理想设计：每个模块只暴露必要 public API，隐藏内部字段。

### 45. 大量 _dummy / _audit / _cost 变量只计算不使用

问题：很多变量像 `_audit`、`_vmap_cost`、`_fragmentation`、`_resolved` 只计算不影响结果。这些代码会干扰阅读，让人误以为有真实逻辑。

最小修法：删除完全无行为影响的代码，或者改成真正的检查逻辑。

理想设计：模拟逻辑和真实状态变化分开，测试辅助代码不要混在核心路径里。

### 46. 公开字段过多，封装不足

问题：大量 struct 字段都是 `pub`，外部可以绕过方法直接修改内部状态，比如 `FramePool::slots`、`BlockCache::chains`、`Task::files`。

最小修法：逐步把字段改成 private，提供方法访问。

理想设计：模块边界清晰，外部只能通过安全 API 修改状态。

## 九、CR 中可以重点讲的总结

如果助教问“你怎么判断这些是不合理的”，可以按下面这条主线回答：

1. 我先看每个结构体维护的核心不变量。例如 `FramePool` 的不变量是一个 frame 不能同时 free 和 allocated；`RunQueue` 的不变量是同一个 task 不应该重复入队；`FdTable` 的不变量是一个 fd 只能对应一个 file entry。

2. 然后看函数有没有破坏这些不变量。例如 `slab_free` 不防 double free，会破坏 slot 唯一性；`Task::add_file` 分配 fd 和插入 fd 分两次锁，会破坏 fd 唯一性；`RunQueue::enqueue` 不用 `_dup`，会破坏队列唯一性。

3. 再看锁的持有范围。比如 `IoQueue::submit_batch` 持有 `pending` 时调用另一个会再次锁 `pending` 的函数，这是直接死锁；`BlockCache::fetch` 如果睡眠时持有 chain lock，就会扩大临界区，引起锁链阻塞。

4. 最后看模拟 OS 和真实 OS 的差距。比如 `SYS_OPEN` 没读用户路径，`SYS_READ/WRITE` 没走 fd table，`SYS_MMAP` 没改 VmMap，说明这些是 syscall 外壳，不是真完整实现。

这份代码比较适合的 refactor 方向不是一次性改成真实 OS，而是先把明显 bug 修掉，再把模块边界拆清楚，把“模拟返回值”和“真实状态改变”分开。
