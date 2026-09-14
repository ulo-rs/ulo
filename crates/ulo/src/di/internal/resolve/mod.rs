mod gateway;
mod grpc;
mod routes;
mod rpc;

pub(crate) use self::gateway::GatewayResolver;
pub(crate) use self::grpc::GrpcServiceResolver;
pub(crate) use self::routes::RoutesResolver;
pub(crate) use self::rpc::RpcControllerResolver;
