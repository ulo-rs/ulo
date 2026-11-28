//! The embedding conformance suite against this host, nested under `PREFIX` and as the fallback.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;

use community_host::{Embedded, Handle, HostService, Hyperlite, run, serve};
use http::{HeaderValue, Request};
use hyper::body::Incoming;
use hyper::service::Service as HyperService;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http::{Response, Routing};
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, OnTokio, PREFIX, ROUTING_HEADER, ReadCount, report, routing_label, startup_failed};

/// The host's middleware, around the app: the header's value into the request's extensions, and
/// the app's `Routing` onto the response.
struct Middleware(HostService);

impl HyperService<Request<Incoming>> for Middleware {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn call(&self, mut req: Request<Incoming>) -> Self::Future {
        let value = req.headers().get(HOST_VALUE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
        if let Some(value) = value {
            req.extensions_mut().insert(HostValue(value));
        }
        let answer = self.0.call(req);
        Box::pin(async move {
            let mut response = answer.await?;
            if let Some(label) = response.extensions().get::<Routing>().map(routing_label) {
                response.headers_mut().insert(ROUTING_HEADER, HeaderValue::from_str(&label).expect("a label is a header value"));
            }
            Ok(response)
        })
    }
}

struct HyperliteHost {
    base_url: String,
    read_count: ReadCount,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Host for HyperliteHost {
    type Harness = OnTokio;

    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        };
        let handle: Handle = server.handle();
        let app = app.bind(server).listen().await.unwrap_or_else(|error| startup_failed!("the app did not listen inside hyperlite: {}", report(&error)));
        let listener = TcpListener::bind("127.0.0.1:0").await
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        let read_count = ReadCount::default();
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn({
            let handle = handle.clone();
            let read_count = read_count.clone();
            async move {
                let signal = async move {
                    let _ = stopped.await;
                    Signal::new("suite")
                };
                let server = {
                    let handle = handle.clone();
                    async move {
                        let service = |peer| match mode {
                            Mode::Nested => HostService::new(&handle, peer).nested_at(PREFIX),
                            Mode::Fallback => HostService::new(&handle, peer),
                        };
                        serve(listener, &handle, |stream| read_count.wrap(stream), |peer| Middleware(service(peer))).await
                    }
                };
                let _ = run(app, &handle, server, signal).await;
            }
        });
        HyperliteHost { base_url: format!("http://{addr}"), read_count, stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Hyperlite as Embed>::limits()
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(HyperliteHost; not_applicable {
    missing_host_refused: "the host owns its wire policy, a request without `Host` among it",
});
