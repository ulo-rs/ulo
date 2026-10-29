use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ex_26_rpc_streams::builder;
use ex_support::rpc::bounded;
use ex_support::{Served, serve};
use futures_util::{StreamExt, stream};
use ulo::Bound;
use ulo_rpc::{RpcClient, RpcError, Server};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

const SHORT: Duration = Duration::from_millis(150);

async fn start() -> Served {
    let app = builder().runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    serve(app.bind(Server::new(Tcp::new("127.0.0.1:0"))).listen().await.expect("listens"))
}

fn client(addr: SocketAddr) -> RpcClient {
    RpcClient::new(Tcp::new(addr), Arc::new(Tokio::current())).expect("the link is usable")
}

fn kinds<T>(items: &[Result<T, RpcError>]) -> Vec<Option<ErrorKind>> {
    items.iter().map(|item| item.as_ref().err().map(RpcError::kind)).collect()
}

#[tokio::test]
async fn a_server_stream_arrives_in_order_and_ends() {
    let server = start().await;
    let replies = bounded(client(server.addr).stream::<_, u32>("stream.up", &3u32)).await.expect("opens");
    let items: Vec<_> = bounded(replies.collect::<Vec<_>>()).await;
    assert_eq!(items.into_iter().map(|item| item.expect("an item")).collect::<Vec<_>>(), [1, 2, 3]);
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_client_stream_is_answered_once_it_ends() {
    let server = start().await;
    let total = bounded(client(server.addr).send_stream::<_, i64>("stream.sum", stream::iter([1i64, 2, 3, 4]))).await;
    assert_eq!(total.expect("answered"), 10);
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_duplex_call_answers_each_item_in_order() {
    let server = start().await;
    let replies = bounded(client(server.addr).duplex::<_, i64>("stream.double", stream::iter([1i64, 2, 3]))).await.expect("opens");
    let items: Vec<_> = bounded(replies.collect::<Vec<_>>()).await;
    assert_eq!(items.into_iter().map(|item| item.expect("an item")).collect::<Vec<_>>(), [2, 4, 6]);
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_silent_stream_ends_with_the_idle_timeout_s_kind() {
    let server = start().await;
    let call = client(server.addr).stream::<_, u32>("stream.stall", &()).idle_timeout(Bound::After(SHORT));
    let items: Vec<_> = bounded(bounded(call).await.expect("opens").collect::<Vec<_>>()).await;
    assert_eq!(*items[0].as_ref().expect("the first item"), 1);
    assert_eq!(kinds(&items), [None, Some(ErrorKind::Timeout)]);
    // The idle timeout is the client's alone, so its message is the one the stream ends with; the
    // deadline and `send_stream`'s timeout are sent to the server too, which may end the call first.
    assert_eq!(items[1].as_ref().expect_err("the idle timeout").message(), "no reply frame arrived within the stream's idle timeout");
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_stream_with_a_deadline_ends_there_whatever_it_still_has_to_say() {
    let server = start().await;
    let call = client(server.addr).stream::<_, u64>("stream.ticks", &()).deadline(Duration::from_millis(300));
    let items: Vec<_> = bounded(bounded(call).await.expect("opens").collect::<Vec<_>>()).await;
    let (last, items) = items.split_last().expect("the stream ended with an error");
    assert_eq!(last.as_ref().expect_err("the deadline").kind(), ErrorKind::Timeout);
    assert!(!items.is_empty(), "no tick arrived before the deadline");
    assert_eq!(items.iter().map(|item| *item.as_ref().expect("a tick")).collect::<Vec<_>>(), (0..items.len() as u64).collect::<Vec<_>>());
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_client_stream_that_never_ends_fails_at_the_call_s_timeout() {
    let server = start().await;
    let items = stream::iter([1i64, 2]).chain(stream::pending());
    let total = bounded(client(server.addr).send_stream::<_, i64>("stream.sum", items).timeout(SHORT)).await;
    assert_eq!(total.expect_err("the timeout").kind(), ErrorKind::Timeout);
    server.stop().await.expect("closes");
}
