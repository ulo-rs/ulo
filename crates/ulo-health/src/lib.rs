//! Health (integrations DESIGN §3.1-§3.3).
//!
//! A module contributes a [`Check`] with `m.contribute::<dyn Check>()`; [`HealthModule`] reads
//! `Many<dyn Check>`, runs the checks concurrently under their bounds and answers three probes
//! with Kubernetes' meanings through [`Readiness`]. A transport crate (`ulo-health-http`,
//! `ulo-health-grpc`) serves the answers.
//!
//! ```
//! # use std::sync::Arc;
//! # use ulo::{BoxFuture, ModuleDef};
//! # use ulo_health::{Check, Health};
//! # #[derive(Default)]
//! # struct QueueCheck;
//! # impl Check for QueueCheck {
//! #     fn name(&self) -> &str { "queue" }
//! #     fn check(&self) -> BoxFuture<'_, Health> { Box::pin(async { Health::Up }) }
//! # }
//! # fn f(m: &mut ModuleDef<'_>) {
//! m.import(ulo_health::HealthModule::new());
//! m.contribute::<dyn ulo_health::Check>().value(Arc::new(QueueCheck::default()));
//! # }
//! ```
//!
//! A check that does not answer within its bound is down, a probe arriving while a run is in
//! flight shares that run, and no result is kept between runs, so a probe never reads a stale
//! "up". The bounds run on the app's `Timer`, which the health module requires.
//!
//! Readiness turns false when the app reaches `Stopping`, in the health module's before-shutdown
//! hook, which runs ahead of every check's hooks and before the first refusal. The hook then waits
//! [`HealthModule::lag`] so the endpoint change reaches kube-proxy and the load balancers before
//! the app refuses work. Leave the lag at zero when the pod has a `preStop` sleep, which gives that
//! time, and set it to a few seconds when it has none. A lag past `shutdown_timeout` is cut by
//! that cap and recorded `TimedOut`.
//!
//! A lazily loaded module cannot contribute a check: `load` refuses the contribution with
//! `LoadRefusal::Contribution`, since the collection was read at `connect`.

mod check;
mod module;
mod readiness;
mod run;

pub use check::{Check, Health, Probe};
pub use module::{HealthModule, ZeroBound};
pub use readiness::{Changes, Readiness};
pub use run::{CheckResult, Report};
