//! The TCP link's frame limit, 1 MiB unset, on both sides of a call: the client refuses a request
//! frame past its link's limit before writing it, `BadRequest` with `payload_too_large`, and a
//! server whose link keeps the default closes the connection of a client whose link allows more,
//! at the length prefix, so that client's call fails `Unavailable`. `max_frame` set on both links
//! carries the larger payload.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use ulo::{App, AppHandle, Bound, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_rpc::RpcClient;
use ulo_rpc_tcp::Tcp;
use ulo_transport::{CallError, ErrorKind};

const MIB: usize = 1024 * 1024;

/// What a frame carries besides its payload, with room to spare: the envelope's fields and the
/// pattern.
const ENVELOPE: usize = 1024;

const PATIENCE: Duration = Duration::from_secs(10);

#[controller]
struct Echo;

#[handlers]
impl Echo {
    #[ulo_rpc::message("frame.echo_len")]
    async fn echo_len(&self, text: ulo_rpc::Payload<String>) -> Result<usize, CallError> {
        Ok(text.0.len())
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Echo>();
    }
}

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
}

impl Running {
    async fn start(link: Tcp) -> Running {
        let server = App::builder(Root)
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .expect("the server app wires")
            .connect()
            .await
            .expect("the server app connects")
            .bind(ulo_rpc::Server::new(link))
            .listen()
            .await
            .expect("the server listens");
        let addr = match server.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = server.handle();
        let serving = tokio::spawn(async move {
            let _ = server.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, addr }
    }

    fn client(&self, link: impl FnOnce(Tcp) -> Tcp) -> RpcClient {
        RpcClient::new(link(Tcp::new(self.addr)), Arc::new(ulo_runtime_tokio::Tokio::current()))
            .expect("the link is usable")
            .timeout(Bound::After(PATIENCE))
            .expect("a nonzero timeout")
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

fn loopback() -> Tcp {
    Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}

async fn echo_len(client: &RpcClient, bytes: usize) -> Result<usize, ulo_rpc::RpcError> {
    client.request::<_, usize>("frame.echo_len", &"x".repeat(bytes)).await
}

#[tokio::test]
async fn a_request_frame_past_one_mib_is_refused_by_the_client_and_one_under_it_is_answered() {
    let running = Running::start(loopback()).await;
    let client = running.client(|link| link);
    let under = MIB - ENVELOPE;
    assert_eq!(echo_len(&client, under).await.expect("a frame under 1 MiB is carried"), under);
    let refused = echo_len(&client, MIB).await.expect_err("a frame past 1 MiB was carried");
    assert_eq!((refused.kind(), refused.reason()), (ErrorKind::BadRequest, Some("payload_too_large")), "{refused:?}");
    running.stop().await;
}

#[tokio::test]
async fn a_server_at_the_default_closes_a_connection_whose_frame_is_past_one_mib() {
    let running = Running::start(loopback()).await;
    let client = running.client(|link| link.max_frame(4 * MIB as u64));
    let refused = echo_len(&client, 2 * MIB).await.expect_err("the server read a frame past 1 MiB");
    assert_eq!(refused.kind(), ErrorKind::Unavailable, "{refused:?}");
    running.stop().await;
}

#[tokio::test]
async fn max_frame_on_both_links_carries_a_larger_payload() {
    let running = Running::start(loopback().max_frame(4 * MIB as u64)).await;
    let client = running.client(|link| link.max_frame(4 * MIB as u64));
    assert_eq!(echo_len(&client, 2 * MIB).await.expect("both links allow 4 MiB"), 2 * MIB);
    running.stop().await;
}
