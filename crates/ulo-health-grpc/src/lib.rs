//! `grpc.health.v1` driven by `ulo-health` (integrations DESIGN §3.4).
//!
//! ```ignore
//! m.import(ulo_health::HealthModule::new());
//! m.import(ulo_health_grpc::HealthGrpc::new());
//! ```
//!
//! The bridge imports `ulo_grpc::GrpcHealth::module()` itself, so the gRPC server serves the
//! reporter the bridge writes; an application importing it as well gets the same module.
//!
//! `""` has one writer per application: this bridge when it is imported, the server otherwise.
//! Named services keep the server's writes at `bind` and `drain` and the application's through
//! `GrpcHealth::set_named`. Every write goes through `GrpcHealth`'s stored copy and reaches
//! tonic-health's reporter only on a change, so a `Watch` subscriber receives one message per
//! change.
//!
//! tonic-health answers a `Watch` on a name nobody registered with `NOT_FOUND`, where
//! `grpc.health.v1` streams `SERVICE_UNKNOWN` and keeps the call open; its `ServingStatus` has no
//! `SERVICE_UNKNOWN`. That is tonic-health's behaviour, not this crate's, and a `Health` service of
//! the bridge's own would be a second implementation of the protocol. Kubernetes' probe sends
//! `Check`, not `Watch`.

mod bridge;
mod interval;

pub use bridge::HealthGrpc;
pub use interval::ZeroInterval;
