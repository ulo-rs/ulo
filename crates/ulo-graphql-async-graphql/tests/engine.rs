//! The async-graphql engine behind `ulo_graphql::Engine` (transports DESIGN §7; SWITCH_INVENTORY
//! G18): a query's round trip, the three outcomes a response carries, the per-execution context,
//! a subscription's responses, and the schema and playground the HTTP binding serves.
//!
//! The outcome decides the HTTP status, so each test asserts it, with the fields of the errors
//! and not their text. Every call runs under a real-clock deadline.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_graphql::{Context, EmptyMutation, Object, Schema, Subscription};
use futures_util::StreamExt;
use futures_util::stream;
use serde_json::{Map, Value, json};
use ulo::scope::PerExecution;
use ulo::{App, Construct, ConstructError, Dep, Dependencies, ExecOptions, Module, ModuleDef, ModuleIdentity, Resolver};
use ulo_graphql::{Engine, GqlRequest, GqlResponse, Location, Outcome, PathSegment};
use ulo_graphql_async_graphql::AsyncGraphql;

const PATIENCE: Duration = Duration::from_secs(10);

static NEXT_CONTEXT: AtomicU32 = AtomicU32::new(1);

/// Built once per execution; the number says which execution a resolver ran in.
struct Gql {
    id: u32,
}

impl Construct for Gql {
    type Scope = PerExecution;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Gql { id: NEXT_CONTEXT.fetch_add(1, Ordering::SeqCst) })
    }
}

/// A context the container cannot build.
struct Broken;

impl Construct for Broken {
    type Scope = PerExecution;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Err(ConstructError::failed(std::io::Error::other("no context")))
    }
}

struct Query;

#[Object]
impl Query {
    async fn hello(&self, name: Option<String>) -> String {
        format!("hello {}", name.unwrap_or_else(|| "world".into()))
    }

    async fn answer(&self) -> i32 {
        42
    }

    /// A non-null field that fails: the error nulls the whole `data`.
    async fn fail(&self) -> async_graphql::Result<i32> {
        Err("refused".into())
    }

    /// A nullable field that fails: the rest of `data` survives.
    async fn soft_fail(&self) -> async_graphql::Result<Option<i32>> {
        Err("refused".into())
    }

    async fn context_id(&self, ctx: &Context<'_>) -> async_graphql::Result<u32> {
        Ok(ctx.data::<Dep<Gql>>()?.id)
    }
}

struct Subscriptions;

#[Subscription]
impl Subscriptions {
    async fn ticks(&self) -> impl futures_util::Stream<Item = i32> {
        stream::iter(1..=3)
    }
}

type Api = Schema<Query, EmptyMutation, Subscriptions>;

fn schema() -> Api {
    Schema::new(Query, EmptyMutation, Subscriptions)
}

/// Binds the schema, the context `C` and the engine over both, as `dyn Engine`.
struct Root<C>(std::marker::PhantomData<fn() -> C>);

impl<C: Construct> Module for Root<C> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(schema());
        m.provide::<C>();
        m.provide::<AsyncGraphql<Api, C>>().also_as::<dyn Engine>(|engine| engine);
    }
}

async fn within<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {PATIENCE:?}"))
}

/// Connects an app over context `C`, runs `f` against the engine inside a fresh execution each
/// time it asks for one, and closes the app.
async fn with_engine<C, F>(f: F)
where
    C: Construct,
    F: AsyncFnOnce(&App<ulo::Connected>, Dep<dyn Engine>),
{
    let app = App::builder(Root::<C>(std::marker::PhantomData))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app wires: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app connects: {error}"));
    let engine = app.get::<dyn Engine>().await.expect("the engine is bound");
    within("the test body", f(&app, engine)).await;
    let _ = within("the app's close", app.close(ulo::Signal::new("test"))).await;
}

async fn execute(app: &App<ulo::Connected>, engine: &Dep<dyn Engine>, request: GqlRequest) -> GqlResponse {
    within("the execution", app.execute(ExecOptions::new(), async |exec| engine.execute(request, exec.handle()).await)).await.expect("the app accepts an execution")
}

