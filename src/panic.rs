//! Panic utilities

use core::fmt;

pub use std::panic::{catch_unwind, AssertUnwindSafe};

///Common panic message interface
pub trait Message: fmt::Debug + fmt::Display + Send {}

impl<T: fmt::Display + fmt::Debug + Send> Message for T {}

#[repr(transparent)]
///Panic error
pub struct Panic(pub Box<dyn core::any::Any + Send + 'static>);

impl Panic {
    #[inline]
    ///Attempts to downcast panic error into most common string types
    pub fn downcast_ref(&self) -> &(dyn Message + '_) {
        const DEFAULT_MESSAGE: &'static str = "panic occurred";
        match self.0.downcast_ref::<&'static str>() {
            Some(message) => message,
            None => match self.0.downcast_ref::<String>() {
                Some(message) => message,
                None => &DEFAULT_MESSAGE,
            },
        }
    }

    #[inline(always)]
    ///Retrieves original boxed panic error
    pub fn as_inner(&self) -> &Box<dyn core::any::Any + Send + 'static> {
        &self.0
    }

    #[inline(always)]
    ///Retrieves original boxed panic error
    pub fn into_inner(self) -> Box<dyn core::any::Any + Send + 'static> {
        self.0
    }
}

impl fmt::Debug for Panic {
    #[inline(always)]
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.downcast_ref(), fmt)
    }
}

impl fmt::Display for Panic {
    #[inline(always)]
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.downcast_ref(), fmt)
    }
}
