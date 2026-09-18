//! What a handler may return, per transport.
//!
//! [`Transport::Output`](crate::dispatch::Transport) is what the pipeline carries; this is what a
//! handler is allowed to write instead. One mechanism serves HTTP, RPC and WebSocket.
//!
//! Conversion is fallible because one transport's is: turning an arbitrary value into `RpcData`
//! runs serde. A failure is the transport's own error, so it reaches the error chain like any
//! other.

use crate::dispatch::transport::{Answer, Transport};

/// A value a handler may return on transport `T`.
///
/// Implement this to make a type returnable from a handler. Answering `Answer<T>` rather than
/// `T::Output` is what keeps a failed conversion on the error path.
pub trait IntoOutput<T: Transport> {
    fn into_output(self) -> Answer<T>;
}

/// A handler's own `Result`, with the error placed on the transport's error side.
///
/// This is the impl that keeps `#[catch]` reachable. An error a handler returns becomes
/// `T::Error` and reaches the chain, whatever the handler's return type is spelled as — the
/// conversion has no way to render an error itself, so no spelling can route around the chain.
impl<T, V, E> IntoOutput<T> for Result<V, E>
where
    T: Transport,
    V: IntoOutput<T>,
    E: Into<T::Error>,
{
    fn into_output(self) -> Answer<T> {
        match self {
            Ok(value) => value.into_output(),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::transport::Http;
    use crate::http::{Body, HttpError};

    #[derive(Debug)]
    struct Domain;
    impl std::fmt::Display for Domain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "domain")
        }
    }
    impl std::error::Error for Domain {}
    impl crate::Error for Domain {
        fn kind(&self) -> crate::ErrorKind {
            crate::ErrorKind::Conflict
        }
    }

    /// A handler's `Err` reaches the error side whatever the return type is spelled as.
    #[test]
    fn a_returned_error_lands_on_the_error_side() {
        let answer: Answer<Http> = Err::<Body, Domain>(Domain).into_output();
        assert!(matches!(answer, Err(HttpError::AppError(_))));
    }

    #[test]
    fn a_returned_value_lands_on_the_output_side() {
        let answer: Answer<Http> = Ok::<Body, Domain>(Body::text("hi")).into_output();
        assert!(answer.is_ok());
    }
}