fn vars(value: Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

#[tokio::test]
async fn a_query_round_trips_with_its_variables_and_the_named_operation() {
    with_engine::<Gql, _>(async |app, engine| {
        let request = GqlRequest {
            query: "query Greet($n: String) { hello(name: $n) } query Other { answer }".into(),
            operation_name: Some("Greet".into()),
            variables: vars(json!({ "n": "ulo" })),
            extensions: None,
        };
        let response = execute(app, &engine, request).await;
        assert_eq!(response.outcome(), Outcome::Executed);
        assert_eq!(response.data(), Some(&json!({ "hello": "hello ulo" })));
        assert!(response.errors().is_empty());

        let other = GqlRequest { operation_name: Some("Other".into()), ..GqlRequest::new("query Greet { hello } query Other { answer }") };
        let response = execute(app, &engine, other).await;
        assert_eq!(response.data(), Some(&json!({ "answer": 42 })), "the operation name picks which of two runs");
    })
    .await;
}

/// A field that fails after execution began is an executed response: its error names the field
/// by `path` and the document by `location`.
#[tokio::test]
async fn a_field_error_is_an_executed_response_naming_its_path_and_location() {
    with_engine::<Gql, _>(async |app, engine| {
        let response = execute(app, &engine, GqlRequest::new("{ answer softFail }")).await;
        assert_eq!(response.outcome(), Outcome::Executed);
        assert_eq!(response.errors().len(), 1);
        assert_eq!(response.errors()[0].path, [PathSegment::Field("softFail".into())]);
        assert_eq!(response.errors()[0].locations, [Location { line: 1, column: 10 }]);

        let response = execute(app, &engine, GqlRequest::new("{ answer fail }")).await;
        assert_eq!(response.outcome(), Outcome::Executed, "a non-null field's failure nulls `data` and is still an executed request");
        assert_eq!(response.data(), Some(&Value::Null));
        assert_eq!(response.errors()[0].path, [PathSegment::Field("fail".into())]);
    })
    .await;
}

/// A document that does not parse or validate, or an operation name it does not hold, fails before
/// execution: `RequestError`, no `data`, and errors that name no field.
#[tokio::test]
async fn a_request_that_fails_before_execution_is_a_request_error_with_no_data() {
    with_engine::<Gql, _>(async |app, engine| {
        let cases = [
            ("a syntax error", GqlRequest::new("{ answer")),
            ("an unknown field", GqlRequest::new("{ nope }")),
            ("an unknown operation", GqlRequest { operation_name: Some("Missing".into()), ..GqlRequest::new("query A { answer }") }),
        ];
        for (what, request) in cases {
            let response = execute(app, &engine, request).await;
            assert_eq!(response.outcome(), Outcome::RequestError, "{what}");
            assert_eq!(response.data(), None, "{what}: no `data` key at all");
            assert!(!response.errors().is_empty(), "{what}");
            assert!(response.errors().iter().all(|error| error.path.is_empty()), "{what}: a request error names no field");
        }
    })
    .await;
}

/// A required variable the request leaves out is a fault in the request, which GraphQL's
/// variable coercion raises before execution begins: `RequestError`, no `data`. async-graphql
/// reports it at the field that reads the variable, with a `path`, so the adapter, which tells a
/// request error by its errors carrying no path, answers `Executed`.
#[tokio::test]
#[ignore = "DEFECT F456: a missing required variable is answered Executed, not RequestError"]
async fn a_missing_required_variable_is_a_request_error() {
    with_engine::<Gql, _>(async |app, engine| {
        let response = execute(app, &engine, GqlRequest::new("query A($n: String!) { hello(name: $n) }")).await;
        assert_eq!(response.outcome(), Outcome::RequestError);
        assert_eq!(response.data(), None);
        assert!(response.errors().iter().all(|error| error.path.is_empty()));
    })
    .await;
}

/// The context is built in the call's execution, once, and shared by every resolver in it; a
/// second call is a second execution with a context of its own.
#[tokio::test]
async fn the_context_is_built_per_execution_and_read_through_the_resolver() {
    with_engine::<Gql, _>(async |app, engine| {
        let both = app
            .execute(ExecOptions::new(), async |exec| {
                let first = engine.execute(GqlRequest::new("{ contextId }"), exec.handle()).await;
                let second = engine.execute(GqlRequest::new("{ contextId }"), exec.handle()).await;
                (first, second)
            })
            .await
            .expect("an execution");
        let id = |response: &GqlResponse| response.data().and_then(|data| data["contextId"].as_u64()).expect("a context id");
        assert_eq!(id(&both.0), id(&both.1), "one execution, one context");

        let other = execute(app, &engine, GqlRequest::new("{ contextId }")).await;
        assert_ne!(id(&other), id(&both.0), "another execution builds its own");
    })
    .await;
}

/// A context the container cannot build is the server's fault: `Failed`, no `data`, and the
/// message does not carry the cause.
#[tokio::test]
async fn a_context_that_cannot_be_built_is_failed_and_not_a_request_error() {
    with_engine::<Broken, _>(async |app, engine| {
        let response = execute(app, &engine, GqlRequest::new("{ answer }")).await;
        assert_eq!(response.outcome(), Outcome::Failed);
        assert_eq!(response.data(), None);
        assert_eq!(response.errors().len(), 1);
        assert!(!response.errors()[0].message.contains("no context"), "the construction's own message stays out of the response");

        let streamed: Vec<GqlResponse> = within("the stream", async {
            let stream = app.execute(ExecOptions::new(), async |exec| engine.subscribe(GqlRequest::new("subscription { ticks }"), exec.handle())).await.expect("an execution");
            stream.collect().await
        })
        .await;
        assert_eq!(streamed.len(), 1);
        assert_eq!(streamed[0].outcome(), Outcome::Failed, "a subscription fails the same way");
    })
    .await;
}

/// A subscription answers one executed response per event; a query sent to `subscribe` answers
/// once, and a request that fails before execution answers one `RequestError` and ends.
#[tokio::test]
async fn a_subscription_yields_one_response_per_event() {
    with_engine::<Gql, _>(async |app, engine| {
        let collect = |request: GqlRequest| {
            let engine = engine.clone();
            async move {
                within("the stream", async {
                    app.execute(ExecOptions::new(), async |exec| engine.subscribe(request, exec.handle()).collect::<Vec<_>>().await).await.expect("an execution")
                })
                .await
            }
        };

        let events = collect(GqlRequest::new("subscription { ticks }")).await;
        let data: Vec<Option<&Value>> = events.iter().map(GqlResponse::data).collect();
        assert_eq!(data, [Some(&json!({ "ticks": 1 })), Some(&json!({ "ticks": 2 })), Some(&json!({ "ticks": 3 }))]);
        assert!(events.iter().all(|event| event.outcome() == Outcome::Executed));

        let once = collect(GqlRequest::new("{ answer }")).await;
        assert_eq!(once.len(), 1);
        assert_eq!(once[0].data(), Some(&json!({ "answer": 42 })));

        let refused = collect(GqlRequest::new("subscription { nope }")).await;
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].outcome(), Outcome::RequestError);
        assert_eq!(refused[0].data(), None);
    })
    .await;
}

/// The schema as SDL names what the application declared, and the playground page is given the
/// endpoint and, when there is one, the subscription path.
#[tokio::test]
async fn the_sdl_names_the_schema_and_the_playground_is_given_both_paths() {
    with_engine::<Gql, _>(async |_app, engine| {
        let sdl = engine.sdl();
        assert!(sdl.contains("type Query") && sdl.contains("contextId") && sdl.contains("type Subscriptions"), "{sdl}");

        let page = engine.playground_html("/graphql", Some("/graphql/ws")).expect("async-graphql has a playground");
        assert!(page.contains("/graphql") && page.contains("/graphql/ws"), "the page connects to both paths");
        let without = engine.playground_html("/graphql", None).expect("a playground without subscriptions");
        assert!(!without.contains("/graphql/ws"));
    })
    .await;
}
