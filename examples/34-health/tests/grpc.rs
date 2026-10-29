use std::time::Duration;

use ex_34_health::{Cache, LAG, builder};
use ex_support::serve;
use tonic::transport::{Channel, Endpoint};
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use ulo::{Dep, Signal};
use ulo_conformance_support::ScriptedTimer;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);
/// The bridge's default interval: `grpc.health.v1` answers from a stored status, which the bridge
/// refreshes this often.
const EVERY: Duration = Duration::from_secs(5);

async fn overall(client: &mut HealthClient<Channel>) -> ServingStatus {
    client.check(HealthCheckRequest { service: String::new() }).await.expect("Check answers").into_inner().status()
}

/// Polls until `""` reads `want`, moving the clock on whenever the bridge waits out its interval.
async fn until(client: &mut HealthClient<Channel>, clock: &ScriptedTimer, want: ServingStatus) {
    let begun = tokio::time::Instant::now();
    while overall(client).await != want {
        assert!(begun.elapsed() < PATIENCE, "the overall status never became {want:?}");
        if clock.live(EVERY) > 0 {
            clock.advance(EVERY);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `grpc.health.v1` follows the same checks as the HTTP probes: NOT_SERVING while the cache is
/// cold, SERVING once it is warm, NOT_SERVING again from the start of the close, a lag before the
/// drain.
#[tokio::test(flavor = "multi_thread")]
async fn the_overall_status_follows_readiness_and_turns_not_serving_when_the_close_begins() {
    let clock = ScriptedTimer::new();
    let app = builder()
        .runtime(Tokio::current())
        .timer(clock.clone())
        .wire()
        .unwrap()
        .connect()
        .await
        .unwrap()
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap();
    let grpc = app.addresses().into_iter().find(|b| b.transport == "grpc").expect("a gRPC address").addr;
    let server = serve(app);
    let channel = Endpoint::from_shared(format!("http://{grpc}")).unwrap().connect().await.unwrap();
    let mut client = HealthClient::new(channel);

    until(&mut client, &clock, ServingStatus::NotServing).await;
    let cache: Dep<Cache> = server.handle.get().await.unwrap();
    cache.warm_up();
    until(&mut client, &clock, ServingStatus::Serving).await;

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    // The bridge writes NOT_SERVING in its before-shutdown hook, so no interval has to pass: the
    // clock stays where it is until the status is read, and the drain is still a lag away.
    let begun = tokio::time::Instant::now();
    while overall(&mut client).await != ServingStatus::NotServing {
        assert!(begun.elapsed() < PATIENCE, "the overall status did not turn NOT_SERVING when the close began");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(tokio::time::timeout(Duration::from_millis(50), server.handle.draining()).await.is_err(), "the drain began before the lag passed");
    clock.advance(LAG);
    server.handle.draining().await;
    closing.await.unwrap().unwrap();
}
