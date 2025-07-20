use core::{time, task, future};
use core::sync::atomic;
use std::collections::HashSet;

use slave_pool::{ThreadPool, JoinError};

const MS: time::Duration = time::Duration::from_millis(1);
const SECOND: time::Duration = time::Duration::from_secs(1);

#[test]
fn it_works() {
    {
        let pool = ThreadPool::new();

        assert_eq!(pool.set_threads(8).unwrap(), 0);
        assert_eq!(pool.set_threads(4).unwrap(), 8);
        pool.spawn(|| {});
        let handle = pool.spawn_handle(|| {
            std::thread::sleep(core::time::Duration::from_millis(100));
            5
        });

        assert_eq!(handle.wait_timeout(SECOND).unwrap(), 5);
    }

    std::thread::sleep(SECOND);
}

#[test]
fn should_spawn_and_complete_once_available() {
    let pool = ThreadPool::new();

    let handle = pool.spawn_handle(|| {
        std::thread::sleep(core::time::Duration::from_millis(100));
        5
    });
    assert!(handle.wait_timeout(SECOND).is_err());
    assert!(handle.try_wait().unwrap().is_none());

    assert_eq!(pool.set_threads(8).unwrap(), 0);
    assert_eq!(pool.set_threads(4).unwrap(), 8);

    assert_eq!(handle.wait_timeout(SECOND).unwrap(), 5);
    assert_eq!(handle.try_wait().unwrap_err(), JoinError::AlreadyConsumed);
}

#[test]
fn should_spawn_over_capacity() {
    let pool = ThreadPool::new();

    assert_eq!(pool.set_threads(8).unwrap(), 0);

    let mut ids = HashSet::new();
    let mut handles = Vec::new();
    for id in 0..=20 {
        ids.insert(id);
        handles.push(pool.spawn_handle(move || {
            std::thread::sleep(core::time::Duration::from_millis(100));
            id
        }));
    }

    for handle in handles.into_iter() {
        let value = handle.wait_timeout(SECOND).unwrap();
        assert!(ids.remove(&value), "Should not repeat");
        assert_eq!(handle.try_wait().unwrap_err(), JoinError::AlreadyConsumed);
    }
}

#[test]
fn should_spawn_and_over_capacity() {
    let pool = ThreadPool::new();

    assert_eq!(pool.set_threads(8).unwrap(), 0);

    let mut ids = HashSet::new();
    let mut handles = Vec::new();
    for id in 0..=20 {
        ids.insert(id);
        handles.push(pool.spawn_handle(move || {
            std::thread::sleep(core::time::Duration::from_millis(100));
            id
        }));
    }

    for (idx, handle) in handles.into_iter().enumerate() {
        if idx % 2 == 0 {
            drop(handle);
        } else {
            let value = handle.wait_timeout(SECOND).unwrap();
            assert!(ids.remove(&value), "Should not repeat");
            assert_eq!(handle.try_wait().unwrap_err(), JoinError::AlreadyConsumed);
        }
    }
}

#[test]
fn should_handle_drop() {
    #[derive(Clone, Debug)]
    struct Guard {
        state: std::sync::Arc<atomic::AtomicUsize>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.state.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }
    let guard = Guard {
        state: std::sync::Arc::new(atomic::AtomicUsize::new(0))
    };
    let guard1 = guard.clone();
    let guard2 = guard.clone();
    let guard3 = guard.clone();

    let pool = ThreadPool::new();
    let handle = pool.spawn_handle(move || {
        guard1
    });
    assert_eq!(handle.wait_timeout(MS).unwrap_err(), JoinError::Timeout);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 0);

    drop(handle);

    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 0);

    assert_eq!(pool.set_threads(1).unwrap(), 0);

    std::thread::sleep(MS * 250);

    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 1);

    let handle = pool.spawn_handle(move || {
        guard2
    });

    std::thread::sleep(MS * 250);
    drop(handle);

    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 2);

    let handle = pool.spawn_handle(move || {
        std::thread::sleep(MS * 500);
        guard3
    });

    std::thread::sleep(MS * 250);
    drop(handle);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 2);

    std::thread::sleep(MS * 500);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 3);

    assert_eq!(pool.set_threads(0).unwrap(), 1);
    std::thread::sleep(MS * 100);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 3);
}

