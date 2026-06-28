# MemFS Design

## Layering

MemFS follows a small rCore-style layering:

```text
Kernel syscall methods
    -> Process fd_table
    -> FileLike
    -> FileHandle
    -> Inode trait
    -> MemInode
```

The syscall layer never reads file data directly. It only receives an fd, finds
the corresponding `FileLike`, and calls `read` or `write`.

## Inode And FileHandle

`MemInode` owns shared file-system state:

- regular file: `Vec<u8>`;
- directory: `BTreeMap<String, Arc<MemInode>>`.

`FileHandle` owns per-open state:

- the opened inode;
- the current offset;
- read/write/append options.

Opening the same path twice creates two file handles. They share one inode but
have independent offsets.

## Locking

Shared inode state is guarded by `spin::RwLock`.

The file offset is guarded by `spin::Mutex`.

This matches the intended OS idea:

- file contents are shared;
- open-file offsets are per handle;
- shared mutable state is protected by locks.

