//! `WsModule`'s settings: its identity is its type, so two imports are one module, and the
//! broadcast adapter's `BroadcastAdapter::settings` followed by the gateway defaults decide
//! whether the second is the same one.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, ModuleSettings, StartupError, Timer, WiringError, WiringErrors};
use ulo_ws::{BroadcastAdapter, NodeId, Target, WsModule};

/// An adapter on the default settings, its type name.
struct Opaque;

impl BroadcastAdapter for Opaque {
    fn publish(&self, _target: Target, _frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn subscribe(&self, _node: NodeId, _timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>> {
        Box::pin(async { Err(BoxError::from("not subscribed in this test")) })
    }
}

/// Imports each module, in order.
struct Root(Vec<WsModule>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Root>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for module in &self.0 {
            m.import(module.clone());
        }
    }
}

fn wired(root: Root) -> Result<(), WiringErrors> {
    match App::builder(root).runtime(ulo_runtime_tokio::Tokio::current()).wire() {
        Ok(_) => Ok(()),
        Err(StartupError::Wiring(errors)) => Err(errors),
        Err(other) => panic!("expected wiring errors, got: {other}"),
    }
}

/// The in-memory adapter beside a user's adapter is refused, `local` against the type name.
/// Planted breaks: `WsModule::settings` left at the trait's default; `InMemory::settings` left
/// at the adapter trait's default.
#[tokio::test]
async fn two_adapter_kinds_under_one_identity_are_refused() {
    let errors = match wired(Root(vec![WsModule::for_root(), WsModule::for_root().broadcast(Opaque)])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two broadcast adapters under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { module, settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    assert_eq!(module.to_string(), "WsModule");
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("broadcast", Some("local".to_owned()), Some(std::any::type_name::<Opaque>().to_owned()))]);
}

/// The gateway defaults are settings too: a second import with another `message_limit` is
/// refused naming it. Planted break: `WsModule::settings` answering the adapter's alone.
#[tokio::test]
async fn two_gateway_defaults_under_one_identity_are_refused() {
    let errors = match wired(Root(vec![WsModule::for_root().message_limit(1024), WsModule::for_root().message_limit(2048)])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two message limits under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("message_limit", Some("Some(1024)".to_owned()), Some("Some(2048)".to_owned()))]);
}

/// Two in-memory modules are one module, and so are two of one user adapter type.
#[tokio::test]
async fn one_adapter_kind_is_one_module() {
    wired(Root(vec![WsModule::for_root(), WsModule::for_root()])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec![WsModule::for_root().broadcast(Opaque), WsModule::for_root().broadcast(Opaque)]))
        .unwrap_or_else(|errors| panic!("{errors}"));
}

/// An adapter writing a name `WsModule` writes too.
struct Colliding;

/// The line of `Colliding`'s `with` call.
const COLLIDING_LINE: u32 = line!() + 4;

impl BroadcastAdapter for Colliding {
    fn settings(&self) -> ModuleSettings {
        ModuleSettings::new().with("broadcast", "colliding").with("message_limit", 7)
    }

    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        Opaque.publish(target, frame)
    }

    fn subscribe(&self, node: NodeId, timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>> {
        Opaque.subscribe(node, timer)
    }
}

/// An adapter's setting under a name the module also writes is refused at `wire()`, naming the
/// adapter's line and the module's. Planted break: `ModuleSettings::with` recording the name
/// again.
#[tokio::test]
async fn an_adapter_setting_named_as_a_gateway_default_is_refused_with_both_lines() {
    let errors = match wired(Root(vec![WsModule::for_root().broadcast(Colliding)])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; the adapter and the module both write `message_limit`"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DuplicateSetting { setting, module, first, second }) = errors.iter().next() else {
        panic!("expected `DuplicateSetting`:\n{errors}");
    };
    assert_eq!(*setting, "message_limit");
    assert_eq!(module.to_string(), "WsModule");
    assert_eq!((first.file(), first.line()), (file!(), COLLIDING_LINE));
    assert!(second.file().ends_with("module.rs") && second.file().contains("ulo-ws"), "the module's own line: {second}");
}
