use chaos_tests::*;
use std::sync::Arc;
use std::time::Duration;

fn run_with_timeout<F: FnOnce() + Send + 'static>(f: F, ms: u64) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_millis(ms)).is_ok()
}

#[test]
fn personal_scheduler_tick_under_gkl() {
    let k = Arc::new(Kernel::new(16));
    let kk = k.clone();

    let done = run_with_timeout(move || {
        GKL.enter(7101);
        kk.tick(7102);
        GKL.leave();
    }, 200);

    if !done {
        GKL.leave();
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(done, "Kernel::tick should not deadlock when GKL is already held");
}

#[test]
fn personal_cache_sync_under_gkl() {
    let cache = Arc::new(BlockCache::new(4));
    let cc = cache.clone();

    let done = run_with_timeout(move || {
        GKL.enter(7201);
        cc.sync_all(7202);
        GKL.leave();
    }, 200);

    if !done {
        GKL.leave();
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(done, "BlockCache::sync_all should not deadlock when GKL is already held");
}


#[test]
fn personal_scheduler_tick_under_cpu_lock() {
    let k = Arc::new(Kernel::new(16));
    let kk = k.clone();

    let done = run_with_timeout(move || {
        let _cpus = kk.cpus.lock().unwrap();
        kk.tick(7301);
    }, 200);

    assert!(done, "Kernel::tick should not deadlock when scheduler cpu lock is already held");
}

#[test]
fn personal_cache_sync_under_chain_lock() {
    let cache = Arc::new(BlockCache::new(4));
    let cc = cache.clone();

    let done = run_with_timeout(move || {
        let ch = &cc.chains[0];
        ch.lk.acquire();
        cc.sync_all(7401);
        ch.lk.release();
    }, 200);

    if !done {
        GKL.leave();
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(done, "BlockCache::sync_all should not deadlock when a cache chain is already held");
}

#[test]
fn personal_cache_memory_foreign_gkl_chain() {
    let k = Arc::new(Kernel::new(16));

    GKL.enter(2010);
    k.cache.sync_all(2010);

    let kk = k.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let r = kk.pool.get(2099);
        let _ = tx.send(r);
    });

    let early = rx.recv_timeout(Duration::from_millis(100));
    GKL.leave();

    if early.is_err() {
        let _ = rx.recv_timeout(Duration::from_millis(1000));
    }
    let _ = worker.join();

    assert!(
        early.is_err(),
        "FramePool::get should not enter get_inner while another thread holds GKL"
    );
}

#[test]
#[ignore]
fn personal_bkl_same_id_reenter_from_helper_thread() {
    GKL.enter(2020);

    let done = run_with_timeout(move || {
        GKL.enter(2020);
        GKL.leave();
    }, 200);

    GKL.leave();

    assert!(
        done,
        "KernLock should treat the same logical id as reentrant even from a helper thread"
    );
}

#[test]
fn personal_repro_owner_2020_blocks_2010_cache_memory_chain() {
    let k = Arc::new(Kernel::new(16));

    GKL.enter(2020);

    let kk = k.clone();
    let done = run_with_timeout(move || {
        GKL.enter(2010);
        kk.cache.sync_all(2010);
        kk.pool.get(2099);
        GKL.leave();
    }, 200);

    GKL.leave();
    std::thread::sleep(Duration::from_millis(50));

    assert!(
        !done,
        "diagnostic repro: owner=2020 should block the 2010 cache/memory chain before the fix"
    );
}

#[test]
#[ignore]
fn personal_check_same_id_2020_helper_thread_completes() {
    GKL.enter(2020);

    let done = run_with_timeout(move || {
        GKL.enter(2020);
        GKL.leave();
    }, 200);

    GKL.leave();
    std::thread::sleep(Duration::from_millis(50));

    assert!(
        done,
        "same logical id=2020 helper thread is not the remaining hidden-test blocker"
    );
}

#[test]
fn personal_model_timeout_worker_keeps_running() {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag_c = flag.clone();

    let done = run_with_timeout(move || {
        std::thread::sleep(Duration::from_millis(150));
        flag_c.store(true, std::sync::atomic::Ordering::SeqCst);
    }, 30);

    assert!(!done, "the timeout should fire before the worker finishes");

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        flag.load(std::sync::atomic::Ordering::SeqCst),
        "run_with_timeout does not kill the worker thread after timeout"
    );
}

#[test]
fn personal_model_gkl_is_shared_across_threads() {
    GKL.enter(8801);

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        GKL.enter(8802);
        GKL.leave();
        let _ = tx.send(());
    });

    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "another thread should be blocked while GKL is held"
    );

    GKL.leave();

    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_ok(),
        "the blocked thread should continue after GKL is released"
    );
}



#[test]
fn personal_repro_hidden_foreign_cleanup_cannot_release_2020() {
    let k = Arc::new(Kernel::new(16));
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    let holder = std::thread::spawn(move || {
        GKL.enter(2020);
        let _ = held_tx.send(());
        let _ = release_rx.recv();
        GKL.leave();
    });

    held_rx.recv_timeout(Duration::from_millis(500)).unwrap();

    let kk = k.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        GKL.enter(2010);
        kk.cache.sync_all(2010);
        kk.pool.get(2099);
        GKL.leave();
        let _ = done_tx.send(());
    });

    assert!(
        done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "2010 should time out while real holder keeps GKL(2020)"
    );

    GKL.leave();
    assert_eq!(
        GKL.owner(),
        2020,
        "foreign cleanup must not release another thread's GKL"
    );

    let _ = release_tx.send(());
    holder.join().unwrap();
    assert!(done_rx.recv_timeout(Duration::from_millis(1000)).is_ok());
    worker.join().unwrap();
}


#[test]
fn personal_repro_holder_waits_for_2010_while_holding_2020() {
    let k = Arc::new(Kernel::new(16));
    let (observed_tx, observed_rx) = std::sync::mpsc::channel();

    let holder = std::thread::spawn(move || {
        GKL.enter(2020);

        let kk = k.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            GKL.enter(2010);
            kk.cache.sync_all(2010);
            kk.pool.get(2099);
            GKL.leave();
            let _ = done_tx.send(());
        });

        let worker_blocked = done_rx.recv_timeout(Duration::from_millis(200)).is_err();
        let _ = observed_tx.send(worker_blocked);

        GKL.leave();
        let _ = done_rx.recv_timeout(Duration::from_millis(1000));
        worker.join().unwrap();
    });

    assert!(
        observed_rx.recv_timeout(Duration::from_millis(1000)).unwrap(),
        "holding GKL(2020) while waiting for a GKL(2010) worker creates the deadlock chain"
    );

    holder.join().unwrap();
}


#[test]
#[ignore]
fn personal_red_scheduler_fs_memory_chain_should_not_be_blocked_by_stray_2020() {
    let k = Arc::new(Kernel::new(16));
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    let holder = std::thread::spawn(move || {
        GKL.enter(2020);
        let _ = held_tx.send(());
        let _ = release_rx.recv();
        GKL.leave();
    });

    held_rx.recv_timeout(Duration::from_millis(500)).unwrap();

    let kk = k.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        GKL.enter(2010);
        kk.cache.sync_all(2010);
        kk.pool.get(2099);
        GKL.leave();
        let _ = done_tx.send(());
    });

    let done = done_rx.recv_timeout(Duration::from_millis(200)).is_ok();

    let _ = release_tx.send(());
    holder.join().unwrap();
    let _ = done_rx.recv_timeout(Duration::from_millis(1000));
    worker.join().unwrap();

    assert!(done, "red repro: 2010 chain is blocked by a stray GKL(2020) holder");
}
