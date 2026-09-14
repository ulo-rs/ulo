use crate::error::SetupResult;
use std::{cell::RefCell, pin::Pin, rc::Rc, sync::Arc};

use crate::{
    di::internal::{Container, InstanceWrapper},
    http::middleware::MiddlewareChain,
    http::{HttpAdapter, HttpRequest, HttpResponse, RequestHandler},
};

/// Wraps an `InstanceWrapper` as an opaque `RequestHandler`.
///
/// This keeps `InstanceWrapper` out of the adapter API — the adapter receives
/// an `Arc<dyn RequestHandler>` and never sees framework internals.
struct InstanceHandler(Arc<InstanceWrapper>);

impl RequestHandler for InstanceHandler {
    fn handle(
        &self,
        req: HttpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>> {
        let wrapper = self.0.clone();
        Box::pin(async move { wrapper.handle_request(req).await })
    }
}

pub(crate) struct RoutesResolver {
    pub(crate) container: Rc<RefCell<Container>>,
    global_chain: Option<MiddlewareChain>,
}

impl RoutesResolver {
    pub(crate) fn new(container: Rc<RefCell<Container>>) -> Self {
        Self {
            container,
            global_chain: None,
        }
    }

    /// Register all routes with the adapter and store the global chain for
    /// `take_global_chain` to hand to `start()` later.
    pub(crate) fn resolve(&mut self, http_adapter: &mut dyn HttpAdapter) -> SetupResult {
        let modules_token = self.container.borrow().module_tokens();

        for module_token in modules_token {
            self.register_routes(module_token, http_adapter)?;
        }

        self.global_chain = Some({
            let container = self.container.borrow();
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
            let mut container = self.container.borrow_mut();
            let controllers = container.get_controller_instances(&module_token)?;
            controllers.collect()
        };

        for (_, mut wrapper) in controllers_vec {
            let route_path = wrapper.path();
            let route_method = wrapper.method();

            let route_middleware = {
                let container = self.container.borrow();
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

            let handler: Arc<dyn RequestHandler> = Arc::new(InstanceHandler(wrapper));
            http_adapter.register_route(route_method, &route_path, handler)?;
        }

        Ok(())
    }
}
