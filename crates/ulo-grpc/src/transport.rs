use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use futures_core::Stream;
use tonic::Status;
use tonic::metadata::MetadataMap;
use ulo::{AppHandle, ExecutionRef, Ext, Extensions, Inputs, LookupError, MountedHandler, Timer, Transport};
use ulo_http::{HttpBody, TlsInfo};
use ulo_transport::{CallError, ExtractError, FromCall, IntoReplyError};

use crate::dispatch;
use crate::metadata::{MetadataEntry, MetadataError};

/// The gRPC transport, key `"grpc"`: one execution per call. `#[guards(grpc = ..)]` scopes an entry
/// to its handlers. Its inputs, [`GrpcMetadata`] and [`PeerAddr`], are seeded into every call's
/// execution and declared by the transport itself.
pub struct Grpc;

impl Transport for Grpc {
    const KEY: &'static str = "grpc";

    type Cx = GrpcCx;
    type Reply = Reply;

    fn inputs(d: &mut Inputs) {
        d.input::<GrpcMetadata>().input::<PeerAddr>();
    }
}

impl ulo_http::__private::Carried for Grpc {}

/// gRPC's calls are HTTP/2 requests, so the pre-dispatch stage runs for them as
/// `PreDispatch<Grpc>`, a metadata key apart from HTTP's (X23).
impl ulo_http::HttpCarried for Grpc {}

/// What a handler answers, and what an interceptor's `next.run()` returns: the HTTP response
/// carrying the encoded message or stream. An interceptor reads and writes reply metadata through
/// its headers and cannot read the message, which lets one enhancer list serve every method of a
/// service. A stream item's `Err` runs `dispatch_late` and is written in the trailers. An error
/// handler builds one with [`GrpcCx::reply`], [`GrpcCx::reply_stream`] or
/// [`GrpcCx::reply_status`].
///
/// A header whose name begins `grpc-` is the protocol's. Once the interceptors have answered, a
/// reply built by a handler or through one of those three keeps the `grpc-` headers it was built
/// with: one an interceptor added is dropped, one it changed or removed is written back, and each
/// name is logged at `warn` the first time in the process, so a trailers-only reply's status
/// stands as the framework wrote it. The response a `PreDispatch<Grpc>` entry passes is screened
/// the same way once the stage answers, the health and reflection services' replies and the
/// dispatcher's own statuses included. The record is kept per writer: a name an interceptor and a
/// pre-dispatch entry both write is logged once for each, both on the first reply.
pub type Reply = http::Response<tonic::body::Body>;

/// One call's context: `Clone + Send + Sync`, every clone the same call. The execution, the method
/// path, the request metadata, the peer, and the app's `Timer`.
///
/// A guard reads the metadata through it, `cx.metadata().get("authorization")`, and passes data to
/// the handler through `cx.extensions()`. A streaming reply holds a clone, through which it reads
/// the execution and observes its cancellation while the stream runs; the server holds the
/// execution itself until the reply body is dropped.
#[derive(Clone)]
pub struct GrpcCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    /// The `:path` as the pre-dispatch stage left it.
    pub(crate) path: Arc<str>,
    /// The request headers as the pre-dispatch stage left them; the seeded `GrpcMetadata` input is
    /// what the client sent.
    pub(crate) metadata: GrpcMetadata,
    pub(crate) peer: Option<SocketAddr>,
    /// What the connection's TLS handshake settled, the client's verified certificate chain among
    /// it; `None` over plaintext.
    pub(crate) tls: Option<TlsInfo>,
    /// The matched handler, whose error handlers a stream item's `Err` reaches. `None` in the
    /// context a failure of the unscoped pre-dispatch sub-step is offered with.
    pub(crate) handler: Option<MountedHandler<Grpc>>,
    /// The request body, taken once by the extractor that decodes it.
    pub(crate) body: Mutex<Option<HttpBody>>,
    /// The encoding the request's `grpc-encoding` names, `None` for `identity`: a message whose
    /// compressed flag is set is decompressed in it.
    pub(crate) encoding: Option<tonic::codec::CompressionEncoding>,
}

