//! Mqtt on a tokio runtime built without its IO driver: the server's `listen()` fails as a
//! permanent `Configure` carrying `NoIoDriver`, within a bound, and `RpcClient::new` refuses the
//! link as `UnusableLink` carrying it. Registering a socket on such a runtime panics, and a
//! configuration mistake must not. Neither refusal reaches the network, so no broker is needed.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Builder;
use ulo::{App, Module, ModuleDef, ModuleIdentity, PrepareError, Signal, StartupError};
use ulo_rpc::{Link, RpcClient, Server, UnusableLink};
use ulo_rpc_mqtt::Mqtt;
use ulo_runtime_tokio::{NoIoDriver, Tokio};

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

fn without_io() -> tokio::runtime::Runtime {
    Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_on_a_runtime_without_io_fails_listen_as_a_permanent_configure() {
    let runtime = without_io();
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let serving = Mqtt::url("mqtt://127.0.0.1:1").with_handle(runtime.handle().clone());
    // A hang is the failure: the bound is a watchdog, not a timing the link keeps.
    let listened = tokio::time::timeout(Duration::from_secs(20), app.bind(Server::new(serving)).listen()).await;
    runtime.shutdown_background();
    let error = match listened {
        Err(_) => panic!("`listen` on a runtime without IO did not return within the watchdog bound"),
        Ok(Ok(app)) => {
            let _ = app.handle().close(Signal::new("no_io")).await;
            panic!("a server link on a runtime with no IO driver listened");
        }
        Ok(Err(error)) => error,
    };
    let StartupError::Configure(errors) = &error else { panic!("expected a `Configure` refusal, got: {error}") };
    assert!(ulo::is_permanent(&error), "retrying on the same runtime cannot mend it: {error}");
    let entry = errors.iter().next().expect("the report lists the link");
    // `Failures` flattens a link's error into the report's text, so the `NoIoDriver` value is
    // not recoverable here; the permanence flag and the text are what the report keeps.
    let prepared = entry.source.downcast_ref::<PrepareError>().unwrap_or_else(|| panic!("the entry is not a `PrepareError`: {entry}"));
    assert!(prepared.is_permanent(), "the entry is not marked permanent: {entry}");
    let entries = prepared.entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].subject(), Some(Mqtt::NAME), "the entry does not carry the link: {entries:?}");
    assert!(entry.to_string().contains("link `mqtt`: "), "the report does not name the link: {entry}");
    assert!(entry.to_string().contains(&NoIoDriver.to_string()), "the report does not carry the `NoIoDriver` text: {entry}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_on_a_runtime_without_io_is_refused_where_it_takes_the_link() {
    let runtime = without_io();
    let link = Mqtt::url("mqtt://127.0.0.1:1").with_handle(runtime.handle().clone());
    let built = RpcClient::new(link, Arc::new(Tokio::current()));
    runtime.shutdown_background();
    let refused: UnusableLink = match built {
        Ok(_) => panic!("a client was built on a runtime with no IO driver"),
        Err(refused) => refused,
    };
    assert_eq!(refused.link(), Mqtt::NAME);
    assert!(refused.to_string().contains("mqtt"), "the refusal does not name the link: {refused}");
    let cause = refused.source().expect("the refusal carries its cause");
    assert!(cause.is::<NoIoDriver>(), "the refusal does not carry `NoIoDriver`: {cause}");
    assert!(cause.to_string().contains("enable_io"), "the refusal names `enable_io`: {cause}");
}
