//! The probe routes of `ulo-health` over HTTP (integrations DESIGN §3.3).
//!
//! ```ignore
//! m.import(ulo_health::HealthModule::new());
//! m.import(ulo_health_http::HealthHttp::new());
//! ```
//!
//! Each route answers 200 when its probe passes and 503 when it does not; Kubernetes counts 200 to
//! 399 as success. During `Draining` the HTTP server's own 503 answers every request before routing,
//! the probes included, and kubelet has stopped the liveness and startup probes by then, so a
//! liveness failure in the drain cannot kill the pod.

mod body;
mod routes;

pub use body::MEDIA_TYPE;
pub use routes::HealthHttp;
