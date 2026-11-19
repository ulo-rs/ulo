//! A gRPC call shed over `max_inflight` is counted into the server's `ShedLog` (transports DESIGN
//! §2.8): the first refusal writes a `warn` naming the transport at once, and the refusals after
//! it within the interval are written when the server drops with the app, none lost.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

mod support;
mod wire;

use tonic::Code;
use ulo_codegen_tests::pb::AddRequest;
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_transport::Count;

use support::{Capture, Running, within};
use wire::{Hold, Root};

/// The `ShedLog` lines among the captured `warn`s, as their `ulo.transport` and `shed` fields.
fn shed_lines(capture: &Capture) -> Vec<String> {
    capture
        .warnings()
        .into_iter()
        .filter(|line| line.starts_with("calls refused over the in-flight limit"))
        .map(|line| line.split_once(" ulo.transport=").map_or(line.clone(), |(_, fields)| fields.to_owned()))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_shed_call_is_logged_at_warn_naming_the_transport_and_none_is_lost() {
    let capture = Capture::default();
    let _logged = tracing::subscriber::set_default(capture.clone());
    let hold = Hold::new();
    let app = Running::start(Root(hold.clone()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_inflight(Count::Max(1))).await;
    let channel = app.channel().await;
    let held = {
        let channel = channel.clone();
        tokio::spawn(async move { CounterClient::new(channel).add(AddRequest { a: 100, b: 1 }).await })
    };
    hold.entered.at_least(1, "the held call entering its handler").await;
    for _ in 0..3 {
        let refused = within("the refusal", CounterClient::new(channel.clone()).add(AddRequest { a: 1, b: 0 })).await;
        assert_eq!(refused.map(|_| ()).map_err(|status| status.code()), Err(Code::Unavailable));
    }
    assert_eq!(shed_lines(&capture), vec!["grpc shed=1".to_owned()], "the first refusal's line, the next two counted");

    hold.gate.notify_one();
    let _ = within("the held call", held).await;
    drop(channel);
    app.stop().await;
    assert_eq!(
        shed_lines(&capture),
        vec!["grpc shed=1".to_owned(), "grpc shed=2".to_owned()],
        "the two counted refusals, written once the server drops"
    );
}
