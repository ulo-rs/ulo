//! A shout reaches the members of its room on both processes of one app, through a real Redis in
//! a container, and no one outside the room on either. Each process is the example's binary.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use ex_support::Bin;
use ex_support::ws::{Socket, connect, hang_up, next, send};
use serde_json::{Value, json};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::time::{Instant, timeout_at};
use ulo_conformance_support::relay::{reachable, unshadowed};
use ulo_conformance_support::{report, startup_failed};

const CONTAINER: &str = "ulo-conf-ex24-redis";
/// How long a shout may take to cross to the other process.
const PATIENCE: Duration = Duration::from_secs(10);

async fn start_redis() -> (ContainerAsync<GenericImage>, String) {
    const PORT: u16 = 6379;
    let (container, addrs) = unshadowed("the Redis container", || async {
        let container = GenericImage::new("redis", "7-alpine")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .with_container_name(CONTAINER)
            .start()
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
        let port = container
            .get_host_port_ipv4(PORT)
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
        (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
    })
    .await;
    reachable(addrs[0], Duration::from_secs(10)).await;
    (container, format!("redis://{}", addrs[0]))
}

async fn client(addr: SocketAddr, room: Option<&str>) -> Socket {
    let mut socket = connect(addr, "/rooms", &[]).await;
    if let Some(room) = room {
        send(&mut socket, json!({ "event": "join", "id": 0, "data": room })).await;
        assert_eq!(next(&mut socket).await, json!({ "id": 0, "complete": true }));
    }
    socket
}

/// The data of the next `shouted` frame within `within`; `None` when none comes.
async fn shouted(socket: &mut Socket, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        let frame: Value = timeout_at(deadline, next(socket)).await.ok()?;
        if frame["event"] == "shouted" {
            return frame["data"].as_str().map(str::to_owned);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shout_reaches_the_room_on_both_processes_and_nobody_outside_it() {
    let (_redis, url) = start_redis().await;
    let exe = env!("CARGO_BIN_EXE_ex-24-ws-rooms");
    let mut first = Bin::server(exe, &[("REDIS_URL", url.as_str())]);
    let mut second = Bin::server(exe, &[("REDIS_URL", url.as_str())]);
    let (first_addr, second_addr) = (first.addr(), second.addr());

    let mut ada = client(first_addr, Some("lobby")).await;
    let mut bob = client(second_addr, Some("lobby")).await;
    let mut cy = client(first_addr, None).await;
    let mut dee = client(second_addr, Some("other")).await;

    // Each process printed its address after `listen`, which returns once its Redis subscription is
    // live, so the first shout is heard.
    send(&mut ada, json!({ "event": "shout", "id": 1, "data": { "room": "lobby", "text": "one" } })).await;
    let heard = shouted(&mut bob, PATIENCE).await.unwrap_or_else(|| panic!("the first shout did not reach bob within {PATIENCE:?}"));
    assert_eq!(heard, "one");

    // Bob answers once, and ada, on the first process, hears it.
    send(&mut bob, json!({ "event": "shout", "id": 1, "data": { "room": "lobby", "text": "two" } })).await;
    // Ada is in the room too, so she hears her own shout first.
    assert_eq!(shouted(&mut ada, PATIENCE).await.as_deref(), Some("one"), "ada's own shout");
    assert_eq!(shouted(&mut ada, PATIENCE).await.as_deref(), Some("two"), "bob's shout, from the other process");

    // Everything has been delivered to the room on both processes; cy and dee, outside it, have
    // been sent nothing ahead of a ping's answer.
    for (socket, id) in [(&mut cy, 7), (&mut dee, 8)] {
        send(socket, json!({ "event": "ping", "id": id })).await;
        assert_eq!(next(socket).await, json!({ "id": id, "data": "pong" }));
    }

    for socket in [ada, bob, cy, dee] {
        hang_up(socket).await;
    }
    for bin in [first, second] {
        let exit = tokio::task::spawn_blocking(move || bin.terminate()).await.unwrap();
        assert!(exit.status.success(), "{exit:?}");
        assert_eq!(exit.stderr, "");
    }
}
