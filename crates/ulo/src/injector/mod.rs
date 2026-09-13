mod container;
pub(crate) use self::container::Container;

mod instance_loader;
pub(crate) use self::instance_loader::InstanceLoader;
mod module;
mod multi_collection_provider;

mod dependency_graph;
pub(crate) use self::dependency_graph::{DependencyGraph, find_dependency_cycle};

mod instance_wrapper;
pub(crate) use self::instance_wrapper::InstanceWrapper;

pub(crate) use crate::di::IntoToken;

mod module_ref;
pub use self::module_ref::ModuleRef;

mod module_ref_provider;

mod role_registry;
pub(crate) use self::role_registry::RoleRegistry;

mod gateway_resolver;
pub(crate) use self::gateway_resolver::GatewayResolver;

mod rpc_controller_resolver;
pub(crate) use self::rpc_controller_resolver::RpcControllerResolver;

mod grpc_service_resolver;
pub(crate) use self::grpc_service_resolver::GrpcServiceResolver;

#[cfg(test)]
mod module_identity_tests;
