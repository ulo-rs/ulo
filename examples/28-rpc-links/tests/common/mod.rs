//! What one link must answer, run against whichever link the URL names.

use std::sync::Arc;
use std::time::Duration;

use ex_28_rpc_links::{Divide, Journal, bind, builder, client};
use ex_support::rpc::eventually;
use ulo::Signal;
use ulo_runtime_tokio::Tokio;
use ulo_rpc::ErrorKind;

/// A broker link's first call waits for the server's consumer to join, which Kafka makes slow.
const FIRST_CALL: Duration = Duration::from_secs(60);

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("a free port").local_addr().expect("its address").port()
}

/// A reply, a handler's error and an event, each with the identity it carries.
pub async fn the_handlers_answer(url: &str) {
    let connected = builder().runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    let app = bind(connected, url).expect("the URL names a link").listen().await.expect("listens");
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let journal = handle.get::<Journal>().await.expect("the journal resolves");
    let rpc = client(url, Arc::new(Tokio::current())).expect("the client's link is usable");

    let answer = rpc.request::<_, i64>("calc.divide", &Divide { dividend: 84, divisor: 2 }).timeout(FIRST_CALL).await;
    assert_eq!(answer.expect("answered"), 42);

    let refused = rpc.request::<_, i64>("calc.divide", &Divide { dividend: 1, divisor: 0 }).timeout(FIRST_CALL).await.expect_err("refused");
    assert_eq!((refused.kind(), refused.message()), (ErrorKind::Unprocessable, "the divisor is zero"));

    rpc.emit("calc.note", "carry the one").await.expect("published");
    eventually("the event's handler", || journal.entries().first().is_some_and(|note| note == "carry the one")).await;

    drop(rpc);
    tokio::time::timeout(Duration::from_secs(30), handle.close(Signal::new("test"))).await.expect("closes in time").expect("closes");
    serving.await.expect("serve joins").expect("serve ends clean");
}
