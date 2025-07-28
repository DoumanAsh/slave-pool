//! Underlying oneshot implementation
use std::time;
use core::{ptr, task, pin};
use core::cell::{Cell, UnsafeCell};
use core::mem::MaybeUninit;
use core::sync::atomic::{Ordering, AtomicU8};
use core::future::Future;

const UNINIT: u8 = 0;
const READY: u8 = 0b00001;
const WAKER_SET: u8 = 0b00010;
const SEND_CLOSED: u8 = 0b00100;
const CONSUMED: u8 = 0b01000;
const RECV_CLOSED: u8 = 0b10000;

use super::JoinError;

enum Notifier {
    Thread(std::thread::Thread),
    Waker(core::task::Waker),
}

struct Payload<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
    notifier: Cell<MaybeUninit<Notifier>>
}

impl<T> Payload<T> {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(UNINIT),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            notifier: Cell::new(MaybeUninit::uninit()),
        }
    }

    #[inline(never)]
    ///Sets notifier, updates state and returns previous state
    fn set_notifier(&self, notifier: Notifier) -> u8 {
        self.notifier.set(MaybeUninit::new(notifier));
        self.state.fetch_or(WAKER_SET, Ordering::AcqRel)
    }

    #[inline(always)]
    fn take_notifier(&self) -> Notifier {
        let storage = self.notifier.replace(MaybeUninit::uninit());

        unsafe {
            storage.assume_init()
        }
    }
}

impl<T> Drop for Payload<T> {
    fn drop(&mut self) {
        let state = self.state.load(Ordering::Relaxed);
        match (state & READY == READY) && (state & CONSUMED != CONSUMED) {
            true => unsafe {
                ptr::drop_in_place((*self.value.get()).as_mut_ptr());
            },
            _ => (),
        }

        //If no one is interested in waker, then just drop it without waking up
        if state & WAKER_SET == WAKER_SET {
            self.take_notifier();
        }
    }
}

#[repr(transparent)]
///Sender end, allows to send message once
///
///On `Drop` will notify `Receiver`
pub struct Sender<T> {
    payload: ptr::NonNull<Payload<T>>,
}

impl<T> Sender<T> {
    #[inline(always)]
    fn payload(&self) -> &Payload<T> {
        unsafe  {
            &*self.payload.as_ptr()
        }
    }

    ///Performs send of the message, waking receiver, if it awaits
    pub fn send(self, value: T) {
        //there is always only one sender
        unsafe {
            ptr::write((*self.payload().value.get()).as_mut_ptr(), value);
        }

        let state = self.payload().state.fetch_or(READY, Ordering::AcqRel);
        if state & WAKER_SET == WAKER_SET {
            let notifier = self.payload().take_notifier();
            self.payload().state.fetch_and(!WAKER_SET, Ordering::Release);

            match notifier {
                Notifier::Thread(thread) => thread.unpark(),
                Notifier::Waker(waker) => waker.wake(),
            }
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        //Make sure to guarantee we acquire RECV_CLOSED prior setting SEND_CLOSED
        let mut state = self.payload().state.load(Ordering::Acquire);
        if state & WAKER_SET == WAKER_SET {
            let notifier = self.payload().take_notifier();
            //Unset WAKER_SET and set SEND_CLOSED
            state = self.payload().state.fetch_xor(WAKER_SET | SEND_CLOSED, Ordering::AcqRel);

            match notifier {
                Notifier::Thread(thread) => thread.unpark(),
                Notifier::Waker(waker) => waker.wake(),
            }
        } else {
            state = self.payload().state.fetch_or(SEND_CLOSED, Ordering::AcqRel);
        }

        if state & RECV_CLOSED == RECV_CLOSED {
            unsafe {
                let _ = Box::from_raw(self.payload.as_ptr());
            }
        }
    }
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Sync> Sync for Sender<T> {}

#[repr(transparent)]
///Receiver end to receive message
///
///Implements `Future`
pub struct Receiver<T> {
    payload: ptr::NonNull<Payload<T>>,
}

impl<T> Receiver<T> {
    #[inline(always)]
    fn payload(&self) -> &Payload<T> {
        unsafe  {
            &*self.payload.as_ptr()
        }
    }

    fn consume(&self) -> T {
        self.payload().state.fetch_or(CONSUMED, Ordering::Release);
        let mut result = MaybeUninit::uninit();

        unsafe {
            ptr::swap(result.as_mut_ptr(), (*self.payload().value.get()).as_mut_ptr());

            result.assume_init()
        }
    }

    #[inline(always)]
    ///Returns whether job has been finished
    pub fn is_ready(&self) -> bool {
        self.payload().state.load(Ordering::Acquire) & READY == READY
    }

    #[inline(always)]
    ///Returns whether receiver has been 'consumed'
    pub fn is_consumed(&self) -> bool {
        self.payload().state.load(Ordering::Acquire) & CONSUMED == CONSUMED
    }

    ///Checks if message is received, returning it, if possible
    ///Otherwise returns `None`
    pub fn try_recv(&self) -> Result<Option<T>, JoinError> {
        let state = self.payload().state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            Err(JoinError::AlreadyConsumed)
        } else if state & READY == READY {
            Ok(Some(self.consume()))
        } else if state & SEND_CLOSED == SEND_CLOSED {
            Err(JoinError::Disconnect)
        } else {
            Ok(None)
        }
    }

    ///Awaits message blocking until message arrives, returning it
    ///Or if `Sender` closes unexpectedly (e.g. due to panic) returns `JoinError::Disconnect`
    pub fn recv(self) -> Result<T, JoinError> {
        let mut state = self.payload().state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return Err(JoinError::AlreadyConsumed);
        } else if state & READY == READY {
            return Ok(self.consume());
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return Err(JoinError::Disconnect);
        }

        state = self.payload().set_notifier(Notifier::Thread(std::thread::current()));

        while state & READY != READY {
            //Make sure we're not dropped yet
            if state & SEND_CLOSED == SEND_CLOSED {
                return Err(JoinError::Disconnect);
            }

            std::thread::park();

            state = self.payload().state.load(Ordering::Acquire);
        }

        Ok(self.consume())
    }

