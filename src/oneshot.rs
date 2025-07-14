use core::{time, ptr, task, pin};
use core::cell::{Cell, UnsafeCell};
use core::mem::MaybeUninit;
use core::sync::atomic::{Ordering, AtomicU8};
use core::future::Future;

const UNINIT: u8 = 0;
const READY: u8 = 0b0001;
const WAKER_SET: u8 = 0b0010;
const SEND_CLOSED: u8 = 0b0100;
const CONSUMED: u8 = 0b1000;

use super::JoinError;

enum Notifier {
    Thread(std::thread::Thread),
    Waker(core::task::Waker),
}

struct Payload<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
    notifier: Cell<Option<Notifier>>
}

unsafe impl<T> Send for Payload<T> {}
unsafe impl<T> Sync for Payload<T> {}

impl<T> Payload<T> {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(UNINIT),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            notifier: Cell::new(None),
        }
    }
}

impl<T> Drop for Payload<T> {
    fn drop(&mut self) {
        let state = self.state.load(Ordering::Acquire);
        match (state & READY == READY) && (state & CONSUMED != CONSUMED) {
            true => unsafe {
                ptr::drop_in_place((*self.value.get()).as_mut_ptr());
            },
            _ => (),
        }
    }
}

pub struct Sender<T> {
    payload: std::sync::Arc<Payload<T>>,
}

impl<T> Sender<T> {
    pub fn send(self, value: T) {
        //there is always only one sender
        unsafe {
            ptr::write((*self.payload.value.get()).as_mut_ptr(), value);
        }

        self.payload.state.fetch_or(READY, Ordering::Release);

        if self.payload.state.load(Ordering::Acquire) & WAKER_SET == WAKER_SET {
            match self.payload.notifier.take() {
                Some(Notifier::Thread(thread)) => thread.unpark(),
                Some(Notifier::Waker(waker)) => waker.wake(),
                _ => unreachable!(),
            }

            self.payload.state.fetch_and(!WAKER_SET, Ordering::Release);
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.payload.state.load(Ordering::Acquire) & READY != READY {
            //If we're already ready, closing here no longer matters
            self.payload.state.fetch_or(SEND_CLOSED, Ordering::Release);
        }

        if self.payload.state.load(Ordering::Acquire) & WAKER_SET == WAKER_SET {
            match self.payload.notifier.take() {
                Some(Notifier::Thread(thread)) => thread.unpark(),
                Some(Notifier::Waker(waker)) => waker.wake(),
                _ => unreachable!(),
            }

            self.payload.state.fetch_and(!WAKER_SET, Ordering::Release);
        }
    }
}

pub struct Receiver<T> {
    payload: std::sync::Arc<Payload<T>>,
}

impl<T> Receiver<T> {
    fn consume(&self) -> T {
        self.payload.state.fetch_or(CONSUMED, Ordering::Release);
        let mut result = MaybeUninit::uninit();

        unsafe {
            ptr::swap(result.as_mut_ptr(), (*self.payload.value.get()).as_mut_ptr());

            result.assume_init()
        }
    }

    pub fn recv(self) -> Result<T, JoinError> {
        let state = self.payload.state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return Err(JoinError::AlreadyConsumed);
        } else if state & READY == READY {
            return Ok(self.consume());
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return Err(JoinError::Disconnect);
        }

        self.payload.notifier.set(Some(Notifier::Thread(std::thread::current())));
        self.payload.state.fetch_or(WAKER_SET, Ordering::Release);

        while self.payload.state.load(Ordering::Acquire) & READY != READY {
            std::thread::park();

            //We should wake up on drop, otherwise receiver is stuck
            if self.payload.state.load(Ordering::Acquire) & SEND_CLOSED == SEND_CLOSED {
                return Err(JoinError::Disconnect);
            }
        }

        Ok(self.consume())
    }

    pub fn recv_timeout(&self, time: time::Duration) -> Result<T, JoinError> {
        let state = self.payload.state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return Err(JoinError::AlreadyConsumed);
        } else if state & READY == READY {
            return Ok(self.consume());
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return Err(JoinError::Disconnect);
        }

        self.payload.notifier.set(Some(Notifier::Thread(std::thread::current())));
        self.payload.state.fetch_or(WAKER_SET, Ordering::Release);

        if self.payload.state.load(Ordering::Acquire) & READY != READY {
            std::thread::park_timeout(time);
        }
        self.payload.state.fetch_and(!WAKER_SET, Ordering::Release);

        if self.payload.state.load(Ordering::Acquire) & READY == READY {
            Ok(self.consume())
        } else {
            Err(JoinError::Timeout)
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: pin::Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let state = self.payload.state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return task::Poll::Ready(Err(JoinError::AlreadyConsumed));
        } else if state & READY == READY {
            return task::Poll::Ready(Ok(self.consume()));
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return task::Poll::Ready(Err(JoinError::Disconnect));
        }

        if self.payload.state.load(Ordering::Acquire) & WAKER_SET != WAKER_SET {
            self.payload.notifier.set(Some(Notifier::Waker(cx.waker().clone())));
            self.payload.state.fetch_or(WAKER_SET, Ordering::Release);
        }

        //Just in case double-check
        if self.payload.state.load(Ordering::Acquire) & SEND_CLOSED == SEND_CLOSED {
            task::Poll::Ready(Err(JoinError::Disconnect))
        } else if self.payload.state.load(Ordering::Acquire) & READY == READY {
            self.payload.state.fetch_and(!WAKER_SET, Ordering::Release);
            task::Poll::Ready(Ok(self.consume()))
        } else {
            task::Poll::Pending
        }
    }
}

pub fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
    let payload = std::sync::Arc::new(Payload::new());

    let sender = Sender {
        payload: payload.clone(),
    };

    let receiver = Receiver {
        payload,
    };

    (sender, receiver)
}
