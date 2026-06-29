# kernel.rs Code Review 讲解文档

本文档用于 Code Review 时解释 `kernel/src/kernel.rs`。目标不是背代码，而是能说明：

- 每个大模块在模拟什么系统功能；
- 重要结构体的字段分别表示什么；
- 关键函数为什么这样写；
- 如果删掉某个抽象，会带来什么问题；
- 当前实现和真实现代操作系统之间有什么差距。

代码文件：

```text
kernel/src/kernel.rs
```

当前文件约 6500 行，是一个把同步、内存、文件、进程、调度、系统调用、IPC、缓存和工具函数都放在一起的教学型 kernel 模型。

## 0. 代码行号索引

CR 现场如果被问到某个设计，建议先用这个索引定位：

```text
1-169       use、常量、syscall 编号、全局配置
171-208     PgFrame、VmFlags、FdEnt、TimerEnt 等基础结构
209-331     KernLock / GKL，大内核锁和可重入深度
333-442     ZoneInfo、CircBuf、Spin、EvFlag、EvBus、SlabEntry、SockAddr
443-588     SyncQueue，park/unpark 等待队列
589-762     Sema、FutexTable、p2v/v2p/k_off 地址转换
763-1431    VmRegion、VmMap、FramePool、SharedPage、KStk、堆检查
1432-1806   环形缓冲、slab、ELF、网络、auxv、权限辅助函数
1807-2466   FHandle、PipeNode、FLike、EpInst、Channel
2467-3245   PageCache、BlockCache、MountTable、IoQueue、Disk
3246-4029   SysV IPC、capability、signal、timer、trap、clock
4032-4702   RunQueue、Task、TaskTable
4704-5894   Kernel 结构体、普通方法、dispatch_syscall
5895-6529   AddrSpace、ProcessGroup、WaitQueue、ResourceLimits、BuddyAllocator
```

`dispatch_syscall` 里常见系统调用的大致位置：

```text
SYS_READ 4840       SYS_WRITE 4871      SYS_OPEN 4902
SYS_CLOSE 4969      SYS_STAT/FSTAT 4990 SYS_MMAP 5006
SYS_MUNMAP 5040     SYS_BRK 5051        SYS_IOCTL 5078
SYS_PIPE 5112       SYS_DUP 5131        SYS_DUP2 5145
SYS_FORK 5164       SYS_EXEC 5187       SYS_EXIT 5208
SYS_WAIT4 5230      SYS_KILL 5318       SYS_FCNTL 5366
SYS_GETPID 5423     SYS_GETPPID 5430    SYS_SETPGID 5443
SYS_GETPGID 5467    SYS_SETSID 5481     SYS_EPOLL_CREATE 5495
SYS_EPOLL_CTL 5503  SYS_EPOLL_WAIT 5518 SYS_CLOCK_GETTIME 5537
SYS_SIGACTION 5563  SYS_SIGPROCMASK 5575 SYS_FUTEX 5601
```

## 1. 总体结构

可以按下面顺序理解：

```text
常量和全局配置
    -> 同步原语
    -> 虚拟内存和物理页
    -> 文件/管道/epoll/channel
    -> page cache/block cache/disk
    -> IPC/信号/定时器/陷入
    -> 调度器/任务/任务表
    -> Kernel 和 syscall 分发
    -> 地址空间、等待队列、资源限制、buddy 分配器
```

最核心的对象是：

```text
Kernel
    tasks: TaskTable
    cache: BlockCache
    pool: FramePool
    cpus: 当前 CPU 上运行的任务
    mnt: MountTable
    sem_store / shm_store: IPC 全局对象
    tty_buf: 终端输入缓冲
    disk: 模拟磁盘
```

它相当于把系统里各个子系统组合起来。

## 2. 常量区

代码开头定义了很多常量，可以按用途分组。

### 内存相关

```text
PAGE_SZ        页面大小，4096 字节
N_FRAMES       物理页帧数量
KERN_BASE      内核虚拟地址起点
PHYS_OFF       物理地址映射到内核虚拟地址时使用的偏移
MEM_OFF        模拟物理内存起点
KHEAP_SZ       内核堆大小
KSTK_SZ        内核栈大小
USR_STK_OFF    用户栈起点
USR_STK_SZ     用户栈大小
```

这些常量用于检查地址是否合法、计算页号、模拟 mmap/brk/栈布局。

### 文件和 fd 相关

```text
F_DUPFD / F_GETFD / F_SETFD / F_GETFL / F_SETFL
FD_CLOEXEC
O_NONBLOCK / O_APPEND / O_CLOEXEC
AT_NOFOLLOW
```

这些模拟 Linux `fcntl/open` flags。CR 里可以说：它们不是文件权限本身，而是“打开文件或控制 fd 行为”的标志。

### 虚拟内存 flags

```text
VM_READ
VM_WRITE
VM_EXEC
VM_SHARED
VM_GROWSDOWN
VM_DONTCOPY
VM_HUGETLB
VM_PFNMAP
```

这些用于描述一段虚拟内存区域的权限和行为。

### 进程/调度/信号/syscall

```text
PRIO_MIN / PRIO_MAX / SCHED_NORMAL ...
NSIG / SIGKILL / SIGSTOP / SIGCHLD ...
SYS_READ / SYS_WRITE / SYS_OPEN ...
```

`dispatch_syscall` 通过这些 syscall number 做 match 分发。

## 3. 同步原语和事件模型

### KernLock / GKL

位置：`KernLock`、`GKL_LOCAL_DEPTH`、`GKL`

`KernLock` 是全局内核锁，也就是 GKL。

字段：

```text
flag    AtomicBool，表示全局锁是否被持有
holder  AtomicUsize，记录逻辑 owner id，方便调试和测试
depth   AtomicUsize，记录全局重入深度
```

还有一个 thread-local：

```text
GKL_LOCAL_DEPTH
```

它记录“当前线程自己拿了几层 GKL”。这个非常重要。

关键函数：

```text
local_depth       读取当前线程本地深度
set_local_depth   设置当前线程本地深度
enter_reentrant   当前线程已经持有 GKL 时增加重入层数
spin_until_acquired  CAS 自旋直到拿到全局锁
record_owner      记录 owner，并把本线程 depth 设为 1
release_global    清空 holder/depth，并 Release 释放 flag
enter             阻塞式进入 GKL
try_enter         尝试进入 GKL，失败不阻塞
leave             释放当前线程持有的一层 GKL
held              是否有人持有锁
held_by_current   是否当前线程持有锁
owner             当前 owner id
level             全局 depth
```

CR 重点：

```text
为什么需要 thread-local depth？
```

因为全局 `flag` 只能说明“某个线程持有锁”，不能说明“当前线程持有锁”。如果只看 `flag`，其他线程可能误以为自己已经持有锁，从而绕过 GKL。

