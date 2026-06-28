use memfs_from_scratch::fs::{OpenFlags, OpenOptions};
use memfs_from_scratch::syscall::Kernel;

#[test]
fn create_write_seek_read() {
    let mut kernel = Kernel::new();
    let fd = kernel
        .sys_open("/hello", true, OpenOptions::read_write())
        .unwrap();

    assert_eq!(kernel.sys_write(fd, b"abc").unwrap(), 3);
    kernel.sys_lseek(fd, 0).unwrap();

    let mut buf = [0; 4];
    assert_eq!(kernel.sys_read(fd, &mut buf).unwrap(), 3);
    assert_eq!(&buf[..3], b"abc");
}

#[test]
fn two_opens_share_inode_but_not_offset() {
    let mut kernel = Kernel::new();
    let fd1 = kernel
        .sys_open("/note", true, OpenOptions::read_write())
        .unwrap();
    assert_eq!(kernel.sys_write(fd1, b"abcdef").unwrap(), 6);

    let fd2 = kernel
        .sys_open("/note", false, OpenOptions::read_only())
        .unwrap();

    let mut first = [0; 3];
    assert_eq!(kernel.sys_read(fd2, &mut first).unwrap(), 3);
    assert_eq!(&first, b"abc");

    let mut second = [0; 3];
    assert_eq!(kernel.sys_read(fd2, &mut second).unwrap(), 3);
    assert_eq!(&second, b"def");

    let mut empty = [0; 1];
    assert_eq!(kernel.sys_read(fd1, &mut empty).unwrap(), 0);
}

#[test]
fn directory_create_list_and_unlink() {
    let mut kernel = Kernel::new();
    kernel.sys_mkdir("/tmp").unwrap();
    kernel
        .sys_open("/tmp/a", true, OpenOptions::read_write())
        .unwrap();
    kernel
        .sys_open("/tmp/b", true, OpenOptions::read_write())
        .unwrap();

    let mut entries = kernel.sys_getdents("/tmp").unwrap();
    entries.sort();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], "a");
    assert_eq!(entries[1], "b");

    kernel.sys_unlink("/tmp/a").unwrap();
    let entries = kernel.sys_getdents("/tmp").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], "b");
}

#[test]
fn truncate_changes_inode_length() {
    let mut kernel = Kernel::new();
    let fd = kernel
        .sys_open("/data", true, OpenOptions::read_write())
        .unwrap();
    kernel.sys_write(fd, b"abcdef").unwrap();
    kernel.sys_ftruncate(fd, 2).unwrap();
    kernel.sys_lseek(fd, 0).unwrap();

    let mut buf = [0; 8];
    assert_eq!(kernel.sys_read(fd, &mut buf).unwrap(), 2);
    assert_eq!(&buf[..2], b"ab");
}

#[test]
fn open_flags_create_rdwr_and_truncate() {
    let mut kernel = Kernel::new();
    let fd = kernel
        .sys_open_flags("/log", OpenFlags::CREATE | OpenFlags::RDWR)
        .unwrap();
    kernel.sys_write(fd, b"old").unwrap();

    let fd2 = kernel
        .sys_open_flags("/log", OpenFlags::TRUNCATE | OpenFlags::RDWR)
        .unwrap();
    assert_eq!(kernel.sys_fstat(fd2).unwrap().len, 0);

    kernel.sys_write(fd2, b"new").unwrap();
    kernel.sys_lseek(fd2, 0).unwrap();
    let mut buf = [0; 3];
    kernel.sys_read(fd2, &mut buf).unwrap();
    assert_eq!(&buf, b"new");
}
