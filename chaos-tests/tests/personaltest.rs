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
