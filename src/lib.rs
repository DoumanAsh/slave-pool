//! Simple thread pool
//!
//! # Usage
//!
//! ```rust
//! use slave_pool::ThreadPool;
//! const SECOND: core::time::Duration = core::time::Duration::from_secs(1);
//!
//! static POOL: ThreadPool = ThreadPool::new();
//!
//! POOL.set_threads(8); //Tell how many threads you want
//!
//! let mut handles = Vec::new();
//! for idx in 0..8 {
//!     handles.push(POOL.spawn_handle(move || {
//!         std::thread::sleep(SECOND);
//!         idx
//!     }));
//! }
//!
//! POOL.set_threads(0); //Tells to shut down threads
//!
//! for (idx, handle) in handles.drain(..).enumerate() {
//!     assert_eq!(handle.wait().unwrap(), idx) //Even though we told  it to shutdown all threads, it is going to finish queued job first
//! }
//!
//! let handle = POOL.spawn_handle(|| {});
//! assert!(handle.wait_timeout(SECOND).is_err()); // All are shutdown now
//!
//! POOL.set_threads(1); //But let's add one more
//!
//! assert!(handle.wait().is_ok());
//!
//! let handle = POOL.spawn_handle(|| panic!("Oh no!")); // We can panic, if we want
//!
//! assert!(handle.wait().is_err()); // In that case we'll get error, but thread will be ok
//!
//! let handle = POOL.spawn_handle(|| {});
//!
//! POOL.set_threads(0);
//!
//! assert!(handle.wait().is_ok());
//! ```

#![warn(missing_docs)]
#![cfg_attr(feature = "cargo-clippy", allow(clippy::style))]

use std::{thread, io};

use core::{ptr, time, fmt};
use core::mem::MaybeUninit;
use core::sync::atomic::{Ordering, AtomicUsize, AtomicU16};

mod spin;
mod oneshot;

#[derive(Debug)]
///Describes possible reasons for join to fail
pub enum JoinError {
    ///Job wasn't finished and aborted.
    Disconnect,
    ///Timeout expired, job continues.
    Timeout,
    ///Job was already consumed
    AlreadyConsumed,
}

///Handle to the job, allowing to await for it to finish
pub struct JobHandle<T> {
    inner: oneshot::Receiver<T>
}

impl<T> fmt::Debug for JobHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JobHandle")
    }
}

impl<T> JobHandle<T> {
    #[inline]
    ///Awaits for job to finish indefinitely.
    pub fn wait(self) -> Result<T, JoinError> {
        self.inner.recv()
    }

    #[inline]
    ///Awaits for job to finish for limited time.
    pub fn wait_timeout(&self, timeout: time::Duration) -> Result<T, JoinError> {
        self.inner.recv_timeout(timeout)
    }
}

impl<T> core::future::Future for JobHandle<T> {
    type Output = Result<T, JoinError>;

    #[inline]
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<Self::Output> {
        let inner = unsafe {
            self.map_unchecked_mut(|this| &mut this.inner)
        };

        core::future::Future::poll(inner, cx)
    }
}

enum Message {
    Execute(Box<dyn FnOnce() + Send + 'static>),
    Shutdown,
}

struct State {
    send: crossbeam_channel::Sender<Message>,
    recv: crossbeam_channel::Receiver<Message>,
}

///Thread pool that allows to change number of threads at runtime.
///
///On `Drop` it instructs threads to shutdown, but doesn't await for them to finish
///
///# Note
///
///The pool doesn't implement any sort of flow control.
///If workers are busy, message will remain in queue until any other thread can take it.
///
///# Clone
///
///Thread pool intentionally doesn't implement `Clone`
///If you want to share it, then share it by using global variable.
///
///# Panic
///
///Each thread wraps execution of job into `catch_unwind` to ensure that thread is not aborted
///on panic
pub struct ThreadPool {
    stack_size: AtomicUsize,
    thread_num: AtomicU16,
    thread_num_lock: spin::Lock,
    name: &'static str,
    init_lock: std::sync::Once,
    state: MaybeUninit<State>,
}

impl ThreadPool {
    ///Creates new thread pool with default params
    pub const fn new() -> Self {
        Self::with_defaults("", 0)
    }

