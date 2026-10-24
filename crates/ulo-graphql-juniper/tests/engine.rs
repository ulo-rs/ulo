//! The juniper engine behind `ulo_graphql::Engine` (transports DESIGN §7; SWITCH_INVENTORY G18):
//! a query's round trip, the three outcomes a response carries, the per-execution context, a
//! subscription's responses, and the schema and playground the HTTP binding serves.
//!
//! The outcome decides the HTTP status, so each test asserts it, with the fields of the errors
//! and not their text. Every call runs under a real-clock deadline.

use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures::{Stream, StreamExt, stream};
use juniper::{EmptyMutation, FieldResult, RootNode, graphql_object, graphql_subscription};
use serde_json::{Map, Value, json};
use ulo::scope::PerExecution;
use ulo::{App, Construct, ConstructError, Dep, Dependencies, ExecOptions, Module, ModuleDef, ModuleIdentity, Resolver};
use ulo_graphql::{Engine, GqlRequest, GqlResponse, Location, Outcome, PathSegment};
use ulo_graphql_juniper::Juniper;

const PATIENCE: Duration = Duration::from_secs(10);

static NEXT_CONTEXT: AtomicU32 = AtomicU32::new(1);

/// Built once per execution; the number says which execution a resolver ran in.
struct Ctx {
    id: u32,
}

impl juniper::Context for Ctx {}

impl Construct for Ctx {
    type Scope = PerExecution;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Ctx { id: NEXT_CONTEXT.fetch_add(1, Ordering::SeqCst) })
    }
}

struct Query;

#[graphql_object(context = Ctx)]
impl Query {
    fn hello(name: Option<String>) -> String {
        format!("hello {}", name.unwrap_or_else(|| "world".into()))
    }

    fn answer() -> i32 {
        42
    }

    /// A non-null field that fails: the error nulls the whole `data`.
    fn fail() -> FieldResult<i32> {
        Err("refused".into())
    }

    /// A nullable field that fails: the rest of `data` survives.
    fn soft_fail() -> FieldResult<Option<i32>> {
        Err("refused".into())
    }

    fn context_id(ctx: &Ctx) -> i32 {
        i32::try_from(ctx.id).unwrap_or(0)
    }
}

struct Subscriptions;

type Ticks = Pin<Box<dyn Stream<Item = FieldResult<i32>> + Send>>;

#[graphql_subscription(context = Ctx)]
impl Subscriptions {
    async fn ticks() -> Ticks {
        Box::pin(stream::iter((1..=3).map(Ok)))
    }
}

type Mutation = EmptyMutation<Ctx>;
type Api = RootNode<'static, Query, Mutation, Subscriptions>;

fn schema() -> Api {
    RootNode::new(Query, Mutation::new(), Subscriptions)
}

/// Binds the schema, the context and the engine over both, as `dyn Engine`.
struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(schema());
        m.provide::<Ctx>();
        m.provide::<Juniper<Query, Mutation, Subscriptions>>().also_as::<dyn Engine>(|engine| engine);
    }
}

async fn within<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {PATIENCE:?}"))
}

async fn with_engine<F>(f: F)
where
    F: AsyncFnOnce(&App<ulo::Connected>, Dep<dyn Engine>),
{
    let app = App::builder(Root)
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
    with_engine(async |app, engine| {
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
    with_engine(async |app, engine| {
        let response = execute(app, &engine, GqlRequest::new("{ answer softFail }")).await;
        assert_eq!(response.outcome(), Outcome::Executed);
        assert_eq!(response.errors().len(), 1);
        assert_eq!(response.errors()[0].path, [PathSegment::Field("softFail".into())]);
        assert_eq!(response.errors()[0].locations, [Location { line: 1, column: 10 }]);

        let response = execute(app, &engine, GqlRequest::new("{ answer fail }")).await;
        assert_eq!(response.outcome(), Outcome::Executed, "a non-null field's failure nulls `data` and is still an executed request");
        assert_eq!(response.errors()[0].path, [PathSegment::Field("fail".into())]);
    })
    .await;
}

/// A document that does not parse or validate, or an operation name it does not hold, fails before
/// execution: `RequestError`, no `data`, and errors that name no field.
#[tokio::test]
async fn a_request_that_fails_before_execution_is_a_request_error_with_no_data() {
    with_engine(async |app, engine| {
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
/// variable coercion raises before execution begins: `RequestError`, no `data`.
#[tokio::test]
async fn a_missing_required_variable_is_a_request_error() {
    with_engine(async |app, engine| {
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
async fn the_context_is_built_per_execution_and_handed_to_the_resolver() {
    with_engine(async |app, engine| {
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

/// A subscription answers one executed response per event; a query sent to `subscribe` answers
/// once, and a request that fails before execution answers one `RequestError` and ends.
#[tokio::test]
async fn a_subscription_yields_one_response_per_event() {
    with_engine(async |app, engine| {
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
    with_engine(async |_app, engine| {
        let sdl = engine.sdl();
        assert!(sdl.contains("type Query") && sdl.contains("contextId") && sdl.contains("type Subscriptions"), "{sdl}");

        let page = engine.playground_html("/graphql", Some("/graphql/ws")).expect("juniper has a playground");
        assert!(page.contains("/graphql") && page.contains("/graphql/ws"), "the page connects to both paths");
    })
    .await;
}