```text
为什么 leave() 里 local_depth == 0 要直接 return？
```

因为没有持有 GKL 的线程不能释放其他线程持有的锁。advanced 调试里出现过 foreign leave 场景，这就是死锁链定位的核心。

### Spin

`Spin` 是一个最简单的自旋锁。

字段：

```text
v: AtomicBool
```

函数：

```text
acquire      CAS 把 false 改成 true，失败就 spin_loop
try_acquire 只尝试一次
release     store false
is_held     查看是否被持有
```

它用于 cache chain、channel guard 等小临界区。

### EvFlag / EvBus

`EvFlag` 是事件位定义：

```text
READABLE / WRITABLE / ERROR / CLOSED
PROC_QUIT / CHILD_QUIT / RECV_SIG
SEM_RM / SEM_ACQ
```

`EvBus` 是事件总线：

```text
ev   当前事件位
cbs  回调列表
```

函数：

```text
set / clear / change 修改事件位
sub                 注册回调
cb_len              回调数量
wait_ev            忙等某个事件位出现
```

系统用途：

```text
进程退出通知
信号到达通知
pipe 可读/关闭通知
semaphore 状态变化通知
```

### SyncQueue

`SyncQueue` 是条件等待队列。

字段：

```text
waiters          等待的线程队列
epoll_waiters    epoll 注册信息
pending_signals  signal 先于 wait 发生时保存的信号数
```

关键函数：

```text
park_on       检查条件，不满足就 park，醒来后重新检查
signal        唤醒一个等待者；如果没人等，增加 pending_signals
broadcast     唤醒所有等待者
signal_n      唤醒 n 个等待者
wait_ev       等待 cond 返回 Some
wait_events   同时挂到多个 SyncQueue 上等待
reg_epoll     注册 epoll 关注关系
unreg_epoll   删除 epoll 关注关系
```

CR 重点：

```text
为什么被唤醒后要重新检查条件？
```

因为 wakeup 只表示“可能有变化”，不等于条件一定满足。否则会有 spurious wakeup bug。

### Sema / Futex

`Sema` 是 System V semaphore 的简化模型。

字段：

```text
cnt  信号量计数
pid  最近操作进程
rm   是否被 remove
bus  事件通知
```

函数：

```text
try_acquire   如果 cnt >= 1 就减一
acquire_spin  自旋直到拿到
release       cnt 加一并发送 SEM_ACQ
remove        标记删除并发送 SEM_RM
access        RAII guard，drop 时自动 release
```

`FutexBucket` / `FutexTable` 模拟用户态地址上的等待队列：

```text
wait    如果原子值等于 expected，把线程挂到 addr 上
wake    唤醒 addr 上最多 count 个线程
requeue 把 src 上的 waiter 移到 dst
```

## 4. 虚拟内存与物理页

### p2v / v2p / k_off

```text
p2v(pa)  物理地址转内核虚拟地址
v2p(va)  内核虚拟地址转物理地址
k_off    计算相对 KERN_BASE 的偏移
```

这些函数用于模拟 direct mapping。

### PgFrame

`PgFrame` 表示一个物理页帧的引用计数。

字段：

```text
rc: AtomicUsize
```

函数：

```text
up / down          引用计数加减
count              读引用计数
set                设置引用计数
cas                compare_exchange
inc_if_nonzero     如果非 0 才加引用，避免复活已释放 frame
```

系统用途：

```text
fork COW、共享页、引用计数释放
```

### VmRegion

`VmRegion` 表示一段连续虚拟地址区域。

字段：

```text
base       起始虚拟地址
len        长度
flags      VM_READ/VM_WRITE/VM_EXEC 等权限和属性
offset     映射到文件或设备时的偏移
tag        区分区域来源或类别
ref_count  区域引用计数
```

关键函数：

```text
new / with_offset  创建区域
end                返回 base + len
contains           判断地址是否在区域内
overlaps           判断两个区域是否重叠
split_at           按地址切成左右两个区域
merge_with         相邻且 flags/tag 相同则合并
ref_up/ref_down    引用计数
```

CR 重点问题：

```text
VmMap 里为什么要有 VmRegion，不直接用一个 Vec<Page> 或一个大区间？
```

答法：

地址空间不是一个连续且权限相同的大数组，而是由很多段组成：

```text
代码段: RX
数据段: RW
堆: RW，可增长
栈: RW，可能 grow down
mmap 文件: 可能 shared/private，带 offset
匿名映射: RW，无文件 offset
```

每一段都有不同的 `base/len/flags/offset/tag`。`VmRegion` 就是把这些“连续且属性相同”的页合成一个区域。如果不用 `VmRegion`：

- 很难快速判断某个地址属于哪种权限；
- `mmap/munmap/mprotect` 需要逐页维护，复杂度和内存占用更高；
- 无法表达文件映射 offset；
- 很难做区域 split/merge；
- fork/COW 时也不好复制映射元信息。

所以 `VmRegion` 是虚拟内存管理的基本单位。

### VmMap

`VmMap` 是一个进程的虚拟地址空间布局。

字段：

```text
regions    按地址排序的 VmRegion 列表
brk        堆顶
mmap_base  mmap 搜索起点
```

关键函数：

```text
new             创建空地址空间
insert          插入 region，检查 overlap，并按 base 排序
find            二分查找 addr 所在 region
remove_range    删除与范围相交的 region
find_free       找一段不冲突且满足 align 的空闲地址
total_mapped    总映射字节数
clone_regions   复制 region 元数据
gap_after       某个 region 后面的空洞大小
```

### FramePool / ZoneInfo

`FramePool` 是物理页帧池。

字段：

```text
slots  Vec<bool>，true 表示空闲
cap    总页数
```

函数：

```text
get             带 GKL 保护的分配入口
get_inner       实际分配，不重复拿 GKL
get_contig      分配连续页
put             释放一页
avail           查询某页是否空闲
free_count      空闲页数量
get_zone_aware  从指定 zone 分配
batch_alloc     批量分配
```

CR 重点：

```text
为什么 get 里要判断 GKL.held_by_current()？
```

如果当前线程已经持有 GKL，再阻塞式拿 GKL 会自锁。所以 `get` 外层负责锁语义，`get_inner` 只负责实际分配。

`ZoneInfo` 模拟 DMA/NORMAL/HIGHMEM 等 zone：

```text
zone_id       zone 编号
base_pfn      起始页帧号
page_count    页数
free_count    空闲数
low/high_watermark 水位线
managed       是否由 allocator 管理
```

### SharedPage / COW

`SharedPage` 模拟 copy-on-write 页。

字段：

```text
frame    当前 frame id
w        是否已经可写
pending  是否还处于 COW 待处理状态
```

`fault()` 在写 fault 时分配新页、减少旧页引用、标记 COW 解决。

### KStk / check_access / heap

