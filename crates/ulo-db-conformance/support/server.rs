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

/// What a test adds to a recipe: files copied into the container before it starts, and server
/// arguments appended to the recipe's own.
#[derive(Clone, Default)]
pub struct Extras {
    pub files: Vec<(String, Vec<u8>)>,
    pub args: Vec<String>,
}

impl Server {
    pub async fn start(recipe: &Recipe) -> Server {
        Server::start_with(recipe, &Extras::default()).await
    }

    pub async fn start_with(recipe: &Recipe, extras: &Extras) -> Server {
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
            if !recipe.cmd.is_empty() || !extras.args.is_empty() {
                image = image.with_cmd(
                    recipe.cmd.iter().map(|arg| (*arg).to_owned()).chain(extras.args.iter().cloned()),
                );
            }
            for (path, bytes) in &extras.files {
                image = image.with_copy_to(path.clone(), bytes.clone());
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

/// Wraps a store's real migrations for the lock scenario. Each replica registers a reader of the
/// version table with the probe, then waits at a two-party rendezvous (500ms at most) before
/// calling the real migrator, so that with no lock both replicas start together. `count` reads the
/// rows the migrator's version table holds, zero while it does not exist; the scenario reads it once
/// after both replicas have finished.
pub struct Counted<S: Store> {
    pub inner: Arc<dyn Migrations<S>>,
    pub probe: MigrationProbe,
    pub count: fn(&S::Client) -> BoxFuture<'static, usize>,
}

impl<S: Store> Migrations<S> for Counted<S> {
    fn run(self: Arc<Self>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move {
            let count = self.count;
            let reading = client.clone();
            self.probe.reads_with(Arc::new(move || count(&reading)));
            self.probe.rendezvous::<ulo_db_conformance::OnTokio>().await;
            Arc::clone(&self.inner).run(client).await
        })
    }
}

/// The one migration every host applies: ordinary DDL, which every engine reads.
pub const CREATE_TABLE: &str = "create table ulo_conf_applied (id integer primary key)";

/// The same migration for SQLite, followed by a query that keeps the transaction open for a while.
/// SQLite serialises writers, so a migrator that starts a moment after the first one can find the
/// table already made and pass without ever overlapping it; the pause is wide enough that a
/// replica which starts beside the first one is still inside its transaction.
pub const CREATE_TABLE_HELD_SQLITE: &str = "create table ulo_conf_applied (id integer primary key); \
    with recursive c(x) as (select 1 union all select x + 1 from c where x < 1500000) select count(*) from c";

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

/// The container directory the TLS material is copied to.
const CERT_DIR: &str = "/certs";

/// A TLS-serving MySQL: `mysql:8` with a server certificate issued by `ca` for `localhost` and
/// `127.0.0.1`, and `require_secure_transport` on so a client that does not speak TLS is refused.
/// Start it with `Server::start_with(&MYSQL, &mysql_tls(&ca))`. A client trusts it through `ca`'s
/// certificate (`ca.cert_pem()`) and verifies the name, since the relay is reached at `127.0.0.1`.
pub fn mysql_tls(ca: &ulo_test_certs::Ca) -> Extras {
    mysql_tls_for(ca, &["localhost", "127.0.0.1"])
}

/// [`mysql_tls`] with a certificate for `names` instead, for a client that reaches the server at a
/// name the certificate does not carry.
pub fn mysql_tls_for(ca: &ulo_test_certs::Ca, names: &[&str]) -> Extras {
    let server = ca.issue(names);
    Extras {
        files: vec![
            (format!("{CERT_DIR}/ca.pem"), ca.cert_pem().into_bytes()),
            (format!("{CERT_DIR}/server-cert.pem"), server.cert_pem().as_bytes().to_vec()),
            (format!("{CERT_DIR}/server-key.pem"), server.key_pem().as_bytes().to_vec()),
        ],
        args: vec![
            format!("--ssl-ca={CERT_DIR}/ca.pem"),
            format!("--ssl-cert={CERT_DIR}/server-cert.pem"),
            format!("--ssl-key={CERT_DIR}/server-key.pem"),
            "--require-secure-transport=ON".to_owned(),
        ],
    }
}

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
