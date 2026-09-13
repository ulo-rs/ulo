use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::http::Route;

use super::provider::Provider;

/// What a controller hands over to be dispatched on.
///
/// The one place the transports differ. HTTP dispatches on routes keyed by path and method, RPC on a
/// set of patterns, gRPC on a registration with the tonic router. Everything else a controller
/// carries — its token, its dependencies, its lifecycle, the scope it is built at — is common to all
/// three and lives on [`Controller`] itself.
pub enum Dispatch {
    Http(Vec<Arc<dyn Route>>),
    Rpc(Arc<dyn crate::rpc::RpcControllerSource>),
    Grpc(Arc<dyn crate::grpc::GrpcServiceSource>),
}

/// A controller: one DI instance exposing what it dispatches on and its lifecycle hooks.
///
/// Built once per controller struct by its [`ControllerFactory`]. [`dispatch`](Controller::dispatch)
/// yields the transport's own dispatch surface — one [`Route`] per handler method on HTTP, a single
/// source on RPC and gRPC. The lifecycle hooks fire once per controller, not once per route or
/// pattern.
#[async_trait]
pub trait Controller: Send + Sync {
    fn token(&self) -> String;
    fn dispatch(&self) -> Dispatch;

    // Lifecycle Hooks

    async fn on_module_init(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_module_destroy(&self) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

#[async_trait]
pub trait ControllerFactory {
    fn token(&self) -> String;
    fn dependency_tokens(&self) -> Vec<String> {
        vec![]
    }
    async fn build(&self, deps: FxHashMap<String, Arc<Box<dyn Provider>>>) -> Arc<dyn Controller>;
}
