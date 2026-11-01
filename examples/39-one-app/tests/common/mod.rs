//! Serves the app on four servers at port 0, in the order `main` binds them, and dials each.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use ex_39_one_app::grpc::pb::notes_client::NotesClient;
use ex_39_one_app::{Gate, Store, builder, config};
use ex_support::{Served, serve};
use tonic::transport::{Channel, Endpoint};
use ulo::Dep;
use ulo_config::{Env, Values};
use ulo::testing::ScriptedTimer;
use ulo_rpc::RpcClient;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

pub struct Running {
    pub served: Served,
    pub http: SocketAddr,
    pub ws: SocketAddr,
    pub rpc: SocketAddr,
    pub grpc: SocketAddr,
    /// The transport keys of `addresses()`, in the order the servers were bound.
    pub order: Vec<&'static str>,
}

pub async fn start(clock: Option<ScriptedTimer>) -> Running {
    start_in(clock, Env::from_map::<&str, &str>([])).await
}

/// As [`start`], with `env` as the process environment the configuration reads.
pub async fn start_in(clock: Option<ScriptedTimer>, env: Env) -> Running {
    let values = ["http", "ws", "rpc", "grpc"].iter().fold(Values::new(), |v, t| v.set(format!("ports.{t}"), 0));
    let config = config(env, values);
    let mut app = builder(config).runtime(Tokio::current());
    if let Some(clock) = clock {
        app = app.timer(clock);
    }
    let app = app.wire().expect("wires").connect().await.expect("connects");
    let app = app
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("listens");
    let bound = app.addresses();
    let order = bound.iter().map(|b| b.transport).collect();
    let [http, ws, rpc, grpc] = [0, 1, 2, 3].map(|i| bound[i].addr);
    Running { served: serve(app), http, ws, rpc, grpc, order }
}

impl Running {
    pub async fn store(&self) -> Dep<Store> {
        self.served.handle.get().await.expect("the store resolves")
    }

    pub async fn gate(&self) -> Dep<Gate> {
        self.served.handle.get().await.expect("the gate resolves")
    }

    pub fn rpc_client(&self) -> RpcClient {
        RpcClient::new(Tcp::new(self.rpc), Arc::new(Tokio::current())).expect("the link is usable")
    }

    pub async fn grpc_client(&self) -> NotesClient<Channel> {
        let channel = Endpoint::from_shared(format!("http://{}", self.grpc)).expect("an endpoint").connect().await.expect("connects");
        NotesClient::new(channel)
    }
}

/// As `ex_support::advancing`, for a close that arms sleeps of several lengths: each time one of
/// `steps` is waiting, the clock moves by it. The drain window and the 500 ms grace the servers'
/// `close` gives their connections after it are two such lengths.
pub async fn advancing_any<T>(clock: &ScriptedTimer, steps: &[std::time::Duration], fut: impl std::future::Future<Output = T>) -> T {
    let mut fut = std::pin::pin!(fut);
    let driven = async {
        loop {
            tokio::select! {
                out = &mut fut => return out,
                () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {
                    if let Some(step) = steps.iter().find(|s| clock.live(**s) > 0) {
                        clock.advance(*step);
                    }
                }
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), driven).await.expect("the future ended within ten seconds of real time")
}
