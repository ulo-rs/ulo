use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ex_27_rpc_deadlines::{Gateway, Relayed, Runs, callee, gateway};
use ex_support::rpc::bounded;
use ex_support::{Served, serve};
use ulo::ExecOptions;
use ulo_rpc::{RpcClient, Server};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

async fn listen(app: ulo::AppBuilder) -> Served {
    let app = app.runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    serve(app.bind(Server::new(Tcp::new("127.0.0.1:0"))).listen().await.expect("listens"))
}

fn client(addr: SocketAddr) -> RpcClient {
    RpcClient::new(Tcp::new(addr), Arc::new(Tokio::current())).expect("the link is usable")
}

/// Under the client's default five seconds, which would otherwise be what the callee is given.
const CALLER: Duration = Duration::from_secs(3);

#[tokio::test]
async fn the_callee_sees_what_is_left_of_the_caller_s_time() {
    let callee = listen(callee()).await;
    let gateway = listen(gateway(callee.addr)).await;
    let answer = bounded(client(gateway.addr).request::<_, Relayed>("gateway.quote", "tea").timeout(CALLER)).await.expect("answered");
    assert_eq!(answer.quote.sku, "tea");
    let (gateway_left, callee_left) = (answer.gateway_left_ms, answer.quote.left_ms);
    assert!(callee_left <= gateway_left && gateway_left <= CALLER.as_millis() as u64, "{answer:?}");
    assert!(callee_left > 1_000, "the call lost its time in transit: {answer:?}");
    assert!(gateway_left - callee_left < 500, "the callee was given less than the gateway had left: {answer:?}");
    gateway.stop().await.expect("closes");
    callee.stop().await.expect("closes");
}

#[tokio::test]
async fn a_call_made_after_the_deadline_answers_timeout_and_never_runs_the_handler() {
    let callee = listen(callee()).await;
    let runs = callee.handle.get::<Runs>().await.expect("the counter resolves");
    let gateway = listen(gateway(callee.addr)).await;
    let service = gateway.handle.get::<Gateway>().await.expect("the gateway resolves");

    let spent = ExecOptions::new().deadline(Instant::now());
    let late = gateway.handle.execute(spent, async |exec| service.quote(&exec.handle(), "tea").await).await.expect("the app is open");
    let error = late.expect_err("refused");
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(runs.count.load(Ordering::SeqCst), 0, "the handler ran for a call whose deadline had passed");

    let open = ExecOptions::new().deadline(Instant::now() + CALLER);
    let on_time = gateway.handle.execute(open, async |exec| service.quote(&exec.handle(), "tea").await).await.expect("the app is open");
    assert_eq!(on_time.expect("answered").sku, "tea");
    assert_eq!(runs.count.load(Ordering::SeqCst), 1);
    gateway.stop().await.expect("closes");
    callee.stop().await.expect("closes");
}

/// The client's timeout, five seconds by default, still bounds a call made `.within` an execution
/// with longer to run.
#[tokio::test]
async fn a_caller_with_more_time_than_the_client_s_timeout_reaches_the_callee_with_the_timeout() {
    let callee = listen(callee()).await;
    let gateway = listen(gateway(callee.addr)).await;
    let service = gateway.handle.get::<Gateway>().await.expect("the gateway resolves");

    let long = ExecOptions::new().deadline(Instant::now() + Duration::from_secs(30));
    let quote = gateway.handle.execute(long, async |exec| service.quote(&exec.handle(), "tea").await).await.expect("the app is open");
    let left = quote.expect("answered").left_ms;
    assert!((4_000..=5_000).contains(&left), "the callee had {left} ms of a 30 s caller under a 5 s client timeout");
    gateway.stop().await.expect("closes");
    callee.stop().await.expect("closes");
}