    ///Awaits message blocking for the duration of `time` until message arrives, returning it
    ///Or if `Sender` closes unexpectedly (e.g. due to panic) returns `JoinError::Disconnect`
    ///
    ///If timeout expires, returns `Ok(None)`
    pub fn recv_timeout(&self, mut time: time::Duration) -> Result<Option<T>, JoinError> {
        let mut state = self.payload().state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return Err(JoinError::AlreadyConsumed);
        } else if state & READY == READY {
            return Ok(Some(self.consume()));
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return Err(JoinError::Disconnect);
        }

        state = self.payload().set_notifier(Notifier::Thread(std::thread::current()));

        let start_time = time::Instant::now();
        while state & READY != READY {
            std::thread::park_timeout(time);

            if let Some(left_over) = time.checked_sub(start_time.elapsed()) {
                //If any time left reload state to check flag again before entering new loop
                time = left_over;
                state = self.payload().state.load(Ordering::Acquire);
            } else {
                break;
            }
        }
        state = self.payload().state.fetch_and(!WAKER_SET, Ordering::AcqRel);

        if state & WAKER_SET == WAKER_SET {
            self.payload().take_notifier();
        }

        if state & READY == READY {
            Ok(Some(self.consume()))
        } else {
            Ok(None)
        }
    }
}

impl<T> Drop for Receiver<T> {
    #[inline(always)]
    fn drop(&mut self) {
        //Make sure to guarantee we acquire SEND_CLOSED prior setting RECV_CLOSED
        let state = self.payload().state.fetch_or(RECV_CLOSED, Ordering::AcqRel);
        if state & SEND_CLOSED == SEND_CLOSED {
            unsafe {
                let _ = Box::from_raw(self.payload.as_ptr());
            }
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: pin::Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let mut state = self.payload().state.load(Ordering::Acquire);

        if state & CONSUMED == CONSUMED {
            return task::Poll::Ready(Err(JoinError::AlreadyConsumed));
        } else if state & READY == READY {
            return task::Poll::Ready(Ok(self.consume()));
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return task::Poll::Ready(Err(JoinError::Disconnect));
        }

        //Account for spontaneous wake up
        if state & WAKER_SET == WAKER_SET {
            state = self.payload().state.load(Ordering::Acquire);
        } else {
            state = self.payload().set_notifier(Notifier::Waker(cx.waker().clone()));
        }

        //Just in case double-check
        if state & CONSUMED == CONSUMED {
            return task::Poll::Ready(Err(JoinError::AlreadyConsumed));
        } else if state & READY == READY {
            return task::Poll::Ready(Ok(self.consume()));
        } else if state & SEND_CLOSED == SEND_CLOSED {
            return task::Poll::Ready(Err(JoinError::Disconnect));
        } else {
            task::Poll::Pending
        }
    }
}

unsafe impl<T: Send> Send for Receiver<T> {}
impl<T> Unpin for Receiver<T> {}

//Impossible to guarantee as we need to write waker without lock
//unsafe impl<T> Sync for Receiver<T> {}

///Creates new oneshot pipe
pub fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
    let payload = ptr::NonNull::from(Box::leak(Box::new(Payload::new())));

    let sender = Sender {
        payload,
    };

    let receiver = Receiver {
        payload,
    };

    (sender, receiver)
}
