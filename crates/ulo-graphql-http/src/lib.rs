//! GraphQL over HTTP for `ulo` (transports DESIGN §7): the endpoint, a controller mounted
//! `.at(config.path)`, so HTTP guards, interceptors and pre-dispatch entries apply to it as to any
//! route; the GraphQL-over-HTTP status rules; the engine's playground; and, with
//! `GraphqlConfig::subscriptions`, the graphql-transport-ws gateway of `ulo-graphql-ws`.
//!
//! ```ignore
//! # // reason: names an engine adapter crate (ulo-graphql-async-graphql) this crate does not depend on
//! #[module(
//!     providers = [
//!         with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),
//!         AsyncGraphql<ApiSchema, GqlContext> as dyn Engine,
//!     ],
//!     exports   = [dyn Engine],
//! )]
//! pub struct ApiSchemaModule;
//!
//! #[module(imports = [
//!     ulo_ws::WsModule::for_root(),
//!     GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").engine_from(ApiSchemaModule)),
//! ])]
//! pub struct AppModule;
//! ```
//!
//! `GraphqlModule`'s controllers read the engine from their own module, which sees its imports'
//! exports and not its importer's bindings, so `engine_from` names the module binding the engine
//! and `GraphqlModule` imports it.
//!
//! The statuses follow the GraphQL-over-HTTP working draft, read as a shape rather than a
//! contract. A request that executed answers 200, errors included. One that failed before
//! execution answers 400 when its JSON body or its document does not parse, and 422 when it is not
//! a well-formed GraphQL request, fails validation, names no operation that can be determined, or
//! carries variables that cannot be coerced. A request the server could not run, its context
//! failing to build, answers 500. The response media type is negotiated from `Accept`: a request
//! with no `Accept` reads as accepting `application/graphql-response+json`, and the statuses are
//! the same under either JSON type; a legacy client, one accepting `application/json` and not
//! `application/graphql-response+json`, gets `application/json` on a 2xx and
//! `application/graphql-response+json` otherwise, as the draft has it. An `Accept` naming neither
//! is refused 406, a POST whose body is not `application/json` 415, and a GET whose operation is a
//! mutation 405 with `Allow`, each through the error handlers as the router's refusals are. The
//! draft's 294 for a result with both `data` and `errors` is not used: an executed request answers
//! 200.

mod controller;
mod module;
mod playground;

pub use module::{GraphqlConfig, GraphqlModule};
