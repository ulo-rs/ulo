//! The shared RPC conformance suite, against this crate's own adapter.
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the `Broker`, which for udp is the
//! adapter on a socket bound here. Datagrams pass through a proxy this file owns, which is what a
//! disruption cuts: udp has no connection, and the proxy drops every datagram for longer than the
//! client's call timeout, then forwards again. Nothing here needs a container.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::task::JoinHandle;
use ulo_rpc_conformance::Broker;
use ulo_rpc_udp::{UdpAdapter, UdpClientTransport};

struct UdpBroker {
    /// The socket the adapter serves on, handed over by the first `adapter` call.
    server: Mutex<Option<std::net::UdpSocket>>,
    proxy_port: u16,
    severed: Arc<AtomicBool>,
    _forward: JoinHandle<()>,
}

impl Broker for UdpBroker {
    type Adapter = UdpAdapter;
    type Transport = UdpClientTransport;

    async fn start() -> Self {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();

        let severed = Arc::new(AtomicBool::new(false));
        let forward = {
            let severed = severed.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65_535];
                let mut client: Option<SocketAddr> = None;
                while let Ok((n, from)) = proxy.recv_from(&mut buf).await {
                    if severed.load(Ordering::SeqCst) {
                        continue;
                    }
                    let to = if from == server_addr {
                        match client {
                            Some(client) => client,
                            None => continue,
                        }
                    } else {
                        client = Some(from);
                        server_addr
                    };
                    let _ = proxy.send_to(&buf[..n], to).await;
                }
            })
        };

        Self {
            server: Mutex::new(Some(server)),
            proxy_port,
            severed,
            _forward: forward,
        }
    }

    /// The first instance serves on the socket the proxy forwards to. A second instance on one
    /// broker gets a socket of its own that nothing sends to: a socket transport has no fan-out,
    /// and a client sends to one server.
    fn adapter(&self) -> Self::Adapter {
        let socket = self
            .server
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| std::net::UdpSocket::bind("127.0.0.1:0").unwrap());
        UdpAdapter::from_socket(socket)
    }

    fn transport(&self) -> Self::Transport {
        UdpClientTransport::new("127.0.0.1", self.proxy_port).with_timeout(Duration::from_secs(2))
    }

    /// Drop every datagram for longer than the client's call timeout, then forward again. Returns
    /// before the window closes, so the case's next call lands in it.
    async fn disrupt(&self) {
        self.severed.store(true, Ordering::SeqCst);
        let severed = self.severed.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            severed.store(false, Ordering::SeqCst);
        });
    }
}

ulo_rpc_conformance::conformance_suite!(UdpBroker);
