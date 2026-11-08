//! An event declared `every_instance` reaches every server instance listening on its pattern:
//! `bind` hands the link each mounted pattern with the `Instances` its handler declares, one for a
//! request and for an event left at its default. `prepare` refuses the option on a request handler,
//! which no attribute can write and a hand-written controller can, and on a link declared without
//! the `every_instance` capability, TCP's, naming the link's reason. No broker is needed: the
//! recording link carries frames over TCP and declares what a broker would.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use ulo::{App, AppHandle, BoundAddr, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Signal, StartupError, controller, handlers};
use ulo::{Construct, ConstructError, Controller, Dependencies, HandlerSpec, Handlers, Mount, Resolver};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, DeliveryMode, Instances, Link, Outbound, Payload, Reply, Rpc, RpcCx, RpcHandler, Subscription};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, Count, ErrorKind};

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[controller]
struct Prices;

#[handlers]
impl Prices {
    #[ulo_rpc::message("prices.quote")]
    async fn quote(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0)
    }

    #[ulo_rpc::event("prices.recorded")]
    async fn recorded(&self, _n: Payload<u32>) -> Result<(), Never> {
        Ok(())
    }

    #[ulo_rpc::event("prices.changed", every_instance)]
    async fn changed(&self, _n: Payload<u32>) -> Result<(), Never> {
        Ok(())
    }
}

struct PricesRoot;

impl Module for PricesRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Prices>();
    }
}

/// A request handler declared `every_instance`, which only a hand-written controller can write.
struct EveryRequest;

impl Construct for EveryRequest {
    type Scope = ulo::scope::Auto;

    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(EveryRequest)
    }
}

async fn answer(_cx: RpcCx) -> Result<Reply, BoxError> {
    Ok(Reply::None)
}

impl Controller for EveryRequest {}

impl Handlers for EveryRequest {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Rpc, _>::new("answer", RpcHandler::message(answer).every_instance()).route("every.request"));
    }
}

struct EveryRequestRoot;

impl Module for EveryRequestRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<EveryRequest>();
    }
}

/// TCP's frames under the capabilities of a broker that delivers both ways, recording what `bind`
/// asks it to listen on.
struct Recording {
    tcp: Tcp,
    listened: Arc<Mutex<Vec<Subscription>>>,
}

impl Link for Recording {
    const NAME: &'static str = "recording";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.tcp.prepare(app).await
    }

    fn max_inflight(&mut self, calls: Count) {
        self.tcp.max_inflight(calls);
    }

    async fn listen(&self, subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        *self.listened.lock().unwrap_or_else(PoisonError::into_inner) = subscriptions.to_vec();
        self.tcp.listen(subscriptions).await
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.tcp.connect(runtime).await
    }

    async fn drain(&self) {
        self.tcp.drain().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.tcp.close().await
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.tcp.bound()
    }
}

/// Binds `link` under `root`, answering the startup error or closing the app at once.
async fn bind<L: Link>(root: impl Module, link: L) -> Result<(), StartupError> {
    let app = App::builder(root).runtime(Tokio::current()).wire()?.connect().await?.bind(ulo_rpc::Server::new(link)).listen().await?;
    app.serve(async { Signal::new("test") }).await.expect("the app shuts down");
    Ok(())
}

#[tokio::test]
async fn bind_hands_the_link_each_pattern_with_the_instances_its_handler_declares() {
    let listened = Arc::new(Mutex::new(Vec::new()));
    bind(PricesRoot, Recording { tcp: Tcp::new("127.0.0.1:0"), listened: Arc::clone(&listened) }).await.expect("the server binds");
    let listened = listened.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(
        listened,
        vec![
            Subscription::new("prices.changed", Instances::Every),
            Subscription::new("prices.quote", Instances::One),
            Subscription::new("prices.recorded", Instances::One),
        ]
    );
}

#[tokio::test]
async fn an_every_instance_event_on_a_link_declared_without_it_is_refused_naming_the_reason() {
    let tcp = Tcp::new("127.0.0.1:0");
    let reason = tcp.capabilities().why_not_every_instance().expect("the TCP link declares `every_instance` absent");
    let refused = bind(PricesRoot, tcp).await.expect_err("an `every_instance` event bound on TCP");
    let text = refused.to_string();
    assert!(matches!(refused, StartupError::Configure(_)), "{text}");
    assert!(text.contains("`Prices::changed`") && text.contains("prices.changed") && text.contains("every_instance"), "{text}");
    assert!(text.contains(reason), "the refusal names the link's reason `{reason}`: {text}");
    assert!(!text.contains("prices.recorded"), "an event left at one instance is refused: {text}");
}

#[tokio::test]
async fn a_request_handler_declared_every_instance_is_refused() {
    let listened = Arc::new(Mutex::new(Vec::new()));
    let refused = bind(EveryRequestRoot, Recording { tcp: Tcp::new("127.0.0.1:0"), listened: Arc::clone(&listened) })
        .await
        .expect_err("a request handler declared `every_instance` bound");
    let text = refused.to_string();
    assert!(matches!(refused, StartupError::Configure(_)), "{text}");
    assert!(text.contains("every.request") && text.contains("request handler declared `every_instance`"), "{text}");
    assert!(listened.lock().unwrap_or_else(PoisonError::into_inner).is_empty(), "the refused server listened");
}
