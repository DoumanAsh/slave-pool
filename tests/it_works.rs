use core::time;
use std::collections::HashSet;

use slave_pool::ThreadPool;

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

    assert_eq!(pool.set_threads(8).unwrap(), 0);
    assert_eq!(pool.set_threads(4).unwrap(), 8);

    assert_eq!(handle.wait_timeout(SECOND).unwrap(), 5);
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
    }
}
