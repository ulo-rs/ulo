use std::time::Duration;

use ex_30_grpc_client::pb::front_client::FrontClient;
use ex_30_grpc_client::pb::Probe;
use ex_30_grpc_client::{Calls, Front, callee_builder, front_builder};
use ex_support::{Served, serve};
use tonic::transport::Endpoint;
use tonic::{Code, Request};
use ulo::{ExecOptions, Signal};
use ulo_grpc::Server;
use ulo_runtime_tokio::Tokio;

async fn grpc(builder: ulo::AppBuilder) -> Served {
    let app = builder
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    serve(app)
}

async fn callee() -> (Served, Calls, String) {
    let calls = Calls::default();
    let served = grpc(callee_builder(calls.clone())).await;
    let origin = format!("http://{}", served.addr);
    (served, calls, origin)
}

#[tokio::test]
async fn the_callee_sees_the_deadline_the_caller_gave_the_front() {
    let (callee, calls, origin) = callee().await;
    let front = grpc(front_builder(origin)).await;
    let channel = Endpoint::from_shared(format!("http://{}", front.addr)).expect("an endpoint").connect().await.expect("the client connects");
    let mut client = FrontClient::new(channel);

    // Two deadlines a second apart, so a front that sends a fixed timeout of its own fails one.
    for secs in [5, 2] {
        let mut request = Request::new(Probe {});
        request.set_timeout(Duration::from_secs(secs));
        let left = client.ask(request).await.expect("Ask answers").into_inner();
        assert!(left.has_deadline);
        let given = u32::try_from(secs * 1000).expect("fits");
        assert!((given - 1000..=given).contains(&left.millis), "{} ms left of a {secs} s deadline", left.millis);
    }

    let none = client.ask(Request::new(Probe {})).await.expect("Ask answers").into_inner();
    assert!(!none.has_deadline, "a caller with no deadline gave the callee one: {none:?}");
    assert_eq!(calls.count(), 3);
    front.stop().await.expect("the front closes");
    callee.stop().await.expect("the callee closes");
}

#[tokio::test]
async fn a_spent_deadline_is_refused_before_anything_reaches_the_callee() {
    let (callee, calls, origin) = callee().await;
    let app = front_builder(origin).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let front: ulo::Dep<Front> = app.get().await.expect("Front resolves");
    let passed = tokio::time::Instant::now().into_std();
    let outcome = app
        .execute(ExecOptions::new().deadline(passed), async |exec| front.remaining(&exec.handle()).await)
        .await
        .expect("the execution opens");
    let status = outcome.expect_err("a spent deadline is refused");
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(status.message(), "the calling execution's deadline has passed");
    assert_eq!(calls.count(), 0, "the call reached the callee");
    app.close(Signal::new("test")).await.expect("the app closes");
    callee.stop().await.expect("the callee closes");
}

#[tokio::test]
async fn a_callee_that_is_down_fails_the_first_call_and_not_the_start() {
    // Port 0 names no endpoint, so the connect fails on every OS.
    let front = grpc(front_builder("http://127.0.0.1:0")).await;
    let channel = Endpoint::from_shared(format!("http://{}", front.addr)).expect("an endpoint").connect().await.expect("the client connects");
    let status = FrontClient::new(channel).ask(Request::new(Probe {})).await.expect_err("the callee is down");
    assert_eq!(status.code(), Code::Unavailable, "{status:?}");
    front.stop().await.expect("the front closes");
}
