//! The gRPC server's in-flight bounds (transports DESIGN §2.8): a call over `max_inflight` or
//! over `max_per_connection` is refused at once with UNAVAILABLE and never reaches a handler, the
//! place of a finished call is free again, and a zero bound is refused at `listen()`. Capability:
//! gRPC admission (G19).

mod support;
mod wire;

use tonic::Code;
use tonic::transport::Channel;
use ulo::{App, StartupError, TypeName};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{AddRequest, Sum};
use ulo_grpc::Grpc;
use ulo_transport::Count;

use support::{Running, within};
use wire::{Hold, Root};

async fn connect(app: &Running) -> Channel {
    app.channel().await
}

/// A held call on `channel`, entered and waiting for the gate.
async fn hold_one(channel: Channel, hold: &Hold, already: usize) -> tokio::task::JoinHandle<Result<tonic::Response<Sum>, tonic::Status>> {
    let call = tokio::spawn(async move { CounterClient::new(channel).add(AddRequest { a: 100, b: 1 }).await });
    hold.entered.at_least(already + 1, "the held call entering its handler").await;
    call
}

async fn add(channel: Channel, a: i64) -> Result<Sum, tonic::Status> {
    within("the call's answer", CounterClient::new(channel).add(AddRequest { a, b: 0 })).await.map(tonic::Response::into_inner)
}

#[tokio::test]
async fn a_call_over_the_servers_bound_is_unavailable_on_any_connection_and_a_freed_place_admits_again() {
    let hold = Hold::new();
    let app = Running::start(Root(hold.clone()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_inflight(Count::Max(1))).await;
    let first = connect(&app).await;
    let held = hold_one(first.clone(), &hold, 0).await;

    let same = add(first.clone(), 100).await;
    assert_eq!(same.as_ref().err().map(tonic::Status::code), Some(Code::Unavailable), "same connection: {same:?}");
    let other = add(connect(&app).await, 100).await;
    assert_eq!(other.as_ref().err().map(tonic::Status::code), Some(Code::Unavailable), "another connection: {other:?}");
    assert_eq!(hold.entered.snapshot().len(), 1, "a refused call reached the handler");

    hold.gate.notify_one();
    let answered = within("the held call", held).await.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(answered.unwrap_or_else(|status| panic!("the held call failed: {status:?}")).into_inner(), Sum { sum: 101 });
    assert_eq!(add(first, 7).await.unwrap_or_else(|status| panic!("{status:?}")), Sum { sum: 7 }, "the place is free again");
    app.stop().await;
}

#[tokio::test]
async fn a_call_over_a_connections_bound_is_unavailable_there_while_another_connection_is_admitted() {
    let hold = Hold::new();
    let server = ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_per_connection(Count::Max(1));
    let app = Running::start(Root(hold.clone()), server).await;
    let first = connect(&app).await;
    let held = hold_one(first.clone(), &hold, 0).await;

    let same = add(first.clone(), 100).await;
    assert_eq!(same.as_ref().err().map(tonic::Status::code), Some(Code::Unavailable), "same connection: {same:?}");
    assert_eq!(add(connect(&app).await, 4).await.unwrap_or_else(|status| panic!("a second connection was refused: {status:?}")), Sum { sum: 4 });
    assert_eq!(hold.entered.snapshot().len(), 1, "a refused call reached the handler");

    hold.gate.notify_one();
    let _ = within("the held call", held).await;
    app.stop().await;
}

#[tokio::test]
async fn a_zero_bound_is_refused_at_listen_as_one_configure_entry_naming_grpc() {
    for server in [
        ulo_grpc::Server::new("127.0.0.1:0").max_inflight(Count::Max(0)),
        ulo_grpc::Server::new("127.0.0.1:0").max_per_connection(Count::Max(0)),
    ] {
        let bound = App::builder(Root(Hold::new()))
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .unwrap_or_else(|error| panic!("wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"))
            .bind(server);
        let Err(error) = within("listen", bound.listen()).await else {
            panic!("a server with a zero bound listened");
        };
        let StartupError::Configure(errors) = error else {
            panic!("expected StartupError::Configure, got {error}");
        };
        let entries: Vec<_> = errors.iter().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].transport, TypeName::of::<Grpc>());
    }
}

#[tokio::test]
async fn an_unlimited_server_holds_two_calls_in_flight_together() {
    let hold = Hold::new();
    let app = Running::start(Root(hold.clone()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_inflight(Count::Unlimited)).await;
    let channel = connect(&app).await;
    let first = hold_one(channel.clone(), &hold, 0).await;
    let second = hold_one(channel.clone(), &hold, 1).await;
    hold.gate.notify_waiters();
    for call in [first, second] {
        let reply = within("a held call", call).await.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(reply.unwrap_or_else(|status| panic!("{status:?}")).into_inner(), Sum { sum: 101 });
    }
    app.stop().await;
}