`KStk` 分配内核栈，`top()` 返回栈顶，`Drop` 时释放 box。

`check_access` / `check_access_rw` 检查用户地址：

```text
不能越过 KERN_BASE
不能整数溢出
写访问可以额外检查 alignment
```

`cfu/ctu` 是 copy-from-user / copy-to-user 的简化模型。

`heap_init/heap_grow` 模拟内核堆初始化和扩展。

## 5. 基础数据结构和工具

### CircBuf

环形缓冲区。

字段：

```text
data  底层数组
rd    逻辑读位置
wr    逻辑写位置
cap   容量
n     当前元素数
```

关键函数：

```text
new / with_pos
next_write_index / next_read_index
push / pop
len / empty / full
peek
drain_to
fill_from
remaining
```

CR 重点：

```text
为什么要 n？
```

因为环形队列里 `rd % cap == wr % cap` 既可能表示空，也可能表示满。`n` 明确记录当前元素数，避免歧义。

### SlabEntry

`SlabEntry` 模拟 slab allocator。

字段：

```text
data       一整块内存
obj_size   对齐后的对象大小
capacity   对象数量
free_list  空闲对象 offset
allocated  已分配数量
tag        类型标签
```

函数：

```text
slab_alloc / slab_free
slab_used / slab_avail
shrink
obj_at / obj_at_mut
```

### ELF / 网络 / 辅助算法

```text
validate_elf_header      检查 ELF magic、class、program header，返回入口地址
tcp_checksum             TCP checksum
parse_ipv4_header        解析 IPv4 header
build_pseudo_header      构造 TCP pseudo header
compute_inet_checksum    通用 internet checksum
compute_load_balance     根据 CPU 负载和优先级选 CPU
audit_fd_table           检查 fd table gaps/异常 fd
rehash_mount_cache       为 mount entry 做 hash
defragment_frame_pool    统计/模拟内存碎片整理
verify_page_alignment    检查地址按 order 对齐
compute_rss_watermark    根据 VmRegion 估计 RSS 水位
```

## 6. 文件、管道、epoll、channel

### FdOpt / FdState / FHandle

`FdOpt` 是打开文件选项：

```text
rd  是否可读
wr  是否可写
ap  append 模式
nb  non-blocking
```

`FdState` 是每个打开文件描述的状态：

```text
off  当前 offset
opt  FdOpt
flk  文件锁状态
```

`FHandle` 是普通文件 handle：

```text
path     路径
data     文件内容，Arc<Mutex<Vec<u8>>>
desc     offset/options，Arc<RwLock<FdState>>
pipe     是否 pipe
cloexec  exec 时是否关闭
```

关键函数：

```text
new / with_data    创建文件 handle
dup                复制 fd，共享 data 和 desc
read / read_at     读并推进 offset / 指定 offset 读
write / write_at   写并推进 offset / 指定 offset 写
seek               修改 offset
transfer           根据 dir 统一 read/write
set_len            truncate
metadata_sz        文件长度
read_entry         简化目录项读取
poll_status        poll 状态
mmap               文件映射入口
inode_ref          返回 data 引用
fallocate          预分配大小
splice_to          从一个文件搬数据到另一个文件
```

设计问题：

```text
FHandle 同时保存 data 和 offset，这和更规范的 inode/FileHandle 分层相比不够清晰。
```

这就是后来写 MemFS 时把 inode 和 file handle 拆开的原因。

### PipeNode / FLike

`PipeNode` 模拟管道一端。

```text
PipeBuf.buf  管道字节队列
PipeBuf.bus  事件总线
PipeBuf.ends 读写端数量
PipeDir      Rd 或 Wr
```

函数：

```text
pair       创建读端和写端
can_read   有数据或对端关闭
can_write  写端且对端还在
read_at    从 VecDeque 弹字节
write_at   写入 VecDeque 并设置 READABLE
poll       返回可读/可写/错误状态
```

`FLike` 是 fd table 中的统一对象：

```text
File(FHandle)
Pipe(PipeNode)
Ep(EpInst)
```

函数：

```text
dup        复制文件、管道或 epoll 对象
read       根据类型分发
write      根据类型分发
io_ctl     ioctl 分发
mmap_fl    mmap 文件
poll       统一 poll 状态
```

这体现了 Unix 的思想：

```text
fd 不只指向普通文件，也可以指向 pipe/epoll/device。
```

### EpInst

`EpInst` 模拟 epoll 实例。

字段：

```text
events   fd -> EpEvent
ready    就绪 fd 集合
new_ctl  最近修改的 fd 集合
```

`control()` 支持 ADD/MOD/DEL。

### Channel

`Channel` 是基于 `CircBuf` 和 `SyncQueue` 的 producer-consumer channel。

字段：

```text
buf    环形缓冲
guard  Spin，保护快速路径
wq     等待队列
shut   是否关闭
```

函数：

```text
recv                循环尝试 pop，没数据则 park_on
send                push 成功后唤醒 waiter
close               设置 shut 并唤醒所有 waiter
try_recv            非阻塞尝试读
send_batch          批量写
depth               当前深度
drain_all           清空并返回所有数据
remaining_capacity  剩余容量
```

CR 重点：

```text
recv 睡眠前必须释放 guard，否则会持锁睡眠导致别人无法 send/唤醒。
```

## 7. 缓存、挂载、I/O、磁盘

### PageCache

`PageCache` 是页缓存。

字段：

```text
entries     page_id -> PageCacheEntry
capacity    容量
hits/misses/evictions 统计
lru_order   LRU 顺序
```

函数：

```text
lookup        命中则更新 LRU 和 hits
insert        插入，满了 evict_lru
evict_lru     选择 pin_count == 0 的旧页
mark_dirty    标脏
writeback_all 清除所有 dirty
stats         统计
pin/unpin     防止页面被淘汰
invalidate    删除某页
flush_range   刷新范围内 dirty 页
```

### BlockCache

`BlockCache` 是块缓存，使用多条 chain 分桶。

结构：

```text
CacheSlot: id, payload, modified
CacheChain: lk + items
BlockCache: chains + width
```

关键函数：

```text
idx                 简单 block_id % width
fetch_chain_index   混合 hash 后选 chain
cached_payload      在 chain 内找 block
synthetic_block     生成模拟块数据
fetch               先查缓存，miss 后模拟读取并插入
sync_all            尝试拿 GKL 和 chain lock，清 dirty
invalidate          删除某个 block
total_entries       总缓存项
dirty_count         dirty 项数量
evict_cold          淘汰冷块
```

CR 重点：

```text
为什么 sync_all 用 try_enter/try_acquire？
```

因为 sync_all 是全局路径，如果阻塞等待 GKL 或某条 cache chain，很容易和 FramePool/scheduler 形成死锁链。跳过忙链比死等更安全。

### MountTable

`MountTable` 保存挂载点。

字段：

