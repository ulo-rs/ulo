//! Transport-neutral GraphQL (transports DESIGN §7): the [`Engine`] SPI, [`GqlRequest`] and
//! [`GqlResponse`]. The specification defines the language, validation and execution, and
//! GraphQL-over-HTTP (`ulo-graphql-http`) and graphql-transport-ws (`ulo-graphql-ws`) are two
//! bindings; an RPC application serves the same engine from an ordinary handler:
//!
//! ```ignore
//! # // reason: names a handler on ulo-rpc, which this crate does not depend on
//! #[ulo_rpc::message("gql")]
//! async fn gql(&self, req: Payload<GqlRequest>, engine: Dep<dyn Engine>) -> GqlResponse { /* .. */ }
//! ```
//!
//! An engine adapter, `ulo-graphql-async-graphql` or `ulo-graphql-juniper`, is bound under the SPI
//! key, `AsyncGraphql<ApiSchema, GqlContext> as dyn Engine`, so a handler on any transport takes
//! `Dep<dyn Engine>`; a second schema is bound with the DI's ordinary qualifier.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ulo::{BoxFuture, ExecutionRef};

pub use futures_core::stream::BoxStream;

/// A GraphQL engine, written without macros. Each call runs inside `exec`, the execution the
/// transport opened, from which the adapter resolves the schema's context type per call.
pub trait Engine: Send + Sync + 'static {
    /// Parses, validates and executes a query or mutation.
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse>;

    /// A subscription's responses, one per event; a request that fails before execution yields one
    /// `RequestError` or `Failed` response and ends.
    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse>;

    /// The schema in SDL.
    fn sdl(&self) -> String;

    /// The engine's own playground page for `endpoint` and, where subscriptions are served,
    /// `subscriptions`; `None` serves no page.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String>;
}

/// A GraphQL request, as GraphQL-over-HTTP's JSON body and graphql-transport-ws's `subscribe`
/// payload carry it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GqlRequest {
    pub query: String,
    #[serde(rename = "operationName", default, skip_serializing_if = "Option::is_none")]
    pub operation_name: Option<String>,
    /// `null` and absent alike read as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

impl GqlRequest {
    pub fn new(query: impl Into<String>) -> Self {
        GqlRequest { query: query.into(), ..GqlRequest::default() }
    }
}

/// A GraphQL response, serialized as `{data, errors, extensions}` without its outcome and fault,
/// which the HTTP binding reads to decide the status.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GqlResponse {
    #[serde(skip)]
    outcome: Outcome,
    #[serde(skip)]
    fault: Option<RequestFault>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<GqlError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<Value>,
}

impl GqlResponse {
    /// A request that executed: its data, `null` where execution failed, and its field errors.
    pub fn executed(data: Value, errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::Executed, fault: None, data: Some(data), errors, extensions: None }
    }

    /// A request that failed before execution began, for a fault in it that the engine does not
    /// name. It carries no `data`. The HTTP binding answers it as the commonest request error,
    /// one in a document that parses, so an engine whose document does not parse answers
    /// [`request_fault`](Self::request_fault) with [`RequestFault::Syntax`] instead.
    pub fn request_error(errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::RequestError, fault: None, data: None, errors, extensions: None }
    }

    /// A request that failed before execution began, for `fault`. It carries no `data`.
    pub fn request_fault(fault: RequestFault, errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::RequestError, fault: Some(fault), data: None, errors, extensions: None }
    }

    /// A request the server could not run for a fault of its own, before execution began: a
    /// context that cannot be built. It carries no `data`, and nothing in the document caused it.
    pub fn failed(errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::Failed, fault: None, data: None, errors, extensions: None }
    }

    pub fn with_extensions(mut self, extensions: Value) -> Self {
        self.extensions = Some(extensions);
        self
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// What stopped a request error, when the engine named it; `None` for every other outcome.
    pub fn fault(&self) -> Option<RequestFault> {
        self.fault
    }

    pub fn data(&self) -> Option<&Value> {
        self.data.as_ref()
    }

    pub fn errors(&self) -> &[GqlError] {
        &self.errors
    }

    pub fn extensions(&self) -> Option<&Value> {
        self.extensions.as_ref()
    }
}

/// Whether execution began, and if not whose fault stopped it, which decides the HTTP status:
/// 200 for `Executed`, errors included, 400 or 422 for `RequestError` by its [`RequestFault`], and
/// 500 for `Failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    Executed,
    /// The request failed before execution for a fault in it: a document that does not parse or
    /// validate, a missing `query`.
    RequestError,
    /// The request failed before execution for a fault in the server.
    Failed,
}

/// The fault in a request that stopped it before execution, one for each request error the
/// GraphQL-over-HTTP draft gives its own status. A binding renders it; over HTTP `Encoding` and
/// `Syntax` are 400 and the others 422.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestFault {
    /// The request's encoding does not parse: a JSON body that is not JSON.
    Encoding,
    /// The request is not a well-formed GraphQL request: no `query`, or a parameter of the wrong
    /// type, such as `variables` that is not a map.
    NotWellFormed,
    /// The GraphQL document does not parse.
    Syntax,
    /// The document does not pass GraphQL validation, the engine's own further rules, such as a
    /// depth limit, included.
    Validation,
    /// The operation to execute cannot be determined: several and none named, or a name that
    /// matches none, or one the call cannot run, a subscription asked to execute once.
    Operation,
    /// The variable values cannot be coerced to the operation's variable definitions.
    Coercion,
}

/// One GraphQL error, as the specification's response format writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GqlError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

impl GqlError {
    pub fn new(message: impl Into<String>) -> Self {
        GqlError { message: message.into(), locations: Vec::new(), path: Vec::new(), extensions: None }
    }
}

/// A position in the request document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub line: u32,
    pub column: u32,
}

/// One step of an error's `path`: a field name or a list index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSegment {
    Field(String),
    Index(usize),
}
