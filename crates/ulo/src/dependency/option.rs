use crate::dependency::{FromContainer, Requirement};
use crate::error::LookupError;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Scope};

/// `None` exactly when `S` would fail with `LookupError::NotFound`: a key no module binds, an
/// extension no guard has written, an input this execution did not seed. Every other error
/// propagates: a construction failure, `ExecutionRequired`, an ambiguous key or module.
impl<S: FromContainer> FromContainer for Option<S> {
    fn describe(req: &mut Requirement) {
        req.optional::<S>();
    }

    fn from_container(r: &Resolver<'_>) -> impl std::future::Future<Output = Result<Self, LookupError>> + Send {
        // The inner read is made outside the `async` block: an `async fn` here fails the `Send`
        // proof for `Option<Dep<dyn Trait>>` (E0477), where this form does not.
        let read = S::from_container(r);
        async move {
            match read.await {
                Ok(value) => Ok(Some(value)),
                Err(LookupError::NotFound { .. }) => Ok(None),
                Err(other) => Err(other),
            }
        }
    }
}

impl<X: AllowedIn<S>, S: Scope> AllowedIn<S> for Option<X> {}
