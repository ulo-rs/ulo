use ex_16_http_enhancers::{Trace, builder};
use ex_support::http::{Call, call};
use ex_support::{Served, start};
use ulo::Dep;

async fn app() -> (Served, Dep<Trace>) {
    let server = start(builder()).await;
    let trace: Dep<Trace> = server.handle.get().await.expect("the trace is bound");
    (server, trace)
}

#[tokio::test]
async fn guards_run_global_then_controller_then_method_and_interceptors_nest_around_the_handler_in_the_same_order() {
    let (server, trace) = app().await;

    let reply = call(server.addr, Call::get("/reports/payroll").header("x-role", "admin")).await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "payroll report"));
    assert_eq!(
        trace.take(),
        [
            "guard global",
            "guard role: admitted",
            "guard controller",
            "guard method",
            "global in",
            "controller in",
            "method in",
            "handler",
            "method out",
            "controller out",
            "global out",
        ]
    );
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_handler_without_a_declared_role_is_open_and_method_tier_enhancers_belong_to_their_handler() {
    let (server, trace) = app().await;

    let reply = call(server.addr, Call::get("/reports")).await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "open report"));
    assert_eq!(
        trace.take(),
        [
            "guard global",
            "guard role: admitted",
            "guard controller",
            "global in",
            "controller in",
            "handler",
            "controller out",
            "global out",
        ]
    );
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_refusing_guard_is_a_403_in_the_error_handlers_words_and_nothing_below_it_runs() {
    let (server, trace) = app().await;

    let refused = call(server.addr, Call::get("/reports/payroll").header("x-role", "intern")).await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.body, "RoleGuard needs role admin");
    assert_eq!(trace.take(), ["guard global", "guard role: refused", "error handler"], "no later guard, interceptor or handler ran");

    let missing = call(server.addr, Call::get("/reports/payroll")).await;
    assert_eq!((missing.status, missing.body.as_str()), (403, "RoleGuard needs role admin"));
    server.stop().await.expect("closes clean");
}
