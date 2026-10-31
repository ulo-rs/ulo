//! The probe routes of `ulo-health` over HTTP (integrations DESIGN §3.3).
//!
//! ```
//! # fn f(m: &mut ulo::ModuleDef<'_>) {
//! m.import(ulo_health::HealthModule::new());
//! m.import(ulo_health_http::HealthHttp::new());
//! # }
//! ```
//!
//! Each route answers 200 when its probe passes and 503 when it does not; Kubernetes counts 200 to
//! 399 as success. The body follows the expired IETF health-check draft as a shape: media type
//! `application/health+json`, `status` `pass` or `fail`, and with `.detail(true)` the draft's
//! `checks` object, each down check's reason passed through the app's redaction.
//!
//! During `Draining` the HTTP server's own 503 answers every request before routing, the probes
//! included, and kubelet has stopped the liveness and startup probes by then, so a liveness
//! failure in the drain cannot kill the pod.

mod body;
mod routes;

pub use body::MEDIA_TYPE;
pub use routes::HealthHttp;
