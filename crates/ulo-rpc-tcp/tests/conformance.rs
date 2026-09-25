//! The shared RPC conformance suite, against this crate's own adapter.
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the `Broker`, which for tcp is the
//! adapter on a socket bound here. The client reaches it through a proxy this file owns, which is
//! what a disruption cuts: the proxy drops every connection it carries, the client's read half sees
//! EOF, and the next call reconnects. Nothing here needs a container.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;
use ulo_rpc_conformance::Broker;
use ulo_rpc_tcp::{TcpAdapter, TcpClientTransport};

struct TcpBroker {
    /// The socket the adapter serves on, handed over by the first `adapter` call.
    server: Mutex<Option<std::net::TcpListener>>,
    proxy_port: u16,
    /// One task per proxied connection, holding both of its sockets.
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    _accept: JoinHandle<()>,
}

impl Broker for TcpBroker {
    type Adapter = TcpAdapter;
    type Transport = TcpClientTransport;

    async fn start() -> Self {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server_port = server.local_addr().unwrap().port();
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();

        let connections: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let accept = {
            let connections = connections.clone();
            tokio::spawn(async move {
                while let Ok((mut client, _)) = proxy.accept().await {
                    let connection = tokio::spawn(async move {
                        let Ok(mut upstream) =
                            tokio::net::TcpStream::connect(("127.0.0.1", server_port)).await
                        else {
                            return;
                        };
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                    });
                    connections.lock().unwrap().push(connection);
                }
            })
        };

        Self {
            server: Mutex::new(Some(server)),
            proxy_port,
            connections,
            _accept: accept,
        }
    }

    /// The first instance serves on the socket the proxy forwards to. A second instance on one
    /// broker gets a socket of its own that nothing is connected to: a socket transport has no
    /// fan-out, and a client talks to one server.
    fn adapter(&self) -> Self::Adapter {
        let listener = self
            .server
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| std::net::TcpListener::bind("127.0.0.1:0").unwrap());
        TcpAdapter::from_listener(listener)
    }

    fn transport(&self) -> Self::Transport {
        TcpClientTransport::new("127.0.0.1", self.proxy_port).with_timeout(Duration::from_secs(2))
    }

    /// Drop every proxied connection: both sockets close, the client's reader sees EOF, and the
    /// server's per-connection task ends.
    async fn disrupt(&self) {
        for connection in self.connections.lock().unwrap().drain(..) {
            connection.abort();
        }
        // Lets the aborts run and the client's reader see EOF before the case's next call;
        // without it the next call goes out on the dead connection and waits out its timeout.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

ulo_rpc_conformance::conformance_suite!(TcpBroker);
