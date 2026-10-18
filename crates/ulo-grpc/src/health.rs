//! `grpc.health.v1` over tonic-health (transports DESIGN §6.2; integrations §3.4, I4, I14).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use tonic::server::NamedService;
use tonic_health::ServingStatus;
use tonic_health::pb::health_server::HealthServer;
use tonic_health::server::{HealthReporter, HealthService};
use ulo::DynamicModule;

/// The health reporter, bound by the gRPC server as `Dep<GrpcHealth>`: SERVING for every known
/// service after `bind`, NOT_SERVING from the drain on, and in between what the application sets.
///
/// `imports = [GrpcHealth::module()]` binds it, global, for any module to read; the server finds
/// it there and serves the statuses it sets. Without the import the server keeps a reporter of its
/// own, and `grpc.health.v1` still answers.
///
/// It is the only path to tonic-health's reporter, which it does not expose. It keeps the last
/// status written per service name, shared by every clone and starting, as the reporter does,
/// with the overall status `""` at SERVING. Every write, the server's at `bind` and the drain and
/// the application's through [`set`](Self::set) and [`set_named`](Self::set_named), compares with
/// that copy and reaches the reporter only when the status changes, so a `Watch` subscriber
/// receives one message per change, as `grpc.health.v1` specifies: tonic-health sends one on every
/// write, an unchanged status included. The comparison and the write hold one lock, so two writers
/// to one name do not interleave, and the last write to a name wins.
///
/// tonic-health answers a `Watch` on a name nothing has written with `NOT_FOUND` and ends the
/// call, where `grpc.health.v1` streams `SERVICE_UNKNOWN` and keeps it open; its `ServingStatus`
/// has no `SERVICE_UNKNOWN`. Kubernetes' gRPC probe sends `Check`, which tonic-health answers as
/// the specification does.
#[derive(Clone)]
pub struct GrpcHealth {
    reporter: HealthReporter,
    written: Arc<Mutex<HashMap<String, ServingStatus>>>,
    /// Set by `delegate_overall`; from then on the server writes no status for `""`.
    delegated: Arc<AtomicBool>,
}

impl GrpcHealth {
    pub(crate) fn new() -> Self {
        GrpcHealth {
            reporter: HealthReporter::new(),
            written: Arc::new(Mutex::new(HashMap::from([(String::new(), ServingStatus::Serving)]))),
            delegated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The global module binding one `GrpcHealth`, which the gRPC server serves through
    /// `grpc.health.v1`.
    pub fn module() -> DynamicModule {
        DynamicModule::new::<GrpcHealth, ()>((), |m| {
            m.value(GrpcHealth::new());
            m.export::<GrpcHealth>();
            m.global();
        })
        .label("GrpcHealth")
    }

    /// Hands the overall status `""` to the caller: from this call on the server writes no status
    /// for `""`, at the drain included, and the caller is its one writer, through
    /// [`set_named`](Self::set_named). Named services keep their writers, the server at `bind` and
    /// the drain and the application through `set` and `set_named`.
    ///
    /// For a component that derives the overall status from something the server cannot see,
    /// `ulo-health-grpc`'s bridge writing it from readiness. Called before `listen()`, it covers
    /// every write the server makes; calling it twice is calling it once.
    pub fn delegate_overall(&self) {
        self.delegated.store(true, Ordering::Release);
    }

    /// Sets service `S`'s status.
    pub async fn set<S: NamedService>(&self, status: ServingStatus) {
        self.write(S::NAME, status).await;
    }

    /// Sets the status of the service named `name`, `users.v1.UserService`, or of the overall
    /// status `""`.
    pub async fn set_named(&self, name: &str, status: ServingStatus) {
        self.write(name, status).await;
    }

    /// A write the server makes at `bind` and at the drain, which leaves `""` alone once
    /// [`delegate_overall`](Self::delegate_overall) has run.
    pub(crate) async fn set_by_server(&self, name: &str, status: ServingStatus) {
        if name.is_empty() && self.delegated.load(Ordering::Acquire) {
            return;
        }
        self.write(name, status).await;
    }

    /// The lock is held across the reporter's write, so a second writer to the name reads the copy
    /// only once the first write has reached the reporter.
    async fn write(&self, name: &str, status: ServingStatus) {
        let mut written = self.written.lock().await;
        if written.get(name) == Some(&status) {
            return;
        }
        self.reporter.set_service_status(name, status).await;
        written.insert(name.to_owned(), status);
    }

    /// `grpc.health.v1.Health` answering this reporter's statuses.
    pub(crate) fn service(&self) -> HealthServer<HealthService> {
        HealthServer::new(HealthService::from_health_reporter(self.reporter.clone()))
    }
}
