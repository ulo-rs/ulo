//! A link whose tokio runtime was built without the IO driver is refused naming `enable_io`: a
//! server in `prepare`, as a permanent `Configure`, and a client where it takes the link.
//! Registering a socket on such a runtime panics, and a configuration mistake must not.

use std::sync::Mutex;
use std::thread;

use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;
use ulo::{App, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

struct ClientRoot(Mutex<Option<Tcp>>);

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(RpcClientModule::for_root(link));
        }
    }
}

async fn listen_on(handle: Handle) -> StartupError {
    listen_at("127.0.0.1:0", handle).await
}

async fn listen_at(endpoint: &str, handle: Handle) -> StartupError {
    let bound = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(Tcp::new(endpoint).with_handle(handle)))
        .listen()
        .await;
    match bound {
        Err(error) => error,
        Ok(app) => {
            let _ = app.handle().close(Signal::new("no_io")).await;
            panic!("a server link on a runtime with no IO driver listened");
        }
    }
}

fn assert_refused(error: StartupError) {
    let StartupError::Configure(refused) = &error else { panic!("expected a `Configure` refusal, got: {error}") };
    assert!(refused.to_string().contains("enable_io"), "the refusal names `enable_io`: {refused}");
    assert!(ulo::is_permanent(&error), "retrying on the same runtime cannot mend it: {error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_multi_thread_runtime_without_io_is_refused_in_prepare() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let error = listen_on(runtime.handle().clone()).await;
    // Dropping a runtime inside another's context panics.
    runtime.shutdown_background();
    assert_refused(error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_current_thread_runtime_without_io_is_refused_in_prepare() {
    let runtime = Builder::new_current_thread().enable_time().build().expect("a runtime without IO");
    let handle = runtime.handle().clone();
    let (stop, stopped) = oneshot::channel::<()>();
    // A current-thread runtime runs work only while a thread blocks on it.
    let driver = thread::spawn(move || runtime.block_on(async { let _ = stopped.await; }));
    assert_refused(listen_on(handle).await);
    let _ = stop.send(());
    driver.join().expect("the runtime's thread ended");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_on_a_runtime_without_io_is_refused_where_it_takes_the_link() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let link = Tcp::new("127.0.0.1:7000").with_handle(runtime.handle().clone());
    let built = RpcClient::new(link, std::sync::Arc::new(Tokio::current()));
    runtime.shutdown_background();
    match built {
        Ok(_) => panic!("a client was built on a runtime with no IO driver"),
        Err(refused) => assert!(refused.to_string().contains("enable_io"), "the refusal names `enable_io`: {refused}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_io_is_refused_for_it_whatever_else_its_endpoint_gets_wrong() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let error = listen_at("not an endpoint", runtime.handle().clone()).await;
    runtime.shutdown_background();
    assert_refused(error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_current_thread_client_on_a_runtime_without_io_is_refused_where_it_takes_the_link() {
    let runtime = Builder::new_current_thread().enable_time().build().expect("a runtime without IO");
    let link = Tcp::new("127.0.0.1:7000").with_handle(runtime.handle().clone());
    let built = RpcClient::new(link, std::sync::Arc::new(Tokio::current()));
    // Dropping a runtime inside another's context panics.
    thread::spawn(move || drop(runtime)).join().expect("the runtime drops");
    match built {
        Ok(_) => panic!("a client was built on a current-thread runtime with no IO driver"),
        Err(refused) => assert!(refused.to_string().contains("enable_io"), "the refusal names `enable_io`: {refused}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_module_on_a_runtime_without_io_fails_connect() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let link = Tcp::new("127.0.0.1:7000").with_handle(runtime.handle().clone());
    let wired = App::builder(ClientRoot(Mutex::new(Some(link)))).runtime(Tokio::current()).wire().expect("the client wires");
    let connected = wired.connect().await;
    runtime.shutdown_background();
    match connected {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(refused), .. })) => {
            assert!(refused.to_string().contains("enable_io"), "the refusal names `enable_io`: {refused}");
        }
        Err(other) => panic!("expected the client module's init hook to fail `connect`, got: {other}"),
        Ok(app) => {
            let _ = app.close(Signal::new("no_io")).await;
            panic!("a client module over a link with no IO driver connected");
        }
    }
}
