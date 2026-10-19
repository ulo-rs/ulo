//! The pieces every store crate's `tests/conformance.rs` shares, included with `#[path]` from each:
//! a database server in a container named `ulo-conf-*` behind a relay the host cuts, the options
//! builders, and migrations that count what they applied.

#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use testcontainers::core::IntoContainerPort;
use testcontainers::core::WaitFor;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo::{BoxError, BoxFuture};
use ulo_db::{Migrations, Store};
use ulo_db_conformance::relay::{Relay, reachable, unshadowed};
use ulo_db_conformance::{MigrationProbe, report, startup_failed};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// A server in a container of its own, reached through a relay.
pub struct Server {
    pub direct: SocketAddr,
    pub relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

/// What starts a server.
pub struct Recipe {
    pub label: &'static str,
    pub image: &'static str,
    pub tag: &'static str,
    pub port: u16,
    pub env: &'static [(&'static str, &'static str)],
    pub cmd: &'static [&'static str],
    pub ready: fn() -> WaitFor,
}

impl Server {
    pub async fn start(recipe: &Recipe) -> Server {
        let (container, addrs) = unshadowed(recipe.label, || async {
            let name = format!(
                "ulo-conf-{}-{}-{}",
                recipe.label,
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::SeqCst)
            );
            let mut image = GenericImage::new(recipe.image, recipe.tag)
                .with_exposed_port(recipe.port.tcp())
                .with_wait_for((recipe.ready)())
                .with_wait_for(WaitFor::millis(1500))
                .with_container_name(name)
                .with_startup_timeout(Duration::from_secs(120));
            for (key, value) in recipe.env {
                image = image.with_env_var(*key, *value);
            }
            if !recipe.cmd.is_empty() {
                image = image.with_cmd(recipe.cmd.iter().copied());
            }
            let container = image
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the {} container did not start: {}", recipe.label, report(&error)));
            let port = container
                .get_host_port_ipv4(recipe.port)
                .await
                .unwrap_or_else(|error| startup_failed!("the {} port is not mapped: {}", recipe.label, report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let direct = addrs[0];
        reachable(direct, Duration::from_secs(30)).await;
        Server { direct, relay: Relay::start(direct).await, _container: container }
    }

    pub async fn cut(&self) {
        self.relay.cut_for(Duration::from_secs(3600)).await;
    }

    pub async fn restore(&self) {
        self.relay.cut_for(Duration::ZERO).await;
    }
}

/// A loopback port nothing listens on.
pub fn dead_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
    listener.local_addr().expect("its address").port()
}

/// Options read the way a configuration section is: from a map of fields.
pub fn options<T: DeserializeOwned>(fields: serde_json::Value) -> T {
    serde_json::from_value(fields).expect("the test's options deserialize")
}

/// Wraps a store's real migrations, counting how many migration rows they added: `count` reads the
/// rows the migrator's version table holds, zero while it does not exist.
pub struct Counted<S: Store> {
    pub inner: Arc<dyn Migrations<S>>,
    pub probe: MigrationProbe,
    pub count: fn(&S::Client) -> BoxFuture<'static, usize>,
}

impl<S: Store> Migrations<S> for Counted<S> {
    fn run(self: Arc<Self>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move {
            let before = (self.count)(&client).await;
            let outcome = Arc::clone(&self.inner).run(client.clone()).await;
            let after = (self.count)(&client).await;
            for _ in before..after {
                self.probe.record();
            }
            outcome
        })
    }
}

/// The one migration every host applies: ordinary DDL, which every engine reads.
pub const CREATE_TABLE: &str = "create table ulo_conf_applied (id integer primary key)";

fn ready_postgres() -> WaitFor {
    WaitFor::log(LogWaitStrategy::stderr("listening on IPv4 address"))
}

fn ready_mysql() -> WaitFor {
    WaitFor::message_on_stderr("port: 3306  MySQL Community Server")
}

fn ready_mongo() -> WaitFor {
    WaitFor::message_on_stdout("\"address\":\"0.0.0.0\"")
}

fn ready_redis() -> WaitFor {
    WaitFor::message_on_stdout("Ready to accept connections")
}

/// The password every server's real account holds; the suite's refused one is `PASSWORD`.
pub const REAL_PASSWORD: &str = "realpw";

pub static POSTGRES: Recipe = Recipe {
    label: "pg",
    image: "postgres",
    tag: "18",
    port: 5432,
    env: &[("POSTGRES_PASSWORD", REAL_PASSWORD)],
    cmd: &[],
    ready: ready_postgres,
};

pub static MYSQL: Recipe = Recipe {
    label: "mysql",
    image: "mysql",
    tag: "8",
    port: 3306,
    env: &[("MYSQL_ROOT_PASSWORD", REAL_PASSWORD), ("MYSQL_DATABASE", "app")],
    cmd: &[],
    ready: ready_mysql,
};

pub static MONGO: Recipe = Recipe {
    label: "mongo",
    image: "mongo",
    tag: "5.0.6",
    port: 27017,
    // No `--auth`: every connection is allowed, and a URI naming a user is refused, so the refused
    // credentials are exercised. An authenticating server makes the driver derive a SCRAM key on the
    // worker thread, tens of milliseconds in a debug build, which the one-thread scenario reads as
    // a stall of the store.
    env: &[],
    cmd: &[],
    ready: ready_mongo,
};

pub static REDIS: Recipe = Recipe {
    label: "redis",
    image: "redis",
    tag: "7",
    port: 6379,
    env: &[],
    cmd: &["redis-server", "--requirepass", REAL_PASSWORD],
    ready: ready_redis,
};

/// A directory of the host's own under the OS temporary directory, removed with the guard.
pub struct Scratch(pub std::path::PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "ulo-conf-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("a directory for the host's files");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
