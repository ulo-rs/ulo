//! A Postgres server in a container of the caller's name, for the examples whose tests need one.
//! Included with `#[path]` by every example that starts a database.

#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo_conformance_support::relay::{reachable, unshadowed};
use ulo_conformance_support::{report, startup_failed};

pub const PASSWORD: &str = "realpw";
const PORT: u16 = 5432;

/// The container is removed when this drops.
pub struct Server {
    pub direct: SocketAddr,
    _container: ContainerAsync<GenericImage>,
}

impl Server {
    /// Starts `postgres:18` as the container `name`.
    pub async fn start(name: &'static str) -> Server {
        let (container, addrs) = unshadowed("the Postgres container", || async {
            let container = GenericImage::new("postgres", "18")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::log(LogWaitStrategy::stderr("listening on IPv4 address")))
                .with_wait_for(WaitFor::millis(1500))
                .with_env_var("POSTGRES_PASSWORD", PASSWORD)
                .with_container_name(name)
                .with_startup_timeout(Duration::from_secs(120))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Postgres container did not start: {}", report(&error)));
            let port = container
                .get_host_port_ipv4(PORT)
                .await
                .unwrap_or_else(|error| startup_failed!("the Postgres port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        reachable(addrs[0], Duration::from_secs(30)).await;
        Server { direct: addrs[0], _container: container }
    }
}

/// A URL for the default database at `addr`, which is the server or a relay in front of it.
pub fn url(addr: SocketAddr) -> String {
    format!("postgres://postgres:{PASSWORD}@{addr}/postgres")
}