```text
entries: Vec<MountEntry>
```

函数：

```text
bind          添加 prefix -> target
resolve       找最长 prefix 并递归解析
unmount       删除 prefix
list_mounts   列挂载
find_mount    找匹配 path 的挂载项
mount_count   数量
has_prefix    是否存在 prefix
```

### IoQueue

`IoQueue` 模拟磁盘 I/O 调度队列。

字段：

```text
pending       请求队列
head_pos      当前磁头位置
direction_up  扫描方向
dispatched    已分发数量
merged        合并数量
```

函数：

```text
submit / submit_batch
dispatch        按类似 elevator 的距离选择请求
merge_adjacent  合并相邻块请求
depth
```

### Disk

`Disk` 是模拟磁盘设备。

字段：

```text
errs     剩余错误次数，usize::MAX 可理解为永久错误
ops      操作次数
label    磁盘名
journal  journal 磁盘
```

函数：

```text
new / failing
attach_journal
set_errs
begin_op
remaining_errors
consume_transient_error
fill_block
fill_limited_read
retry_journal
read_block
read_block_n
write_block
flush
```

CR 重点：

```text
read_block 和 read_block_n 的区别？
```

`read_block` 会一直重试直到成功；`read_block_n` 有 retry limit，达到 limit 返回 `"limit"`。

## 8. IPC、信号、timer、trap

### SemArr / SemCtx

`SemArr` 模拟 System V semaphore array。

```text
ds    元数据 SemDs
sems  多个 Sema
```

`get_or_create` 根据 key 查找或创建 semaphore array。

`SemCtx` 是进程持有的 semaphore 上下文：

```text
arrays  semid -> SemArr
undos   退出时需要回滚的 semop
```

### ShmCtx

`ShmTag`：

```text
addr   attach 地址
pages  共享页列表
```

`shm_get_or_create` 根据 key 获取共享内存对象。

`ShmCtx` 保存进程 attach 的 shared memory id。

### ProcInit

`ProcInit` 用于 exec 初始化用户栈。

字段：

```text
args
envs
auxv
```

函数：

```text
push_at      计算 args/envs/auxv 放到用户栈后新的 sp
total_size   总大小
```

### CapSet

`CapSet` 模拟 Linux capabilities。

字段：

```text
bits       拥有的能力
effective  当前生效的能力
ambient    ambient capabilities
```

函数：

```text
check / grant / drop_cap
inherit
has_any
clear_ambient / raise_ambient
```

### SigSet

字段：

```text
pending  等待处理的信号位图
blocked  被屏蔽的信号位图
actions  每个信号的处理方式
```

函数：

```text
sig_raise / sig_clear
sig_block / sig_unblock / sig_setmask
deliverable
set_action / get_action
is_ignored
clear_non_caught
```

CR 重点：

```text
SIGKILL 和 SIGSTOP 不能被 block，也不能被自定义处理。
```

### TimerEntry / TimerWheel

`TimerEntry` 表示一个定时器：

```text
deadline
interval
callback_id
active
repeat
```

`TimerWheel` 用固定大小 slot 管理 timer。

函数：

```text
add_timer
advance
cancel
active_count
```

### Context / TrapCtl

`Context` 模拟 CPU 寄存器上下文。

字段：

```text
r      通用寄存器数组
ip     指令指针
flags  状态位
```

函数：

```text
new / capture / apply
set_ip / set_sp / set_ret / set_tls
transform
syscall_args
clone_with_ret
diff
hash
reg_class
```

`TrapCtl` 模拟 trap/interrupt/page fault 控制器。

字段：

```text
active      当前是否在 handler
hw_mask     硬件中断 mask
sw_mask     软件中断 mask
nest        嵌套深度
frame       当前上下文
stack       上下文栈
irq_on      IRQ 开关
suppressed  是否抑制
```

关键函数：

```text
configure
hw / sw
in_handler
dispatch
current
handle_irq
on_pgfault
dispatch_vector
push_frame / pop_frame
suppress / unsuppress
```

`on_pgfault` 当前语义：

```text
内核地址 >= KERN_BASE 返回 fault
用户地址解析 page 和 offset 后 Ok
```

## 9. 时钟和调度

### CLK / CLK_ALL

```text
CLK      CPU0 tick
CLK_ALL  所有 CPU 总 tick
```

函数：

```text
wclk      读 CLK
cclk      读 CLK_ALL
dtk       tick 增加
up_ms     uptime 毫秒
tmr       timer tick wrapper
ser       串口回车转换行
```

### SchedulePolicy

字段：

```text
policy      调度策略
prio        优先级
nice        nice 值
time_slice  时间片
vruntime    虚拟运行时间
```

`weight()` 根据 nice 返回类似 CFS 权重。

### RunQueue

字段：

```text
queue          runnable tasks
current        当前任务 id
preempt_count  禁抢占计数
```

函数：

```text
enqueue / dequeue / pick_next
rebalance
set_current / clear_current
remove
update_vruntime
preempt_disable / preempt_enable / preemptible
boost_priority
yield_current
```

系统用途：

```text
模拟按优先级/vruntime 选择下一个任务。
```

## 10. Task 和 TaskTable

### Task

`Task` 是进程/线程的主要对象。

重要字段：

```text
info        id/tag/status/fds
parent      父任务
subtasks    子任务
files       fd table: fd -> FLike
cwd         当前工作目录
exec_path   当前执行路径
futexes     futex 地址 -> FutexBucket
sem_ctx     semaphore 上下文
shm_ctx     shared memory 上下文
pid/pgid    进程 id / 进程组 id
threads     线程 id 列表
ev          事件总线
exit_code   退出码
sig_queue   待处理信号
sig_mask    信号屏蔽字
ep_inst     epoll 实例
kstk        内核栈
thd_ctx     线程上下文
vm_token    地址空间/堆顶 token
```

关键函数：

```text
make              创建 Task
id / tag          读取基本信息
link_parent/link_child
done / n_children
get_free_fd / get_free_fd_from
add_file / get_file
get_futex
exit_proc         关闭 fd、设置事件、保存退出码
exited
get_ep_mut / set_ep
begin_run / end_run
has_sig / send_sig
close_fd
dup_fd / dup2_fd
fd_count
set_cloexec
```

CR 重点：

```text
Task.files 就是进程文件描述符表。
```

这和 MemFS 里的 `Process.fd_table` 是同一个抽象。

### TaskTable

`TaskTable` 管理所有任务。

字段：

```text
map   pid/tid -> Task
seq   下一个 id
root  init 任务
```

关键函数：

```text
spawn / spawn_root
find / find_by_tag
process_of_tid
pgid_group
register
reap
count
fork_task
clone_thread
new_user_task
terminate_and_collect
active_tasks / zombie_tasks
send_signal_group
```

`fork_task` 会复制 cwd、exec_path、fd table、pgid、sem/shm/sig 等上下文。

