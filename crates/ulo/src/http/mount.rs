use crate::error::SetupResult;
use parking_lot::RwLock;
use std::{pin::Pin, sync::Arc};

use crate::{
    di::internal::Container,
    http::RoutePipeline,
    http::middleware::MiddlewareChain,
    http::{HttpAdapter, HttpRequest, HttpResponse, RequestHandler},
};

/// One mounted route, as the adapter sees it.
///
/// This keeps `RoutePipeline` out of the adapter API — the adapter receives
/// an `Arc<dyn RequestHandler>` and never sees framework internals.
struct MountedRoute(Arc<RoutePipeline>);

impl RequestHandler for MountedRoute {
    fn handle(
        &self,
        req: HttpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>> {
        let wrapper = self.0.clone();
        Box::pin(async move { wrapper.handle_request(req).await })
    }
}

pub(crate) struct RouteMount {
    container: Arc<RwLock<Container>>,
    global_chain: Option<MiddlewareChain>,
}

impl RouteMount {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self {
            container,
            global_chain: None,
        }
    }

    /// Register every route with the adapter, and keep the global chain for
    /// `take_global_chain` to hand to `start()` later.
    pub(crate) fn mount(&mut self, http_adapter: &mut dyn HttpAdapter) -> SetupResult {
        let modules_token = self.container.read().module_tokens();

        for module_token in modules_token {
            self.register_routes(module_token, http_adapter)?;
        }

        self.global_chain = Some({
            let container = self.container.read();
            let mut chain = MiddlewareChain::new();
            if let Some(mm) = container.middleware_manager() {
                for mw in mm.global_middleware() {
                    chain.use_middleware(mw.clone());
                }
            }
            chain
        });

        Ok(())
    }

    /// Hand the global chain to `UloApplication::start` so it can wrap the
    /// adapter's routing handler with it.
    pub(crate) fn take_global_chain(&mut self) -> MiddlewareChain {
        self.global_chain.take().unwrap_or_default()
    }

    fn register_routes(
        &mut self,
        module_token: String,
        http_adapter: &mut dyn HttpAdapter,
    ) -> SetupResult {
        let controllers_vec: Vec<_> = {
            let mut container = self.container.write();
            let controllers = container.get_controller_instances(&module_token)?;
            controllers.collect()
        };

        for (_, mut wrapper) in controllers_vec {
            let route_path = wrapper.path();
            let route_method = wrapper.method();

            let route_middleware = {
                let container = self.container.read();
                if let Some(mm) = container.middleware_manager() {
                    mm.get_middleware_for_route(&module_token, &route_path, route_method.as_str())
                } else {
                    Vec::new()
                }
            };

            tracing::debug!(
                method = %route_method.as_str(),
                path = %route_path,
                middleware = route_middleware.len(),
                "route registered"
            );

            if let Some(w) = Arc::get_mut(&mut wrapper) {
                w.set_middleware(route_middleware);
            }

            if wrapper.streams() && !http_adapter.streams_responses() {
                return Err(format!(
                    "route {} {route_path} answers with a stream, and this HTTP adapter collects a \
                     response body before sending it. A stream that does not end would never \
                     finish collecting, so the route would register and never answer. Serve it \
                     with an adapter that streams.",
                    route_method.as_str(),
                )
                .into());
            }

            let handler: Arc<dyn RequestHandler> = Arc::new(MountedRoute(wrapper));
            http_adapter.register_route(route_method, &route_path, handler)?;
        }

        Ok(())
    }
}
