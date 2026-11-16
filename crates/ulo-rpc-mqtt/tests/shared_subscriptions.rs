//! Q29-11: a broker whose CONNACK announces no shared subscriptions, Shared Subscription Available
//! (property 0x2A) at 0, fails `listen` only for a server with a pattern reaching one instance,
//! whose `$share/<group>/` filter the broker would not honour, and before it subscribes; a server
//! whose patterns are all events declared `every_instance` subscribes plain filters and listens.
//! The broker is a socket that answers CONNECT with a scripted CONNACK, SUBSCRIBE with a SUBACK
//! granting every filter, and PINGREQ with PINGRESP, recording each SUBSCRIBE's filters; no
//! available broker lacks shared subscriptions.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{Instances, Link, Subscription};
use ulo_rpc_mqtt::Mqtt;
use ulo_runtime_tokio::Tokio;

/// How long the test waits for each step of the link or the broker before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

const CONNECT: u8 = 1;
const SUBSCRIBE: u8 = 8;
const PINGREQ: u8 = 12;
const DISCONNECT: u8 = 14;

/// An MQTT 5 CONNACK, no session present and reason code success, whose one property is Shared
/// Subscription Available set to `available`.
fn connack(available: u8) -> [u8; 7] {
    [0x20, 0x05, 0x00, 0x00, 0x02, 0x2A, available]
}

/// One control packet: its first byte and the body its Remaining Length counts; `None` once the
/// connection ends.
async fn packet(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let first = stream.read_u8().await.ok()?;
    let (mut length, mut shift) = (0usize, 0);
    loop {
        let byte = stream.read_u8().await.ok()?;
        length |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.ok()?;
    Some((first, body))
}

/// A variable byte integer at the start of `bytes`: its value and how many bytes it takes.
fn variable(bytes: &[u8]) -> (usize, usize) {
    let (mut value, mut read) = (0usize, 0);
    while let Some(&byte) = bytes.get(read) {
        value |= usize::from(byte & 0x7f) << (7 * read);
        read += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
    (value, read)
}

/// A SUBSCRIBE body's packet identifier and topic filters.
fn subscribed(body: &[u8]) -> ([u8; 2], Vec<String>) {
    let id = [body[0], body[1]];
    let (properties, read) = variable(&body[2..]);
    let mut at = 2 + read + properties;
    let mut filters = Vec::new();
    while at < body.len() {
        let length = usize::from(u16::from_be_bytes([body[at], body[at + 1]]));
        filters.push(String::from_utf8(body[at + 2..at + 2 + length].to_vec()).expect("a topic filter is UTF-8"));
        // The filter's subscription options byte follows it.
        at += 2 + length + 1;
    }
    (id, filters)
}

/// Serves one connection: the scripted CONNACK, then a SUBACK granting QoS 1 to every filter of
/// each SUBSCRIBE, whose filters go to `subscriptions`, until the link disconnects or the
/// connection ends.
async fn broker(listener: TcpListener, available: u8, subscriptions: mpsc::UnboundedSender<Vec<String>>) {
    let (mut stream, _) = listener.accept().await.expect("the broker's socket accepts");
    let (first, _) = packet(&mut stream).await.expect("the link sent no CONNECT");
    assert_eq!(first >> 4, CONNECT, "the link's first packet is not a CONNECT");
    stream.write_all(&connack(available)).await.expect("the broker writes its CONNACK");
    while let Some((first, body)) = packet(&mut stream).await {
        match first >> 4 {
            SUBSCRIBE => {
                let (id, filters) = subscribed(&body);
                let granted = u8::try_from(filters.len()).expect("one SUBSCRIBE holds a few filters");
                let mut suback = vec![0x90, 3 + granted, id[0], id[1], 0x00];
                suback.extend(std::iter::repeat_n(0x01, filters.len()));
                let _ = subscriptions.send(filters);
                if stream.write_all(&suback).await.is_err() {
                    return;
                }
            }
            PINGREQ => {
                if stream.write_all(&[0xD0, 0x00]).await.is_err() {
                    return;
                }
            }
            DISCONNECT => return,
            _ => {}
        }
    }
}

/// What a server's `listen` on `subscriptions` answers against a broker announcing
/// `available`, and the filters of every SUBSCRIBE the broker read.
async fn listened(available: u8, subscriptions: &[Subscription]) -> (Result<(), BoxError>, Vec<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let mut link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address"))).group("shares");
    let app = ulo::App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    link.prepare(&app.handle()).await.expect("the link prepares");
    let (sent, mut read) = mpsc::unbounded_channel();
    let serving = tokio::spawn(broker(listener, available, sent));
    let listened = tokio::time::timeout(PATIENCE, link.listen(subscriptions)).await.expect("`listen` did not answer within the patience");
    let outcome = match listened {
        Ok(inbound) => {
            drop(inbound);
            tokio::time::timeout(PATIENCE, link.close()).await.expect("the link did not close within the patience").expect("the link closes");
            Ok(())
        }
        Err(error) => Err(error),
    };
    tokio::time::timeout(PATIENCE, serving)
        .await
        .expect("the link's connection was still open after `listen` failed or the link closed")
        .expect("the broker's task");
    let mut filters = Vec::new();
    while let Ok(subscribed) = read.try_recv() {
        filters.push(subscribed);
    }
    (outcome, filters)
}

/// A server with a pattern reaching one instance fails `listen` against a broker announcing no
/// shared subscriptions, naming the setting, and subscribes nothing. Planted break: the CONNACK's
/// property not read.
#[tokio::test(flavor = "multi_thread")]
async fn a_broker_without_shared_subscriptions_refuses_a_pattern_reaching_one_instance() {
    let subscriptions = [Subscription::new("shares.request", Instances::One), Subscription::new("shares.every", Instances::Every)];
    let (outcome, filters) = listened(0, &subscriptions).await;
    let error = outcome.expect_err("a server with a `$share` filter listened against a broker without shared subscriptions");
    assert!(
        error.to_string().contains("announces no shared-subscription support") && error.to_string().contains("`Mqtt::group`"),
        "the refusal does not name the missing support and the setting: {error}"
    );
    assert!(filters.is_empty(), "the link subscribed before refusing: {filters:?}");
}

/// A server whose patterns are all events declared `every_instance` listens against the same
/// broker, subscribing the plain filter and the control topic, no `$share` filter among them.
/// Planted break: the refusal made whatever the filters.
#[tokio::test(flavor = "multi_thread")]
async fn a_broker_without_shared_subscriptions_serves_every_instance_events() {
    let (outcome, filters) = listened(0, &[Subscription::new("shares.every", Instances::Every)]).await;
    outcome.expect("a server whose patterns all reach every instance was refused against a broker without shared subscriptions");
    assert_eq!(filters, [vec!["shares.every".to_owned(), "ulo/rpc/control".to_owned()]], "the filters the link subscribed");
}

/// The same broker announcing shared subscriptions takes the one-instance pattern's `$share`
/// filter: the refusal above is the property's, not the script's.
#[tokio::test(flavor = "multi_thread")]
async fn a_broker_with_shared_subscriptions_serves_a_pattern_reaching_one_instance() {
    let (outcome, filters) = listened(1, &[Subscription::new("shares.request", Instances::One)]).await;
    outcome.expect("a server was refused against a broker announcing shared subscriptions");
    assert_eq!(filters, [vec!["$share/shares/shares.request".to_owned(), "ulo/rpc/control".to_owned()]], "the filters the link subscribed");
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}