`new_user_task` 会设置 exec path、初始化用户栈、放入 fd0/fd1/fd2。

## 11. Kernel 和 syscall 分发

### Kernel 字段

```text
tasks      TaskTable，所有任务
cache      BlockCache，块缓存
pool       FramePool，物理页分配器
cpus       每个 CPU 当前运行的任务
mnt        MountTable，挂载点
sem_store  全局 semaphore store
shm_store  全局 shared memory store
tty_buf    终端输入缓冲
disk       模拟磁盘
```

### Kernel 普通函数

```text
new              初始化 kernel
tick             尝试拿 GKL，清理 cache dirty 状态
cur_task         获取某 CPU 当前任务
set_cur          设置某 CPU 当前任务
handle_pgfault   处理 page fault
proc_init        创建 init 任务和内核栈
tty_push/pop     终端输入输出缓冲
get_sem/get_shm  获取 IPC 对象
spawn_thread     启动模拟线程运行 task
```

### dispatch_syscall

`dispatch_syscall(nr, a0..a5)` 是 syscall 主入口。

它先做：

```text
_audit        参数 hash
_ts_enter     进入 tick
_caller_token 当前任务 vm_token
```

然后根据 syscall number 分发。

按类别理解：

#### 文件 I/O

```text
SYS_READ
SYS_WRITE
SYS_OPEN
SYS_CLOSE
SYS_STAT / SYS_FSTAT
SYS_IOCTL
SYS_FCNTL
```

`SYS_READ/WRITE` 主要检查用户地址、count、cache 状态，然后返回模拟传输长度。

`SYS_OPEN` 解析 flags，创建 `FHandle`，插入当前任务 fd table。

`SYS_CLOSE` 清 cache 中对应 fd，并返回成功。

`SYS_FCNTL` 支持 dup、get/set fd flags、get/set file flags、lock 操作的简化检查。

#### 内存管理

```text
SYS_MMAP
SYS_MUNMAP
SYS_BRK
```

`SYS_MMAP` 根据 prot/flags 生成 `vm_flags`，选择地址，并检查 frame pool 是否有足够页。

`SYS_BRK` 调整堆顶，增长时分配页，缩小时释放/模拟释放。

#### 进程控制

```text
SYS_FORK
SYS_EXEC
SYS_EXIT
SYS_WAIT4
SYS_GETPID
SYS_GETPPID
SYS_SETPGID
SYS_GETPGID
SYS_SETSID
```

这些围绕 `TaskTable` 和 `Task`：

- fork 创建新 pid；
- exec 检查 ELF、清理 cloexec fd、重建用户栈；
- exit 设置退出状态并通知父进程；
- wait4 查找 zombie child；
- pgid/session 管进程组。

#### pipe/epoll/futex/信号/time

```text
SYS_PIPE
SYS_EPOLL_CREATE / SYS_EPOLL_CTL / SYS_EPOLL_WAIT
SYS_FUTEX
SYS_SIGACTION / SYS_SIGPROCMASK
SYS_CLOCK_GETTIME
SYS_KILL
```

这些分别对应：

- 创建 pipe 两端并插入 fd table；
- 管理 epoll 实例；
- futex wait/wake/requeue 参数检查；
- 信号动作和屏蔽字；
- 时钟读取；
- 给任务或进程组发信号。

### Kernel 后续辅助函数

```text
schedule_tick     tick + 是否需要抢占
balance_load      计算 CPU 负载并选 CPU
reclaim_zombies   回收 zombie
lookup_path       mount resolve
alloc_pages       从 FramePool 分配页
free_pages        释放页
memory_pressure   计算内存压力百分比
cache_stats       block cache 统计
do_fork           高层 fork helper
do_exec           高层 exec helper
do_pipe           高层 pipe helper
do_wait           高层 wait helper
```

## 12. AddrSpace、ProcessGroup、WaitQueue、ResourceLimits、BuddyAllocator

### AddrSpace

`AddrSpace` 是更完整的地址空间对象。

字段：

```text
vm_map           VmMap
page_table_root  页表根
asid             地址空间 id
ref_count        引用计数
cow_pages        COW 页表
```

函数：

```text
new
fork_from
handle_cow_fault
unmap_range
protect
rss_pages
cow_sharers
split_region
```

`fork_from` 会复制父进程的 `VmRegion`，并对可写区域设置 COW 引用。

`handle_cow_fault` 在写 fault 时分配新 frame。

### ProcessGroup

模拟进程组。

字段：

```text
pgid
leader
members
session_id
foreground
```

函数：

```text
add_member / remove_member
is_empty / member_count
is_leader
set_foreground / is_foreground
broadcast_signal
```

### WaitQueue

比 `SyncQueue` 更通用的按 key 等待队列。

字段：

```text
inner      (key, thread, flags)
wake_count 统计唤醒次数
```

函数：

```text
sleep / sleep_timeout
wake_one / wake_all
wake_filtered
pending_count / total_wakes
has_waiters_for
reorder_by_priority
```

### ResourceLimits

模拟 `rlimit`。

字段：

```text
max_fds
max_threads
max_stack_size
max_data_size
max_file_size
max_mappings
cpu_time_limit
```

函数：

```text
check_fd/check_threads/check_stack/check_data/check_filesize/check_mappings
inherit
set_limit/get_limit
exceeds_any
```

### BuddyAllocator

伙伴系统分配器。

字段：

```text
free_lists  每个 order 的空闲块
max_order
base_addr
total_pages
allocated
```

函数：

```text
new
alloc_order
free_order
free_pages_count
largest_free_order
fragmentation_score
snapshot
```

CR 重点：

```text
BuddyAllocator 适合分配 2^order 连续页。
FramePool 更像简单 bitmap 分配器。
```

## 13. 重要 CR 问题和答法

### Q1: VmMap 里为什么要弄 VmRegion，不弄会怎么样？

答：

`VmRegion` 是一段连续且属性相同的虚拟地址区域。不同区域有不同权限、来源和 offset。没有它的话，`mmap/munmap/mprotect/fork/COW` 都必须逐页管理，不仅更慢、更占内存，也很难表达文件映射 offset 和区域 split/merge。

### Q2: GKL 为什么要 thread-local depth？

答：

因为全局锁状态只能表示“有人拿锁”，不能表示“当前线程拿锁”。thread-local depth 表示当前线程自己重入了几层。这样 `leave()` 才能判断当前线程是否真的有资格释放锁，避免 foreign leave 释放别人的锁。

### Q3: FramePool::get 为什么还要 get_inner？

答：

`get` 是带 GKL 语义的外层入口，`get_inner` 是纯分配逻辑。如果当前线程已经持有 GKL，再在 `get` 里阻塞式拿 GKL 会自锁，所以已持有时直接调用 `get_inner`。