/// What a call's context reads of the connection it arrived on.
#[derive(Clone)]
pub(crate) struct Caller {
    pub(crate) peer: Option<SocketAddr>,
    pub(crate) tls: Option<TlsInfo>,
}

impl Caller {
    pub(crate) fn of(conn: &ulo_http::ConnInfo) -> Self {
        Caller { peer: conn.peer, tls: conn.tls.clone() }
    }
}

impl GrpcCx {
    pub(crate) fn new(inner: CxInner) -> Self {
        GrpcCx { inner: Arc::new(inner) }
    }

    pub fn exec(&self) -> &ExecutionRef {
        &self.inner.exec
    }

    pub fn extensions(&self) -> &Extensions {
        self.inner.exec.extensions()
    }

    /// The extension `T` a guard wrote, as `Ext<T>` reads it.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        self.inner.exec.resolver().ext::<T>()
    }

    /// The path the caller dialled, `/users.v1.UserService/GetUser`.
    pub fn path(&self) -> &str {
        &self.inner.path
    }

    /// The request metadata.
    pub fn metadata(&self) -> &GrpcMetadata {
        &self.inner.metadata
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }

    /// What the TLS handshake of the call's connection settled, `None` over plaintext: the ALPN
    /// protocol, the SNI name, and the certificate chain the client presented once the server's
    /// client verifier accepted it, which a guard authorising by client certificate reads,
    /// `cx.tls().and_then(|tls| tls.peer_certificates.as_ref())`. Read off the TLS session, never
    /// off the call's metadata, which any client can write.
    pub fn tls(&self) -> Option<&TlsInfo> {
        self.inner.tls.as_ref()
    }

    pub fn app(&self) -> &AppHandle {
        &self.inner.app
    }

    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.inner.timer
    }

    /// One reply message, or [`Response`] around one with reply metadata, encoded as a handler
    /// returning it is answered: what an error handler claiming an error answers with a message of
    /// its own. A `Response` holding an entry its builder refused fails as `IntoReplyError`, as it
    /// does when a handler returns it.
    ///
    /// ```ignore
    /// # // reason: a fragment of an error handler: it needs a generated message type
    /// async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
    ///     match err.downcast_ref::<CallError>().and_then(|call| call.source_as::<NotCached>()) {
    ///         Some(_) => Ok(cx.reply(Response::new(pb::User::default()).metadata("x-cache", "miss"))?),
    ///         None => Err(err),
    ///     }
    /// }
    /// ```
    pub fn reply<V: IntoGrpcReply>(&self, value: V) -> Result<Reply, IntoReplyError> {
        value.into_grpc_reply()
    }

    /// A reply stream, or [`Response`] around one with reply metadata, as a handler returning it is
    /// answered: each `Ok` item written as a message, an `Err` item run through the matched
    /// handler's error handlers on the late path and written in the trailers, which end the
    /// stream. A `Response` holding an entry its builder refused fails as `IntoReplyError` before
    /// any message is written.
    pub fn reply_stream<S: IntoGrpcStream>(&self, stream: S) -> Result<Reply, IntoReplyError> {
        stream.into_grpc_stream(self)
    }

    /// `status` as a trailers-only reply. Answered with `Ok`, it claims the error and ends the
    /// error handlers; returned as the error instead, the next error handler is offered it and the
    /// wire sends it as it stands when none claims it.
    pub fn reply_status(&self, status: Status) -> Reply {
        dispatch::status_reply(status)
    }

    /// The request body, once: the first body-consuming extractor takes it.
    pub(crate) fn take_body(&self) -> Option<HttpBody> {
        self.inner.body.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl AsRef<ExecutionRef> for GrpcCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}

impl FromCall<Grpc> for GrpcCx {
    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        Ok(cx.clone())
    }
}

/// The call's request metadata, as an execution input and as a handler parameter: a token guard
/// reads a bearer token from it.
#[derive(Clone, Debug, Default)]
pub struct GrpcMetadata {
    pub(crate) map: MetadataMap,
}

impl GrpcMetadata {
    pub fn map(&self) -> &MetadataMap {
        &self.map
    }