#[test]
fn should_handle_drop_many() {
    #[derive(Clone, Debug)]
    struct Guard {
        state: std::sync::Arc<atomic::AtomicUsize>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.state.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }
    let guard = Guard {
        state: std::sync::Arc::new(atomic::AtomicUsize::new(0))
    };

    let pool = ThreadPool::new();
    let mut handles = Vec::new();
    for _ in 0..100 {
        let guard1 = guard.clone();
        let handle = pool.spawn_handle(move || {
            std::thread::sleep(MS * 50);
            guard1
        });
        handles.push(handle);
    }

    assert_eq!(pool.set_threads(2).unwrap(), 0);

    let expected_counter = handles.len();
    for (idx, handle) in handles.into_iter().enumerate() {
        if idx % 2 == 0 {
            drop(handle);
        } else {
            let _ = handle.wait();
        }
    }

    assert_eq!(pool.set_threads(0).unwrap(), 2);
    std::thread::yield_now();
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), expected_counter);
}

#[test]
fn should_process_sender_drop_after_all_handles_dead() {
    #[derive(Clone, Debug)]
    struct Guard {
        state: std::sync::Arc<atomic::AtomicUsize>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.state.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }
    let guard = Guard {
        state: std::sync::Arc::new(atomic::AtomicUsize::new(0))
    };

    let mut pool = ThreadPool::new();
    for _ in 0..100 {
        let guard1 = guard.clone();
        let handle = pool.spawn_handle(move || {
            std::thread::sleep(MS * 50);
            guard1
        });
        drop(handle);
    }

    pool.shutdown();
    std::thread::sleep(MS * 100);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 100);
}

#[test]
fn should_process_receiver_drop_after_all_senders_shutdown() {
    #[derive(Clone, Debug)]
    struct Guard {
        state: std::sync::Arc<atomic::AtomicUsize>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.state.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }
    let guard = Guard {
        state: std::sync::Arc::new(atomic::AtomicUsize::new(0))
    };

    let mut pool = ThreadPool::new();
    assert_eq!(pool.set_threads(4).unwrap(), 0);
    let mut handles = Vec::new();
    for _ in 0..50 {
        let guard1 = guard.clone();
        let handle = pool.spawn_handle(move || {
            std::thread::sleep(MS * 50);
            guard1
        });
        handles.push(handle);
    }

    pool.shutdown();

    {
        let last_handle = handles.pop().unwrap();
        let waker = thread_waker::waker(std::thread::current());
        let mut fut = core::pin::pin!(last_handle);
        let mut context = task::Context::from_waker(&waker);
        assert!(!future::Future::poll(fut.as_mut(), &mut context).is_ready());

        let prev_handle = handles.pop().unwrap();

        prev_handle.wait_timeout(MS * 10).expect_err("Should fail");
        assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 0);
        drop(handles);
        std::thread::sleep(SECOND);
        assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 48);
        prev_handle.wait().expect("Get message");
        assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 49);
        std::thread::sleep(MS * 100);
    }

    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 50);
}

#[test]
fn should_shutdown_and_restart() {
    let mut pool = ThreadPool::new();

    assert_eq!(pool.set_threads(2).unwrap(), 0);

    let handle = pool.spawn_handle(|| {
        std::thread::sleep(core::time::Duration::from_millis(100));
        10
    });

    pool.shutdown();

    assert_eq!(pool.set_threads(2).unwrap(), 0);

    assert_eq!(handle.wait().expect("success"), 10);

    pool.shutdown();

    std::thread::sleep(MS * 100);
}
