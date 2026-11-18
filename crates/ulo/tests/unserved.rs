//! A controller no bound server serves is refused by `listen()` (transports DESIGN §2.7, X6): one
//! `StartupError::Configure` entry per controller, naming it and its transport, collected with
//! the servers' own `prepare` failures. `AppBuilder::unserved::<T>()` passes the controllers of
//! `T`, and an app that never calls `listen()` is not asked.

#[macro_use]
mod common;

use ulo::scope::Singleton;
use ulo::{App, ExecOptions, HandlerSpec, PrepareError, StartupError, TestApp, TypeName};
use ulo_runtime_tokio::Tokio;

use common::*;

struct A;

controller!(OnA, Singleton, [], |m| {
    m.handler::<TA, _>(HandlerSpec::new("first", ()).route("first"));
    m.handler::<TA, _>(HandlerSpec::new("second", ()).route("second"));
});

controller!(OnB, Singleton, [], |m| m.handler::<TB, _>(HandlerSpec::new("only", ()).route("only")));

// Mounted under `TA` and served by `TB`'s server, as a WebSocket gateway on the HTTP server's
// port is a `Ws` controller served by the HTTP server.
controller!(ServedByB, Singleton, [], |m| {
    m.served_by::<TA, TB>();
    m.handler::<TA, _>(HandlerSpec::new("guest", ()).route("guest"));
});

