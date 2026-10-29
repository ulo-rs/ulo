use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tonic::transport::Channel;
use tonic::{Request, Status};
use ulo::{App, AppBuilder, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_grpc::{GrpcClientModule, GrpcEndpoint, Message};

pub mod pb {
    ulo_grpc::include_proto!("clock.v1");
}

use pb::clock_client::ClockClient;

/// How many calls reached the callee.
#[derive(Clone, Default)]
pub struct Calls(Arc<AtomicUsize>);

impl Calls {
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[injectable]
pub struct ClockService {
    calls: Dep<Calls>,
}

#[routes]
impl ClockService {
    /// The time left before the deadline this call was given, in milliseconds rounded up.
    #[ulo_grpc::method(pb::clock::Remaining)]
    async fn remaining(&self, _req: Message<pb::Probe>, exec: ExecutionRef) -> pb::Left {
        self.calls.0.fetch_add(1, Ordering::SeqCst);
        match exec.deadline() {
            None => pb::Left { has_deadline: false, millis: 0 },
            Some(deadline) => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
                pb::Left { has_deadline: true, millis: u32::try_from(left.as_millis()).unwrap_or(u32::MAX).saturating_add(1) }
            }
        }
    }
}

/// The callee's application.
pub struct CalleeModule(pub Calls);

impl Module for CalleeModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<ClockService>();
    }
}

pub fn callee_builder(calls: Calls) -> AppBuilder {
    App::builder(CalleeModule(calls))
}

/// Calls `Clock` with the deadline of the execution it runs in.
#[injectable]
pub struct Front {
    clock: Dep<ClockClient<Channel>>,
}

impl Front {
    /// The callee's remaining time. `outgoing` writes `grpc-timeout` from the execution's
    /// deadline, and refuses with DEADLINE_EXCEEDED, before anything is sent, when it has passed.
    pub async fn remaining(&self, exec: &ExecutionRef) -> Result<pb::Left, Status> {
        let request = ulo_grpc::outgoing(exec, Request::new(pb::Probe {}))?;
        let mut clock = (*self.clock).clone();
        Ok(clock.remaining(request).await?.into_inner())
    }
}

#[routes]
impl Front {
    #[ulo_grpc::method(pb::front::Ask)]
    async fn ask(&self, _req: Message<pb::Probe>, exec: ExecutionRef) -> Result<pb::Left, Status> {
        self.remaining(&exec).await
    }
}

/// The caller's application: the generated client bound under its own type, built over a channel
/// that connects on the first call.
pub struct FrontModule {
    pub callee: String,
}

impl Module for FrontModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(GrpcClientModule::<ClockClient<Channel>>::for_root(GrpcEndpoint::new(self.callee.clone())));
        m.controller::<Front>();
    }
}

pub fn front_builder(callee: impl Into<String>) -> AppBuilder {
    App::builder(FrontModule { callee: callee.into() })
}
