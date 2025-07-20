use core::{future, task, time};

use slave_pool::{JoinError, oneshot};

const TIMEOUT: time::Duration = time::Duration::from_millis(50);

#[test]
fn should_correctly_receive_after_poll() {
    let (sender, receiver) = oneshot::oneshot();

    let waker = thread_waker::waker(std::thread::current());
    let mut fut = core::pin::pin!(receiver);
    let mut context = task::Context::from_waker(&waker);
    assert!(!future::Future::poll(fut.as_mut(), &mut context).is_ready());

    sender.send("should_be_received".to_owned());

    match future::Future::poll(fut.as_mut(), &mut context) {
        task::Poll::Pending => panic!("Should be ready"),
        task::Poll::Ready(result) => {
            assert_eq!(result.expect("should not fail"), "should_be_received");
        }
    }
}

#[test]
fn should_correctly_drop_receiver_after_poll() {
    let (sender, receiver) = oneshot::oneshot();

    let waker = thread_waker::waker(std::thread::current());
    let mut fut = core::pin::pin!(receiver);
    let mut context = task::Context::from_waker(&waker);
    assert!(!future::Future::poll(fut.as_mut(), &mut context).is_ready());

    sender.send("should_be_dropped".to_owned());
}

#[test]
fn should_correctly_error_on_send_dropped_after_poll() {
    let (sender, receiver) = oneshot::oneshot::<String>();

    let waker = thread_waker::waker(std::thread::current());
    let mut fut = core::pin::pin!(receiver);
    let mut context = task::Context::from_waker(&waker);
    assert!(!future::Future::poll(fut.as_mut(), &mut context).is_ready());

    drop(sender);

    match future::Future::poll(fut.as_mut(), &mut context) {
        task::Poll::Pending => panic!("Should be ready"),
        task::Poll::Ready(result) => {
            assert_eq!(result.expect_err("should not fail"), JoinError::Disconnect);
        }
    }
}

#[test]
fn should_correctly_receive_after_try() {
    let (sender, receiver) = oneshot::oneshot();

    assert!(receiver.try_recv().expect("ok").is_none());
    sender.send("should_be_received".to_owned());

    let result = receiver.try_recv().expect("ok").expect("message should be present");
    assert_eq!(result, "should_be_received");
}

#[test]
fn should_correctly_drop_receiver_after_try() {
    let (sender, receiver) = oneshot::oneshot();

    assert!(receiver.try_recv().expect("ok").is_none());
    sender.send("should_be_dropped".to_owned());
}

#[test]
fn should_correctly_error_on_send_dropped_after_try() {
    let (sender, receiver) = oneshot::oneshot::<String>();

    assert!(receiver.try_recv().expect("ok").is_none());
    drop(sender);

    let error = receiver.try_recv().expect_err("should disconnect");
    assert_eq!(error, JoinError::Disconnect);
}

#[test]
fn should_correctly_receive_after_timeout() {
    let (sender, receiver) = oneshot::oneshot();

    let error = receiver.recv_timeout(TIMEOUT).expect_err("should timeout");
    assert_eq!(error, JoinError::Timeout);

    sender.send("should_be_received".to_owned());

    let result = receiver.recv_timeout(TIMEOUT).expect("ok");
    assert_eq!(result, "should_be_received");
}

#[test]
fn should_correctly_drop_receiver_after_timeout() {
    let (sender, receiver) = oneshot::oneshot();

    let error = receiver.recv_timeout(TIMEOUT).expect_err("should timeout");
    assert_eq!(error, JoinError::Timeout);

    sender.send("should_be_dropped".to_owned());
}

#[test]
fn should_correctly_error_on_send_dropped_after_timeout() {
    let (sender, receiver) = oneshot::oneshot::<String>();

    let error = receiver.recv_timeout(TIMEOUT).expect_err("should timeout");
    assert_eq!(error, JoinError::Timeout);
    drop(sender);

    let error = receiver.recv_timeout(TIMEOUT).expect_err("should disconnect");
    assert_eq!(error, JoinError::Disconnect);
}

#[test]
fn should_correctly_drop_sender_after_receiver_poll() {
    let (sender, receiver) = oneshot::oneshot::<String>();

    {
        let waker = thread_waker::waker(std::thread::current());
        let mut fut = core::pin::pin!(receiver);
        let mut context = task::Context::from_waker(&waker);
        assert!(!future::Future::poll(fut.as_mut(), &mut context).is_ready());
    }

    drop(sender);
}