### Q4: SyncQueue::park_on 为什么醒来后还要检查条件？

答：

因为唤醒不等于条件成立。可能是广播、虚假唤醒或其他事件。正确模型是“醒来后重新检查 predicate”。

### Q5: Channel::recv 为什么睡眠前要释放 guard？

答：

如果持有自旋锁睡眠，发送者可能无法拿到锁写入数据或唤醒等待者，导致死锁。正确做法是只在短临界区拿 guard，睡眠前释放。

### Q6: FHandle 和 MemFS 里 FileHandle 的设计差别是什么？

答：

chaos 的 `FHandle` 同时保存 `path/data/offset/options`，概念混在一起。MemFS 重构后把内容放进 inode，把 offset/options 放进 FileHandle，更接近 rCore/VFS。

### Q7: BlockCache::sync_all 为什么跳过忙的 chain？

答：

全局 sync 路径如果阻塞等待某条 chain，可能和 GKL、FramePool、scheduler 形成死锁链。使用 `try_acquire` 可以避免全局路径死等局部锁。

### Q8: 这个 kernel.rs 和真实 OS 差距在哪里？

答：

它保留了 OS 的抽象：fd table、task table、vm region、frame pool、block cache、syscall dispatch、signal、futex、scheduler。但很多行为是模拟的：

- 没有真实页表；
- 没有真实磁盘 I/O；
- syscall 主要返回模拟结果；
- 文件系统没有真正 inode/VFS 分层；
- 调度不真正切换 CPU 上下文；
- 权限、signal、epoll、futex 都是简化模型。

所以它适合教学和测试“系统概念”，不是完整可启动内核。

## 14. 现场讲解建议顺序

推荐 CR 时按这个顺序讲：

1. 先说全局：这是一个单文件教学内核模型。
2. 讲 `Kernel` 聚合哪些子系统。
3. 讲 GKL 和 advanced 死锁调试。
4. 讲 `VmMap/VmRegion` 和虚拟内存抽象。
5. 讲 `FramePool` 和页分配。
6. 讲 `FHandle/FLike/Task.files`，再联系 MemFS 重构。
7. 讲 `BlockCache/Disk` 的缓存和 retry。
8. 讲 `Task/TaskTable` 和 syscall dispatch。
9. 最后说哪些地方是简化模拟，哪些地方是你重构时改进的方向。

## 15. 函数速查附录

这一节用于现场被问到某个函数名时快速回答。回答时建议用三句话：

```text
它属于哪个子系统；
它读写了哪些核心状态；
它为什么需要单独存在。
```

### KernLock / GKL

```text
KernLock::new
```

创建大内核锁，初始化全局占用标记、持有者 id、递归深度和持有线程。

```text
local_depth / set_local_depth
```

读写 thread-local 的当前线程持锁层数。它不是全局锁状态，而是“本线程视角”的重入计数。

```text
enter_reentrant
```

当前线程已经持有 GKL 时，不再抢全局 `flag`，只把本线程 depth 和全局 depth 增加一层。

```text
spin_until_acquired
```

自旋抢 `flag`，直到从 unlocked 变成 locked。它是阻塞式 enter 的核心。

```text
record_owner
```

抢到锁后记录 owner id、owner line、holder thread，方便后续判断是不是当前线程持有。

```text
release_global
```

真正释放全局锁，清空 owner/depth/thread，再把 `flag` 置回 false。

```text
enter
```

阻塞进入 GKL。已经持有时走重入，否则自旋等待。

```text
try_enter
```

非阻塞进入 GKL。拿不到返回 false，适合 cache sync、tick 这种不能卡死的路径。

```text
leave
```

释放当前线程的一层 GKL。关键点是只允许持有者释放，不能让 foreign leave 解掉别人的锁。

```text
held / held_by_current / owner / level
```

调试和分支判断用。`held` 只表示锁被某线程持有，`held_by_current` 才表示当前线程持有。

### Spin / EvBus / SyncQueue

```text
Spin::new / acquire / try_acquire / release / is_held
```

简单自旋锁。它只保护很短的临界区，不负责睡眠等待，也没有 owner/depth 语义。

```text
EvBus::make / set / clear / change / sub / cb_len
```

事件总线。`set/clear/change` 更新事件位图，`sub` 注册回调，`cb_len` 查询回调数量。

```text
wait_ev
```

在事件总线上等待 mask 对应的事件出现。

```text
SyncQueue::new
```

创建等待队列、pending signal 计数和 epoll 订阅记录。

```text
enqueue_current_thread / pop_waiter
```

把当前线程加入等待队列，或者从队列头取出一个等待者。

```text
wake_one_waiter / wake_all_waiters
```

用 `unpark` 唤醒一个或所有被 `park` 的线程。

```text
condition_is_ready
```

在持有外部数据锁时检查谓词条件，避免错过已经满足的状态。

```text
park_on
```

条件不满足就入队睡眠，被唤醒后重新检查条件。它必须处理 spurious wakeup。

```text
signal / broadcast / signal_n
```

发出唤醒。`signal` 一个，`broadcast` 全部，`signal_n` 指定数量。

```text
pending / wait_ev / wait_events / wait_guard / wait_timeout
```

查询未消费信号，或者用不同形式等待条件、多个队列、锁保护对象和超时事件。

```text
reg_epoll / unreg_epoll
```

记录或取消 epoll 订阅关系。

### Sema / Futex

```text
Sema::new / remove / release
```

创建、删除、释放计数信号量。`cnt` 表示可用资源数量。

```text
try_acquire / acquire_spin / access
```

尝试获取、忙等获取、RAII 获取。`access` 返回 guard，drop 时自动释放。

```text
get_val / set_val / get_ncnt / get_pid / set_pid
```

模拟 SysV semaphore 的查询和元数据更新。

```text
FutexBucket::wait / wake / requeue / pending_at
```

按用户地址等待、唤醒、迁移等待者、查询某地址等待者数量。

```text
FutexTable::new / ftx_wait / ftx_wake / ftx_requeue
```

把用户地址映射到 bucket，是 futex syscall 的底层实现。

### 内存映射和页分配

```text
p2v / v2p / k_off
```

模拟物理地址和内核虚拟地址互转，`k_off` 得到内核映射偏移。

```text
PgFrame::new / with_rc / up / down / count / set / cas / inc_if_nonzero
```

维护页帧引用计数。COW 和共享页会依赖这些操作判断能不能释放或复制。

```text
VmRegion::new / with_offset
```

创建虚拟地址区域，`with_offset` 用于文件映射这类带文件偏移的区域。

```text
VmRegion::end / contains / overlaps / split_at / merge_with
```

计算区域范围、检查包含和重叠、拆分区域、合并相邻兼容区域。

```text
VmRegion::ref_up / ref_down / ref_get
```

维护区域层面的共享引用计数。

```text
VmMap::new / insert / find / remove_range / find_free
```

