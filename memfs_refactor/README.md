# MemFS Refactor

这是一个手写的 rCore 风格内存文件系统原型。它的目标不是实现完整的现代文件系统，而是把 chaos 里原本混在 `kernel.rs` 中的 fd、file handle、文件数据、目录操作等概念重新拆开，做成一个更清楚、方便解释和扩展的设计。

## 项目目标

这个 MemFS 需要支持最基础的文件系统路径：

```text
open -> write -> seek -> read
```

同时支持：

- 创建文件
- 创建目录
- 打开文件
- 读文件
- 写文件
- 修改当前文件 offset
- 截断文件
- 列出目录项
- 删除目录项
- 通过 open flags 控制 create、truncate、read/write、append 等行为

## 设计思路

整体设计采用类似 rCore / VFS 的分层：

```text
syscall layer
        |
        v
Process fd_table
        |
        v
FileLike / FileHandle
        |
        v
Inode trait
        |
        v
MemInode
        |
        v
Vec<u8> / BTreeMap
```

每一层的职责是：

- `Kernel`：提供 `sys_open`、`sys_read`、`sys_write` 等 syscall 风格接口。
- `Process`：维护当前进程的文件描述符表，也就是 `fd -> FileLike` 的映射。
- `FileLike`：表示 fd 指向的“类文件对象”，目前只实现了普通文件。
- `FileHandle`：表示一次 `open` 产生的打开文件对象，保存自己的 offset 和 open options。
- `Inode`：抽象文件系统节点，上层只依赖这个 trait，不直接关心具体文件系统实现。
- `MemInode`：内存文件系统中的真实节点，普通文件用 `Vec<u8>` 保存内容，目录用 `BTreeMap<String, Arc<MemInode>>` 保存目录项。

一个关键设计是：**inode 保存文件内容，FileHandle 保存打开状态**。因此同一个文件被打开两次时，两个 fd 会共享同一个 inode，但它们的 offset 是独立的。

## 和 chaos 的关系

chaos 原来的 `kernel.rs` 中已经有类似的结构，例如 `FHandle`、`FLike`、`Task.files` 和 syscall 里的 open/read/write 逻辑。这说明原项目本身已经有 fd table 和 file handle 的思想。

不过原实现里很多概念混在一起，例如 `FHandle` 同时保存路径、文件内容、offset 和选项，路径查找和 inode 层也不够清晰。这个 MemFS 的重构目标就是把这些概念拆开：

```text
chaos 原思路:
Task.files -> FLike -> FHandle -> Vec<u8>

MemFS 重构后:
Process.fd_table -> FileLike -> FileHandle -> Inode -> MemInode -> Vec<u8> / BTreeMap
```

所以它不是一个和 chaos 无关的新项目，而是对 chaos 现有 fd/file 模型的重新整理和简化实现。

## 目录结构

```text
memfs_refactor/
├── Cargo.toml
├── Makefile
├── README.md
├── src
│   ├── lib.rs
│   ├── error.rs
│   ├── process.rs
│   ├── fs
│   │   ├── mod.rs
│   │   ├── vfs.rs
│   │   ├── memfs.rs
│   │   ├── file.rs
│   │   └── path.rs
│   └── syscall
│       └── mod.rs
└── tests
    └── basic.rs
```

核心文件说明：

- `src/lib.rs`：crate 入口，声明 `no_std`、`alloc` 和各个模块。
- `src/error.rs`：统一错误类型和 `Result<T>`。
- `src/fs/vfs.rs`：定义 `Inode` trait、`FileType` 和 `Metadata`。
- `src/fs/memfs.rs`：实现 `MemFS` 和 `MemInode`，保存真实文件/目录数据。
- `src/fs/file.rs`：实现 `OpenFlags`、`OpenOptions`、`FileHandle` 和 `FileLike`。
- `src/fs/path.rs`：提供路径拆分和路径遍历工具。
- `src/process.rs`：实现进程文件描述符表。
- `src/syscall/mod.rs`：实现 syscall 风格接口。
- `tests/basic.rs`：基础功能测试。

## 如何运行

进入目录：

```bash
cd memfs_refactor
```

检查能否编译：

```bash
make
```

运行全部测试：

```bash
cargo test --test basic -- --nocapture
```

当前基础测试包括：

- `create_write_seek_read`
- `two_opens_share_inode_but_not_offset`
- `directory_create_list_and_unlink`
- `truncate_changes_inode_length`
- `open_flags_create_rdwr_and_truncate`

目前 5 个 basic 测试均可通过。

## 最小工作流程

最简单的使用路径是：

```text
sys_open("/hello", create=true)
sys_write(fd, "abc")
sys_lseek(fd, 0)
sys_read(fd)
```

期望读回：

```text
abc
```

这条路径验证了 MemFS 的主干：

```text
path -> inode -> FileHandle -> fd_table -> fd
fd -> FileHandle -> inode -> Vec<u8>
```

## 当前边界

这个实现是教学和 code review 用的原型，不追求完整 POSIX 语义。它暂时不实现：

- 真实磁盘持久化
- block cache
- journal
- 硬链接和软链接
- 完整 Unix 权限系统
- 多进程调度和复杂并发语义
- crash consistency

它保留的是现代操作系统文件系统中最核心的抽象：文件描述符表、打开文件对象、inode、VFS 分层和目录树。
