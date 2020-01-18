use slave_pool::ThreadPool;

const SECOND: core::time::Duration = core::time::Duration::from_secs(1);
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