创建映射表、插入 region、按地址查询、删除范围、查找空洞。

```text
VmMap::total_mapped / clone_regions / gap_after
```

统计映射大小、fork 时复制 region 描述、计算 region 之后的空洞。

```text
FramePool::new / take_first_free
```

创建物理页池，并从 slots 里找第一个空闲页。

```text
FramePool::get / get_inner
```

`get` 是对外页分配入口，负责 GKL 语义；`get_inner` 是纯分配逻辑，避免已经持锁时自锁。

```text
get_contig / put / avail / free_count
```

分配连续页、释放页、查询页是否空闲、统计空闲页。

```text
get_zone_aware / put_zone_aware / batch_alloc
```

按 zone 范围分配/释放，或一次分配多个页。

```text
ZoneInfo::new / zone_can_alloc / zone_pressure / reclaim_target / contains_pfn
```

描述内存 zone 的范围、水位和回收压力。

```text
frame_alloc / frame_dealloc / frame_alloc_contig
```

FramePool 的全局辅助封装。

```text
SharedPage::new / fault / is_cow_resolved / frame_id
```

管理共享页和 COW fault，把共享页在写入时转成私有页。

```text
KStk::new / top / drop
```

模拟内核栈分配、栈顶计算和释放。

```text
check_access / check_access_rw / cfu / ctu
```

检查用户地址是否合法，并模拟 copy-from-user / copy-to-user。

```text
rdu_fixup / heap_init / heap_grow
```

模拟读用户数据修复、堆初始化和堆增长。

### 基础数据结构和工具

```text
CircBuf::new / with_pos / next_write_index / next_read_index
```

创建环形缓冲区，计算并推进读写位置。

```text
push / pop / len / empty / full / remaining
```

环形队列的入队、出队和状态查询。

```text
peek / drain_to / fill_from
```

查看队首、批量读出、批量写入。

```text
SlabEntry::new / slab_alloc / slab_free
```

创建 slab，并分配/释放固定大小对象。

```text
slab_used / slab_avail / shrink / obj_at / obj_at_mut
```

统计 slab 状态、收缩、按 offset 访问对象。

```text
validate_elf_header
```

检查 ELF header 是否合理。

```text
tcp_checksum / parse_ipv4_header / build_pseudo_header / compute_inet_checksum
```

网络包解析和校验和计算工具。

```text
compute_load_balance / audit_fd_table / rehash_mount_cache
```

调度负载估计、fd table 审计、mount cache 重建。

```text
defragment_frame_pool / verify_page_alignment / compute_rss_watermark
```

frame pool 整理、页对齐检查、RSS 水位估计。

### 文件、管道、epoll、channel

```text
FHandle::new / with_data / dup
```

创建普通文件 handle，或复制 handle。

```text
set_opt / get_opt / read / read_at / write / write_at / seek
```

设置选项、读取选项、按 offset 或指定位置读写、移动 offset。

```text
transfer
```

统一处理 read/write/read_at/write_at 这类传输路径。

```text
set_len / sync_all / sync_data / metadata_sz
```

调整长度、同步文件、读取文件大小。

```text
lookup / read_entry / poll_status / io_ctl / mmap / inode_ref
```

模拟目录查找、目录项读取、poll、ioctl、mmap 和底层数据引用。

```text
advise_readahead / fallocate / splice_to
```

模拟预读建议、预分配空间、文件间数据转移。

```text
PipeNode::pair / can_read / can_write / read_at / write_at / poll
```

管道端点创建、可读写判断、读写和 poll 状态。

```text
FLike::dup / read / write / io_ctl / mmap_fl / poll
```

对普通文件、管道、epoll 等 fd 类型做统一分发。

```text
EpEvent::has / EpInst::new / EpInst::control
```

判断 epoll 事件，创建 epoll 实例，增删改监听 fd。

```text
Channel::new / recv / send / close / try_recv
```

创建 channel，接收、发送、关闭和非阻塞接收。

```text
send_batch / depth / drain_all / is_closed / remaining_capacity
```

批量发送、查询深度、全部取出、查询关闭状态和剩余容量。

### 缓存、挂载、磁盘

```text
PageCache::new / lookup / insert / evict_lru
```

创建 page cache，查找、插入、LRU 淘汰。

```text
mark_dirty / writeback_all / stats / pin / unpin / invalidate / flush_range
```

标脏、写回、统计、固定页、解除固定、失效和范围刷新。

```text
CacheRegistry::new / register / register_child / unregister
```

登记缓存对象及父子关系。

```text
find_by_type / dump_graph / gc_sweep / ref_up / ref_down / count / owner_objects
```

查询、导出依赖图、垃圾扫描、引用计数和 owner 查询。

```text
CacheChain::new / acquire / try_acquire / release
```

单条 block cache chain 的锁和条目容器。

```text
BlockCache::new / idx / fetch_chain_index / cached_payload / synthetic_block
```

创建多链 block cache，把 block id 映射到 chain，读缓存或生成模拟块。

```text
fetch / sync_all / invalidate / total_entries / dirty_count / evict_cold
```

读取 block、同步 dirty block、失效、统计和淘汰冷块。

```text
MountTable::new / bind / resolve / unmount / list_mounts / find_mount / mount_count / has_prefix
```

维护挂载表，做路径前缀映射和查询。

```text
IoQueue::new / submit / submit_batch / dispatch / merge_adjacent / depth
```

维护 I/O 请求队列，提交、调度、合并和查询深度。

```text
Disk::new / failing / attach_journal / set_errs
```

创建正常或故障磁盘，挂接 journal，设置错误次数。

```text
begin_op / remaining_errors / consume_transient_error / fill_block / fill_limited_read / retry_journal
```

记录 I/O、读取和消耗错误、填充模拟数据、尝试 journal 重试。

```text
read_block / read_block_n / total_ops / reset_ops / write_block / flush
```

读写磁盘块、统计和重置操作次数、刷新磁盘。

### IPC、信号、timer、trap

```text
SemArr::index / remove / otime_now / ctime_now / set_ds
```

访问信号量数组元素、删除数组、更新时间戳、更新描述符。

```text
SemCtx::get_or_create / add / remove / free_id / get / add_undo
```

管理进程的 SysV semaphore 集合和 undo 记录。

```text
shm_get_or_create / ShmCtx::add / get / set / get_id_by_addr / pop
```

查找/创建共享内存，并维护进程 attach 的共享内存映射。

```text
ProcInit::push_at / total_size
```

把 argv/env/auxv 这类进程初始化数据压到用户栈。

```text
CapSet::new / full / check / grant / drop_cap / inherit / has_any / clear_ambient / raise_ambient
```

维护 capability 位图和继承规则。

```text
SigSet::new / sig_pending / sig_raise / coalesce_pending / sig_clear
```