    /// The ASCII value under `key`, `None` when absent or not ASCII.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).and_then(|value| value.to_str().ok())
    }

    pub(crate) fn from_headers(headers: &http::HeaderMap) -> Self {
        GrpcMetadata { map: MetadataMap::from_headers(headers.clone()) }
    }
}

/// The caller's address, as an execution input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// A reply message with reply metadata: `Response::new(user).metadata("x-cache", "miss")`. Wraps a
/// reply stream the same way, `Response::new(stream)`, for a streaming method's reply headers.
///
/// [`metadata`](Self::metadata) and [`metadata_entry`](Self::metadata_entry) write the reply's
/// headers, its initial metadata; [`trailer`](Self::trailer) and
/// [`trailer_entry`](Self::trailer_entry) its trailers, the trailing metadata written beside
/// `grpc-status` when the reply ends. A unary reply's trailers follow its message. A stream's are
/// set when the reply is built and written when the stream ends on its own: after its last item,
/// or with the status of an `Err` item, beside that status's own metadata. A stream the server
/// ends, at the deadline, at the drain's end or after a panic, ends with the status alone.
///
/// [`metadata`](Self::metadata) and [`trailer`](Self::trailer) build their entry as
/// [`MetadataEntry::ascii`] does, and a key or value it refuses, a reserved name among them, fails
/// the reply: converting the `Response` answers `IntoReplyError` holding the first refusal's
/// [`MetadataError`], which names the key and the reason, and nothing is written. The error
/// handlers see a `CallError` of kind `Internal` with that `IntoReplyError` as its source, and
/// unclaimed it is sent as INTERNAL with the message withheld, as HTTP sends a reply whose header
/// does not parse as a 500.
///
/// ```
/// use ulo_grpc::{MetadataEntry, MetadataError, Response};
///
/// let reply = Response::new(())
///     .metadata("x-cache", "miss")
///     .trailer("x-rows", "12")
///     .trailer_entry(MetadataEntry::binary("x-digest-bin", vec![0x00, 0xff])?);
/// # let _ = reply;
/// # Ok::<(), MetadataError>(())
/// ```
#[derive(Debug)]
pub struct Response<T> {
    pub(crate) message: T,
    pub(crate) metadata: MetadataMap,
    pub(crate) trailers: MetadataMap,
    /// The first entry `metadata` or `trailer` refused, which fails the reply.
    pub(crate) refused: Option<MetadataError>,
}

impl<T> Response<T> {
    pub fn new(message: T) -> Self {
        Response { message, metadata: MetadataMap::new(), trailers: MetadataMap::new(), refused: None }
    }

    /// One ASCII entry on the reply's headers, built as [`MetadataEntry::ascii`] builds it; a key
    /// or value it refuses fails the reply. A key written twice carries both values.
    pub fn metadata(mut self, key: &'static str, value: &str) -> Self {
        match MetadataEntry::ascii(key, value) {
            Ok(entry) => entry.append_to(&mut self.metadata),
            Err(refused) => self.refuse(refused),
        }
        self
    }

    /// One entry on the reply's headers, ASCII or binary. A key written twice carries both values.
    pub fn metadata_entry(mut self, entry: MetadataEntry) -> Self {
        entry.append_to(&mut self.metadata);
        self
    }

    /// One ASCII entry on the reply's trailers, built as [`MetadataEntry::ascii`] builds it; a key
    /// or value it refuses fails the reply. A key written twice carries both values.
    pub fn trailer(mut self, key: &'static str, value: &str) -> Self {
        match MetadataEntry::ascii(key, value) {
            Ok(entry) => entry.append_to(&mut self.trailers),
            Err(refused) => self.refuse(refused),
        }
        self
    }

    /// One entry on the reply's trailers, ASCII or binary. A key written twice carries both values.
    pub fn trailer_entry(mut self, entry: MetadataEntry) -> Self {
        entry.append_to(&mut self.trailers);
        self
    }

    pub fn into_inner(self) -> T {
        self.message
    }

    fn refuse(&mut self, refused: MetadataError) {
        self.refused.get_or_insert(refused);
    }

