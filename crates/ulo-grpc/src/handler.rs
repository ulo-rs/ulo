use std::future::Future;
use std::sync::Arc;

use ulo::{BoxError, BoxFuture, HandlerSpec, Shape};

use crate::method::Method;
use crate::transport::{Grpc, GrpcCx, Reply};

/// A handler's call: extraction, the controller, the handler and the reply, run by `dispatch`
/// after every guard admits.
pub(crate) type HandlerFn = Arc<dyn Fn(GrpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// One gRPC handler: the value a controller's [`mount`](ulo::Controller::mount) passes to
/// [`HandlerSpec::new`](ulo::HandlerSpec::new), which the server reads back when it prepares.
/// `#[ulo_grpc::method(Marker)]` builds this value, and a controller written without it builds it
/// by hand.
///
/// The method is the marker `M` that `ulo-build` generated, `pb::user_service::GetUser`: the
/// server answers `M::PATH` with the call, and [`spec`](Self::spec) writes `M::PATH` and `M::SHAPE`
/// into the spec as its [`route`](ulo::HandlerSpec::route) and [`shape`](ulo::HandlerSpec::shape),
/// which [`HandlerInfo`](ulo::HandlerInfo) reports. The spec takes the rest as for every
/// transport: enhancers, declared metadata and what the call reads from the container. A spec
/// whose route or shape differs from the marker's, one built by `HandlerSpec::new` with neither
/// set among them, is refused when the server prepares, naming both.
///
/// The call runs once per call, after every guard admits, and inside the interceptors. It reads
/// its request through [`FromCall`](crate::FromCall), one of [`Message<T>`](crate::Message),
/// [`Request<T>`](crate::Request) and [`Streaming<T>`](crate::Streaming), its controller through
/// `cx.exec().get::<C>()`, and answers a [`Reply`]: [`GrpcCx::reply`] encodes one message or a
/// [`Response`](crate::Response) around one, and [`GrpcCx::reply_stream`] a stream of them. The
/// attribute checks the message types and the shape against `M` at compile time; a call written
/// by hand gets no such check, and the request is decoded by the extractor it names and the reply
/// encoded as the call answers it. An error that classifies itself is converted with
/// [`CallError::from`](crate::CallError) first, as the attribute does, since a `BoxError` carries
/// no kind of its own and renders as INTERNAL unclaimed.
///
/// ```
/// use ulo::scope::Auto;
/// use ulo::{BoxError, Construct, ConstructError, Controller, Dependencies, Mount, Resolver};
/// use ulo_grpc::{FromCall, Grpc, GrpcCx, GrpcHandler, Message, Reply, Shape};
///
/// #[derive(Clone, PartialEq, prost::Message)]
/// pub struct GetUserRequest {
///     #[prost(uint64, tag = "1")]
///     pub id: u64,
/// }
///
/// #[derive(Clone, PartialEq, prost::Message)]
/// pub struct User {
///     #[prost(uint64, tag = "1")]
///     pub id: u64,
/// }
///
/// // The marker `ulo-build` writes for `rpc GetUser(GetUserRequest) returns (User)`.
/// pub struct GetUser;
///
/// impl ulo_grpc::Method for GetUser {
///     const PATH: &'static str = "/users.v1.UserService/GetUser";
///     const SHAPE: Shape = Shape::Unary;
///     type Request = GetUserRequest;
///     type Response = User;
/// }
///
/// struct Users;
///
/// impl Construct for Users {
///     type Scope = Auto;
///
///     fn dependencies(_: &mut Dependencies) {}
///
///     async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
///         Ok(Users)
///     }
/// }
///
/// async fn get_user(cx: GrpcCx) -> Result<Reply, BoxError> {
///     let Message(request) = <Message<GetUserRequest> as FromCall<Grpc>>::from_call(&cx).await?;
///     cx.exec().get::<Users>().await?;
///     Ok(cx.reply(User { id: request.id }))
/// }
///
/// impl Controller for Users {
///     fn mount(m: &mut Mount<'_>) {
///         m.handler(GrpcHandler::new::<GetUser, _, _>(get_user).spec("get_user"));
///     }
/// }
/// ```
pub struct GrpcHandler {
    pub(crate) path: &'static str,
    pub(crate) shape: Shape,
    pub(crate) call: HandlerFn,
}

impl GrpcHandler {
    /// The handler answering the method `M` with `call`.
    pub fn new<M, F, Fut>(call: F) -> Self
    where
        M: Method,
        F: Fn(GrpcCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply, BoxError>> + Send + 'static,
    {
        let call: HandlerFn = Arc::new(move |cx| Box::pin(call(cx)));
        GrpcHandler { path: M::PATH, shape: M::SHAPE, call }
    }

    /// The spec mounting this handler as `name`, the method's identifier as reports print it, with
    /// the marker's path as its route and the marker's shape as its shape.
    pub fn spec(self, name: &'static str) -> HandlerSpec<Grpc, GrpcHandler> {
        let (path, shape) = (self.path, self.shape);
        HandlerSpec::new(name, self).route(path).shape(shape)
    }
}