维护 pending signal，并模拟普通 signal 合并。

```text
sig_block / sig_unblock / sig_setmask / deliverable
```

维护 signal mask，并找出当前可递送信号。

```text
set_action / get_action / is_ignored / clear_non_caught
```

维护 signal handler 行为，exec 时清理非保留 handler。

```text
TimerEntry::new / expired / reset / remaining / cancel
```

创建 timer，判断过期，重设周期 timer，计算剩余时间，取消 timer。

```text
TimerWheel::new / add_timer / advance / cancel / active_count
```

管理 timer 集合，每次 tick 推进并返回过期 timer。

```text
Context::new / capture / apply / set_ip / set_sp / set_ret / set_tls
```

创建、捕获、恢复和修改寄存器上下文。

```text
transform / syscall_args / clone_with_ret / diff / hash / reg_class
```

上下文变换、取 syscall 参数、fork 返回值设置、比较和分类寄存器。

```text
TrapCtl::new / configure / hw / sw / in_handler
```

创建 trap 控制器，配置 mask，判断是否在 handler 中。

```text
dispatch / current / handle_irq / on_pgfault / dispatch_vector
```

处理 trap、读取当前上下文、处理中断、处理缺页、按向量分发。

```text
push_frame / pop_frame / nest_depth / suppress / unsuppress
```

维护嵌套 trap 栈，并禁止或恢复 trap 处理。

### Clock / Scheduler

```text
wclk / cclk / dtk / up_ms / tmr / ser
```

读取时钟、推进 CPU tick、计算 uptime、timer 入口、串口换行处理。

```text
SchedulePolicy::new / with_prio / weight
```

创建调度策略，并把优先级转成权重。

```text
RunQueue::new / enqueue / dequeue / pick_next
```

创建运行队列、加入任务、取出任务、选择下一个任务。

```text
cmp_priority / rebalance / set_current / clear_current
```

比较优先级、重新平衡、记录/清除当前任务。

```text
len / remove / update_vruntime
```

查询队列长度、删除任务、更新虚拟运行时间。

```text
preempt_disable / preempt_enable / preemptible / boost_priority / yield_current
```

控制抢占、提高优先级、主动让出 CPU。

### Task / TaskTable

```text
Pid::new / get / is_init
```

进程 id 的小封装。

```text
Task::make / id / tag
```

创建任务，并读取任务 id 和标签。

```text
link_parent / link_child / done / n_children
```

维护父子关系，查询退出状态和子任务数量。

```text
get_free_fd / get_free_fd_from / add_file / get_file
```

维护 fd table：找空位、插入文件、按 fd 取文件。

```text
get_futex / exit_proc / exited
```

获取 futex bucket，执行退出流程，查询是否退出。

```text
get_ep_mut / get_ep_ref / set_ep
```

读取或更新 epoll 实例。

```text
begin_run / end_run / has_sig / send_sig
```

维护任务运行上下文，检查和发送信号。

```text
close_fd / dup_fd / dup2_fd / fd_count / set_cloexec
```

fd table 的关闭、复制、定向复制、计数和 close-on-exec 设置。

```text
TaskTable::new / spawn / spawn_root / find / find_by_tag / process_of_tid
```

创建任务表，创建任务，按 pid/tag/tid 查找任务。

```text
pgid_group / register / reap / count
```

查询进程组、注册任务、回收任务、统计任务数。

```text
fork_task / clone_thread / new_user_task
```

复制进程、创建线程、创建用户任务。

```text
terminate_and_collect / active_tasks / zombie_tasks / send_signal_group
```

结束和回收任务，列出活跃/僵尸任务，向进程组发信号。

### Kernel

```text
Kernel::new
```

创建整个 kernel，把任务表、cache、frame pool、mount、IPC、磁盘等子系统组装起来。

```text
tick
```

时钟 tick 路径，推进时间并做轻量维护。关键是不能阻塞式拿 GKL。

```text
cur_task / set_cur / handle_pgfault / handle_pgfault_ext
```

读取/设置当前 CPU 任务，并处理缺页异常。

```text
proc_init / tty_push / tty_pop
```

初始化第一个进程，维护终端输入缓冲。

```text
get_sem / get_shm / spawn_thread
```

获取 IPC 对象，并在宿主线程里运行模拟任务。

```text
dispatch_syscall
```

syscall 总入口。根据 syscall number 进入文件、内存、进程、信号、时间等具体逻辑。

```text
schedule_tick / balance_load / reclaim_zombies
```

调度 tick、负载均衡、清理僵尸任务。

```text
lookup_path / alloc_pages / free_pages / memory_pressure / cache_stats
```

路径解析、页分配释放、内存压力和 cache 统计。

```text
do_fork / do_exec / do_pipe / do_wait
```

把复杂 syscall 的核心逻辑抽成辅助函数：fork、exec、pipe、wait。

### 后半部分独立结构

```text
validate_access
```

按 pid/mode 检查用户地址访问。

```text
mem_scan_pattern / compute_crc32 / encode_varint / decode_varint
```

通用数据处理工具：模式扫描、CRC、变长整数编码解码。

```text
AddrSpace::new / fork_from / handle_cow_fault
```

创建地址空间、复制地址空间、处理 COW fault。

```text
unmap_range / protect / rss_pages / cow_sharers / split_region
```

取消映射、修改权限、统计 RSS/COW、切分 region。

```text
ProcessGroup::new / add_member / remove_member / is_empty / member_count / is_leader
```

维护进程组成员和 leader 状态。

```text
set_foreground / is_foreground / broadcast_signal
```

设置前台进程组，并向组内成员广播信号。

```text
WaitQueue::new / sleep / sleep_timeout / wake_one / wake_all / wake_filtered
```

创建等待队列，按 key 睡眠，并唤醒一个、全部或满足条件的等待者。

```text
pending_count / total_wakes / has_waiters_for / reorder_by_priority
```

查询等待者和唤醒统计，并按优先级重排。

```text
ResourceLimits::default_limits / check_fd / check_threads / check_stack
```

创建默认资源限制，并检查 fd、线程、栈大小。

```text
check_data / check_filesize / check_mappings / inherit / set_limit / get_limit / exceeds_any
```

检查数据段、文件大小、映射数量，继承/设置/读取资源限制。

```text
bitwise_merge / rotate_bits / popcount64 / clz64 / ffs64
```

位操作工具函数。

```text
align_up / align_down / is_power_of_two / log2_floor
```

内存对齐和幂次计算工具。

```text
hash_combine / murmurhash3_finalize
```

hash 组合和尾部混合函数。

```text
BuddyAllocator::new / alloc_order / free_order
```

创建 buddy allocator，分配和释放指定 order 的连续页块。

```text
free_pages_count / largest_free_order / fragmentation_score / snapshot
```

统计空闲页、最大连续块、碎片分数、复制当前 allocator 状态。