    ///Creates new instance by specifying all params
    pub const fn with_defaults(name: &'static str, stack_size: usize) -> Self {
        Self {
            stack_size: AtomicUsize::new(stack_size),
            thread_num: AtomicU16::new(0),
            thread_num_lock: spin::Lock::new(),
            name,
            init_lock: std::sync::Once::new(),
            state: MaybeUninit::uninit(),
        }
    }

    fn get_state(&self) -> &State {
        self.init_lock.call_once(|| {
            let (send, recv) = crossbeam_channel::unbounded();
            unsafe {
                ptr::write(self.state.as_ptr() as *mut State, State {
                    send,
                    recv,
                });
            }
        });

        unsafe {
            &*self.state.as_ptr()
        }
    }

    #[inline]
    ///Sets stack size to use.
    ///
    ///By default it uses default value, used by Rust's stdlib.
    ///But setting this variable overrides it, allowing to customize it.
    ///
    ///This setting takes effect only when creating new threads
    pub fn set_stack_size(&self, stack_size: usize) -> usize {
        self.stack_size.swap(stack_size, Ordering::AcqRel)
    }

    ///Sets worker number, starting new threads if it is greater than previous
    ///
    ///In case if it is less, extra threads are shut down.
    ///Returns previous number of threads.
    ///
    ///By default when pool is created no threads are started.
    ///
    ///If any thread fails to start, function returns immediately with error.
    ///
    ///# Note
    ///
    ///Any calls to this method are serialized, which means under hood it locks out
    ///any attempt to change number of threads, until it is done
    pub fn set_threads(&self, thread_num: u16) -> io::Result<u16> {
        let mut _guard = self.thread_num_lock.lock();
        let old_thread_num = self.thread_num.load(Ordering::Relaxed);
        self.thread_num.store(thread_num, Ordering::Relaxed);

        if old_thread_num > thread_num {
            let state = self.get_state();

            let shutdown_num = old_thread_num - thread_num;
            for _ in 0..shutdown_num {
                if state.send.send(Message::Shutdown).is_err() {
                    break;
                }
            }

        } else if thread_num > old_thread_num {
            let create_num = thread_num - old_thread_num;
            let stack_size = self.stack_size.load(Ordering::Acquire);
            let state = self.get_state();

            for num in 0..create_num {
                let recv = state.recv.clone();

                let builder = match self.name {
                    "" => thread::Builder::new(),
                    name => thread::Builder::new().name(name.to_owned()),
                };

                let builder = match stack_size {
                    0 => builder,
                    stack_size => builder.stack_size(stack_size),
                };

                let result = builder.spawn(move || loop { match recv.recv() {
                    Ok(Message::Execute(job)) => {
                        //TODO: for some reason closures has no impl, wonder why?
                        let job = std::panic::AssertUnwindSafe(job);
                        let _ = std::panic::catch_unwind(|| (job.0)());
                    },
                    Ok(Message::Shutdown) | Err(_) => break,
                }});

                match result {
                    Ok(_) => (),
                    Err(error) => {
                        self.thread_num.store(old_thread_num + num, Ordering::Relaxed);
                        return Err(error);
                    }
                }
            }
        }

        Ok(old_thread_num)
    }

    ///Schedules new execution, sending it over to one of the workers.
    pub fn spawn<F: FnOnce() + Send + 'static>(&self, job: F) {
        let state = self.get_state();
        let _ = state.send.send(Message::Execute(Box::new(job)));
    }

    ///Schedules execution, that allows to await and receive it's result.
    pub fn spawn_handle<R: Send + 'static, F: FnOnce() -> R + Send + 'static>(&self, job: F) -> JobHandle<R> {
        let (send, recv) = oneshot::oneshot();
        let job = move || {
            let _ = send.send(job());
        };
        let _ = self.get_state().send.send(Message::Execute(Box::new(job)));

        JobHandle {
            inner: recv
        }
    }
}

impl fmt::Debug for ThreadPool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ThreadPool {{ threads: {} }}", self.thread_num.load(Ordering::Relaxed))
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.get_state();
        unsafe {
            ptr::drop_in_place(self.state.as_mut_ptr());
        }
    }
}
