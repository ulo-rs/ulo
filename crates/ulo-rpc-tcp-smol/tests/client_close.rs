//! The client app's close ends its TCP connection cleanly: `RpcClientModule`'s destroy hook calls
//! the link's `close`, which shuts the socket the client opened, and the peer reads its end with
//! nothing written after the call's frame.

use std::sync::Mutex;
use std::time::Duration;

use async_net::TcpListener;
use futures_util::AsyncReadExt;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, Timer};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;

/// Longer than any wait below, so the call is still waiting at the close and no timeout's `cancel`
/// reaches the peer.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the close has to show at the peer.
const PATIENCE: Duration = Duration::from_secs(5);

struct ClientRoot {
    link: Mutex<Option<Tcp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    let expired = OnSmol::runtime().sleep(PATIENCE);
    match futures_util::future::select(std::pin::pin!(fut), expired).await {
        futures_util::future::Either::Left((output, _)) => output,
        futures_util::future::Either::Right(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

#[test]
fn closing_the_client_app_ends_its_connection_cleanly() {
    OnSmol::block_on(async {
        // A peer that reads the call and never answers it.
        let peer = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
        let addr = peer.local_addr().expect("the peer's address");
        let app = App::builder(ClientRoot { link: Mutex::new(Some(Tcp::new(addr))) })
            .runtime(OnSmol::runtime())
            .wire()
            .expect("the client app wires")
            .connect()
            .await
            .expect("the client app connects");
        let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
        let _call = ulo::spawn_with(&OnSmol::runtime(), async move { rpc.request::<String, String>("held", &"x".to_owned()).await });

        let (mut connection, _) = within("the call's connection", peer.accept()).await.expect("the peer accepts");
        let mut prefix = [0u8; 4];
        connection.read_exact(&mut prefix).await.expect("the call's frame arrives");
        let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
        connection.read_exact(&mut frame).await.expect("the call's frame arrives whole");

        if let Err(error) = app.close(Signal::new("client_close")).await {
            panic!("the client app's close reported a failure: {error}");
        }

        let mut rest = [0u8; 1];
        let read = within("the client's connection's end", connection.read(&mut rest)).await;
        assert_eq!(read.expect("the connection ends cleanly"), 0, "the connection ends at the app's close, with nothing more sent");
    });
}
