use std::future::pending;

use ex_support::get;
use ulo::Signal;
use ulo_http_hyper::Server;
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn get_root_answers_200_and_close_returns_a_clean_shutdown() {
    let app = ex_01_hello::builder()
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
    let addr = app.addresses()[0].addr;
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(pending()));

    let reply = get(addr, "/").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "Hello, ulo!");
    assert_eq!(reply.header("content-type"), Some("text/plain; charset=utf-8"));

    let closed = handle.close(Signal::new("test")).await.expect("close is clean");
    assert_eq!(closed.signal.name(), "test");
    assert_eq!((closed.abandoned, closed.terminal_skipped), (0, 0));
    let served = serving.await.expect("serve joins").expect("serve is clean");
    assert_eq!(served.signal.name(), "test");
}