fn root(register: fn(&mut ulo::ModuleDef<'_>)) -> Mod<A> {
    Mod::<A>::new(register)
}

fn with_on_a(m: &mut ulo::ModuleDef<'_>) {
    m.controller::<OnA>();
}

fn with_both(m: &mut ulo::ModuleDef<'_>) {
    m.controller::<OnA>();
    m.controller::<OnB>();
}

fn with_guest(m: &mut ulo::ModuleDef<'_>) {
    m.controller::<ServedByB>();
}

fn builder(register: fn(&mut ulo::ModuleDef<'_>)) -> ulo::AppBuilder {
    App::builder(root(register)).runtime(Tokio::current())
}

fn on_a(log: &Log) -> Queued {
    server(TestServer::<TA>::new("a", log))
}

fn on_b(log: &Log) -> Queued {
    server(TestServer::<TB>::new("b", log))
}

async fn listen_err(builder: ulo::AppBuilder, servers: Vec<Queued>) -> ulo::ConfigureErrors {
    match listened(builder, servers).await {
        Err(StartupError::Configure(errors)) => errors,
        Err(other) => panic!("expected configuration errors, got: {other}"),
        Ok(_) => panic!("listen() succeeded; a controller was expected to be refused"),
    }
}

/// The names the entry's `PrepareError` lists: the controller, its transport, and the transport of
/// the server that serves it.
fn names(entry: &ulo::ConfigureError) -> Vec<TypeName> {
    let error = entry.source.downcast_ref::<PrepareError>().expect("the entry is a `PrepareError`");
    let [listed] = error.entries() else { panic!("one entry per controller: {entry}") };
    assert_eq!(listed.subject(), Some("controller"), "{entry}");
    assert!(listed.location().is_some_and(|at| at.file().ends_with("unserved.rs")), "the registration site: {entry}");
    listed.names().to_vec()
}

#[tokio::test]
async fn a_controller_of_a_transport_no_server_is_bound_for_is_refused_once_naming_it_and_its_transport() {
    let log = Log::new();
    let errors = listen_err(builder(with_on_a), vec![on_b(&log)]).await;

    assert_eq!(errors.len(), 1, "one entry for the controller's two handlers: {errors}");
    let entry = errors.iter().next().unwrap();
    assert_eq!(entry.transport, TypeName::of::<TA>(), "{errors}");
    assert_eq!(names(entry), [TypeName::of::<OnA>(), TypeName::of::<TA>(), TypeName::of::<TA>()]);
    let text = entry.to_string();
    assert!(text.starts_with("transport `TA`: the controller `OnA` of module Mod<A>, registered at "), "{text}");
    assert!(
        text.ends_with(
            "has `TA` handlers, and the app binds no `TA` server, so they would never run: bind one, or write \
             `AppBuilder::unserved::<TA>()` for a binary that serves no `TA`"
        ),
        "{text}"
    );
    assert!(!log.has("bind b"), "no server bound: {:?}", log.events());
}

#[tokio::test]
async fn the_refusal_is_collected_with_the_servers_prepare_failures() {
    let log = Log::new();
    let errors = listen_err(builder(with_both), vec![server(TestServer::<TB>::new("b", &log).fail_prepare())]).await;

    let transports: Vec<TypeName> = errors.iter().map(|entry| entry.transport).collect();
    assert_eq!(transports, [TypeName::of::<TA>(), TypeName::of::<TB>()], "the controller's entry, then the server's: {errors}");
    assert!(errors.iter().nth(1).is_some_and(|entry| entry.source.downcast_ref::<Boom>().is_some()), "{errors}");
    assert_eq!(log.events(), ["prepare b"], "the server prepared and nothing bound");
}

#[tokio::test]
async fn an_app_listening_with_no_server_refuses_every_controller() {
    let errors = listen_err(builder(with_both), Vec::new()).await;

    let transports: Vec<TypeName> = errors.iter().map(|entry| entry.transport).collect();
    assert_eq!(transports, [TypeName::of::<TA>(), TypeName::of::<TB>()], "{errors}");
}

#[tokio::test]
async fn a_transport_declared_unserved_passes_its_controllers_and_only_those() {
    let log = Log::new();
    let app = listened(builder(with_on_a).unserved::<TA>(), vec![on_b(&log)]).await;
    assert!(app.is_ok(), "the binary declared it serves no `TA`");

    let errors = listen_err(builder(with_both).unserved::<TA>(), Vec::new()).await;
    let transports: Vec<TypeName> = errors.iter().map(|entry| entry.transport).collect();
    assert_eq!(transports, [TypeName::of::<TB>()], "`TB` is still refused: {errors}");
}

#[tokio::test]
async fn a_bound_server_of_the_controller_s_transport_serves_it() {
    let log = Log::new();
    let app = listened(builder(with_both), vec![on_a(&log), on_b(&log)]).await;
    assert!(app.is_ok(), "every controller has its server");
    assert_eq!(log.events(), ["prepare a", "prepare b", "bind a", "bind b"]);
}

#[tokio::test]
async fn a_controller_served_by_another_transport_s_server_is_refused_without_that_server() {
    let log = Log::new();
    let errors = listen_err(builder(with_guest), vec![on_a(&log)]).await;

    let entry = errors.iter().next().unwrap();
    assert_eq!(errors.len(), 1, "{errors}");
    assert_eq!(entry.transport, TypeName::of::<TA>(), "the refusal names the controller's transport: {errors}");
    assert_eq!(names(entry), [TypeName::of::<ServedByB>(), TypeName::of::<TA>(), TypeName::of::<TB>()]);
    assert!(
        entry.to_string().contains("has `TA` handlers a `TB` server serves, and the app binds none"),
        "{entry}"
    );
}

#[tokio::test]
async fn a_controller_served_by_another_transport_s_server_passes_with_it_or_either_transport_unserved() {
    let log = Log::new();
    assert!(listened(builder(with_guest), vec![on_b(&log)]).await.is_ok(), "its server is bound");
    assert!(listened(builder(with_guest).unserved::<TA>(), Vec::new()).await.is_ok(), "its own transport is unserved");
    assert!(listened(builder(with_guest).unserved::<TB>(), Vec::new()).await.is_ok(), "the server's transport is unserved");
}

#[tokio::test]
async fn an_app_that_never_listens_is_not_asked() {
    let app = builder(with_both).wire().expect("wires").connect().await.expect("connects");
    let ran = app.execute(ExecOptions::new(), async |_exec| 7).await.expect("a connected app executes");
    assert_eq!(ran, 7);

    let app = TestApp::of(root(with_both)).runtime(Tokio::current()).connect().await.expect("a test app connects");
    assert_eq!(app.execute(ExecOptions::new(), async |_exec| 8).await.expect("it executes"), 8);
}