    /// The message, headers and trailers, or the first refusal as the conversion's failure.
    fn parts(self) -> Result<(T, MetadataMap, MetadataMap), IntoReplyError> {
        match self.refused {
            Some(refused) => Err(IntoReplyError::new(refused)),
            None => Ok((self.message, self.metadata, self.trailers)),
        }
    }
}

/// A reply of one message, what a unary or client-streaming handler returns and
/// [`GrpcCx::reply`] takes: the message, or [`Response`] around it with reply metadata. `()` is
/// one too, being prost's `google.protobuf.Empty`. The conversion fails for a `Response` holding
/// an entry its builder refused.
pub trait IntoGrpcReply: Send + 'static {
    type Message;
    fn into_grpc_reply(self) -> Result<Reply, IntoReplyError>;
}

impl<T: prost::Message + Default + Send + 'static> IntoGrpcReply for T {
    type Message = T;

    fn into_grpc_reply(self) -> Result<Reply, IntoReplyError> {
        Ok(dispatch::encode_one(self, MetadataMap::new(), MetadataMap::new()))
    }
}

impl<T: prost::Message + Default + Send + 'static> IntoGrpcReply for Response<T> {
    type Message = T;

    fn into_grpc_reply(self) -> Result<Reply, IntoReplyError> {
        let (message, metadata, trailers) = self.parts()?;
        Ok(dispatch::encode_one(message, metadata, trailers))
    }
}

/// A reply stream, what a server-streaming or bidi handler returns and [`GrpcCx::reply_stream`]
/// takes: any stream of [`IntoGrpcItem`]s, or [`Response`] around one with reply metadata. The
/// conversion fails for a `Response` holding an entry its builder refused.
pub trait IntoGrpcStream: Send + 'static {
    type Message;
    fn into_grpc_stream(self, cx: &GrpcCx) -> Result<Reply, IntoReplyError>;
}

impl<S> IntoGrpcStream for S
where
    S: Stream + Send + 'static,
    S::Item: IntoGrpcItem,
{
    type Message = <S::Item as IntoGrpcItem>::Message;

    fn into_grpc_stream(self, cx: &GrpcCx) -> Result<Reply, IntoReplyError> {
        Ok(dispatch::encode_stream(self, MetadataMap::new(), MetadataMap::new(), cx))
    }
}

impl<S> IntoGrpcStream for Response<S>
where
    S: Stream + Send + 'static,
    S::Item: IntoGrpcItem,
{
    type Message = <S::Item as IntoGrpcItem>::Message;

    fn into_grpc_stream(self, cx: &GrpcCx) -> Result<Reply, IntoReplyError> {
        let (stream, metadata, trailers) = self.parts()?;
        Ok(dispatch::encode_stream(stream, metadata, trailers, cx))
    }
}

/// One item of a reply stream: a message, or an error whose kind survives into the error handlers
/// on the late path, since it converts into a `CallError`. An item ending the stream with a
/// `tonic::Status` of its own, its trailing metadata included, is
/// [`from_status(status)`](crate::from_status).
pub trait IntoGrpcItem: Send + 'static {
    type Message: prost::Message + Default + Send + 'static;
    fn into_grpc_item(self) -> Result<Self::Message, CallError>;
}

impl<T, E> IntoGrpcItem for Result<T, E>
where
    T: prost::Message + Default + Send + 'static,
    E: Into<CallError> + Send + 'static,
{
    type Message = T;

    fn into_grpc_item(self) -> Result<T, CallError> {
        self.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    fn refusal(converted: Result<Reply, IntoReplyError>) -> MetadataError {
        let error = converted.err().unwrap_or_else(|| panic!("the reply converted"));
        error.source().and_then(|source| source.downcast_ref::<MetadataError>()).cloned().unwrap_or_else(|| panic!("no refusal in {error:?}"))
    }

    /// The first entry refused is the one the reply fails with, whichever position it is in.
    #[test]
    fn a_reply_fails_with_its_first_refusal() {
        let reply = Response::new(()).metadata("x-a", "1").trailer("te", "trailers").metadata("grpc-x", "v");
        assert_eq!(refusal(reply.into_grpc_reply()), MetadataError::Reserved { key: "te".into() });
    }
}
