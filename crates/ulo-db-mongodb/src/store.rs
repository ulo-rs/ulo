use mongodb::Client;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::Store;

use crate::options::MongoOptions;

/// The MongoDB store.
///
/// `ErrorKind::Authentication` and `InvalidArgument` are permanent; `ServerSelection` is retryable.
/// Which failures of the SRV lookup in `create` are permanent is the area's to decide.
pub struct Mongo;

impl Store for Mongo {
    const NAME: &'static str = "mongodb";
    type Options = MongoOptions;
    type Client = Client;
    const CREATE_CONNECTS: bool = true;

    fn validate(o: &MongoOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URI's password component.
    fn secrets(o: &MongoOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    /// `ClientOptions::parse`, with the SRV lookup, then `Client::with_options`, on the app's
    /// runtime through `spawn_with`.
    fn create(o: &MongoOptions) -> BoxFuture<'static, Result<Client, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &Client) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: Client) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}
