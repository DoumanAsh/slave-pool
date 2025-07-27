# slave-pool

[![Crates.io](https://img.shields.io/crates/v/slave-pool.svg)](https://crates.io/crates/slave-pool)
[![Documentation](https://docs.rs/slave-pool/badge.svg)](https://docs.rs/crate/slave-pool/)
[![Rust](https://github.com/DoumanAsh/slave-pool/actions/workflows/rust.yml/badge.svg)](https://github.com/DoumanAsh/slave-pool/actions/workflows/rust.yml)

Simple thread pool

# usage

```rust
use slave_pool::ThreadPool;
const SECOND: core::time::Duration = core::time::Duration::from_secs(1);

static POOL: ThreadPool = ThreadPool::new();

POOL.set_threads(8); //Tell how many threads you want

let mut handles = Vec::new();
for idx in 0..8 {
    handles.push(POOL.spawn_handle(move || {
        std::thread::sleep(SECOND);
        idx
    }));
}

POOL.set_threads(0); //Tells to shut down threads

for (idx, handle) in handles.drain(..).enumerate() {
    assert_eq!(handle.wait().unwrap().expect("no panic"), idx) //Even though we told  it to shutdown all threads, it is going to finish queued job first
}

let handle = POOL.spawn_handle(|| {});
assert!(handle.wait_timeout(SECOND).is_err()); // All are shutdown now

POOL.set_threads(1); //But let's add one more

assert!(handle.wait().unwrap().is_ok());

let handle = POOL.spawn_handle(|| panic!("Oh no!")); // We can panic, if we want

assert!(handle.wait().unwrap().is_err()); // In that case we'll get error, but thread will be ok

let handle = POOL.spawn_handle(|| {});

POOL.set_threads(0);

assert!(handle.wait().unwrap().is_ok());
std::thread::sleep(SECOND);
```
