# MemFS Refactor

This directory is an independent rCore-style refactor playground.

Goal:
- implement an in-memory file system by hand;
- keep the design close to rCore;
- make the call path easy to explain during code review;
- later connect it back to the existing kernel or a small runnable kernel.

Core path:

```text
sys_open/sys_read/sys_write
        |
        v
Process.fd_table
        |
        v
FileHandle
        |
        v
Inode
        |
        v
MemFS data
```

Important rules:
- syscall code only touches file descriptors;
- each file descriptor points to a file handle;
- a file handle owns its own offset;
- multiple file handles may point to the same inode;
- inode owns file contents;
- shared state is protected by locks.

