use ulo::BoxError;

/// Wraps in `ulo::Permanent` the errors retrying cannot help and leaves the rest bare.
///
/// diesel-async does not tell them apart by variant: wrong credentials and an unknown database both
/// arrive as `DatabaseError(Unknown, message)` with the SQLSTATE lost. `UnableToSendCommand` and
/// `BadConnection` are retryable.
pub(crate) fn permanent_or_not(error: BoxError) -> BoxError {
    let _ = error;
    todo!()
}
