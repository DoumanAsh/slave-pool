//! Thread pool

#![warn(missing_docs)]
#![cfg_attr(feature = "cargo-clippy", allow(clippy::style))]

use std::{thread, io};

use core::{ptr, time};
use core::mem::MaybeUninit;
use core::sync::atomic::{Ordering, AtomicU16, AtomicUsize};

#[derive(Debug)]
///Describes possible reasons for join to fail
pub enum JoinError {
    ///Job wasn't finished and aborted.
    Aborted,
    ///Timeout expired, job continues.
    Timeout,
}

impl Into<JoinError> for crossbeam_channel::RecvTimeoutError {
    fn into(self) -> JoinError {
        match self {
            crossbeam_channel::RecvTimeoutError::Timeout => JoinError::Timeout,
            crossbeam_channel::RecvTimeoutError::Disconnected => JoinError::Aborted,
        }
    }
}

///Handle to the job, allowing to await for it to finish
pub struct JobHandle<T> {
    inner: crossbeam_channel::Receiver<T>
}

impl<T> JobHandle<T> {
    #[inline]
    ///Awaits for job to finish indefinitely.
    pub fn wait(self) -> Result<T, JoinError> {
        self.inner.recv().map_err(|_| JoinError::Aborted)
    }

    #[inline]
    ///Awaits for job to finish for limited time.
    pub fn wait_timeout(&self, timeout: time::Duration) -> Result<T, JoinError> {
        self.inner.recv_timeout(timeout).map_err(|err| err.into())
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
    thread_num: AtomicU16,
    stack_size: AtomicUsize,
    name: &'static str,
    init_lock: parking_lot::Once,
    state: MaybeUninit<State>,
}

impl ThreadPool {
    ///Creates new thread pool with default params
    pub const fn new() -> Self {
        Self::with_defaults(0, "", 0)
    }

    ///Creates new instance by specifying all params
    pub const fn with_defaults(thread_num: u16, name: &'static str, stack_size: usize) -> Self {
        Self {
            thread_num: AtomicU16::new(thread_num),
            stack_size: AtomicUsize::new(stack_size),
            name,
            init_lock: parking_lot::Once::new(),
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

    ///Sets stack size to use.
    ///
    ///By default it uses default value, used by Rust's stdlib.
    ///But setting this variable overrides it, allowing to customize it.
    ///
    ///This setting takes effect only when creating new threads
    pub fn set_stack_size(&self, stack_size: usize) -> usize {
        let old_stack_size = self.stack_size.load(Ordering::Acquire);
        self.stack_size.store(stack_size, Ordering::Release);
        old_stack_size
    }

    ///Sets worker number, starting new threads if it is greater than previous
    ///
    ///In case if it is less, then it shutdowns.
    ///Returns previous number of threads.
    ///
    ///By default no threads are started.
    ///
    ///If any thread fails to start, function returns immediately with error.
    pub fn set_threads(&self, thread_num: u16) -> io::Result<u16> {
        let old_thread_num = self.thread_num.load(Ordering::Acquire);
        self.thread_num.store(thread_num, Ordering::Release);

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
                        self.thread_num.store(old_thread_num + num + 1, Ordering::Release);
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
        let (send, recv) = crossbeam_channel::bounded(1);
        let state = self.get_state();
        let job = move || {
            let _ = send.send(job());
        };
        let _ = state.send.send(Message::Execute(Box::new(job)));

        JobHandle {
            inner: recv
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.state.as_mut_ptr());
        }
    }
}