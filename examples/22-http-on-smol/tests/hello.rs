use std::net::SocketAddr;
use std::sync::Arc;

use async_net::TcpStream;
use futures::{AsyncReadExt, AsyncWriteExt};
use ulo::Signal;
use ulo_runtime_smol::{Executor, Smol};

async fn get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(format!("GET {path} HTTP/1.1\r\nhost: example\r\nconnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    raw
}

#[test]
fn get_root_answers_what_example_01_answers_and_close_returns_a_clean_shutdown() {
    let smol = Smol::new(Arc::new(Executor::new())).with_workers(2);
    smol.block_on(async {
        let app = ex_22_http_on_smol::builder()
            .runtime(smol.clone())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ex_22_http_on_smol::server("127.0.0.1:0"))
            .listen()
            .await
            .expect("the app listens");
        let addr = app.addresses()[0].addr;
        let handle = app.handle();
        let serving = smol.executor().spawn(app.serve(std::future::pending()));

        let reply = get(addr, "/").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.to_ascii_lowercase().contains("content-type: text/plain; charset=utf-8"), "{reply}");
        assert!(reply.ends_with("Hello, ulo!"), "{reply}");

        let closed = handle.close(Signal::new("test")).await.expect("close is clean");
        assert_eq!((closed.signal.name(), closed.abandoned, closed.terminal_skipped), ("test", 0, 0));
        let served = serving.await.expect("serve is clean");
        assert_eq!(served.signal.name(), "test");
    });
}
