use ex_16_http_enhancers::{Trace, builder};
use ex_support::http::{Call, call};
use ex_support::{Served, start};
use ulo::Dep;

/// What the pipeline recorded, less the global `Stamp` the whole app carries (example 16's other test).
fn own(trace: &Dep<Trace>) -> Vec<String> {
    trace.take().into_iter().filter(|line| !["guard global", "global in", "global out"].contains(&line.as_str())).collect()
}

async fn app() -> (Served, Dep<Trace>) {
    let server = start(builder()).await;
    let trace: Dep<Trace> = server.handle.get().await.expect("the trace is bound");
    (server, trace)
}

#[tokio::test]
async fn a_guard_is_built_only_after_the_one_before_it_admits() {
    let (server, trace) = app().await;

    let open = call(server.addr, Call::get("/lazy/open")).await;
    assert_eq!((open.status, open.body.as_str()), (200, "open"));
    assert_eq!(own(&trace), ["built first", "ran first", "built second", "ran second", "handler"]);

    let closed = call(server.addr, Call::get("/lazy/closed")).await;
    assert_eq!(closed.status, 403);
    assert_eq!(own(&trace), ["built first", "ran first", "controller"], "the refusal reached the controller's error handler; the second guard was never built and the handler never ran");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn error_handlers_are_asked_last_declared_first_then_controller_then_global_and_the_first_answer_claims() {
    let (server, trace) = app().await;

    let reply = call(server.addr, Call::get("/stock/7")).await;
    assert_eq!((reply.status, reply.body.as_str()), (409, "sku 7 is sold out"));
    assert_eq!(
        own(&trace),
        ["method", "telemetry: sku 7 is out of stock", "controller", "global"],
        "method tier last-declared first, then the controller's, then the global one, which answered"
    );
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_handler_that_answers_ends_the_search_so_those_asked_after_it_never_see_the_error() {
    let (server, trace) = app().await;

    let reply = call(server.addr, Call::get("/stock-claimed/7")).await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "claimed by method"));
    assert_eq!(own(&trace), ["claimer"], "no telemetry, controller or global entry ran");
    server.stop().await.expect("closes clean");
}
