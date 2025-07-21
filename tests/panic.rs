use core::{time, task, future};
use core::sync::atomic;

use slave_pool::ThreadPool;

const MS: time::Duration = time::Duration::from_millis(1);
const SECOND: time::Duration = time::Duration::from_secs(1);

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

        prev_handle.wait_timeout(MS).expect_err("Should fail");
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

