use core::{time, task, future};
use core::sync::atomic;

use slave_pool::ThreadPool;

const MS: time::Duration = time::Duration::from_millis(1);
const SECOND: time::Duration = time::Duration::from_secs(1);

#[test]
fn should_verify_worker_restart_after_panic() {
    let mut pool = ThreadPool::with_defaults("revived-worker", 0);
    assert_eq!(pool.set_threads(1).unwrap(), 0);

    let mut handles = Vec::new();
    for idx in 0..10 {
        let handle = pool.spawn_handle(move || {
            if idx < 9 {
                panic!("oh no: {idx}");
            }
        });
        handles.push(handle);
    }

    let mut success = 0;
    let mut disconnects = 0;
    for handle in handles {
        match handle.wait() {
            Ok(()) => {
                success += 1;
            },
            Err(slave_pool::JoinError::Disconnect) => {
                disconnects += 1;
            },
            Err(slave_pool::JoinError::AlreadyConsumed) => panic!("Unexpected error AlreadyConsumed"),
        }
    }

    assert_eq!(success, 1);
    assert_eq!(disconnects, 9);
    pool.shutdown_and_join();
}

#[test]
fn should_process_receiver_drop_after_all_senders_dead() {
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

    let mut pool = ThreadPool::with_defaults("dead-worker", 0);
    assert_eq!(pool.set_threads(4).unwrap(), 0);
    let mut handles = Vec::new();
    for idx in 0..10 {
        let guard1 = guard.clone();
        let handle = pool.spawn_handle(move || {
            std::thread::sleep(MS * 250);
            if idx < 8 {
                std::panic!("oh no: {}", guard1.state.load(atomic::Ordering::Relaxed));
            } else {
                guard1
            }
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

        assert!(prev_handle.wait_timeout(MS).unwrap().is_none(), "should timeout");
        assert!(guard.state.load(atomic::Ordering::SeqCst) < 8);
        drop(handles);
        std::thread::sleep(SECOND);
        assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 8);
        prev_handle.wait().expect("Get message");
        assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 9);
        std::thread::sleep(MS * 100);
    }

    std::thread::sleep(SECOND);
    assert_eq!(guard.state.load(atomic::Ordering::SeqCst), 10);
}

